// SPDX-License-Identifier: Apache-2.0
//! Command-line parsing for task planning fields, task list filters and planning items.
//!
//! Values are validated before dispatch with field-specific messages; the store re-validates
//! everything inside its write transaction.
use crate::{
    error::{Failure, Result, invalid},
    protocol::valid_id,
    store::planning::valid_date,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub type Flags = BTreeMap<String, Vec<String>>;

/// Flags that take no value.
pub const SWITCHES: [&str; 14] = [
    "clear-labels",
    "clear-depends-on",
    "clear-related",
    "priority-unassigned",
    "type-unassigned",
    "unlabeled",
    "start-unassigned",
    "due-unassigned",
    "points-unassigned",
    "no-milestone",
    "no-cycle",
    "no-relationships",
    "confirm-development-reset",
    "prompt-stdin",
];

fn last(flags: &mut Flags, key: &str) -> Option<String> {
    flags.remove(key).and_then(|v| v.last().cloned())
}
fn switch(flags: &mut Flags, key: &str) -> bool {
    last(flags, key).is_some_and(|v| v == "true")
}
fn all(flags: &mut Flags, key: &str) -> Vec<String> {
    flags.remove(key).unwrap_or_default()
}
fn is_none(value: &str) -> bool {
    value.eq_ignore_ascii_case("none")
}
fn uuid(flag: &str, value: &str) -> Result<String> {
    if valid_id(value) {
        Ok(value.to_owned())
    } else {
        Err(invalid(&format!("--{flag} must be a UUID v7")))
    }
}
fn uuids(flag: &str, values: Vec<String>) -> Result<Vec<String>> {
    values
        .iter()
        .flat_map(|v| v.split(','))
        .filter(|v| !v.is_empty())
        .map(|v| uuid(flag, v))
        .collect()
}
/// A UUID, or `none` meaning clear (`Ok(None)`).
fn optional_uuid(flag: &str, value: &str) -> Result<Option<String>> {
    if is_none(value) {
        Ok(None)
    } else {
        uuid(flag, value).map(Some)
    }
}
fn date(flag: &str, value: &str) -> Result<String> {
    if valid_date(value) {
        Ok(value.to_owned())
    } else {
        Err(invalid(&format!("--{flag} must be a real YYYY-MM-DD date")))
    }
}
fn optional_date(flag: &str, value: &str) -> Result<Option<String>> {
    if is_none(value) {
        Ok(None)
    } else {
        date(flag, value).map(Some)
    }
}
fn points(flag: &str, value: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n <= 2_147_483_647)
        .ok_or_else(|| {
            Failure::constraint(
                "FIELD_RANGE",
                "",
                &format!("--{flag} must be an integer from 0 to 2147483647"),
            )
        })
}
fn enum_value(flag: &str, value: &str, prefix: &str, allowed: &[&str]) -> Result<String> {
    let lower = value.to_ascii_lowercase();
    if lower == "none" {
        Ok(format!("{prefix}UNSPECIFIED"))
    } else if allowed.contains(&lower.as_str()) {
        Ok(format!("{prefix}{}", lower.to_ascii_uppercase()))
    } else {
        Err(invalid(&format!(
            "--{flag} must be none, {}",
            allowed.join(", ")
        )))
    }
}
const PRIORITIES: [&str; 4] = ["urgent", "high", "normal", "low"];
const TYPES: [&str; 5] = [
    "bug",
    "feature",
    "improvement",
    "documentation",
    "maintenance",
];
fn priority(value: &str) -> Result<String> {
    enum_value("priority", value, "TASK_PRIORITY_", &PRIORITIES)
}
fn task_type(value: &str) -> Result<String> {
    enum_value("type", value, "TASK_TYPE_", &TYPES)
}

