// SPDX-License-Identifier: FSL-1.1-ALv2
//! The supported JSON Schema subset for `ExtractDataFromText`, the provider response envelope and
//! server-side output validation.
//!
//! The subset is the intersection of what strict structured output accepts and what DeliTools can
//! verify exactly. It is documented in the server contract; this module is its implementation.
//! A schema is validated completely before any AI call. Unsupported keywords, combinators, `$ref`
//! (so recursion), optional properties, open objects, nesting beyond the limit and provider budget
//! overruns are rejected with an English message that names the schema location but never repeats
//! schema content beyond property names.
use crate::{decimal::Exact, json::Json};
use std::collections::{BTreeMap, BTreeSet};

/// Caller schema nesting limit. The root object is level 1 and each nested object or array adds
/// one level; scalar nodes sit inside their container and never add a level.
pub const MAX_SCHEMA_LEVELS: usize = 8;
/// Provider nesting levels reserved for the service's response envelope. The envelope uses one;
/// the second stays reserved so the envelope can grow without changing the caller limit.
pub const RESERVED_ENVELOPE_LEVELS: usize = 2;
/// Provider property budget and the two envelope properties.
const MAX_PROPERTIES: usize = 5000;
const ENVELOPE_PROPERTIES: usize = 2;
const MAX_ENUM_VALUES: usize = 1000;
/// The envelope `status` enum holds `extracted` and `impossible`.
const ENVELOPE_ENUM_VALUES: usize = 2;
const MAX_NAME_AND_ENUM_CHARS: usize = 120_000;
/// Characters the envelope contributes: the property names `status` and `data` and the enum
/// values `extracted` and `impossible`.
const ENVELOPE_CHARS: usize =
    "status".len() + "data".len() + STATUS_EXTRACTED.len() + STATUS_IMPOSSIBLE.len();
const LARGE_ENUM_VALUES: usize = 250;
const LARGE_ENUM_STRING_CHARS: usize = 15_000;
/// Raw JSON nesting accepted while parsing (schema levels need two JSON levels each).
const MAX_JSON_DEPTH: usize = 64;
/// Longest accepted property name.
const MAX_PROPERTY_NAME_CHARS: usize = 256;

pub const STATUS_EXTRACTED: &str = "extracted";
pub const STATUS_IMPOSSIBLE: &str = "impossible";

const ALLOWED_KEYWORDS: [&str; 7] = [
    "type",
    "description",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
}

