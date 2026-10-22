// SPDX-License-Identifier: FSL-1.1-ALv2
//! Provider usage interpretation and billing events.
//!
//! The billed quantities are exactly the provider-reported totals: `usage.prompt_tokens` is the
//! input total and `usage.completion_tokens` the output total. Breakdown fields such as cached
//! prompt tokens or reasoning completion tokens are already part of those totals and are never
//! added, discounted or surcharged. A response without valid totals has unknown usage: it is
//! neither charged nor estimated.

use serde_json::Value;

/// Confirmed provider-reported token totals of one attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
}

/// Why usage could not be confirmed. Bounded metric and log label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownUsage {
    Missing,
    Invalid,
}

impl UnknownUsage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageReading {
    Known(TokenUsage),
    Unknown(UnknownUsage),
}

/// Reads token totals from an OpenAI-compatible chat completion body.
pub fn interpret(body: &Value) -> UsageReading {
    let Some(usage) = body.get("usage").filter(|u| !u.is_null()) else {
        return UsageReading::Unknown(UnknownUsage::Missing);
    };
    if !usage.is_object() {
        return UsageReading::Unknown(UnknownUsage::Invalid);
    }
    let total = |key: &str| {
        usage
            .get(key)
            .and_then(Value::as_u64)
            .filter(|n| i64::try_from(*n).is_ok())
    };
    match (total("prompt_tokens"), total("completion_tokens")) {
        (Some(input), Some(output)) => UsageReading::Known(TokenUsage { input, output }),
        (None, None)
            if usage.get("prompt_tokens").is_none() && usage.get("completion_tokens").is_none() =>
        {
            UsageReading::Unknown(UnknownUsage::Missing)
        }
        _ => UsageReading::Unknown(UnknownUsage::Invalid),
    }
}

/// Token direction of an outbox event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "in",
            Self::Output => "out",
        }
    }
}

/// Stable event key: `<request_id>:<attempt>:in|out`. Retries of delivery reuse it, so Delcore's
/// service-scoped idempotency prevents duplicate charges.
pub fn event_key(request_id: &str, attempt: u32, direction: Direction) -> String {
    format!("{request_id}:{attempt}:{}", direction.as_str())
}

/// One billable quantity of one attempt, before persistence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewEvent {
    pub direction: Direction,
    pub event_key: String,
    pub meter_key: String,
    pub quantity: i64,
}

/// Builds the events of one attempt. A zero quantity charges nothing and creates no event.
pub fn events_for(
    request_id: &str,
    attempt: u32,
    input_meter: &str,
    output_meter: &str,
    usage: TokenUsage,
) -> Vec<NewEvent> {
    [
        (Direction::Input, input_meter, usage.input),
        (Direction::Output, output_meter, usage.output),
    ]
    .into_iter()
    .filter(|(_, _, quantity)| *quantity > 0)
    .map(|(direction, meter, quantity)| NewEvent {
        direction,
        event_key: event_key(request_id, attempt, direction),
        meter_key: meter.to_owned(),
        quantity: quantity as i64,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn totals_are_the_provider_reported_prompt_and_completion_tokens() {
        let body = json!({"usage":{"prompt_tokens":120,"completion_tokens":30,"total_tokens":150}});
        assert_eq!(
            interpret(&body),
            UsageReading::Known(TokenUsage {
                input: 120,
                output: 30
            })
        );
    }

    #[test]
    fn cached_and_reasoning_breakdowns_are_not_added() {
        let body = json!({"usage":{
            "prompt_tokens":1000,
            "completion_tokens":400,
            "total_tokens":1400,
            "prompt_tokens_details":{"cached_tokens":800,"audio_tokens":5},
            "completion_tokens_details":{"reasoning_tokens":300,"accepted_prediction_tokens":10},
            "cache_creation_input_tokens":50,
            "cached_tokens":800,
            "reasoning_tokens":300
        }});
        assert_eq!(
            interpret(&body),
            UsageReading::Known(TokenUsage {
                input: 1000,
                output: 400
            })
        );
        let events = events_for(
            "r",
            1,
            "in_m",
            "out_m",
            TokenUsage {
                input: 1000,
                output: 400,
            },
        );
        assert_eq!(events.iter().map(|e| e.quantity).sum::<i64>(), 1400);
    }

    #[test]
    fn missing_usage_is_unknown() {
        for body in [
            json!({}),
            json!({"usage":null}),
            json!({"usage":{}}),
            json!({"choices":[]}),
        ] {
            assert_eq!(
                interpret(&body),
                UsageReading::Unknown(UnknownUsage::Missing),
                "{body}"
            );
        }
    }

    #[test]
    fn invalid_usage_is_unknown() {
        for body in [
            json!({"usage":{"prompt_tokens":10}}),
            json!({"usage":{"completion_tokens":10}}),
            json!({"usage":{"prompt_tokens":-1,"completion_tokens":2}}),
            json!({"usage":{"prompt_tokens":1.5,"completion_tokens":2}}),
            json!({"usage":{"prompt_tokens":"10","completion_tokens":2}}),
            json!({"usage":{"prompt_tokens":null,"completion_tokens":2}}),
            json!({"usage":{"prompt_tokens":u64::MAX,"completion_tokens":2}}),
            json!({"usage":"12"}),
        ] {
            assert_eq!(
                interpret(&body),
                UsageReading::Unknown(UnknownUsage::Invalid),
                "{body}"
            );
        }
    }

    #[test]
    fn event_keys_are_stable_and_directional() {
        assert_eq!(event_key("req", 2, Direction::Input), "req:2:in");
        assert_eq!(event_key("req", 2, Direction::Output), "req:2:out");
        let first = events_for(
            "req",
            1,
            "i",
            "o",
            TokenUsage {
                input: 1,
                output: 2,
            },
        );
        let again = events_for(
            "req",
            1,
            "i",
            "o",
            TokenUsage {
                input: 1,
                output: 2,
            },
        );
        assert_eq!(first, again);
        assert_eq!(first[0].meter_key, "i");
        assert_eq!(first[1].meter_key, "o");
    }

    #[test]
    fn zero_quantities_create_no_event() {
        let events = events_for(
            "r",
            1,
            "i",
            "o",
            TokenUsage {
                input: 0,
                output: 5,
            },
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].direction, Direction::Output);
        assert!(
            events_for(
                "r",
                1,
                "i",
                "o",
                TokenUsage {
                    input: 0,
                    output: 0
                }
            )
            .is_empty()
        );
    }
}