/// Planning options of `task create` and `task edit`. `None` leaves a field unchanged on edit
/// and unassigned on create; `Some(None)` clears it.
#[derive(Default)]
pub struct TaskPlanning {
    priority: Option<String>,
    task_type: Option<String>,
    labels: Option<Vec<String>>,
    start_date: Option<Option<String>>,
    due_date: Option<Option<String>>,
    story_points: Option<Option<u64>>,
    milestone: Option<Option<String>>,
    cycle: Option<Option<String>>,
    parent: Option<Option<String>>,
    duplicate_of: Option<Option<String>>,
    depends_on: Option<Vec<String>>,
    related: Option<Vec<String>>,
    edits: Value,
}
impl TaskPlanning {
    pub fn take(flags: &mut Flags) -> Result<Self> {
        let mut out = Self::default();
        if let Some(v) = last(flags, "priority") {
            out.priority = Some(priority(&v)?);
        }
        if let Some(v) = last(flags, "type") {
            out.task_type = Some(task_type(&v)?);
        }
        let labels = all(flags, "label-id");
        let clear = switch(flags, "clear-labels");
        if clear && !labels.is_empty() {
            return Err(invalid("--clear-labels cannot be combined with --label-id"));
        }
        if clear {
            out.labels = Some(vec![]);
        } else if !labels.is_empty() {
            out.labels = Some(uuids("label-id", labels)?);
        }
        if let Some(v) = last(flags, "start-date") {
            out.start_date = Some(optional_date("start-date", &v)?);
        }
        if let Some(v) = last(flags, "due-date") {
            out.due_date = Some(optional_date("due-date", &v)?);
        }
        if let Some(v) = last(flags, "story-points") {
            out.story_points = Some(if is_none(&v) {
                None
            } else {
                Some(points("story-points", &v)?)
            });
        }
        for (flag, slot) in [
            ("milestone-id", &mut out.milestone),
            ("cycle-id", &mut out.cycle),
            ("parent-id", &mut out.parent),
            ("duplicate-of", &mut out.duplicate_of),
        ] {
            if let Some(v) = last(flags, flag) {
                *slot = Some(optional_uuid(flag, &v)?);
            }
        }
        for (flag, clear_flag, slot) in [
            ("depends-on", "clear-depends-on", &mut out.depends_on),
            ("related", "clear-related", &mut out.related),
        ] {
            let values = all(flags, flag);
            let clear = switch(flags, clear_flag);
            if clear && !values.is_empty() {
                return Err(invalid(&format!(
                    "--{clear_flag} cannot be combined with --{flag}"
                )));
            }
            if clear {
                *slot = Some(vec![]);
            } else if !values.is_empty() {
                *slot = Some(uuids(flag, values)?);
            }
        }
        let mut edits = json!({});
        for (flag, field) in [
            ("add-subtask", "addSubtaskIds"),
            ("remove-subtask", "removeSubtaskIds"),
            ("add-blocks", "addBlockedTaskIds"),
            ("remove-blocks", "removeBlockedTaskIds"),
        ] {
            let values = uuids(flag, all(flags, flag))?;
            if !values.is_empty() {
                edits[field] = json!(values);
            }
        }
        if !edits.as_object().is_some_and(serde_json::Map::is_empty) {
            out.edits = edits;
        }
        Ok(out)
    }
    /// Relationship edits for other tasks, or `Null` when none were requested.
    #[must_use]
    pub fn edits(&self) -> &Value {
        &self.edits
    }
    /// Set the flat planning fields of a `CreateTask` request.
    pub fn apply_to_request(&self, q: &mut Value) {
        self.apply(q);
    }
    /// Merge the supplied options into a stored task snapshot.
    pub fn apply_to_task(&self, task: &mut Value) {
        self.apply(task);
    }
    fn apply(&self, target: &mut Value) {
        let object = target.as_object_mut().expect("object");
        let mut put = |key: &str, value: Option<Value>| match value {
            Some(v) => {
                object.insert(key.to_owned(), v);
            }
            None => {
                object.remove(key);
            }
        };
        if let Some(v) = &self.priority {
            put("priority", Some(json!(v)));
        }
        if let Some(v) = &self.task_type {
            put("type", Some(json!(v)));
        }
        if let Some(v) = &self.labels {
            put("labelIds", Some(json!(v)));
        }
        for (key, value) in [
            ("startDate", &self.start_date),
            ("dueDate", &self.due_date),
            ("milestoneId", &self.milestone),
            ("cycleId", &self.cycle),
            ("parentTaskId", &self.parent),
            ("duplicateOfTaskId", &self.duplicate_of),
        ] {
            if let Some(v) = value {
                put(key, v.as_ref().map(|s| json!(s)));
            }
        }
        if let Some(v) = &self.story_points {
            put("storyPoints", v.map(|n| json!(n)));
        }
        if let Some(v) = &self.depends_on {
            put("dependsOnTaskIds", Some(json!(v)));
        }
        if let Some(v) = &self.related {
            put("relatedTaskIds", Some(json!(v)));
        }
    }
}