impl Kind {
    fn parse(name: &str) -> Option<Kind> {
        Some(match name {
            "object" => Kind::Object,
            "array" => Kind::Array,
            "string" => Kind::String,
            "number" => Kind::Number,
            "integer" => Kind::Integer,
            "boolean" => Kind::Boolean,
            "null" => Kind::Null,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    properties: BTreeMap<String, Node>,
    items: Option<Box<Node>>,
    enum_values: Option<Vec<Json>>,
}

/// A schema accepted by the supported subset.
#[derive(Clone, Debug)]
pub struct Schema {
    root: Node,
    /// The accepted schema in compact JSON, with exact number lexemes, embedded in the envelope.
    compact: String,
}

/// Why a schema was rejected. The message is safe to return to the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError(pub String);

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn reject<T>(path: &str, why: &str) -> Result<T, SchemaError> {
    let mut location: String = path.chars().take(200).collect();
    if location.is_empty() {
        location.push('/');
    }
    Err(SchemaError(format!(
        "unsupported schema at {location}: {why}"
    )))
}

#[derive(Default)]
struct Budget {
    properties: usize,
    enum_values: usize,
    chars: usize,
}

impl Schema {
    /// Parse and validate a caller schema.
    pub fn parse(raw: &str) -> Result<Schema, SchemaError> {
        let json = Json::parse(raw, MAX_JSON_DEPTH).map_err(|_| {
            SchemaError("jsonSchema must be one valid JSON document without duplicate keys".into())
        })?;
        let Json::Object(root) = &json else {
            return reject("", "the schema must be a JSON object");
        };
        if root.get("type").and_then(Json::as_str) != Some("object") {
            return reject("", "the root type must be \"object\"");
        }
        let mut budget = Budget {
            properties: ENVELOPE_PROPERTIES,
            enum_values: ENVELOPE_ENUM_VALUES,
            chars: ENVELOPE_CHARS,
        };
        let node = build(&json, "", 1, &mut budget)?;
        if budget.properties > MAX_PROPERTIES
            || budget.enum_values > MAX_ENUM_VALUES
            || budget.chars > MAX_NAME_AND_ENUM_CHARS
        {
            return reject(
                "",
                "the schema exceeds the provider limits for properties, enum values or name characters",
            );
        }
        Ok(Schema {
            root: node,
            compact: json.serialize(),
        })
    }

    /// The strict response-format schema sent to the provider: `status` selects between a
    /// grounded extraction and the extraction-impossible sentinel, and `data` carries the caller
    /// schema.
    pub fn envelope(&self) -> String {
        format!(
            "{{\"type\":\"object\",\"properties\":{{\"status\":{{\"type\":\"string\",\"enum\":[\"{STATUS_EXTRACTED}\",\"{STATUS_IMPOSSIBLE}\"]}},\"data\":{}}},\"required\":[\"status\",\"data\"],\"additionalProperties\":false}}",
            self.compact
        )
    }

    /// Interpret the provider's structured content.
    pub fn read_output(&self, content: &str) -> Output {
        let Ok(Json::Object(envelope)) = Json::parse(content, MAX_JSON_DEPTH) else {
            return Output::Malformed;
        };
        if envelope.len() != 2 {
            return Output::Malformed;
        }
        let (Some(status), Some(data)) = (
            envelope.get("status").and_then(Json::as_str),
            envelope.get("data"),
        ) else {
            return Output::Malformed;
        };
        match status {
            // The data of an impossible extraction is placeholder content that is discarded
            // unread; it only has to be well-formed JSON.
            STATUS_IMPOSSIBLE => Output::Impossible,
            STATUS_EXTRACTED if conforms(&self.root, data) => Output::Extracted(data.serialize()),
            _ => Output::Malformed,
        }
    }

    /// Whether a value satisfies the schema (used by tests and fixtures).
    pub fn accepts(&self, value: &Json) -> bool {
        conforms(&self.root, value)
    }
}

/// Result of reading the provider content for an extraction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Validated data serialized as compact JSON.
    Extracted(String),
    /// The model reported that required information is missing from the source.
    Impossible,
    /// Not parseable, wrong envelope shape or nonconforming data.
    Malformed,
}

/// Build one schema node. `level` is the level this node would occupy if it is an object or an
/// array; a scalar node is accepted below a container at the limit.
fn build(value: &Json, path: &str, level: usize, budget: &mut Budget) -> Result<Node, SchemaError> {
    let Json::Object(map) = value else {
        return reject(path, "a schema must be a JSON object");
    };
    for keyword in map.keys() {
        if !ALLOWED_KEYWORDS.contains(&keyword.as_str()) {
            return reject(path, &format!("keyword {keyword:?} is not supported"));
        }
    }
    if let Some(description) = map.get("description")
        && description.as_str().is_none()
    {
        return reject(path, "description must be a string");
    }
    let Some(kind) = map.get("type").and_then(Json::as_str).and_then(Kind::parse) else {
        return reject(
            path,
            "type must be one of object, array, string, number, integer, boolean or null",
        );
    };
    if matches!(kind, Kind::Object | Kind::Array) && level > MAX_SCHEMA_LEVELS {
        return reject(path, &format!("nesting exceeds {MAX_SCHEMA_LEVELS} levels"));
    }
    if kind != Kind::Object {
        for keyword in ["properties", "required", "additionalProperties"] {
            if map.contains_key(keyword) {
                return reject(path, &format!("{keyword} is only valid on objects"));
            }
        }
    }
    if kind != Kind::Array && map.contains_key("items") {
        return reject(path, "items is only valid on arrays");
    }
    let mut node = Node {
        kind,
        properties: BTreeMap::new(),
        items: None,
        enum_values: None,
    };
    match kind {
        Kind::Object => object(map, path, level, budget, &mut node)?,
        Kind::Array => {
            let Some(items) = map.get("items") else {
                return reject(path, "arrays require items");
            };
            node.items = Some(Box::new(build(
                items,
                &format!("{path}/items"),
                level + 1,
                budget,
            )?));
        }
        _ => {}
    }
    if let Some(values) = map.get("enum") {
        node.enum_values = Some(enumeration(values, kind, path, budget)?);
    }
    Ok(node)
}

fn object(
    map: &BTreeMap<String, Json>,
    path: &str,
    level: usize,
    budget: &mut Budget,
    node: &mut Node,
) -> Result<(), SchemaError> {
    let Some(Json::Object(properties)) = map.get("properties") else {
        return reject(path, "objects require properties");
    };
    if map.get("additionalProperties") != Some(&Json::Bool(false)) {
        return reject(path, "objects require additionalProperties: false");
    }
    let Some(Json::Array(required)) = map.get("required") else {
        return reject(path, "objects require a required list");
    };
    let mut listed = BTreeSet::new();
    for name in required {
        let Some(name) = name.as_str() else {
            return reject(path, "required entries must be strings");
        };
        if !listed.insert(name) {
            return reject(path, "required lists a property twice");
        }
        if !properties.contains_key(name) {
            return reject(path, "required names an undefined property");
        }
    }
    if listed.len() != properties.len() {
        return reject(path, "every property must be listed in required");
    }
    budget.properties += properties.len();
    if budget.properties > MAX_PROPERTIES {
        return reject(path, "the schema exceeds the provider property limit");
    }
    for (name, child) in properties {
        let characters = name.chars().count();
        if characters == 0 || characters > MAX_PROPERTY_NAME_CHARS {
            return reject(path, "property names contain 1 to 256 characters");
        }
        budget.chars += characters;
        node.properties.insert(
            name.clone(),
            build(
                child,
                &format!("{path}/properties/{name}"),
                level + 1,
                budget,
            )?,
        );
    }
    Ok(())
}

fn enumeration(
    values: &Json,
    kind: Kind,
    path: &str,
    budget: &mut Budget,
) -> Result<Vec<Json>, SchemaError> {
    let Json::Array(values) = values else {
        return reject(path, "enum must be an array");
    };
    if matches!(kind, Kind::Object | Kind::Array) {
        return reject(path, "enum is only valid on scalar types");
    }
    if values.is_empty() {
        return reject(path, "enum must not be empty");
    }
    // Bound the quadratic uniqueness check below before running it.
    if values.len() > MAX_ENUM_VALUES {
        return reject(path, "the schema exceeds the provider enum value limit");
    }
    let mut string_chars = 0;
    for value in values {
        if !scalar_of_kind(kind, value) {
            return reject(path, "enum values must match the declared type");
        }
        match value {
            Json::String(text) => string_chars += text.chars().count(),
            other => budget.chars += other.serialize().len(),
        }
    }
    for (index, value) in values.iter().enumerate() {
        if values[..index].iter().any(|seen| same_value(seen, value)) {
            return reject(path, "enum values must be unique");
        }
    }
    budget.enum_values += values.len();
    budget.chars += string_chars;
    if values.len() > LARGE_ENUM_VALUES && string_chars > LARGE_ENUM_STRING_CHARS {
        return reject(
            path,
            "an enum with more than 250 values is limited to 15000 string characters",
        );
    }
    if budget.enum_values > MAX_ENUM_VALUES {
        return reject(path, "the schema exceeds the provider enum value limit");
    }
    Ok(values.clone())
}

fn scalar_of_kind(kind: Kind, value: &Json) -> bool {
    match (kind, value) {
        (Kind::String, Json::String(_))
        | (Kind::Boolean, Json::Bool(_))
        | (Kind::Null, Json::Null) => true,
        (Kind::Number, Json::Number(lexeme)) => Exact::parse(lexeme).is_some(),
        (Kind::Integer, Json::Number(lexeme)) => {
            Exact::parse(lexeme).is_some_and(|number| number.is_integer())
        }
        _ => false,
    }
}

/// Equality for enum members: numbers compare as exact decimals, everything else structurally.
fn same_value(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => match (Exact::parse(x), Exact::parse(y)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
        _ => a == b,
    }
}

fn conforms(node: &Node, value: &Json) -> bool {
    let type_ok = match (node.kind, value) {
        (Kind::Object, Json::Object(_))
        | (Kind::Array, Json::Array(_))
        | (Kind::String, Json::String(_))
        | (Kind::Boolean, Json::Bool(_))
        | (Kind::Null, Json::Null) => true,
        (Kind::Number, Json::Number(lexeme)) => Exact::parse(lexeme).is_some(),
        (Kind::Integer, Json::Number(lexeme)) => {
            Exact::parse(lexeme).is_some_and(|number| number.is_integer())
        }
        _ => false,
    };
    if !type_ok {
        return false;
    }
    if let Some(values) = &node.enum_values
        && !values.iter().any(|allowed| same_value(allowed, value))
    {
        return false;
    }
    match value {
        Json::Object(map) => {
            map.len() == node.properties.len()
                && node
                    .properties
                    .iter()
                    .all(|(name, child)| map.get(name).is_some_and(|v| conforms(child, v)))
        }
        Json::Array(items) => node
            .items
            .as_ref()
            .is_some_and(|child| items.iter().all(|item| conforms(child, item))),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(schema: &str) -> Schema {
        Schema::parse(schema).unwrap_or_else(|e| panic!("{schema}: {e}"))
    }
    fn bad(schema: &str) -> String {
        Schema::parse(schema).expect_err(schema).to_string()
    }

    /// Supported fixtures documented in the server contract.
    pub const SUPPORTED: &[&str] = &[
        r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}"#,
        r#"{"type":"object","description":"invoice","properties":{"id":{"type":"integer","description":"number"},"total":{"type":"number"},"paid":{"type":"boolean"},"note":{"type":"null"}},"required":["id","total","paid","note"],"additionalProperties":false}"#,
        r#"{"type":"object","properties":{"kind":{"type":"string","enum":["a","b"]},"level":{"type":"integer","enum":[1,2,3]},"flag":{"type":"boolean","enum":[true]}},"required":["kind","level","flag"],"additionalProperties":false}"#,
        r#"{"type":"object","properties":{"lines":{"type":"array","items":{"type":"object","properties":{"sku":{"type":"string"},"qty":{"type":"integer"}},"required":["sku","qty"],"additionalProperties":false}}},"required":["lines"],"additionalProperties":false}"#,
        r#"{"type":"object","properties":{},"required":[],"additionalProperties":false}"#,
    ];

    /// Unsupported fixtures with the reason fragment each must report.
    pub const UNSUPPORTED: &[(&str, &str)] = &[
        (r#"[]"#, "JSON object"),
        (r#"{"type":"array","items":{"type":"string"}}"#, "root type"),
        (r#"{"type":"string"}"#, "root type"),
        (
            r#"{"type":["object","null"],"properties":{},"required":[],"additionalProperties":false}"#,
            "root type",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["a"]}"#,
            "additionalProperties",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":true}"#,
            "additionalProperties",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a"],"additionalProperties":false}"#,
            "every property",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["a","a"],"additionalProperties":false}"#,
            "twice",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"}},"required":["z"],"additionalProperties":false}"#,
            "undefined property",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","minLength":1}},"required":["a"],"additionalProperties":false}"#,
            "minLength",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","pattern":"^a"}},"required":["a"],"additionalProperties":false}"#,
            "pattern",
        ),
        (
            r##"{"type":"object","properties":{"a":{"$ref":"#/$defs/a"}},"required":["a"],"additionalProperties":false,"$defs":{}}"##,
            "$defs",
        ),
        (
            r##"{"type":"object","properties":{"a":{"$ref":"#"}},"required":["a"],"additionalProperties":false}"##,
            "$ref",
        ),
        (
            r#"{"type":"object","properties":{"a":{"anyOf":[{"type":"string"}]}},"required":["a"],"additionalProperties":false}"#,
            "anyOf",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","const":"x"}},"required":["a"],"additionalProperties":false}"#,
            "const",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"array"}},"required":["a"],"additionalProperties":false}"#,
            "items",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","items":{"type":"string"}}},"required":["a"],"additionalProperties":false}"#,
            "items is only valid",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","properties":{}}},"required":["a"],"additionalProperties":false}"#,
            "only valid on objects",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","enum":[]}},"required":["a"],"additionalProperties":false}"#,
            "empty",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","enum":["x","x"]}},"required":["a"],"additionalProperties":false}"#,
            "unique",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"integer","enum":[1.5]}},"required":["a"],"additionalProperties":false}"#,
            "declared type",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","enum":[1]}},"required":["a"],"additionalProperties":false}"#,
            "declared type",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"number","enum":[1,1.0]}},"required":["a"],"additionalProperties":false}"#,
            "unique",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"object","enum":[{}],"properties":{},"required":[],"additionalProperties":false}},"required":["a"],"additionalProperties":false}"#,
            "scalar",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"strng"}},"required":["a"],"additionalProperties":false}"#,
            "type must be",
        ),
        (
            r#"{"type":"object","properties":{"":{"type":"string"}},"required":[""],"additionalProperties":false}"#,
            "property names",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string","description":1}},"required":["a"],"additionalProperties":false}"#,
            "description",
        ),
        (
            r#"{"type":"object","properties":{"a":{"type":"string"},"a":{"type":"string"}},"required":["a"],"additionalProperties":false}"#,
            "duplicate keys",
        ),
        ("not json", "valid JSON"),
    ];

    #[test]
    fn supported_fixtures_are_accepted() {
        for schema in SUPPORTED {
            ok(schema);
        }
    }

    #[test]
    fn unsupported_fixtures_are_rejected_with_a_reason() {
        for (schema, fragment) in UNSUPPORTED {
            let message = bad(schema);
            assert!(
                message.contains(fragment),
                "{schema}: expected {fragment:?} in {message:?}"
            );
        }
    }

    #[test]
    fn nesting_limit_is_eight_container_levels() {
        // `containers` nested objects (the root is the first) around one scalar leaf.
        let build = |containers: usize| {
            let mut inner = r#"{"type":"string"}"#.to_owned();
            for _ in 0..containers {
                inner = format!(
                    r#"{{"type":"object","properties":{{"a":{inner}}},"required":["a"],"additionalProperties":false}}"#
                );
            }
            inner
        };
        // The scalar leaf below the eighth container does not add a level.
        ok(&build(8));
        assert!(bad(&build(9)).contains("nesting exceeds 8 levels"));
        // Arrays count like objects: the root plus seven nested arrays is eight levels.
        let arrays = |nested: usize| {
            let mut inner = r#"{"type":"string"}"#.to_owned();
            for _ in 0..nested {
                inner = format!(r#"{{"type":"array","items":{inner}}}"#);
            }
            format!(
                r#"{{"type":"object","properties":{{"a":{inner}}},"required":["a"],"additionalProperties":false}}"#
            )
        };
        ok(&arrays(7));
        assert!(bad(&arrays(8)).contains("nesting exceeds 8 levels"));
    }

    #[test]
    fn provider_budgets_are_enforced() {
        let properties = |count: usize| {
            let props: Vec<String> = (0..count)
                .map(|i| format!(r#""p{i}":{{"type":"string"}}"#))
                .collect();
            let required: Vec<String> = (0..count).map(|i| format!(r#""p{i}""#)).collect();
            format!(
                r#"{{"type":"object","properties":{{{}}},"required":[{}],"additionalProperties":false}}"#,
                props.join(","),
                required.join(",")
            )
        };
        ok(&properties(4998));
        assert!(bad(&properties(4999)).contains("provider"));
        let enum_of = |count: usize, width: usize| {
            let values: Vec<String> = (0..count).map(|i| format!("\"{i:0>width$}\"")).collect();
            format!(
                r#"{{"type":"object","properties":{{"a":{{"type":"string","enum":[{}]}}}},"required":["a"],"additionalProperties":false}}"#,
                values.join(",")
            )
        };
        ok(&enum_of(250, 100));
        assert!(bad(&enum_of(251, 100)).contains("250 values"));
        ok(&enum_of(251, 10));
        assert!(bad(&enum_of(1001, 1)).contains("enum value limit"));
        // The envelope sends two enum values of its own, so the caller has 998 left.
        ok(&enum_of(998, 4));
        assert!(bad(&enum_of(999, 4)).contains("enum value limit"));
    }

    #[test]
    fn the_character_budget_reserves_every_envelope_string() {
        assert_eq!(ENVELOPE_CHARS, 29);
        // Property names of exactly `total` characters in 200-character pieces plus a remainder.
        let named = |total: usize| {
            let mut names: Vec<String> = (0..total / 200)
                .map(|i| format!("{i:04}{}", "n".repeat(196)))
                .collect();
            names.push(format!("r{}", "m".repeat(total % 200 - 1)));
            let props: Vec<String> = names
                .iter()
                .map(|name| format!(r#""{name}":{{"type":"string"}}"#))
                .collect();
            let required: Vec<String> = names.iter().map(|name| format!(r#""{name}""#)).collect();
            format!(
                r#"{{"type":"object","properties":{{{}}},"required":[{}],"additionalProperties":false}}"#,
                props.join(","),
                required.join(",")
            )
        };
        // 120,000 minus the 29 envelope characters is the caller's whole share.
        let limit = MAX_NAME_AND_ENUM_CHARS - ENVELOPE_CHARS;
        ok(&named(limit));
        assert!(bad(&named(limit + 1)).contains("provider limits"));
    }

    #[test]
    fn envelope_embeds_the_caller_schema_exactly() {
        let schema = ok(
            r#"{ "type":"object", "properties": {"x": {"type":"number","enum":[1.10,0.1000000000000000055511151231257827]}}, "required":["x"], "additionalProperties": false }"#,
        );
        let envelope = schema.envelope();
        assert!(envelope.contains("0.1000000000000000055511151231257827"));
        assert!(envelope.contains("1.10"));
        let parsed = Json::parse(&envelope, 64).unwrap();
        let object = parsed.as_object().unwrap();
        assert_eq!(object["required"].serialize(), r#"["status","data"]"#);
        assert_eq!(object["additionalProperties"], Json::Bool(false));
    }

    fn items_schema() -> Schema {
        ok(
            r#"{"type":"object","properties":{"name":{"type":"string"},"qty":{"type":"integer"},"price":{"type":"number"},"kind":{"type":"string","enum":["a","b"]},"tier":{"type":"number","enum":[1.5,2]},"tags":{"type":"array","items":{"type":"string"}},"none":{"type":"null"},"ok":{"type":"boolean"}},"required":["name","qty","price","kind","tier","tags","none","ok"],"additionalProperties":false}"#,
        )
    }
    const GOOD: &str = r#"{"name":"x","qty":2,"price":3.25,"kind":"a","tier":2.0,"tags":["t"],"none":null,"ok":true}"#;

    fn wrap(status: &str, data: &str) -> String {
        format!(r#"{{"status":"{status}","data":{data}}}"#)
    }

    #[test]
    fn conforming_output_is_extracted_with_exact_numbers() {
        let schema = items_schema();
        let data = r#"{"name":"x","qty":2,"price":3.250,"kind":"a","tier":2.0,"tags":["t"],"none":null,"ok":true}"#;
        let Output::Extracted(json) = schema.read_output(&wrap("extracted", data)) else {
            panic!("expected extraction");
        };
        // Keys are re-serialized in sorted order; numbers keep their exact spelling.
        assert!(json.contains("\"price\":3.250"));
        assert!(json.contains("\"tier\":2.0"));
        assert!(matches!(
            schema.read_output(&wrap("extracted", GOOD)),
            Output::Extracted(_)
        ));
    }

    #[test]
    fn nonconforming_output_is_malformed() {
        let schema = items_schema();
        let cases = [
            // wrong types
            GOOD.replace("\"qty\":2", "\"qty\":\"2\""),
            // integer must be whole, including exponent spellings
            GOOD.replace("\"qty\":2", "\"qty\":2.5"),
            GOOD.replace("\"qty\":2", "\"qty\":25e-1"),
            // enum miss and a float-near enum miss
            GOOD.replace("\"kind\":\"a\"", "\"kind\":\"c\""),
            GOOD.replace("\"tier\":2.0", "\"tier\":2.0000000000000001"),
            // missing and extra properties
            GOOD.replace("\"ok\":true", "\"okay\":true"),
            GOOD.replace("}", ",\"extra\":1}"),
            // array element type
            GOOD.replace("[\"t\"]", "[1]"),
            // null where a string is required
            GOOD.replace("\"name\":\"x\"", "\"name\":null"),
            // not an object
            "[]".to_owned(),
        ];
        for data in cases {
            assert_eq!(
                schema.read_output(&wrap("extracted", &data)),
                Output::Malformed,
                "{data}"
            );
        }
        for content in [
            "",
            "not json",
            "{}",
            r#"{"status":"extracted"}"#,
            r#"{"status":"maybe","data":{}}"#,
            r#"{"status":"extracted","data":{},"x":1}"#,
            &format!(r#"{{"status":"extracted","data":{GOOD}}}trailing"#),
            r#"{"status":"extracted","data":{"name":1,"name":2}}"#,
        ] {
            assert_eq!(schema.read_output(content), Output::Malformed, "{content}");
        }
    }

    #[test]
    fn impossible_status_is_the_missing_information_sentinel() {
        let schema = items_schema();
        // The placeholder data need not conform, only be JSON.
        assert_eq!(
            schema.read_output(&wrap("impossible", "{}")),
            Output::Impossible
        );
        assert_eq!(
            schema.read_output(&wrap("impossible", GOOD)),
            Output::Impossible
        );
    }

    #[test]
    fn huge_numbers_do_not_expand() {
        let schema = ok(
            r#"{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"],"additionalProperties":false}"#,
        );
        assert!(matches!(
            schema.read_output(&wrap("extracted", r#"{"n":1e999999}"#)),
            Output::Extracted(_)
        ));
        assert_eq!(
            schema.read_output(&wrap("extracted", r#"{"n":1e-999999}"#)),
            Output::Malformed
        );
        assert_eq!(
            schema.read_output(&wrap("extracted", r#"{"n":1e99999999999}"#)),
            Output::Malformed
        );
    }
}
