// SPDX-License-Identifier: FSL-1.1-ALv2

mod sync;
mod yaml;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
pub use sync::{SyncResult, sync};
use yaml::{Node, ScalarKind};

#[derive(Clone, Debug, Default)]
struct Reference {
    service: String,
    key: String,
}
#[derive(Clone, Debug, Default)]
struct Filter {
    attribute: String,
    operator: String,
    values: Vec<Node>,
}
#[derive(Clone, Debug, Default)]
struct Grant {
    service: String,
    key: String,
    enabled: Option<bool>,
    limit: Option<String>,
    configuration: Option<Node>,
}
#[derive(Clone, Debug, Default)]
struct Definition {
    kind: String,
    key: String,
    name: String,
    client_id: String,
    capabilities: Vec<String>,
    scopes: Vec<String>,
    service: String,
    event_type: String,
    aggregation: String,
    value_attribute: String,
    value_source: String,
    markup: String,
    filters: Vec<Filter>,
    group_by: Vec<String>,
    feature_type: String,
    value_type: String,
    meter: String,
    rank: Option<i32>,
    free: Option<bool>,
    default_free: Option<bool>,
    postpaid: Option<bool>,
    features: Vec<Grant>,
    plan: String,
    add_on: Option<Reference>,
    price_kind: String,
    unit_price: String,
    included: String,
    currency: String,
    external_price_id: String,
    path: PathBuf,
}
fn string(m: &BTreeMap<String, Node>, key: &str) -> Result<String> {
    m.get(key)
        .filter(|node| !node.is_null())
        .map(Node::text)
        .transpose()
        .map(|s| s.unwrap_or_default().to_owned())
}
fn strings(m: &BTreeMap<String, Node>, key: &str) -> Result<Vec<String>> {
    m.get(key)
        .map(|n| {
            n.sequence()?
                .iter()
                .map(|n| Ok(n.text()?.to_owned()))
                .collect()
        })
        .unwrap_or_else(|| Ok(vec![]))
}
fn boolean(m: &BTreeMap<String, Node>, key: &str) -> Result<Option<bool>> {
    m.get(key)
        .filter(|node| !node.is_null())
        .map(Node::boolean)
        .transpose()
}
fn fields(m: &BTreeMap<String, Node>, allowed: &str) -> Result<()> {
    for key in m.keys() {
        ensure!(
            allowed.split_whitespace().any(|a| a == key),
            "unknown {key} field"
        );
    }
    Ok(())
}
fn nodes<'a>(m: &'a BTreeMap<String, Node>, key: &str) -> Result<&'a [Node]> {
    m.get(key).map(Node::sequence).unwrap_or(Ok(&[]))
}
fn grant(node: &Node) -> Result<Grant> {
    let m = node.mapping()?;
    fields(m, "service key enabled limit configuration")?;
    Ok(Grant {
        service: string(m, "service")?,
        key: string(m, "key")?,
        enabled: boolean(m, "enabled")?,
        limit: m
            .get("limit")
            .filter(|node| !node.is_null())
            .map(|n| n.text().map(str::to_owned))
            .transpose()?,
        configuration: m.get("configuration").cloned(),
    })
}
fn decode(node: &Node) -> Result<Definition> {
    let m = node.mapping()?;
    let kind = string(m, "kind")?;
    let allowed = match kind.as_str() {
        "service" => "name client_id capabilities scopes",
        "meter" => {
            "service event_type aggregation value_attribute value_source markup filters group_by"
        }
        "feature" => "service name type value_type meter",
        "plan" => "name rank free default_free postpaid features",
        "addon" => "service name features",
        "price" => {
            "service plan add_on meter price_kind unit_price included currency external_price_id"
        }
        _ => bail!("unknown kind"),
    };
    fields(m, &format!("kind key {allowed}"))?;
    let filters = nodes(m, "filters")?
        .iter()
        .map(|node| {
            let f = node.mapping()?;
            fields(f, "attribute operator values")?;
            Ok(Filter {
                attribute: string(f, "attribute")?,
                operator: string(f, "operator")?,
                values: nodes(f, "values")?.to_vec(),
            })
        })
        .collect::<Result<_>>()?;
    let add_on = m
        .get("add_on")
        .filter(|node| !node.is_null())
        .map(|node| {
            let a = node.mapping()?;
            fields(a, "service key")?;
            Ok::<_, anyhow::Error>(Reference {
                service: string(a, "service")?,
                key: string(a, "key")?,
            })
        })
        .transpose()?;
    Ok(Definition {
        kind,
        key: string(m, "key")?,
        name: string(m, "name")?,
        client_id: string(m, "client_id")?,
        capabilities: strings(m, "capabilities")?,
        scopes: strings(m, "scopes")?,
        service: string(m, "service")?,
        event_type: string(m, "event_type")?,
        aggregation: string(m, "aggregation")?,
        value_attribute: string(m, "value_attribute")?,
        value_source: string(m, "value_source")?,
        markup: string(m, "markup")?,
        filters,
        group_by: strings(m, "group_by")?,
        feature_type: string(m, "type")?,
        value_type: string(m, "value_type")?,
        meter: string(m, "meter")?,
        rank: m
            .get("rank")
            .filter(|node| !node.is_null())
            .map(|v| Ok::<i32, anyhow::Error>(v.integer()?.try_into()?))
            .transpose()?,
        free: boolean(m, "free")?,
        default_free: boolean(m, "default_free")?,
        postpaid: boolean(m, "postpaid")?,
        features: nodes(m, "features")?
            .iter()
            .map(grant)
            .collect::<Result<_>>()?,
        plan: string(m, "plan")?,
        add_on,
        price_kind: string(m, "price_kind")?,
        unit_price: string(m, "unit_price")?,
        included: string(m, "included")?,
        currency: string(m, "currency")?,
        external_price_id: string(m, "external_price_id")?,
        path: PathBuf::new(),
    })
}
fn load(dir: &Path) -> Result<Vec<Definition>> {
    let cwd = std::env::current_dir()?;
    let dir = if dir == Path::new("servers/delcore-api/catalog")
        && cwd.file_name().is_some_and(|n| n == "delcore-api")
        && cwd
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "servers")
    {
        Path::new("catalog")
    } else {
        dir
    };
    ensure!(
        dir.is_dir(),
        "catalog directory is missing or not a directory"
    );
    fn walk(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), paths)?;
            } else if entry
                .path()
                .extension()
                .is_some_and(|e| e == "yaml" || e == "yml")
            {
                paths.push(entry.path());
            }
        }
        Ok(())
    }
    let mut paths = vec![];
    walk(dir, &mut paths).context("catalog discovery")?;
    ensure!(
        !paths.is_empty(),
        "catalog directory contains no YAML files"
    );
    paths.sort();
    let mut items = vec![];
    let mut seen = BTreeMap::new();
    for path in paths {
        let mut item = (|| {
            let node = yaml::parse(&std::fs::read_to_string(&path)?)?;
            let item = decode(&node)?;
            validate(&item)?;
            Ok::<_, anyhow::Error>(item)
        })()
        .with_context(|| format!("catalog file {}", path.display()))?;
        item.path = path.clone();
        let scope = match item.kind.as_str() {
            "meter" | "feature" | "addon" => item.service.clone(),
            "price" => item
                .add_on
                .as_ref()
                .map(|a| format!("addon:{}:{}", a.service, a.key))
                .unwrap_or_else(|| format!("plan:{}", item.plan)),
            _ => String::new(),
        };
        let identity = format!("{}:{scope}:{}", item.kind, item.key);
        if let Some(prior) = seen.insert(identity, path.clone()) {
            bail!(
                "catalog file {}: duplicate identity also declared in {}",
                path.display(),
                prior.display()
            );
        }
        items.push(item);
    }
    Ok(items)
}
fn valid_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200
}
fn aggregation(s: &str) -> i32 {
    match s {
        "count" => 1,
        "sum" => 2,
        "min" => 3,
        "max" => 4,
        "avg" => 5,
        "unique_count" => 6,
        _ => 0,
    }
}
fn operator(s: &str) -> i32 {
    match s {
        "equal" => 1,
        "not_equal" => 2,
        "in" => 3,
        "greater_than" => 4,
        "greater_than_or_equal" => 5,
        "less_than" => 6,
        "less_than_or_equal" => 7,
        "exists" => 8,
        _ => 0,
    }
}
fn value_type(s: &str) -> i32 {
    match s {
        "boolean" => 1,
        "integer" => 2,
        "decimal" => 3,
        "string" => 4,
        "string_list" => 5,
        _ => 0,
    }
}
fn feature_kind(s: &str) -> i32 {
    match s {
        "access" => 1,
        "metered" => 2,
        "configuration" => 3,
        _ => 0,
    }
}
fn decimal(s: &str) -> Result<String> {
    Ok(crate::accounting::Decimal::parse(s)?.to_string())
}
fn nonnegative(s: &str) -> bool {
    s.len() <= 64 && decimal(s).is_ok_and(|n| !n.starts_with('-'))
}
fn operand(node: &Node, op: &str) -> Result<Value> {
    let Node::Scalar(kind, text) = node else {
        bail!("filter values must be scalars of at most 4096 bytes")
    };
    ensure!(
        text.len() <= 4096,
        "filter values must be scalars of at most 4096 bytes"
    );
    let comparison = !matches!(op, "equal" | "not_equal" | "in");
    match kind {
        ScalarKind::String if !comparison => Ok(json!({"kind":1,"value":text})),
        ScalarKind::Number | ScalarKind::String => {
            ensure!(
                decimal(text).is_ok_and(|d| d == *text),
                "numeric filter values must be canonical decimals"
            );
            Ok(json!({"kind":2,"value":text}))
        }
        ScalarKind::Boolean if !comparison => {
            Ok(json!({"kind":3,"value":node.boolean()?.to_string()}))
        }
        _ => bail!("filter values must be strings, canonical decimal numbers, or booleans"),
    }
}
fn validate(d: &Definition) -> Result<()> {
    ensure!(valid_key(&d.key), "invalid stable key");
    match d.kind.as_str() {
        "service" => {
            ensure!(
                valid_name(&d.name)
                    && valid_name(&d.client_id)
                    && !d.capabilities.is_empty()
                    && d.capabilities.len() <= 100
                    && d.scopes.len() <= 10,
                "invalid service definition"
            );
            for c in &d.capabilities {
                ensure!(!c.is_empty() && c.len() <= 100, "invalid capability");
            }
            for s in &d.scopes {
                ensure!(
                    matches!(s.as_str(), "authorize" | "usage:write" | "usage:read"),
                    "unknown service scope"
                );
            }
        }
        "meter" => {
            ensure!(
                valid_key(&d.service)
                    && !d.event_type.is_empty()
                    && d.event_type.len() <= 100
                    && d.value_attribute.len() <= 100
                    && d.filters.len() <= 100
                    && d.group_by.len() <= 20,
                "invalid meter definition"
            );
            ensure!(
                aggregation(&d.aggregation) != 0,
                "unknown meter aggregation"
            );
            ensure!(
                matches!(d.value_source.as_str(), "" | "attribute" | "ai_token_usd"),
                "unknown meter value source"
            );
            if d.value_source == "ai_token_usd" {
                // Delcore owns the markup: the catalog declares it, reporters never send it.
                ensure!(
                    d.aggregation == "sum" && d.value_attribute.is_empty(),
                    "ai_token_usd meters use sum aggregation without a value attribute"
                );
                crate::metering::ai::parse_markup(&decimal(&d.markup)?)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            } else {
                ensure!(
                    d.markup.is_empty(),
                    "markup requires the ai_token_usd value source"
                );
                if d.aggregation == "count" {
                    ensure!(
                        d.value_attribute.is_empty(),
                        "count aggregation cannot have a value attribute"
                    );
                } else {
                    ensure!(
                        !d.value_attribute.is_empty(),
                        "aggregation requires a value attribute"
                    );
                }
            }
            let mut groups = BTreeSet::new();
            for g in &d.group_by {
                ensure!(
                    !g.is_empty() && g.len() <= 100 && groups.insert(g),
                    "invalid or repeated group attribute"
                );
            }
            for f in &d.filters {
                ensure!(
                    !f.attribute.is_empty() && f.attribute.len() <= 100,
                    "invalid filter attribute"
                );
                ensure!(operator(&f.operator) != 0, "unknown filter operator");
                if f.operator == "exists" {
                    ensure!(f.values.is_empty(), "exists filter has no values");
                } else {
                    ensure!(
                        !f.values.is_empty() && (f.operator == "in" || f.values.len() == 1),
                        "filter requires the expected number of values"
                    );
                    for v in &f.values {
                        operand(v, &f.operator)?;
                    }
                }
            }
        }
        "feature" => {
            ensure!(
                valid_key(&d.service) && valid_name(&d.name),
                "invalid feature"
            );
            match d.feature_type.as_str() {
                "access" => ensure!(
                    d.value_type.is_empty() && d.meter.is_empty(),
                    "access feature cannot have a meter or value type"
                ),
                "metered" => ensure!(
                    valid_key(&d.meter) && d.value_type.is_empty(),
                    "metered feature requires a meter key"
                ),
                "configuration" => ensure!(
                    d.meter.is_empty() && value_type(&d.value_type) != 0,
                    "configuration feature requires a value type"
                ),
                _ => bail!("unknown feature type"),
            }
        }
        "plan" => {
            ensure!(
                valid_name(&d.name)
                    && d.rank.is_some_and(|r| r >= 0)
                    && d.free.is_some()
                    && (!d.default_free.unwrap_or(false) || d.free == Some(true))
                    && !(d.free == Some(true) && d.postpaid == Some(true)),
                "invalid plan"
            );
            validate_grants(&d.features)?;
        }
        "addon" => {
            ensure!(
                valid_key(&d.service) && valid_name(&d.name),
                "invalid add-on"
            );
            validate_grants(&d.features)?;
        }
        "price" => {
            ensure!(
                valid_key(&d.service)
                    && d.plan.is_empty() != d.add_on.is_none()
                    && d.add_on
                        .as_ref()
                        .is_none_or(|a| valid_key(&a.service) && valid_key(&a.key))
                    && (d.plan.is_empty() || valid_key(&d.plan))
                    && valid_key(&d.meter)
                    && d.currency == "USD",
                "invalid price references or currency"
            );
            ensure!(
                matches!(d.price_kind.as_str(), "usage" | "recurring"),
                "price_kind must be usage or recurring"
            );
            ensure!(
                d.price_kind != "recurring" || !d.external_price_id.is_empty(),
                "recurring price requires an external price ID"
            );
            ensure!(
                nonnegative(&d.unit_price) && nonnegative(&d.included),
                "price and allowance must be nonnegative decimal strings"
            );
        }
        _ => bail!("unknown kind"),
    }
    Ok(())
}
fn validate_grants(grants: &[Grant]) -> Result<()> {
    ensure!(grants.len() <= 100, "too many feature grants");
    let mut seen = BTreeSet::new();
    for g in grants {
        ensure!(
            valid_key(&g.service) && valid_key(&g.key),
            "invalid feature reference"
        );
        ensure!(seen.insert((&g.service, &g.key)), "duplicate feature grant");
        ensure!(
            usize::from(g.enabled.is_some())
                + usize::from(g.limit.is_some())
                + usize::from(g.configuration.is_some())
                == 1,
            "feature grant requires exactly one value"
        );
    }
    Ok(())
}
fn normalize_grant(g: &Grant, kind: i32, value_type: i32) -> Result<Value> {
    match kind {
        1 => return Ok(json!({"enabled":g.enabled.context("access feature requires enabled")?})),
        2 => {
            let value = g
                .limit
                .as_ref()
                .context("metered feature requires a limit")?;
            ensure!(
                nonnegative(value),
                "metered limit must be a nonnegative decimal"
            );
            return Ok(json!({"limit":decimal(value)?}));
        }
        3 => {
            let node = g
                .configuration
                .as_ref()
                .context("configuration feature requires a value")?;
            match (value_type, node) {
                (1, Node::Scalar(ScalarKind::Boolean, _)) => {
                    return Ok(json!({"boolean":node.boolean()?}));
                }
                (2, Node::Scalar(ScalarKind::Number, _)) => {
                    if let Ok(i) = node.integer() {
                        return Ok(json!({"integer":i}));
                    }
                }
                (3, Node::Scalar(ScalarKind::String, s)) if s.len() <= 64 => {
                    return Ok(json!({"decimal":decimal(s)?}));
                }
                (4, Node::Scalar(ScalarKind::String, s)) if s.len() <= 4096 => {
                    return Ok(json!({"text":s}));
                }
                (5, Node::Sequence(items)) if items.len() <= 100 => {
                    let values = items
                        .iter()
                        .map(|v| match v {
                            Node::Scalar(ScalarKind::String, s) if s.len() <= 4096 => Ok(s.clone()),
                            _ => bail!("string list items must be strings of at most 4096 bytes"),
                        })
                        .collect::<Result<Vec<_>>>()?;
                    return Ok(json!({"string_list":values}));
                }
                _ => {}
            }
        }
        _ => {}
    }
    bail!("feature value does not match its declared type")
}
#[cfg(test)]
mod tests;
