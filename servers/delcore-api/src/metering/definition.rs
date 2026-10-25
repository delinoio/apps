// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{MeteringError, Result};
use crate::accounting::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    String,
    Number,
    Boolean,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Value {
    pub kind: ValueKind,
    pub text: String,
}
impl Value {
    pub fn string(s: impl Into<String>) -> Self {
        Self {
            kind: ValueKind::String,
            text: s.into(),
        }
    }
    pub fn boolean(v: bool) -> Self {
        Self {
            kind: ValueKind::Boolean,
            text: v.to_string(),
        }
    }
    pub fn number(s: &str) -> Result<Self> {
        let d: Decimal = s
            .parse()
            .map_err(|e| MeteringError::Event(format!("number: {e}")))?;
        Ok(Self {
            kind: ValueKind::Number,
            text: d.to_string(),
        })
    }
    pub fn canonical(&self) -> Result<Self> {
        match self.kind {
            ValueKind::String => Ok(self.clone()),
            ValueKind::Number => Self::number(&self.text),
            ValueKind::Boolean if self.text == "true" || self.text == "false" => Ok(self.clone()),
            _ => Err(MeteringError::Event("invalid boolean".into())),
        }
    }
    pub fn decimal(&self) -> Result<Decimal> {
        if self.kind != ValueKind::Number {
            return Err(MeteringError::Event("numeric attribute required".into()));
        }
        self.text
            .parse()
            .map_err(|e| MeteringError::Event(format!("number: {e}")))
    }
}
/// JSON must be decoded with serde_json's arbitrary_precision feature to preserve numbers.
pub fn parse_value(value: &serde_json::Value) -> Result<Value> {
    match value {
        serde_json::Value::String(s) => Ok(Value::string(s)),
        serde_json::Value::Bool(b) => Ok(Value::boolean(*b)),
        serde_json::Value::Number(n) => Value::number(&n.to_string()),
        _ => Err(MeteringError::Event(
            "attribute must be a string, boolean, or exact number".into(),
        )),
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Count,
    Sum,
    Min,
    Max,
    Avg,
    UniqueCount,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOperator {
    Equal,
    NotEqual,
    In,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Exists,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub attribute: String,
    pub operator: FilterOperator,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub values: Vec<Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub key: String,
    pub event_type: String,
    pub aggregation: Aggregation,
    #[serde(default)]
    pub value_attribute: String,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
}
impl Definition {
    pub fn validate(&self) -> Result<()> {
        let invalid = |s: &str| MeteringError::Definition(s.into());
        if self.key.is_empty() || self.event_type.is_empty() {
            return Err(invalid("key and event type are required"));
        }
        if (self.aggregation == Aggregation::Count) != self.value_attribute.is_empty() {
            return Err(invalid("invalid value attribute for aggregation"));
        }
        let mut seen = BTreeSet::new();
        for name in &self.group_by {
            if name.is_empty() || !seen.insert(name) {
                return Err(invalid("invalid group attribute"));
            }
        }
        for f in &self.filters {
            if f.attribute.is_empty() {
                return Err(invalid("filter attribute is required"));
            }
            if f.operator == FilterOperator::In {
                if f.values.is_empty() || f.value.is_some() {
                    return Err(invalid("membership filter needs a value list"));
                }
                for v in &f.values {
                    v.canonical().map_err(|e| invalid(&e.to_string()))?;
                }
                continue;
            }
            if !f.values.is_empty() {
                return Err(invalid("scalar filter has a value list"));
            }
            let v = f
                .value
                .as_ref()
                .ok_or_else(|| invalid("scalar filter needs a value"))?;
            v.canonical().map_err(|e| invalid(&e.to_string()))?;
            match f.operator {
                FilterOperator::Equal | FilterOperator::NotEqual => (),
                FilterOperator::Exists if v.kind == ValueKind::Boolean => (),
                FilterOperator::GreaterThan
                | FilterOperator::GreaterThanOrEqual
                | FilterOperator::LessThan
                | FilterOperator::LessThanOrEqual
                    if v.kind == ValueKind::Number => {}
                _ => return Err(invalid("invalid filter operand type")),
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub event_type: String,
    pub attributes: BTreeMap<String, Value>,
}
impl Event {
    pub fn canonical(&self) -> Result<Self> {
        if self.event_type.is_empty() {
            return Err(MeteringError::Event("event type is required".into()));
        }
        let mut attributes = BTreeMap::new();
        for (name, value) in &self.attributes {
            if name.is_empty() {
                return Err(MeteringError::Event("attribute name is required".into()));
            }
            attributes.insert(name.clone(), value.canonical()?);
        }
        Ok(Self {
            event_type: self.event_type.clone(),
            attributes,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    pub meter_key: String,
    pub group_key: String,
    pub group_values: BTreeMap<String, Value>,
    pub value: Option<Value>,
}
pub fn match_event(definitions: &[Definition], event: &Event) -> Result<Vec<Contribution>> {
    match_validated(&validate_definitions(definitions)?, event)
}
pub(super) fn validate_definitions(definitions: &[Definition]) -> Result<Vec<Definition>> {
    let mut out = definitions.to_vec();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    let mut seen = BTreeSet::new();
    for d in &out {
        d.validate()?;
        if !seen.insert(&d.key) {
            return Err(MeteringError::Definition(format!(
                "duplicate meter key {:?}",
                d.key
            )));
        }
    }
    Ok(out)
}
pub(super) fn match_validated(
    definitions: &[Definition],
    event: &Event,
) -> Result<Vec<Contribution>> {
    let e = event.canonical()?;
    let mut matches = Vec::new();
    for d in definitions {
        if d.event_type != e.event_type || !matches_filters(&d.filters, &e.attributes) {
            continue;
        }
        let mut group_values = BTreeMap::new();
        for name in &d.group_by {
            let value = e.attributes.get(name).ok_or_else(|| {
                MeteringError::Event(format!(
                    "meter {:?} missing group attribute {name:?}",
                    d.key
                ))
            })?;
            group_values.insert(name.clone(), value.clone());
        }
        let value = if d.aggregation == Aggregation::Count {
            None
        } else {
            let value = e.attributes.get(&d.value_attribute).ok_or_else(|| {
                MeteringError::Event(format!(
                    "meter {:?} missing value attribute {:?}",
                    d.key, d.value_attribute
                ))
            })?;
            if d.aggregation != Aggregation::UniqueCount && value.kind != ValueKind::Number {
                return Err(MeteringError::Event(format!(
                    "meter {:?} needs a numeric value",
                    d.key
                )));
            }
            Some(value.clone())
        };
        matches.push(Contribution {
            meter_key: d.key.clone(),
            group_key: group_key(&group_values)?,
            group_values,
            value,
        });
    }
    if matches.is_empty() {
        return Err(MeteringError::NoMatch);
    }
    Ok(matches)
}
fn matches_filters(filters: &[Filter], attrs: &BTreeMap<String, Value>) -> bool {
    filters.iter().all(|f| {
        let value = attrs.get(&f.attribute);
        if f.operator == FilterOperator::Exists {
            return value.is_some() == f.value.as_ref().is_some_and(|v| v.text == "true");
        }
        let Some(value) = value else {
            return false;
        };
        if f.operator == FilterOperator::In {
            return f
                .values
                .iter()
                .any(|member| member.canonical().as_ref() == Ok(value));
        }
        let Some(wanted) = f.value.as_ref() else {
            return false;
        };
        if matches!(f.operator, FilterOperator::Equal | FilterOperator::NotEqual) {
            return (wanted.canonical().as_ref() == Ok(value))
                == (f.operator == FilterOperator::Equal);
        }
        let (Ok(left), Ok(right)) = (value.decimal(), wanted.decimal()) else {
            return false;
        };
        match f.operator {
            FilterOperator::GreaterThan => left > right,
            FilterOperator::GreaterThanOrEqual => left >= right,
            FilterOperator::LessThan => left < right,
            FilterOperator::LessThanOrEqual => left <= right,
            _ => false,
        }
    })
}
/// Encode canonical group values in stable name order.
pub fn group_key(values: &BTreeMap<String, Value>) -> Result<String> {
    #[derive(Serialize)]
    struct Part<'a> {
        name: &'a str,
        kind: ValueKind,
        value: String,
    }
    let mut parts = Vec::new();
    for (name, value) in values {
        if name.is_empty() {
            return Err(MeteringError::Event(
                "group attribute name is required".into(),
            ));
        }
        let v = value.canonical()?;
        parts.push(Part {
            name,
            kind: v.kind,
            value: v.text,
        });
    }
    serde_json::to_string(&parts).map_err(|e| MeteringError::Event(e.to_string()))
}

macro_rules! string_enum {
    ($type:ty, $($variant:ident => $name:literal),+ $(,)?) => {
        impl $type {
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $name),+ } }
        }
        impl std::fmt::Display for $type {
            fn fmt(&self,f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
        impl std::str::FromStr for $type {
            type Err = MeteringError;
            fn from_str(s: &str) -> Result<Self> {
                match s { $($name => Ok(Self::$variant)),+, _ => Err(MeteringError::Definition(format!("unknown {}: {s}", stringify!($type)))) }
            }
        }
    }
}
string_enum!(ValueKind, String => "string", Number => "number", Boolean => "boolean");
string_enum!(Aggregation, Count => "count", Sum => "sum", Min => "min", Max => "max", Avg => "avg", UniqueCount => "unique_count");
string_enum!(FilterOperator, Equal => "equal", NotEqual => "not_equal", In => "in", GreaterThan => "greater_than", GreaterThanOrEqual => "greater_than_or_equal", LessThan => "less_than", LessThanOrEqual => "less_than_or_equal", Exists => "exists");
string_enum!(super::RevisionKind, Insert => "insert", Replace => "replace", Cancel => "cancel");
