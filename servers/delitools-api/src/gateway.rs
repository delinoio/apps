// SPDX-License-Identifier: FSL-1.1-ALv2
//! Vercel AI Gateway client (OpenAI-compatible chat completions, public model catalog and
//! generation lookup). Verified against the Gateway documentation:
//!
//! * `POST {GATEWAY_URL}/chat/completions` with `stream`, `stream_options.include_usage`, the
//!   standard `user` reference, `response_format.json_schema` and `providerOptions.gateway`
//!   routing (`only`/`order`). The generation ID is the `id` of the first streamed chunk.
//! * `GET {GATEWAY_URL}/models` (public, `pricing.input|output|input_cache_read|input_cache_write`
//!   and `*_tiers`) and `GET {GATEWAY_URL}/models/{creator}/{model}/endpoints`.
//! * `GET {GATEWAY_URL}/generation?id=` (`native_tokens_prompt|completion|cached|cache_creation`),
//!   which answers 404 until the usage event is ingested asynchronously.
//!
//! `GATEWAY_URL` is the base including the `/v1` prefix. Bodies and error text are never logged
//! and never returned to callers. Every response read is bounded by `MAX_GATEWAY_RESPONSE_BYTES`.
use crate::decimal;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

/// The fixed extraction model: the Gateway catalog entry for `gpt-6-luna`
/// (https://vercel.com/ai-gateway/models/gpt-6-luna).
pub const EXTRACTION_MODEL: &str = "openai/gpt-6-luna";

const MAX_GENERATION_ID_BYTES: usize = 200;

/// Measured token counts of one AI call. `input` excludes cache reads and cache writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// What is known about the tokens an attempt consumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// Confirmed counts, from the stream or from a Gateway generation lookup.
    Known(Tokens),
    /// Proven to have consumed nothing (the request never reached the provider).
    Zero,
    /// The call may have consumed tokens that are not yet confirmed. Never estimated.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// Retry-eligible provider or transport failure.
    Transient,
    /// Not retry-eligible provider failure (rejected request, authentication, configuration).
    Permanent,
    /// Provider output was unusable (truncated, refused or oversized). Never retried.
    Output,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    /// Bounded log category.
    pub category: &'static str,
}

pub struct ChatOutcome {
    pub generation_id: Option<String>,
    pub usage: Usage,
    pub result: Result<String, Failure>,
}

/// Receives the generation ID as soon as the first chunk arrives, so it is durable even if the
/// call is later cancelled.
#[async_trait]
pub trait GenerationSink: Send + Sync {
    async fn captured(&self, generation_id: &str);
}

pub struct ChatRequest<'a> {
    pub model: &'a str,
    /// Frozen provider route; empty means the model's default routing.
    pub route: &'a str,
    /// Safe attempt UUID sent as the provider user reference.
    pub user: &'a str,
    pub system: &'a str,
    pub user_message: &'a str,
    pub schema_name: &'a str,
    pub schema: &'a str,
}

/// A catalog entry reduced to what price synchronization needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: String,
    pub language: bool,
    pub price: Option<CatalogPrice>,
}

/// Canonical plain-decimal USD per-token prices. `None` means no published price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPrice {
    pub input: String,
    pub output: String,
    pub cache_read: Option<String>,
    pub cache_write: Option<String>,
    /// Context-tiered pricing is present, so a single flat price does not apply.
    pub tiered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub provider: String,
    pub active: bool,
    pub price: Option<CatalogPrice>,
}

