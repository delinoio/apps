// SPDX-License-Identifier: FSL-1.1-ALv2

//! Preserve scalar spelling and YAML types: parsing decimals through a floating
//! point representation would silently accept noncanonical filter operands.
use anyhow::{Result, bail, ensure};
use std::collections::BTreeMap;
use yaml_rust2::{
    parser::{Event, EventReceiver, Parser},
    scanner::TScalarStyle,
};

#[derive(Clone, Debug, PartialEq)]
pub enum ScalarKind {
    String,
    Number,
    Boolean,
    Null,
    Other,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Scalar(ScalarKind, String),
    Sequence(Vec<Node>),
    Mapping(BTreeMap<String, Node>),
}
#[derive(Default)]
struct Events(Vec<Event>);
impl EventReceiver for Events {
    fn on_event(&mut self, event: Event) {
        self.0.push(event);
    }
}

pub fn parse(input: &str) -> Result<Node> {
    let mut events = Events::default();
    Parser::new_from_str(input).load(&mut events, true)?;
    ensure!(
        events
            .0
            .iter()
            .filter(|e| matches!(e, Event::DocumentStart))
            .count()
            == 1,
        "expected exactly one entity"
    );
    let mut at = events
        .0
        .iter()
        .position(|e| matches!(e, Event::DocumentStart))
        .unwrap()
        + 1;
    parse_node(&events.0, &mut at, &mut BTreeMap::new(), 0)
}
fn parse_node(
    events: &[Event],
    at: &mut usize,
    anchors: &mut BTreeMap<usize, Node>,
    depth: usize,
) -> Result<Node> {
    ensure!(depth < 100, "YAML nesting exceeds limit");
    let event = events
        .get(*at)
        .ok_or_else(|| anyhow::anyhow!("incomplete YAML"))?;
    *at += 1;
    let (node, anchor) = match event {
        Event::Scalar(value, style, anchor, tag) => {
            let kind = if let Some(tag) = tag {
                match tag.suffix.as_str() {
                    "str" => ScalarKind::String,
                    "int" | "float" => ScalarKind::Number,
                    "bool" => ScalarKind::Boolean,
                    "null" => ScalarKind::Null,
                    _ => ScalarKind::Other,
                }
            } else if *style != TScalarStyle::Plain {
                ScalarKind::String
            } else {
                match value.as_str() {
                    "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => ScalarKind::Boolean,
                    "" | "~" | "null" | "Null" | "NULL" => ScalarKind::Null,
                    _ if value.replace('_', "").parse::<f64>().is_ok()
                        || value.starts_with("0x")
                        || value.starts_with("0o")
                        || value.starts_with("0b")
                        || matches!(
                            value.to_ascii_lowercase().as_str(),
                            ".inf" | "-.inf" | "+.inf" | ".nan"
                        ) =>
                    {
                        ScalarKind::Number
                    }
                    _ if chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
                        || chrono::DateTime::parse_from_rfc3339(value).is_ok() =>
                    {
                        ScalarKind::Other
                    }
                    _ => ScalarKind::String,
                }
            };
            (Node::Scalar(kind, value.clone()), *anchor)
        }
        Event::SequenceStart(anchor, _) => {
            let mut values = Vec::new();
            while !matches!(events.get(*at), Some(Event::SequenceEnd)) {
                values.push(parse_node(events, at, anchors, depth + 1)?);
            }
            *at += 1;
            (Node::Sequence(values), *anchor)
        }
        Event::MappingStart(anchor, _) => {
            let mut values = BTreeMap::new();
            while !matches!(events.get(*at), Some(Event::MappingEnd)) {
                let key = parse_node(events, at, anchors, depth + 1)?
                    .text()?
                    .to_owned();
                let value = parse_node(events, at, anchors, depth + 1)?;
                ensure!(
                    values.insert(key.clone(), value).is_none(),
                    "unknown or duplicate {key} field"
                );
            }
            *at += 1;
            (Node::Mapping(values), *anchor)
        }
        Event::Alias(anchor) => {
            return anchors
                .get(anchor)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("recursive or missing YAML alias"));
        }
        _ => bail!("expected YAML value"),
    };
    if anchor != 0 {
        anchors.insert(anchor, node.clone());
    }
    Ok(node)
}
impl Node {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Scalar(ScalarKind::Null, _))
    }
    pub fn integer(&self) -> Result<i64> {
        let Self::Scalar(ScalarKind::Number, value) = self else {
            bail!("expected integer")
        };
        let value = value.replace('_', "");
        let (negative, unsigned) = if let Some(s) = value.strip_prefix('-') {
            (true, s)
        } else {
            (false, value.strip_prefix('+').unwrap_or(&value))
        };
        let (radix, digits) = if let Some(s) = unsigned.strip_prefix("0x") {
            (16, s)
        } else if let Some(s) = unsigned.strip_prefix("0o") {
            (8, s)
        } else if let Some(s) = unsigned.strip_prefix("0b") {
            (2, s)
        } else if unsigned.len() > 1 && unsigned.starts_with('0') {
            (8, &unsigned[1..])
        } else {
            (10, unsigned)
        };
        let parsed = i128::from_str_radix(digits, radix)?;
        Ok(i64::try_from(if negative { -parsed } else { parsed })?)
    }

    pub fn text(&self) -> Result<&str> {
        match self {
            Node::Scalar(ScalarKind::String | ScalarKind::Number | ScalarKind::Boolean, s) => Ok(s),
            _ => bail!("expected scalar text"),
        }
    }
    pub fn mapping(&self) -> Result<&BTreeMap<String, Node>> {
        match self {
            Node::Mapping(m) => Ok(m),
            _ => bail!("expected an entity mapping"),
        }
    }
    pub fn sequence(&self) -> Result<&[Node]> {
        match self {
            Node::Sequence(v) => Ok(v),
            Node::Scalar(ScalarKind::Null, _) => Ok(&[]),
            _ => bail!("expected sequence"),
        }
    }
    pub fn boolean(&self) -> Result<bool> {
        match self {
            Node::Scalar(ScalarKind::Boolean, s) => match s.to_ascii_lowercase().as_str() {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => bail!("invalid boolean"),
            },
            _ => bail!("expected boolean"),
        }
    }
}
