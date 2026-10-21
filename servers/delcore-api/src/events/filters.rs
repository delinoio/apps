// SPDX-License-Identifier: FSL-1.1-ALv2
use super::*;
use metering::{Aggregation, Definition, Filter, FilterOperator as Op, Value, ValueKind};
fn enum_number(v: &Json, prefix: &str, names: &[&str]) -> i32 {
    v.as_i64().map(|n| n as i32).unwrap_or_else(|| {
        v.as_str()
            .and_then(|s| {
                names
                    .iter()
                    .position(|x| s == format!("{prefix}{x}") || s == *x)
            })
            .map(|n| n as i32)
            .unwrap_or(0)
    })
}
pub fn domain_filter(filter: &Json) -> Result<Filter> {
    let op = enum_number(
        &filter["operator"],
        "METER_FILTER_OPERATOR_",
        &[
            "UNSPECIFIED",
            "EQUAL",
            "NOT_EQUAL",
            "IN",
            "GREATER",
            "GREATER_OR_EQUAL",
            "LESS",
            "LESS_OR_EQUAL",
            "EXISTS",
        ],
    );
    let operator = match op {
        1 => Op::Equal,
        2 => Op::NotEqual,
        3 => Op::In,
        4 => Op::GreaterThan,
        5 => Op::GreaterThanOrEqual,
        6 => Op::LessThan,
        7 => Op::LessThanOrEqual,
        8 => Op::Exists,
        _ => return Err(Fault::invalid("unknown meter filter operator")),
    };
    let untyped = filter["values"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let typed = filter
        .get("typed_values")
        .or_else(|| filter.get("typedValues"))
        .and_then(Json::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if !untyped.is_empty() && !typed.is_empty() {
        return Err(Fault::invalid(
            "a meter filter uses values or typed_values, not both",
        ));
    }
    let mut operands = Vec::new();
    for v in typed {
        let text = v["value"].as_str().unwrap_or("");
        let kind = enum_number(
            &v["kind"],
            "METER_FILTER_VALUE_KIND_",
            &["UNSPECIFIED", "STRING", "NUMBER", "BOOLEAN"],
        );
        operands.push(match kind {
            1 => Value::string(text),
            2 => Value::number(text)
                .map_err(|_| Fault::invalid("number filter values must be decimal strings"))?,
            3 if text == "true" || text == "false" => Value::boolean(text == "true"),
            _ => return Err(Fault::invalid("invalid typed meter filter value")),
        });
    }
    for v in untyped {
        let text = v
            .as_str()
            .ok_or_else(|| Fault::invalid("invalid meter filter value"))?;
        operands.push(match operator {
            Op::Equal | Op::NotEqual | Op::In => Value::string(text),
            Op::Exists if text == "true" || text == "false" => Value::boolean(text == "true"),
            Op::Exists => {
                return Err(Fault::invalid(
                    "exists meter filters take no value or one boolean value",
                ));
            }
            _ => Value::number(text)
                .map_err(|_| Fault::invalid("numeric meter filter requires a decimal value"))?,
        });
    }
    let mut item = Filter {
        attribute: filter["attribute"].as_str().unwrap_or("").into(),
        operator,
        value: None,
        values: vec![],
    };
    match operator {
        Op::In if !operands.is_empty() => item.values = operands,
        Op::Exists if operands.is_empty() => item.value = Some(Value::boolean(true)),
        Op::Equal | Op::NotEqual if operands.len() == 1 => item.value = operands.pop(),
        Op::Exists if operands.len() == 1 && operands[0].kind == ValueKind::Boolean => {
            item.value = operands.pop()
        }
        Op::GreaterThan | Op::GreaterThanOrEqual | Op::LessThan | Op::LessThanOrEqual
            if operands.len() == 1 && operands[0].kind == ValueKind::Number =>
        {
            item.value = operands.pop()
        }
        _ => return Err(Fault::invalid("invalid meter filter operands")),
    }
    Ok(item)
}
pub fn canonical_filters(filters: &[Json]) -> Result<Vec<Json>> {
    let mut out = Vec::new();
    for filter in filters {
        let item = domain_filter(filter)?;
        let op = match item.operator {
            Op::Equal => 1,
            Op::NotEqual => 2,
            Op::In => 3,
            Op::GreaterThan => 4,
            Op::GreaterThanOrEqual => 5,
            Op::LessThan => 6,
            Op::LessThanOrEqual => 7,
            Op::Exists => 8,
        };
        let mut stored = json!({"attribute":item.attribute,"operator":op});
        if matches!(item.operator, Op::Equal | Op::NotEqual | Op::In) {
            let mut operands = if item.operator == Op::In {
                item.values
            } else {
                vec![item.value.expect("validated operand")]
            };
            if operands.iter().all(|v| v.kind == ValueKind::String) {
                let mut values: Vec<_> = operands.into_iter().map(|v| v.text).collect();
                if item.operator == Op::In {
                    values.sort();
                }
                stored["values"] = json!(values);
            } else {
                let number = |k| match k {
                    ValueKind::String => 1,
                    ValueKind::Number => 2,
                    ValueKind::Boolean => 3,
                };
                if item.operator == Op::In {
                    operands
                        .sort_by(|a, b| (number(a.kind), &a.text).cmp(&(number(b.kind), &b.text)));
                }
                stored["typed_values"] = json!(
                    operands
                        .into_iter()
                        .map(|v| json!({"kind":number(v.kind),"value":v.text}))
                        .collect::<Vec<_>>()
                );
            }
        } else {
            stored["values"] = json!([item.value.expect("validated operand").text]);
        }
        out.push(stored);
    }
    out.sort_by_key(go_json);
    Ok(out)
}
/// Stored `value_source` numbers follow the protocol enum `MeterValueSource`.
pub const VALUE_SOURCE_AI_TOKEN_USD: i32 = 2;
pub fn value_source(number: i32) -> Result<metering::ValueSource> {
    match number {
        0 | 1 => Ok(metering::ValueSource::Attribute),
        VALUE_SOURCE_AI_TOKEN_USD => Ok(metering::ValueSource::AiTokenUsd),
        _ => Err(Fault::invalid("unknown meter value source")),
    }
}
/// Canonical text of a catalog markup factor.
pub fn canonical_markup(text: &str) -> Result<String> {
    crate::accounting::Decimal::parse(text)
        .map(|d| d.to_string())
        .map_err(|_| Fault::invalid("markup must be a canonical decimal of at least 1"))
}
#[allow(clippy::too_many_arguments)]
pub fn catalog_definition(
    key: &str,
    event_type: &str,
    aggregation: i32,
    value_attribute: &str,
    filters: &[Json],
    group_by: &[String],
    value_source_number: i32,
    markup: &str,
) -> Result<Definition> {
    let aggregation = match aggregation {
        1 => Aggregation::Count,
        2 => Aggregation::Sum,
        3 => Aggregation::Min,
        4 => Aggregation::Max,
        5 => Aggregation::Avg,
        6 => Aggregation::UniqueCount,
        _ => return Err(Fault::invalid("unknown meter aggregation")),
    };
    let d = Definition {
        key: key.into(),
        event_type: event_type.into(),
        aggregation,
        value_attribute: value_attribute.into(),
        filters: filters.iter().map(domain_filter).collect::<Result<_>>()?,
        group_by: group_by.to_vec(),
        value_source: value_source(value_source_number)?,
        markup: markup.into(),
    };
    d.validate().map_err(|e| Fault::invalid(e.to_string()))?;
    Ok(d)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_filters_preserve_scalar_types() {
        let number = canonical_filters(&[
            json!({"attribute":"x","operator":1,"typed_values":[{"kind":2,"value":"1.00"}]}),
        ])
        .unwrap();
        assert_eq!(number[0]["typed_values"][0]["value"], "1");
        let string = canonical_filters(&[
            json!({"attribute":"x","operator":1,"typed_values":[{"kind":1,"value":"1"}]}),
        ])
        .unwrap();
        assert_eq!(string[0]["values"], json!(["1"]));
        assert_ne!(number, string);
    }
    #[test]
    fn filter_order_matches_persisted_go_encoding() {
        let sorted = canonical_filters(&[
            json!({"attribute":"<","operator":8}),
            json!({"attribute":"A","operator":8}),
        ])
        .unwrap();
        assert_eq!(sorted[0]["attribute"], "A");
    }
    #[test]
    fn default_exists_and_numeric_canonicalization() {
        assert_eq!(
            canonical_filters(&[json!({"attribute":"x","operator":8})]).unwrap()[0]["values"],
            json!(["true"])
        );
        assert_eq!(
            canonical_filters(&[json!({"attribute":"x","operator":4,"values":["1.00"]})]).unwrap()
                [0]["values"],
            json!(["1"])
        );
        assert!(domain_filter(&json!({"attribute":"x","operator":1,"values":["1"],"typed_values":[{"kind":2,"value":"1"}]})).is_err());
    }
}