pub enum Lookup {
    Usage {
        model: String,
        tokens: Tokens,
    },
    /// The Gateway has not ingested the usage event yet (or no such generation).
    NotYet,
    /// The generation exists but does not carry complete token counts.
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatewayError(pub &'static str);
impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for GatewayError {}

pub struct Gateway {
    http: reqwest::Client,
    base: String,
    key: String,
    max_bytes: usize,
    /// Deadline of one non-execution request (catalog, endpoints, generation lookup).
    request_timeout: Duration,
}

impl Gateway {
    /// `timeout` bounds connecting for every call and the whole of each non-execution request
    /// (`TIMEOUT_SECONDS`). A streamed chat completion has no client-side total timeout here: the
    /// executor bounds it with the request's execution deadline and drops the future.
    pub fn new(base: &str, key: &str, max_bytes: usize, timeout: Duration) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base: base.trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            max_bytes,
            request_timeout: timeout,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Build the streaming request body. The schema text is spliced in verbatim so exact number
    /// lexemes inside enums reach the provider unchanged.
    pub fn chat_body(request: &ChatRequest<'_>) -> String {
        let quote = |text: &str| serde_json::to_string(text).expect("string serializes");
        let routing = if request.route.is_empty() {
            String::new()
        } else {
            let route = quote(request.route);
            format!(
                ",\"providerOptions\":{{\"gateway\":{{\"only\":[{route}],\"order\":[{route}]}}}}"
            )
        };
        format!(
            "{{\"model\":{},\"stream\":true,\"stream_options\":{{\"include_usage\":true}},\"user\":{},\"messages\":[{{\"role\":\"system\",\"content\":{}}},{{\"role\":\"user\",\"content\":{}}}],\"response_format\":{{\"type\":\"json_schema\",\"json_schema\":{{\"name\":{},\"strict\":true,\"schema\":{}}}}}{routing}}}",
            quote(request.model),
            quote(request.user),
            quote(request.system),
            quote(request.user_message),
            quote(request.schema_name),
            request.schema
        )
    }

    /// Run one streaming completion. Dropping the returned future closes the connection, which
    /// cancels the in-flight call.
    pub async fn chat(&self, request: &ChatRequest<'_>, sink: &dyn GenerationSink) -> ChatOutcome {
        let mut outcome = ChatOutcome {
            generation_id: None,
            usage: Usage::Unknown,
            result: Err(Failure {
                kind: FailureKind::Transient,
                category: "transport",
            }),
        };
        let response = self
            .http
            .post(self.url("/chat/completions"))
            .bearer_auth(&self.key)
            .header("content-type", "application/json")
            .body(Self::chat_body(request))
            .send()
            .await;
        let mut response = match response {
            Ok(response) => response,
            Err(error) => {
                // Only a connection that was never established proves nothing was consumed.
                outcome.usage = if error.is_connect() {
                    Usage::Zero
                } else {
                    Usage::Unknown
                };
                outcome.result = Err(Failure {
                    kind: FailureKind::Transient,
                    category: if error.is_connect() {
                        "connect"
                    } else {
                        "transport"
                    },
                });
                return outcome;
            }
        };
        let status = response.status().as_u16();
        if status != 200 {
            let (failure, usage) = classify_status(status);
            outcome.usage = usage;
            outcome.result = Err(failure);
            return outcome;
        }
        let mut stream = Stream::new(self.max_bytes);
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    for event in stream.feed(&chunk) {
                        let StreamEvent::Generation(id) = event;
                        sink.captured(&id).await;
                    }
                    if let Some(failure) = stream.failure {
                        outcome.result = Err(failure);
                        break;
                    }
                    if stream.done {
                        outcome.result = stream.finish();
                        break;
                    }
                }
                Ok(None) => {
                    outcome.result = stream.finish();
                    break;
                }
                Err(_) => {
                    outcome.result = Err(Failure {
                        kind: FailureKind::Transient,
                        category: "stream_interrupted",
                    });
                    break;
                }
            }
        }
        outcome.generation_id = stream.generation.clone();
        outcome.usage = stream.usage.map_or(Usage::Unknown, Usage::Known);
        outcome
    }

    async fn read_bounded(&self, mut response: reqwest::Response) -> Result<Vec<u8>, GatewayError> {
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() + chunk.len() > self.max_bytes {
                        return Err(GatewayError("response too large"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(bytes),
                Err(_) => return Err(GatewayError("transport")),
            }
        }
    }

    async fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
        authenticated: bool,
    ) -> Result<(u16, Value), GatewayError> {
        // The per-request timeout covers connecting, the headers and reading the body, so a
        // Gateway that accepts the connection and then stalls cannot block catalog
        // synchronization or usage reconciliation indefinitely.
        let mut request = self
            .http
            .get(self.url(path))
            .query(query)
            .timeout(self.request_timeout);
        if authenticated {
            request = request.bearer_auth(&self.key);
        }
        let response = request
            .send()
            .await
            .map_err(|_| GatewayError("transport"))?;
        let status = response.status().as_u16();
        let bytes = self.read_bounded(response).await?;
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, value))
    }

    /// The public model catalog. No credential is sent: the endpoint is public.
    pub async fn models(&self) -> Result<Vec<CatalogModel>, GatewayError> {
        let (status, body) = self.get_json("/models", &[], false).await?;
        if status != 200 {
            return Err(GatewayError("catalog rejected"));
        }
        let data = body["data"]
            .as_array()
            .ok_or(GatewayError("catalog malformed"))?;
        Ok(data.iter().filter_map(parse_model).collect())
    }

    /// Provider endpoints of one model (public).
    pub async fn endpoints(&self, model: &str) -> Result<Vec<Endpoint>, GatewayError> {
        let (status, body) = self
            .get_json(&format!("/models/{model}/endpoints"), &[], false)
            .await?;
        if status != 200 {
            return Err(GatewayError("endpoints rejected"));
        }
        let endpoints = body["data"]["endpoints"]
            .as_array()
            .ok_or(GatewayError("endpoints malformed"))?;
        Ok(endpoints.iter().filter_map(parse_endpoint).collect())
    }

    /// Look up the measured usage of one generation.
    pub async fn generation(&self, id: &str) -> Result<Lookup, GatewayError> {
        let (status, body) = self.get_json("/generation", &[("id", id)], true).await?;
        match status {
            404 => return Ok(Lookup::NotYet),
            200 => {}
            _ => return Err(GatewayError("generation lookup rejected")),
        }
        let data = &body["data"];
        let count = |key: &str| data[key].as_u64();
        let (Some(model), Some(prompt), Some(completion), Some(cached), Some(created)) = (
            data["model"].as_str(),
            count("native_tokens_prompt"),
            count("native_tokens_completion"),
            count("native_tokens_cached"),
            count("native_tokens_cache_creation"),
        ) else {
            return Ok(Lookup::Unreadable);
        };
        if data["id"].as_str() != Some(id) {
            return Ok(Lookup::Unreadable);
        }
        match split_prompt(prompt, cached, created) {
            Some(input) => Ok(Lookup::Usage {
                model: model.to_owned(),
                tokens: Tokens {
                    input,
                    output: completion,
                    cache_read: cached,
                    cache_write: created,
                },
            }),
            None => Ok(Lookup::Unreadable),
        }
    }
}

