// SPDX-License-Identifier: FSL-1.1-ALv2

//! Evaluate immutable grants without reading database state.
use crate::{accounting::Decimal, metering};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FeatureKind {
    Unspecified = 0,
    Access = 1,
    Metered = 2,
    Configuration = 3,
}
impl From<i32> for FeatureKind {
    fn from(value: i32) -> Self {
        match value {
            1 => Self::Access,
            2 => Self::Metered,
            3 => Self::Configuration,
            _ => Self::Unspecified,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum BlockReason {
    Unspecified = 0,
    Renewal = 3,
    FeatureUnknown = 7,
    FeatureNotEntitled = 8,
    FeatureLimitReached = 9,
}
#[derive(Clone, Debug)]
pub struct Grant {
    pub kind: FeatureKind,
    pub enabled: bool,
    pub limit: Decimal,
    pub configuration: Option<Value>,
}

pub fn decode_grant(kind: FeatureKind, value_type: i32, raw: &Value) -> Result<Grant> {
    let fields = raw.as_object().filter(|v| v.len() == 1).ok_or_else(|| {
        anyhow::anyhow!("invalid stored feature grant: grant must contain exactly one value")
    })?;
    let mut grant = Grant {
        kind,
        enabled: false,
        limit: Decimal::zero(),
        configuration: None,
    };
    let mismatch = || {
        anyhow::anyhow!(
            "invalid stored feature grant: configuration value does not match its declared type"
        )
    };
    match kind {
        FeatureKind::Access => {
            grant.enabled = match fields.get("enabled") {
                Some(v) => v.as_bool().ok_or_else(mismatch)?,
                None => return Err(mismatch()),
            };
        }
        FeatureKind::Metered => {
            let text = fields
                .get("limit")
                .and_then(Value::as_str)
                .ok_or_else(mismatch)?;
            grant.limit = Decimal::parse(text)?;
            if grant.limit < Decimal::zero() {
                bail!(
                    "invalid stored feature grant: metered grant needs a nonnegative decimal limit"
                );
            }
        }
        FeatureKind::Configuration => {
            grant.configuration = Some(match value_type {
                1 => {
                    let v = fields.get("boolean").ok_or_else(mismatch)?;
                    json!({"boolean":v.as_bool().ok_or_else(mismatch)?})
                }
                2 => {
                    let v = fields.get("integer").ok_or_else(mismatch)?;
                    json!({"integer":v.as_i64().ok_or_else(mismatch)?.to_string()})
                }
                3 => {
                    let v = fields
                        .get("decimal")
                        .and_then(Value::as_str)
                        .ok_or_else(mismatch)?;
                    json!({"decimal":Decimal::parse(v)?.to_string()})
                }
                4 => {
                    let v = fields.get("text").ok_or_else(mismatch)?;
                    json!({"text":v.as_str().ok_or_else(mismatch)?})
                }
                5 => {
                    let v = fields.get("string_list").ok_or_else(mismatch)?;
                    let values: Vec<String> =
                        serde_json::from_value(v.clone()).map_err(|_| mismatch())?;
                    json!({"stringList":{"values":values}})
                }
                _ => bail!("invalid stored feature grant: unknown configuration type"),
            });
        }
        FeatureKind::Unspecified => bail!("invalid stored feature grant: unknown feature kind"),
    }
    Ok(grant)
}

#[derive(Default, Debug, Deserialize)]
#[serde(default)]
pub struct MeterDefinition {
    pub event_type: String,
    pub aggregation: i32,
    pub group_by: Vec<String>,
}
pub fn decode_meter(raw: Option<&Value>) -> Result<MeterDefinition> {
    match raw {
        None | Some(Value::Null) => bail!("missing meter definition"),
        Some(raw) => Ok(serde_json::from_value(raw.clone())?),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decision {
    pub entitled: bool,
    pub reason: BlockReason,
}
pub fn evaluate(grants: &[Grant], applied: &Decimal) -> Decision {
    let blocked = Decision {
        entitled: false,
        reason: BlockReason::FeatureNotEntitled,
    };
    let [grant] = grants else {
        return blocked;
    };
    match grant.kind {
        FeatureKind::Access if !grant.enabled => blocked,
        FeatureKind::Unspecified => blocked,
        FeatureKind::Metered if applied >= &grant.limit => Decision {
            entitled: true,
            reason: BlockReason::FeatureLimitReached,
        },
        _ => Decision {
            entitled: true,
            reason: BlockReason::Unspecified,
        },
    }
}

pub fn group_key(
    group_by: &[String],
    values: &BTreeMap<String, metering::Value>,
) -> Result<String> {
    if values.len() != group_by.len() {
        bail!("invalid feature group values: supply exactly the meter group-by attributes");
    }
    for name in group_by {
        let value = values.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "invalid feature group values: supply exactly the meter group-by attributes"
            )
        })?;
        match value.kind {
            metering::ValueKind::String
                if value.text.len() > crate::events::MAX_ATTRIBUTE_VALUE_BYTES =>
            {
                bail!(
                    "invalid feature group values: string group values contain at most 1024 bytes"
                )
            }
            metering::ValueKind::Boolean if value.text != "true" && value.text != "false" => {
                bail!("invalid feature group values: invalid boolean group value")
            }
            metering::ValueKind::Number
                if value.text.len() > crate::events::MAX_NUMBER_BYTES
                    || metering::Value::number(&value.text)?.text != value.text =>
            {
                bail!(
                    "invalid feature group values: numeric group values must be canonical decimals"
                );
            }
            _ => {}
        }
    }
    Ok(metering::group_key(values)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grants_enforce_declared_types_and_exact_limits() {
        assert!(
            decode_grant(FeatureKind::Access, 0, &json!({"enabled":true}))
                .unwrap()
                .enabled
        );
        for (kind, raw) in [
            (FeatureKind::Access, json!({"enabled":"yes"})),
            (FeatureKind::Access, json!({"enabled":true,"limit":"1"})),
            (FeatureKind::Metered, json!({"limit":"-1"})),
            (FeatureKind::Metered, json!({"limit":1})),
            (FeatureKind::Unspecified, json!({"enabled":true})),
        ] {
            assert!(decode_grant(kind, 0, &raw).is_err());
        }
        let g = decode_grant(FeatureKind::Metered, 0, &json!({"limit":"10.50"})).unwrap();
        assert_eq!(g.limit.to_string(), "10.5");
        for (kind, raw, expected) in [
            (1, json!({"boolean":true}), json!({"boolean":true})),
            (2, json!({"integer":-30}), json!({"integer":"-30"})),
            (3, json!({"decimal":"2.50"}), json!({"decimal":"2.5"})),
            (4, json!({"text":"eu"}), json!({"text":"eu"})),
            (
                5,
                json!({"string_list":["a","b"]}),
                json!({"stringList":{"values":["a","b"]}}),
            ),
            (
                5,
                json!({"string_list":[]}),
                json!({"stringList":{"values":[]}}),
            ),
        ] {
            assert_eq!(
                decode_grant(FeatureKind::Configuration, kind, &raw)
                    .unwrap()
                    .configuration,
                Some(expected)
            );
        }
        for (kind, raw) in [
            (1, json!({"integer":1})),
            (2, json!({"integer":1.5})),
            (3, json!({"decimal":"1e3"})),
            (4, json!({"text":1})),
            (5, json!({"string_list":"a"})),
            (0, json!({"text":"a"})),
        ] {
            assert!(decode_grant(FeatureKind::Configuration, kind, &raw).is_err());
        }
    }
    #[test]
    fn authorization_fails_closed_and_compares_exact_applied_usage() {
        let g = decode_grant(FeatureKind::Metered, 0, &json!({"limit":"10"})).unwrap();
        assert!(!evaluate(&[], &Decimal::zero()).entitled);
        assert!(!evaluate(&[g.clone(), g.clone()], &Decimal::zero()).entitled);
        for (usage, reason) in [
            ("9.999999", BlockReason::Unspecified),
            ("-20", BlockReason::Unspecified),
            ("10.000", BlockReason::FeatureLimitReached),
            ("11", BlockReason::FeatureLimitReached),
        ] {
            assert_eq!(
                evaluate(std::slice::from_ref(&g), &Decimal::parse(usage).unwrap()),
                Decision {
                    entitled: true,
                    reason
                }
            );
        }
        let disabled = decode_grant(FeatureKind::Access, 0, &json!({"enabled":false})).unwrap();
        assert!(!evaluate(&[disabled], &Decimal::zero()).entitled);
    }
    #[test]
    fn ungrouped_catalog_meter_authorizes_with_empty_group_by() {
        let definition = decode_meter(Some(&json!({
            "event_type": "use", "aggregation": 2, "group_by": []
        })))
        .unwrap();
        assert_eq!(definition.event_type, "use");
        assert_eq!(definition.aggregation, 2);
        assert!(definition.group_by.is_empty());
        assert_eq!(
            group_key(&definition.group_by, &BTreeMap::new()).unwrap(),
            "[]"
        );
        let grant = decode_grant(FeatureKind::Metered, 0, &json!({"limit":"10"})).unwrap();
        assert_eq!(
            evaluate(&[grant], &Decimal::zero()),
            Decision {
                entitled: true,
                reason: BlockReason::Unspecified
            }
        );
        assert!(decode_meter(Some(&json!({"group_by": "region"}))).is_err());
    }
    #[test]
    fn groups_keep_scalar_types_and_validate_exact_names() {
        let names = vec!["region".into(), "tier".into()];
        let mut values = BTreeMap::from([
            ("region".into(), metering::Value::string("eu")),
            ("tier".into(), metering::Value::number("5").unwrap()),
        ]);
        let numeric = group_key(&names, &values).unwrap();
        assert_eq!(numeric, metering::group_key(&values).unwrap());
        values.insert("tier".into(), metering::Value::string("5"));
        assert_ne!(numeric, group_key(&names, &values).unwrap());
        values.insert(
            "tier".into(),
            metering::Value {
                kind: metering::ValueKind::Number,
                text: "5.0".into(),
            },
        );
        assert!(group_key(&names, &values).is_err());
        values.remove("tier");
        assert!(group_key(&names, &values).is_err());
        assert_eq!(group_key(&[], &BTreeMap::new()).unwrap(), "[]");
        assert!(decode_meter(None).is_err());
        assert_eq!(
            decode_meter(Some(&json!({"group_by":["region"],"aggregation":2})))
                .unwrap()
                .aggregation,
            2
        );
    }
}
