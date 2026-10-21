//! Request and response types from the Clairic public JSON schemas.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Supported extraction and prediction types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Number,
    Date,
    Time,
    Datetime,
    Duration,
    List,
    Estimate,
    Forecast,
}

/// Element type of a list field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListItems {
    Text,
    Number,
    Date,
}

/// Estimate grid scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    Linear,
    Log,
}

/// Forecast interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interval {
    Step,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

/// Numeric bound or ISO date bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Bound {
    Number(f64),
    Date(String),
}

/// Calendar step for an estimate grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalendarStep {
    Month,
    Year,
}

/// Positive numeric step (days for dates), or calendar step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Number(f64),
    Calendar(CalendarStep),
}

/// A named field's specification. Cloudflare validates type-specific constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    #[serde(rename = "type")]
    pub kind: FieldType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integer: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<ListItems>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<Bound>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<Bound>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub null_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub horizon: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<Interval>,
}

impl Field {
    /// Create a field with no optional constraints.
    pub fn new(kind: FieldType) -> Self {
        Self {
            kind,
            instructions: None,
            unit: None,
            integer: None,
            decimals: None,
            items: None,
            min: None,
            max: None,
            step: None,
            scale: None,
            null_threshold: None,
            horizon: None,
            interval: None,
        }
    }
}

/// Embedded image media type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
}

/// Embedded image. Remote URLs are not supported by Cloudflare.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Image {
    DataUrl(String),
    Base64 {
        content_type: ImageType,
        base64: String,
    },
}

/// Optional response controls.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Options {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_quantiles: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_state_text: Option<bool>,
}

/// Model selector, fixed to Clairic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Model {
    #[default]
    #[serde(rename = "clairic", alias = "@cf/cloudflare/clairic")]
    Clairic,
}

/// Inference input. Supply text or embedded images and at least one field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: Model,
    pub state: String,
    pub fields: BTreeMap<String, Field>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Options>,
}

/// An answer retains arbitrary value shapes defined by the preview API.
/// Spans use character offsets; do not use them directly to index Rust strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    #[serde(rename = "type")]
    pub kind: FieldType,
    pub value: Value,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

/// Token counts. Optional counters may be absent from the upstream response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub state_tokens: Option<u64>,
    pub image_tokens: Option<u64>,
    pub schema_tokens: Option<u64>,
    pub passes: Option<u64>,
}

/// Model result, after decoding the Cloudflare REST envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub truncated: bool,
    pub state_text: Option<String>,
    pub usage: Usage,
}