/// Prompt totals include cached and cache-creation tokens (OpenAI-compatible accounting), so the
/// plain input count is the remainder. Inconsistent counts are rejected, never repaired.
pub fn split_prompt(prompt: u64, cached: u64, created: u64) -> Option<u64> {
    prompt.checked_sub(cached)?.checked_sub(created)
}

/// Classify a non-200 chat response. A definitive client-side rejection consumed nothing; a
/// server-side failure may have. HTTP 408 is not provably unsent: the Gateway aborts a provider
/// request that timed out after forwarding it, and some providers still bill an aborted call, so
/// its usage stays unknown and is reconciled instead of being recorded as zero.
pub fn classify_status(status: u16) -> (Failure, Usage) {
    match status {
        408 => (
            Failure {
                kind: FailureKind::Transient,
                category: "request_timeout",
            },
            Usage::Unknown,
        ),
        425 | 429 => (
            Failure {
                kind: FailureKind::Transient,
                category: "rate_limited",
            },
            Usage::Zero,
        ),
        500..=599 => (
            Failure {
                kind: FailureKind::Transient,
                category: "server_error",
            },
            Usage::Unknown,
        ),
        _ => (
            Failure {
                kind: FailureKind::Permanent,
                category: "rejected",
            },
            Usage::Zero,
        ),
    }
}

fn price_of(
    value: &Value,
    input: &str,
    output: &str,
    read: &str,
    write: &str,
) -> Option<CatalogPrice> {
    let text = |key: &str| value[key].as_str().and_then(decimal::price);
    // A cache category may be unpublished (absent or null), which means "no price". A field that
    // is present but not a valid price is a malformed entry, not an unpublished category: it
    // would otherwise pin a route whose cache usage can never be reported, so the whole price is
    // rejected (`Err`) and the model stays unpriced.
    let optional = |key: &str| match &value[key] {
        Value::Null => Ok(None),
        Value::String(text) => decimal::price(text).map(Some).ok_or(()),
        _ => Err(()),
    };
    let tiered = [
        "input_tiers",
        "output_tiers",
        "prompt_tiers",
        "completion_tiers",
    ]
    .iter()
    .any(|key| value[key].as_array().is_some_and(|tiers| !tiers.is_empty()));
    Some(CatalogPrice {
        input: text(input)?,
        output: text(output)?,
        cache_read: optional(read).ok()?,
        cache_write: optional(write).ok()?,
        tiered,
    })
}