/// Filter and sort options of `task list`, merged into the request.
pub fn take_list(flags: &mut Flags, q: &mut Value) -> Result<()> {
    let mut filter = json!({});
    let mut present = false;
    let mut set = |filter: &mut Value, key: &str, value: Value| {
        filter[key] = value;
        present = true;
    };
    let priorities = all(flags, "priority");
    if !priorities.is_empty() {
        let values = priorities
            .iter()
            .flat_map(|v| v.split(','))
            .map(|v| {
                if is_none(v) {
                    Err(invalid(
                        "--priority none is not a filter; use --priority-unassigned",
                    ))
                } else {
                    priority(v)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        set(&mut filter, "priorities", json!(values));
    }
    if switch(flags, "priority-unassigned") {
        set(&mut filter, "includeUnassignedPriority", json!(true));
    }
    let types = all(flags, "type");
    if !types.is_empty() {
        let values = types
            .iter()
            .flat_map(|v| v.split(','))
            .map(|v| {
                if is_none(v) {
                    Err(invalid(
                        "--type none is not a filter; use --type-unassigned",
                    ))
                } else {
                    task_type(v)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        set(&mut filter, "types", json!(values));
    }
    if switch(flags, "type-unassigned") {
        set(&mut filter, "includeUnassignedType", json!(true));
    }
    let labels = uuids("label-id", all(flags, "label-id"))?;
    if !labels.is_empty() {
        set(&mut filter, "labelIds", json!(labels));
    }
    if switch(flags, "unlabeled") {
        set(&mut filter, "includeUnlabeled", json!(true));
    }
    for (name, field) in [("start", "startDate"), ("due", "dueDate")] {
        let mut range = json!({});
        if let Some(v) = last(flags, &format!("{name}-from")) {
            range["from"] = date(&format!("{name}-from"), &v)?.into();
        }
        if let Some(v) = last(flags, &format!("{name}-to")) {
            range["to"] = date(&format!("{name}-to"), &v)?.into();
        }
        if matches!((range["from"].as_str(), range["to"].as_str()), (Some(a), Some(b)) if a > b) {
            return Err(invalid(&format!(
                "--{name}-from must not be after --{name}-to"
            )));
        }
        if switch(flags, &format!("{name}-unassigned")) {
            range["includeUnassigned"] = true.into();
        }
        if !range.as_object().is_some_and(serde_json::Map::is_empty) {
            set(&mut filter, field, range);
        }
    }
    let mut range = json!({});
    for (flag, key) in [("points-min", "min"), ("points-max", "max")] {
        if let Some(v) = last(flags, flag) {
            range[key] = points(flag, &v)?.into();
        }
    }
    if range["min"].is_u64()
        && range["max"].is_u64()
        && range["min"].as_u64() > range["max"].as_u64()
    {
        return Err(invalid("--points-min must not exceed --points-max"));
    }
    if switch(flags, "points-unassigned") {
        range["includeUnassigned"] = true.into();
    }
    if !range.as_object().is_some_and(serde_json::Map::is_empty) {
        set(&mut filter, "storyPoints", range);
    }
    for (flag, none_flag, key, none_key) in [
        (
            "milestone-id",
            "no-milestone",
            "milestoneIds",
            "includeNoMilestone",
        ),
        ("cycle-id", "no-cycle", "cycleIds", "includeNoCycle"),
    ] {
        let values = uuids(flag, all(flags, flag))?;
        if !values.is_empty() {
            set(&mut filter, key, json!(values));
        }
        if switch(flags, none_flag) {
            set(&mut filter, none_key, json!(true));
        }
    }
    let kinds = all(flags, "relationship");
    if !kinds.is_empty() {
        let values = kinds
            .iter()
            .flat_map(|v| v.split(','))
            .map(|v| match v {
                "parent" | "subtasks" | "depends-on" | "blocks" | "related" | "duplicate-of" => {
                    Ok(format!(
                        "RELATIONSHIP_KIND_{}",
                        v.to_ascii_uppercase().replace('-', "_")
                    ))
                }
                _ => Err(invalid(
                    "--relationship must be parent, subtasks, depends-on, blocks, related or duplicate-of",
                )),
            })
            .collect::<Result<Vec<_>>>()?;
        set(&mut filter, "relationships", json!(values));
    }
    if switch(flags, "no-relationships") {
        set(&mut filter, "includeNoRelationships", json!(true));
    }
    if present {
        q["filter"] = filter;
    }
    if let Some(v) = last(flags, "sort") {
        let known = [
            "priority",
            "type",
            "start-date",
            "due-date",
            "story-points",
            "milestone-name",
            "cycle-start-date",
        ];
        if !known.contains(&v.as_str()) {
            return Err(invalid(&format!(
                "--sort must be one of {}",
                known.join(", ")
            )));
        }
        q["sortKey"] = format!("TASK_SORT_KEY_{}", v.to_ascii_uppercase().replace('-', "_")).into();
    }
    if let Some(v) = last(flags, "direction") {
        q["sortDirection"] = match v.as_str() {
            "asc" => "SORT_DIRECTION_ASCENDING",
            "desc" => "SORT_DIRECTION_DESCENDING",
            _ => return Err(invalid("--direction must be asc or desc")),
        }
        .into();
    }
    Ok(())
}

/// The `EntityKind` name of a `label`, `milestone` or `cycle` command group.
#[must_use]
pub fn group_kind(group: &str) -> Option<&'static str> {
    match group {
        "label" => Some("ENTITY_KIND_LABEL"),
        "milestone" => Some("ENTITY_KIND_MILESTONE"),
        "cycle" => Some("ENTITY_KIND_CYCLE"),
        _ => None,
    }
}
/// Fields supplied on the command line for one planning item, merged over `base`.
/// `base` is the current stored body for an edit and empty for a create.
pub fn item_body(group: &str, flags: &mut Flags, q: &mut Value, base: &Value) -> Result<Value> {
    let mut body = if base.is_object() {
        base.clone()
    } else {
        json!({})
    };
    for (key, field) in [
        ("name", "name"),
        ("description", "description"),
        ("repositoryId", "repositoryId"),
    ] {
        if let Some(value) = q.as_object_mut().and_then(|m| m.remove(key)) {
            if field == "repositoryId" && base.is_object() && body[field] != value {
                return Err(invalid("A planning item's repository cannot be changed"));
            }
            body[field] = value;
        }
    }
    match group {
        "label" => {
            if let Some(color) = last(flags, "label-color") {
                body["color"] = color.into();
            }
        }
        "milestone" => {
            if let Some(v) = last(flags, "target-date") {
                match optional_date("target-date", &v)? {
                    Some(d) => body["targetDate"] = d.into(),
                    None => {
                        body.as_object_mut().expect("object").remove("targetDate");
                    }
                }
            }
            if let Some(v) = last(flags, "completed") {
                body["completed"] = v
                    .parse::<bool>()
                    .map_err(|_| invalid("--completed must be true or false"))?
                    .into();
            }
        }
        _ => {
            for (flag, field) in [("start-date", "startDate"), ("end-date", "endDate")] {
                if let Some(v) = last(flags, flag) {
                    body[field] = date(flag, &v)?.into();
                }
            }
        }
    }
    Ok(body)
}
