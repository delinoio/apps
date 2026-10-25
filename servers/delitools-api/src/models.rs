// SPDX-License-Identifier: FSL-1.1-ALv2
//! Operator model allowlist and the mapping from models to generic Delcore meters.
//!
//! The allowlist file is JSON (`.json`) or YAML (any other extension):
//!
//! ```yaml
//! models:
//!   - model: openai/gpt-6-luna      # Vercel AI Gateway model identifier
//!     input_meter_key: luna_input   # Delcore meter key charged for input tokens
//!     output_meter_key: luna_output # Delcore meter key charged for output tokens
//! ```
//!
//! Delcore stays generic: a meter key is the event type of an event with a numeric `quantity`
//! attribute, as declared in the catalog. DeliTools never sets or reads prices
//! except to confirm that the input and output meters of a model have an active usage price.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The only model used by extraction. It must also appear in the allowlist file.
pub const EXTRACTION_MODEL: &str = "openai/gpt-6-luna";

const MAX_MODELS: usize = 200;
const MAX_MODEL_BYTES: usize = 200;
const MAX_METER_BYTES: usize = 100;

/// One allowed model and the meters that bill its token directions.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    pub model: String,
    pub input_meter_key: String,
    pub output_meter_key: String,
}

/// The validated allowlist.
#[derive(Clone, Debug, Default)]
pub struct ModelCatalog {
    entries: BTreeMap<String, ModelEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    models: Vec<ModelEntry>,
}

fn valid_model(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_MODEL_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

fn valid_meter(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_METER_BYTES
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

impl ModelCatalog {
    /// Parses and validates an allowlist document.
    pub fn parse(text: &str, json: bool) -> Result<Self> {
        let value: Value = if json {
            serde_json::from_str(text).context("the file is not valid JSON")?
        } else {
            yaml_to_json(text)?
        };
        let file: File = serde_json::from_value(value).context("unexpected file structure")?;
        Self::from_entries(file.models)
    }

    pub fn from_entries(models: Vec<ModelEntry>) -> Result<Self> {
        ensure!(!models.is_empty(), "at least one model is required");
        ensure!(
            models.len() <= MAX_MODELS,
            "at most {MAX_MODELS} models are allowed"
        );
        let mut entries = BTreeMap::new();
        let mut meters = std::collections::HashSet::new();
        for entry in models {
            ensure!(valid_model(&entry.model), "a model identifier is invalid");
            ensure!(
                valid_meter(&entry.input_meter_key) && valid_meter(&entry.output_meter_key),
                "a meter key is invalid for model {}",
                entry.model
            );
            ensure!(
                entry.input_meter_key != entry.output_meter_key,
                "input and output meters must differ for model {}",
                entry.model
            );
            ensure!(
                meters.insert(entry.input_meter_key.clone())
                    && meters.insert(entry.output_meter_key.clone()),
                "meter keys must not be shared between models (model {})",
                entry.model
            );
            ensure!(
                entries.insert(entry.model.clone(), entry).is_none(),
                "a model is listed twice"
            );
        }
        Ok(Self { entries })
    }

    /// Extraction is fixed to one model, so the allowlist must map it.
    pub fn require_extraction_model(&self) -> Result<()> {
        ensure!(
            self.entries.contains_key(EXTRACTION_MODEL),
            "the extraction model {EXTRACTION_MODEL} must be listed"
        );
        Ok(())
    }

    pub fn get(&self, model: &str) -> Option<&ModelEntry> {
        self.entries.get(model)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ModelEntry> {
        self.entries.values()
    }
}

/// Converts the restricted YAML subset used by the allowlist (maps, sequences and scalars).
fn yaml_to_json(text: &str) -> Result<Value> {
    use yaml_rust2::{Yaml, YamlLoader};
    fn convert(node: &Yaml) -> Result<Value> {
        Ok(match node {
            Yaml::String(s) => Value::String(s.clone()),
            Yaml::Integer(i) => Value::from(*i),
            Yaml::Boolean(b) => Value::Bool(*b),
            Yaml::Array(items) => Value::Array(items.iter().map(convert).collect::<Result<_>>()?),
            Yaml::Hash(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| match k {
                        Yaml::String(k) => Ok((k.clone(), convert(v)?)),
                        _ => bail!("map keys must be strings"),
                    })
                    .collect::<Result<_>>()?,
            ),
            _ => bail!("unsupported YAML value"),
        })
    }
    let mut documents = YamlLoader::load_from_str(text).context("the file is not valid YAML")?;
    ensure!(
        documents.len() == 1,
        "exactly one YAML document is required"
    );
    convert(&documents.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"models":[
        {"model":"openai/gpt-6-luna","input_meter_key":"luna_in","output_meter_key":"luna_out"},
        {"model":"anthropic/claude-x","input_meter_key":"x_in","output_meter_key":"x_out"}]}"#;

    #[test]
    fn parses_json_and_yaml() {
        let json = ModelCatalog::parse(GOOD, true).unwrap();
        assert_eq!(json.len(), 2);
        let yaml = ModelCatalog::parse(
            "models:\n  - model: openai/gpt-6-luna\n    input_meter_key: luna_in\n    output_meter_key: luna_out\n",
            false,
        )
        .unwrap();
        assert_eq!(
            yaml.get(EXTRACTION_MODEL).unwrap().input_meter_key,
            "luna_in"
        );
        assert!(yaml.get("other").is_none());
        json.require_extraction_model().unwrap();
    }

    #[test]
    fn rejects_invalid_documents() {
        let bad = [
            r#"{"models":[]}"#,
            r#"{"models":[{"model":"a b","input_meter_key":"i","output_meter_key":"o"}]}"#,
            r#"{"models":[{"model":"m","input_meter_key":"","output_meter_key":"o"}]}"#,
            r#"{"models":[{"model":"m","input_meter_key":"same","output_meter_key":"same"}]}"#,
            r#"{"models":[{"model":"m","input_meter_key":"i","output_meter_key":"o","price":"1"}]}"#,
            r#"{"models":[{"model":"m","input_meter_key":"i","output_meter_key":"o"},{"model":"m","input_meter_key":"i2","output_meter_key":"o2"}]}"#,
            r#"{"models":[{"model":"m","input_meter_key":"i","output_meter_key":"o"},{"model":"n","input_meter_key":"i","output_meter_key":"o2"}]}"#,
            r#"{"other":[]}"#,
            "not json",
        ];
        for text in bad {
            assert!(ModelCatalog::parse(text, true).is_err(), "{text}");
        }
        assert!(ModelCatalog::parse("models: [", false).is_err());
        assert!(ModelCatalog::parse("a: 1\n---\nb: 2\n", false).is_err());
    }

    #[test]
    fn extraction_model_is_required() {
        let catalog = ModelCatalog::parse(
            r#"{"models":[{"model":"m","input_meter_key":"i","output_meter_key":"o"}]}"#,
            true,
        )
        .unwrap();
        assert!(catalog.require_extraction_model().is_err());
    }
}