fn parse_model(entry: &Value) -> Option<CatalogModel> {
    let id = entry["id"].as_str()?.to_owned();
    Some(CatalogModel {
        language: entry["type"].as_str() == Some("language"),
        price: price_of(
            &entry["pricing"],
            "input",
            "output",
            "input_cache_read",
            "input_cache_write",
        ),
        id,
    })
}

fn parse_endpoint(entry: &Value) -> Option<Endpoint> {
    Some(Endpoint {
        provider: entry["provider_name"].as_str()?.to_owned(),
        active: entry["status"].as_i64() == Some(0),
        price: price_of(
            &entry["pricing"],
            "prompt",
            "completion",
            "input_cache_read",
            "input_cache_write",
        ),
    })
}

/// A syntactically safe generation ID.
pub fn valid_generation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_GENERATION_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

#[derive(Debug, PartialEq, Eq)]
pub enum StreamEvent {
    Generation(String),
}

/// Incremental parser for the server-sent event stream.
pub struct Stream {
    buffer: Vec<u8>,
    consumed: usize,
    max_bytes: usize,
    pub generation: Option<String>,
    content: String,
    finish_reason: Option<String>,
    refused: bool,
    pub usage: Option<Tokens>,
    pub done: bool,
    pub failure: Option<Failure>,
}

impl Stream {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            consumed: 0,
            max_bytes,
            generation: None,
            content: String::new(),
            finish_reason: None,
            refused: false,
            usage: None,
            done: false,
            failure: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        self.consumed += bytes.len();
        if self.consumed > self.max_bytes {
            self.failure.get_or_insert(Failure {
                kind: FailureKind::Output,
                category: "response_too_large",
            });
            return events;
        }
        self.buffer.extend_from_slice(bytes);
        while let Some(newline) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if self.done || self.failure.is_some() {
                continue;
            }
            let Ok(line) = std::str::from_utf8(&line) else {
                self.failure = Some(Failure {
                    kind: FailureKind::Transient,
                    category: "malformed_stream",
                });
                continue;
            };
            if let Some(event) = self.line(line.trim_end_matches(['\r', '\n'])) {
                events.push(event);
            }
        }
        events
    }

    fn line(&mut self, line: &str) -> Option<StreamEvent> {
        let data = line.strip_prefix("data:")?.trim();
        if data == "[DONE]" {
            self.done = true;
            return None;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            self.failure = Some(Failure {
                kind: FailureKind::Transient,
                category: "malformed_stream",
            });
            return None;
        };
        if chunk.get("error").is_some_and(|e| !e.is_null()) {
            // A provider error event inside the stream; usage stays unknown unless reported.
            self.failure = Some(Failure {
                kind: FailureKind::Transient,
                category: "stream_error",
            });
            return None;
        }
        let mut event = None;
        if self.generation.is_none()
            && let Some(id) = chunk["id"].as_str()
            && valid_generation_id(id)
        {
            self.generation = Some(id.to_owned());
            event = Some(StreamEvent::Generation(id.to_owned()));
        }
        if let Some(choices) = chunk["choices"].as_array() {
            for choice in choices {
                if let Some(text) = choice["delta"]["content"].as_str() {
                    self.content.push_str(text);
                }
                if choice["delta"]["refusal"]
                    .as_str()
                    .is_some_and(|r| !r.is_empty())
                {
                    self.refused = true;
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    self.finish_reason = Some(reason.to_owned());
                }
            }
        }
        if let Some(tokens) = parse_usage(&chunk["usage"]) {
            self.usage = Some(tokens);
        }
        event
    }

    /// The assembled content when the stream completed normally (`[DONE]` after a `stop`).
    pub fn finish(&mut self) -> Result<String, Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if !self.done {
            return Err(Failure {
                kind: FailureKind::Transient,
                category: "stream_interrupted",
            });
        }
        if self.refused || self.finish_reason.as_deref() != Some("stop") {
            return Err(Failure {
                kind: FailureKind::Output,
                category: "incomplete_output",
            });
        }
        Ok(std::mem::take(&mut self.content))
    }
}

/// Measured usage from a chunk's `usage` object. Complete only when the prompt, completion, cache
/// read and cache creation counts are all reported explicitly: a missing cache count is unknown,
/// not zero, and is then resolved by generation lookup instead of being assumed.
pub fn parse_usage(usage: &Value) -> Option<Tokens> {
    let prompt = usage["prompt_tokens"].as_u64()?;
    let completion = usage["completion_tokens"].as_u64()?;
    let details = &usage["prompt_tokens_details"];
    let cached = details["cached_tokens"].as_u64()?;
    let created = match (
        usage["cache_creation_input_tokens"].as_u64(),
        details["cache_creation_input_tokens"].as_u64(),
    ) {
        (Some(a), Some(b)) if a != b => return None,
        (Some(a), _) | (None, Some(a)) => a,
        (None, None) => return None,
    };
    Some(Tokens {
        input: split_prompt(prompt, cached, created)?,
        output: completion,
        cache_read: cached,
        cache_write: created,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(chunks: &[&str]) -> (Stream, Result<String, Failure>) {
        let mut stream = Stream::new(1 << 20);
        for chunk in chunks {
            stream.feed(chunk.as_bytes());
        }
        let result = stream.finish();
        (stream, result)
    }

    const OK: [&str; 4] = [
        "data: {\"id\":\"gen_01ARZ3NDEKTSV4RRFFQ69G5FAV\",\"choices\":[{\"delta\":{\"content\":\"{\\\"a\\\"\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"gen_01ARZ3NDEKTSV4RRFFQ69G5FAV\",\"choices\":[{\"delta\":{\"content\":\":1}\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":30},\"cache_creation_input_tokens\":10}}\n\n",
        "data: [DONE]\n\n",
    ];

    #[test]
    fn complete_stream_yields_content_generation_and_usage() {
        let (stream, result) = run(&OK);
        assert_eq!(result.unwrap(), "{\"a\":1}");
        assert_eq!(
            stream.generation.as_deref(),
            Some("gen_01ARZ3NDEKTSV4RRFFQ69G5FAV")
        );
        assert_eq!(
            stream.usage,
            Some(Tokens {
                input: 60,
                output: 20,
                cache_read: 30,
                cache_write: 10
            })
        );
    }

    #[test]
    fn chunk_boundaries_do_not_matter() {
        let joined: String = OK.concat();
        let mut stream = Stream::new(1 << 20);
        let mut captured = Vec::new();
        for byte in joined.as_bytes().chunks(7) {
            captured.extend(stream.feed(byte));
        }
        assert_eq!(captured.len(), 1, "the generation is reported once");
        assert_eq!(stream.finish().unwrap(), "{\"a\":1}");
    }

    #[test]
    fn crlf_and_comment_lines_are_tolerated() {
        let (_, result) = run(&[
            ": keepalive\r\n\r\n",
            "data: {\"id\":\"g1\",\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        ]);
        assert_eq!(result.unwrap(), "x");
    }

    #[test]
    fn incomplete_streams_are_transient_and_keep_whatever_usage_was_seen() {
        let (stream, result) = run(&OK[..3]);
        assert_eq!(result.unwrap_err().kind, FailureKind::Transient);
        assert!(stream.usage.is_some());
        let (_, result) = run(&[]);
        assert_eq!(result.unwrap_err().category, "stream_interrupted");
    }

    #[test]
    fn truncated_refused_or_filtered_output_is_not_retried() {
        for reason in ["length", "content_filter", "tool_calls"] {
            let chunk = format!(
                "data: {{\"id\":\"g1\",\"choices\":[{{\"delta\":{{\"content\":\"x\"}},\"finish_reason\":\"{reason}\"}}]}}\n\ndata: [DONE]\n\n"
            );
            let (_, result) = run(&[&chunk]);
            assert_eq!(result.unwrap_err().kind, FailureKind::Output, "{reason}");
        }
        let refusal = "data: {\"id\":\"g1\",\"choices\":[{\"delta\":{\"refusal\":\"no\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let (_, result) = run(&[refusal]);
        assert_eq!(result.unwrap_err().kind, FailureKind::Output);
    }

    #[test]
    fn provider_error_events_and_garbage_are_transient() {
        let (_, result) = run(&["data: {\"error\":{\"message\":\"overloaded\"}}\n\n"]);
        assert_eq!(result.unwrap_err().category, "stream_error");
        let (_, result) = run(&["data: not json\n\n"]);
        assert_eq!(result.unwrap_err().category, "malformed_stream");
    }

    #[test]
    fn oversized_streams_are_cut_off_as_output_failures() {
        let mut stream = Stream::new(64);
        stream.feed(&[b'x'; 100]);
        assert_eq!(stream.finish().unwrap_err().kind, FailureKind::Output);
    }

    #[test]
    fn unsafe_generation_ids_are_not_captured() {
        let (stream, _) = run(&["data: {\"id\":\"has space\",\"choices\":[]}\n\n"]);
        assert!(stream.generation.is_none());
        assert!(!valid_generation_id(&"x".repeat(201)));
        assert!(valid_generation_id("chatcmpl-123"));
    }

    #[test]
    fn usage_requires_every_count_and_consistent_totals() {
        let full = json!({"prompt_tokens":10,"completion_tokens":5,
            "prompt_tokens_details":{"cached_tokens":4,"cache_creation_input_tokens":1}});
        assert_eq!(
            parse_usage(&full),
            Some(Tokens {
                input: 5,
                output: 5,
                cache_read: 4,
                cache_write: 1
            })
        );
        // Missing cache details or creation count: unknown, never assumed zero.
        assert_eq!(
            parse_usage(&json!({"prompt_tokens":10,"completion_tokens":5})),
            None
        );
        assert_eq!(
            parse_usage(&json!({"prompt_tokens":10,"completion_tokens":5,
                "prompt_tokens_details":{"cached_tokens":4}})),
            None
        );
        // Cached tokens above the prompt total are inconsistent.
        assert_eq!(
            parse_usage(&json!({"prompt_tokens":3,"completion_tokens":5,
                "prompt_tokens_details":{"cached_tokens":4,"cache_creation_input_tokens":0}})),
            None
        );
        // Conflicting creation counts are rejected.
        assert_eq!(
            parse_usage(&json!({"prompt_tokens":10,"completion_tokens":5,
                "cache_creation_input_tokens":1,
                "prompt_tokens_details":{"cached_tokens":0,"cache_creation_input_tokens":2}})),
            None
        );
        // Token counts must be non-negative integers.
        assert_eq!(
            parse_usage(&json!({"prompt_tokens":-1,"completion_tokens":5,
                "prompt_tokens_details":{"cached_tokens":0,"cache_creation_input_tokens":0}})),
            None
        );
    }

    #[test]
    fn status_classification_separates_zero_unknown_and_retryable() {
        for status in [425, 429] {
            let (failure, usage) = classify_status(status);
            assert_eq!((failure.kind, usage), (FailureKind::Transient, Usage::Zero));
        }
        // A Gateway timeout may follow a forwarded provider request, so it is not provably unsent.
        for status in [408, 500, 502, 503, 504, 599] {
            let (failure, usage) = classify_status(status);
            assert_eq!(
                (failure.kind, usage),
                (FailureKind::Transient, Usage::Unknown)
            );
        }
        for status in [400, 401, 403, 404, 422] {
            let (failure, usage) = classify_status(status);
            assert_eq!((failure.kind, usage), (FailureKind::Permanent, Usage::Zero));
        }
    }

    #[test]
    fn request_body_pins_the_route_and_carries_exact_schema_text() {
        let schema = r#"{"type":"object","properties":{"x":{"type":"number","enum":[0.1000000000000000055511151231257827]}}}"#;
        let request = ChatRequest {
            model: "openai/gpt-6-luna",
            route: "openai",
            user: "018f0000-0000-7000-8000-000000000001",
            system: "sys \"quoted\"",
            user_message: "{\"sourceText\":\"hi\"}",
            schema_name: "n",
            schema,
        };
        let body = Gateway::chat_body(&request);
        let parsed: Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(parsed["model"], "openai/gpt-6-luna");
        assert_eq!(parsed["stream"], true);
        assert_eq!(parsed["stream_options"]["include_usage"], true);
        assert_eq!(parsed["user"], "018f0000-0000-7000-8000-000000000001");
        assert_eq!(parsed["response_format"]["type"], "json_schema");
        assert_eq!(parsed["response_format"]["json_schema"]["strict"], true);
        assert_eq!(
            parsed["providerOptions"]["gateway"]["only"],
            json!(["openai"])
        );
        assert_eq!(
            parsed["providerOptions"]["gateway"]["order"],
            json!(["openai"])
        );
        assert!(
            parsed.get("models").is_none(),
            "no model fallback is requested"
        );
        assert!(body.contains("0.1000000000000000055511151231257827"));
        let unrouted = Gateway::chat_body(&ChatRequest {
            route: "",
            ..request
        });
        assert!(!unrouted.contains("providerOptions"));
    }

    #[test]
    fn catalog_entries_parse_to_canonical_prices() {
        let model = parse_model(&json!({
            "id":"openai/gpt-6-luna","type":"language",
            "pricing":{"input":"0.0000001","output":"5e-7","input_cache_read":"0.00000001"}
        }))
        .unwrap();
        let price = model.price.unwrap();
        assert_eq!(price.input, "0.0000001");
        assert_eq!(price.output, "0.0000005");
        assert_eq!(price.cache_read.as_deref(), Some("0.00000001"));
        assert_eq!(price.cache_write, None);
        assert!(!price.tiered);
        assert!(model.language);
        let tiered = parse_model(&json!({"id":"a/b","type":"language",
            "pricing":{"input":"1","output":"2","input_tiers":[{"cost":"3","min":0}]}}))
        .unwrap();
        assert!(tiered.price.unwrap().tiered);
        assert!(
            parse_model(&json!({"id":"a/b","type":"embedding","pricing":{"input":"1"}}))
                .unwrap()
                .price
                .is_none()
        );
        assert!(
            parse_model(&json!({"id":"a/b","pricing":{"input":"-1","output":"2"}}))
                .unwrap()
                .price
                .is_none()
        );
        // An unpublished cache category (absent or null) is "no price"; a present but invalid
        // one makes the whole entry unusable instead of silently becoming "unpublished".
        let cache = |read: Value, write: Value| {
            let pricing =
                json!({"input":"1","output":"2","input_cache_read":read,"input_cache_write":write});
            parse_model(&json!({"id":"a/b","type":"language","pricing":pricing}))
                .unwrap()
                .price
        };
        let published = cache(json!("0.5"), json!(null)).unwrap();
        assert_eq!(published.cache_read.as_deref(), Some("0.5"));
        assert_eq!(published.cache_write, None);
        for malformed in [
            json!("-1"),
            json!("abc"),
            json!(""),
            json!(0.5),
            json!("1".repeat(80)),
        ] {
            assert!(
                cache(malformed.clone(), json!(null)).is_none(),
                "{malformed}"
            );
            assert!(
                cache(json!(null), malformed.clone()).is_none(),
                "{malformed}"
            );
        }
        let endpoint_cache = parse_endpoint(&json!({"provider_name":"openai","status":0,
            "pricing":{"prompt":"1","completion":"2","input_cache_read":"-1"}}))
        .unwrap();
        assert!(endpoint_cache.price.is_none());
        let endpoint = parse_endpoint(&json!({"provider_name":"openai","status":0,
            "pricing":{"prompt":"0.0000001","completion":"0.0000005"}}))
        .unwrap();
        assert!(endpoint.active);
        assert_eq!(endpoint.price.unwrap().output, "0.0000005");
    }

    /// A server that accepts connections and then stalls, optionally after partial headers and
    /// body bytes. Returns the base URL; the task runs until the test ends.
    async fn stalled_server(reply: &'static [u8]) -> String {
        use tokio::{io::AsyncWriteExt, net::TcpListener};
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((mut socket, _)) = listener.accept().await {
                if !reply.is_empty() {
                    let _ = socket.write_all(reply).await;
                }
                // Keep the socket open and silent.
                held.push(socket);
            }
        });
        base
    }

    #[tokio::test]
    async fn stalled_non_execution_requests_time_out() {
        let limit = Duration::from_millis(300);
        for reply in [
            &b""[..],
            // Headers promise a body that never arrives.
            &b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"data\":"[..],
        ] {
            let base = stalled_server(reply).await;
            let gateway = Gateway::new(&base, "key", 1 << 20, limit).unwrap();
            let started = std::time::Instant::now();
            let all = async {
                assert!(gateway.models().await.is_err());
                assert!(gateway.endpoints("openai/gpt-6-luna").await.is_err());
                assert!(gateway.generation("gen_1").await.is_err());
            };
            tokio::time::timeout(Duration::from_secs(10), all)
                .await
                .expect("stalled requests are bounded by the configured timeout");
            assert!(started.elapsed() < Duration::from_secs(5));
        }
    }
}
