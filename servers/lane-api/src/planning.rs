// SPDX-License-Identifier: FSL-1.1-ALv2
//! Task planning rules: metadata validation, repository planning items, relationship graph
//! checks, move/delete cleanup and task list filtering, sorting and keyset paging.
//!
//! The pure part of this module (`Snapshot`, `ListQuery`) mirrors the shared reference evaluator
//! in `protos/lane/v1/planning_*_test.go` and is replayed against the shared fixtures. The
//! database part loads a repository snapshot inside the caller's account-locked transaction, so
//! name, reference and graph constraints are serialized per account.
use crate::entities::{self as e, CYCLE, LABEL, MILESTONE, TASK, fault, uuid};
use crate::{Fault, Principal, Result, string};
use connectrpc::ErrorCode;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use unicode_casefold::UnicodeCaseFold;

pub(crate) const MAX_NAME_SCALARS: usize = 100;
pub(crate) const MAX_DESCRIPTION_SCALARS: usize = 4000;
pub(crate) const MAX_LABELS: usize = 100;
pub(crate) const MAX_STORY_POINTS: u64 = 2_147_483_647;

/// Protocol `ConstraintViolation` values. The numbers are the wire values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Violation {
    ParentCycle = 1,
    DependencyCycle = 2,
    DuplicateCycle = 3,
    SelfLink = 4,
    CrossRepository = 5,
    ReferenceNotFound = 6,
    DuplicateName = 7,
    LabelLimit = 8,
    DateOrder = 9,
    FieldRange = 10,
}
impl Violation {
    pub fn name(self) -> &'static str {
        match self {
            Self::ParentCycle => "CONSTRAINT_VIOLATION_PARENT_CYCLE",
            Self::DependencyCycle => "CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE",
            Self::DuplicateCycle => "CONSTRAINT_VIOLATION_DUPLICATE_CYCLE",
            Self::SelfLink => "CONSTRAINT_VIOLATION_SELF_LINK",
            Self::CrossRepository => "CONSTRAINT_VIOLATION_CROSS_REPOSITORY",
            Self::ReferenceNotFound => "CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND",
            Self::DuplicateName => "CONSTRAINT_VIOLATION_DUPLICATE_NAME",
            Self::LabelLimit => "CONSTRAINT_VIOLATION_LABEL_LIMIT",
            Self::DateOrder => "CONSTRAINT_VIOLATION_DATE_ORDER",
            Self::FieldRange => "CONSTRAINT_VIOLATION_FIELD_RANGE",
        }
    }
    pub fn from_number(n: i32) -> Option<Self> {
        Some(match n {
            1 => Self::ParentCycle,
            2 => Self::DependencyCycle,
            3 => Self::DuplicateCycle,
            4 => Self::SelfLink,
            5 => Self::CrossRepository,
            6 => Self::ReferenceNotFound,
            7 => Self::DuplicateName,
            8 => Self::LabelLimit,
            9 => Self::DateOrder,
            10 => Self::FieldRange,
            _ => return None,
        })
    }
}
/// `ErrorReason.CONSTRAINT_VIOLATION`.
pub const CONSTRAINT_REASON: i32 = 19;
/// A constraint violation is a `Fault` whose reason carries the violation in bits 8 and up.
/// `Fault` belongs to the shared server crate and has no field for it; `error::detail` splits
/// the packed value back into `ErrorDetail.reason` and `ErrorDetail.violation`.
pub(crate) fn violation(v: Violation, message: &str) -> Fault {
    Fault::new(
        ErrorCode::FailedPrecondition,
        CONSTRAINT_REASON | ((v as i32) << 8),
        message,
    )
}
pub fn violation_of(f: &Fault) -> Option<Violation> {
    if f.reason & 0xff == CONSTRAINT_REASON {
        Violation::from_number(f.reason >> 8)
    } else {
        None
    }
}
/// Content-free result category for logs: the violation name or the plain reason number.
pub fn category(f: &Fault) -> String {
    match violation_of(f) {
        Some(v) => v.name().to_owned(),
        None => format!("reason_{}", f.reason & 0xff),
    }
}

pub(crate) fn is_item(kind: i16) -> bool {
    matches!(kind, LABEL | MILESTONE | CYCLE)
}

// ---------------------------------------------------------------------------------------------
// Scalar rules
// ---------------------------------------------------------------------------------------------

/// A real proleptic Gregorian `YYYY-MM-DD` date.
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let (y, m, d): (u32, usize, u32) = (
        s[0..4].parse().unwrap(),
        s[5..7].parse().unwrap(),
        s[8..10].parse().unwrap(),
    );
    if !(1..=12).contains(&m) || d < 1 {
        return false;
    }
    let mut days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
        days[1] = 29;
    }
    d <= days[m - 1]
}
/// Unicode simple lower-casing, one scalar at a time (no context-sensitive or multi-scalar
/// mappings), matching the reference evaluator.
pub(crate) fn simple_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}
fn invalid(message: &str) -> Fault {
    fault(1, message)
}
fn color_ok(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------------------------
// Task metadata fields
// ---------------------------------------------------------------------------------------------

const PRIORITY: &[&str] = &[
    "TASK_PRIORITY_URGENT",
    "TASK_PRIORITY_HIGH",
    "TASK_PRIORITY_NORMAL",
    "TASK_PRIORITY_LOW",
];
const TYPE: &[&str] = &[
    "TASK_TYPE_BUG",
    "TASK_TYPE_FEATURE",
    "TASK_TYPE_IMPROVEMENT",
    "TASK_TYPE_DOCUMENTATION",
    "TASK_TYPE_MAINTENANCE",
];
const RELATIONSHIPS: &[&str] = &[
    "RELATIONSHIP_KIND_PARENT",
    "RELATIONSHIP_KIND_SUBTASKS",
    "RELATIONSHIP_KIND_DEPENDS_ON",
    "RELATIONSHIP_KIND_BLOCKS",
    "RELATIONSHIP_KIND_RELATED",
    "RELATIONSHIP_KIND_DUPLICATE_OF",
];
const SORT_KEYS: &[&str] = &[
    "TASK_SORT_KEY_PRIORITY",
    "TASK_SORT_KEY_TYPE",
    "TASK_SORT_KEY_START_DATE",
    "TASK_SORT_KEY_DUE_DATE",
    "TASK_SORT_KEY_STORY_POINTS",
    "TASK_SORT_KEY_MILESTONE_NAME",
    "TASK_SORT_KEY_CYCLE_START_DATE",
];
const DIRECTIONS: &[&str] = &["SORT_DIRECTION_ASCENDING", "SORT_DIRECTION_DESCENDING"];

fn optional_date(v: &Value, key: &str) -> Result<Option<String>> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(None),
        Some(Value::String(s)) if valid_date(s) => Ok(Some(s.clone())),
        Some(_) => Err(invalid("dates must be real YYYY-MM-DD calendar dates")),
    }
}
fn optional_id(v: &Value, key: &str) -> Result<Option<String>> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(None),
        Some(Value::String(s)) => {
            uuid(s)?;
            Ok(Some(s.clone()))
        }
        Some(_) => Err(invalid("expected an entity ID")),
    }
}
fn id_set(v: &Value, key: &str) -> Result<Vec<String>> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(Vec::new()),
        Some(Value::Array(_)) => e::set_ids(&v[key]),
        Some(_) => Err(invalid("expected a set of entity IDs")),
    }
}
fn story_points(v: &Value) -> Result<Option<u64>> {
    let out_of_range = || violation(Violation::FieldRange, "story points are out of range");
    match v.get("storyPoints").filter(|x| !x.is_null()) {
        None => Ok(None),
        Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(out_of_range),
        Some(Value::String(s)) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            Ok(Some(s.parse::<u64>().map_err(|_| out_of_range())?))
        }
        Some(_) => Err(out_of_range()),
    }
}

/// Task-local planning rules in reference order: priority, type, dates, story points, date
/// order, then the label set. Reference lookups need stored state and run later.
pub(crate) fn task_planning_fields(v: &Value, body: &mut Map<String, Value>) -> Result<()> {
    let priority = e::enum_value(&v["priority"], PRIORITY, Some("TASK_PRIORITY_UNSPECIFIED"))?;
    let kind = e::enum_value(&v["type"], TYPE, Some("TASK_TYPE_UNSPECIFIED"))?;
    let start = optional_date(v, "startDate")?;
    let due = optional_date(v, "dueDate")?;
    let points = story_points(v)?;
    if points.is_some_and(|n| n > MAX_STORY_POINTS) {
        return Err(violation(
            Violation::FieldRange,
            "story points exceed 2147483647",
        ));
    }
    if let (Some(s), Some(d)) = (&start, &due)
        && s > d
    {
        return Err(violation(
            Violation::DateOrder,
            "start date is after the due date",
        ));
    }
    let labels = id_set(v, "labelIds")?;
    if labels.len() > MAX_LABELS {
        return Err(violation(
            Violation::LabelLimit,
            "a task has at most 100 labels",
        ));
    }
    let milestone = optional_id(v, "milestoneId")?;
    let cycle = optional_id(v, "cycleId")?;
    let parent = optional_id(v, "parentTaskId")?;
    let duplicate = optional_id(v, "duplicateOfTaskId")?;
    let depends = id_set(v, "dependsOnTaskIds")?;
    let related = id_set(v, "relatedTaskIds")?;
    // Unassigned values are omitted; 0 story points is an assigned value.
    if priority != "TASK_PRIORITY_UNSPECIFIED" {
        body.insert("priority".into(), json!(priority));
    }
    if kind != "TASK_TYPE_UNSPECIFIED" {
        body.insert("type".into(), json!(kind));
    }
    for (key, set) in [
        ("labelIds", labels),
        ("dependsOnTaskIds", depends),
        ("relatedTaskIds", related),
    ] {
        if !set.is_empty() {
            body.insert(key.into(), json!(set));
        }
    }
    for (key, value) in [("startDate", start), ("dueDate", due)] {
        if let Some(value) = value {
            body.insert(key.into(), json!(value));
        }
    }
    if let Some(points) = points {
        body.insert("storyPoints".into(), json!(points));
    }
    for (key, value) in [
        ("milestoneId", milestone),
        ("cycleId", cycle),
        ("parentTaskId", parent),
        ("duplicateOfTaskId", duplicate),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), json!(value));
        }
    }
    Ok(())
}
/// True when a stored task body carries any planning field (requires envelope format 3).
pub(crate) fn has_planning_fields(task: &Value) -> bool {
    [
        "priority",
        "type",
        "labelIds",
        "startDate",
        "dueDate",
        "storyPoints",
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "dependsOnTaskIds",
        "relatedTaskIds",
        "duplicateOfTaskId",
    ]
    .iter()
    .any(|k| task.get(k).is_some_and(|v| !v.is_null()))
}

fn strings(b: &Value, key: &str) -> Vec<String> {
    b[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
fn opt_str<'a>(b: &'a Value, key: &str) -> Option<&'a str> {
    b[key].as_str()
}
fn sorted_unique(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}
fn set_strings(b: &mut Value, key: &str, values: Vec<String>) {
    let obj = b.as_object_mut().expect("task body is an object");
    if values.is_empty() {
        obj.remove(key);
    } else {
        obj.insert(key.into(), json!(values));
    }
}
fn set_opt(b: &mut Value, key: &str, value: Option<&str>) {
    let obj = b.as_object_mut().expect("task body is an object");
    match value {
        Some(v) => obj.insert(key.into(), json!(v)),
        None => obj.remove(key),
    };
}
fn clear_references(b: &mut Value) {
    let obj = b.as_object_mut().expect("task body is an object");
    for key in [
        "labelIds",
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "dependsOnTaskIds",
        "relatedTaskIds",
        "duplicateOfTaskId",
    ] {
        obj.remove(key);
    }
}
/// Drop every reference to `id` from `b`; true when the body changed.
fn remove_task_reference(b: &mut Value, id: &str) -> bool {
    let mut changed = false;
    for key in ["parentTaskId", "duplicateOfTaskId"] {
        if opt_str(b, key) == Some(id) {
            set_opt(b, key, None);
            changed = true;
        }
    }
    for key in ["dependsOnTaskIds", "relatedTaskIds"] {
        let mut set = strings(b, key);
        if let Some(i) = set.iter().position(|s| s == id) {
            set.remove(i);
            set_strings(b, key, set);
            changed = true;
        }
    }
    changed
}

/// Relationship edits that change other tasks' stored fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Edits {
    pub add_subtasks: Vec<String>,
    pub remove_subtasks: Vec<String>,
    pub add_blocked: Vec<String>,
    pub remove_blocked: Vec<String>,
}
impl Edits {
    pub fn from_value(v: &Value) -> Result<Self> {
        if v.is_null() {
            return Ok(Self::default());
        }
        if !v.is_object() {
            return Err(invalid("relationship edits must be an object"));
        }
        Ok(Self {
            add_subtasks: id_set(v, "addSubtaskIds")?,
            remove_subtasks: id_set(v, "removeSubtaskIds")?,
            add_blocked: id_set(v, "addBlockedTaskIds")?,
            remove_blocked: id_set(v, "removeBlockedTaskIds")?,
        })
    }
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    /// Canonical request form: sorted unique sets, empty sets omitted.
    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        for (key, set) in [
            ("addSubtaskIds", &self.add_subtasks),
            ("removeSubtaskIds", &self.remove_subtasks),
            ("addBlockedTaskIds", &self.add_blocked),
            ("removeBlockedTaskIds", &self.remove_blocked),
        ] {
            if !set.is_empty() {
                out.insert(key.into(), json!(set));
            }
        }
        Value::Object(out)
    }
}
/// Every other entity ID a task write names, so one query can classify them.
pub(crate) fn referenced_ids(body: &Value, edits: &Edits) -> Vec<String> {
    let mut ids = strings(body, "labelIds");
    for key in ["dependsOnTaskIds", "relatedTaskIds"] {
        ids.extend(strings(body, key));
    }
    for key in [
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "duplicateOfTaskId",
    ] {
        ids.extend(opt_str(body, key).map(str::to_owned));
    }
    for set in [&edits.add_subtasks, &edits.add_blocked] {
        ids.extend(set.iter().cloned());
    }
    sorted_unique(ids)
}

// ---------------------------------------------------------------------------------------------
// Planning item bodies
// ---------------------------------------------------------------------------------------------

pub(crate) fn item_key(kind: i16) -> &'static str {
    match kind {
        LABEL => "label",
        MILESTONE => "milestone",
        _ => "cycle",
    }
}
/// Split a `PlanningBody` into its kind and normalized inner body (local rules only).
pub(crate) fn planning_body(item: &Value) -> Result<(i16, Value)> {
    let object = item
        .as_object()
        .ok_or_else(|| invalid("a planning item body is required"))?;
    let mut present = [LABEL, MILESTONE, CYCLE]
        .into_iter()
        .filter(|k| object.get(item_key(*k)).is_some_and(|v| !v.is_null()));
    let (Some(kind), None) = (present.next(), present.next()) else {
        return Err(invalid("choose exactly one of label, milestone or cycle"));
    };
    Ok((kind, item_body(kind, &item[item_key(kind)])?))
}
/// Normalize one planning item body: trim the name, check lengths, colors and dates.
pub(crate) fn item_body(kind: i16, v: &Value) -> Result<Value> {
    if !v.is_object() {
        return Err(invalid("a planning item body is required"));
    }
    let repository = string(v, "repositoryId");
    uuid(repository)?;
    let name = string(v, "name").trim();
    if !(1..=MAX_NAME_SCALARS).contains(&name.chars().count()) {
        return Err(invalid("name must be 1 to 100 characters after trimming"));
    }
    let description = string(v, "description");
    if description.chars().count() > MAX_DESCRIPTION_SCALARS {
        return Err(invalid("description exceeds 4000 characters"));
    }
    let mut body = json!({"repositoryId":repository,"name":name});
    if !description.is_empty() {
        body["description"] = json!(description);
    }
    match kind {
        LABEL => {
            let color = string(v, "color");
            if !color_ok(color) {
                return Err(invalid("color must be #RRGGBB"));
            }
            body["color"] = json!(color.to_ascii_uppercase());
        }
        MILESTONE => {
            if let Some(date) = optional_date(v, "targetDate")? {
                body["targetDate"] = json!(date);
            }
            if v.get("completed")
                .is_some_and(|c| !c.is_null() && !c.is_boolean())
            {
                return Err(invalid("completed must be a boolean"));
            }
            if v["completed"].as_bool() == Some(true) {
                body["completed"] = json!(true);
            }
        }
        _ => {
            let start = optional_date(v, "startDate")?;
            let end = optional_date(v, "endDate")?;
            let (Some(start), Some(end)) = (start, end) else {
                return Err(invalid("a cycle requires start and end dates"));
            };
            if start > end {
                return Err(violation(
                    Violation::DateOrder,
                    "cycle start date is after its end date",
                ));
            }
            body["startDate"] = json!(start);
            body["endDate"] = json!(end);
        }
    }
    Ok(body)
}
// ---------------------------------------------------------------------------------------------
// Repository snapshot and graph rules
// ---------------------------------------------------------------------------------------------

/// Current heads of one live task. `bodies[0]` is the accepted head; further bodies belong to
/// a conflicted task and only contribute references to cycle checks.
#[derive(Clone, Debug)]
pub(crate) struct TaskState {
    pub repo: String,
    pub bodies: Vec<Value>,
}
#[derive(Clone, Debug)]
pub(crate) struct ItemState {
    pub kind: i16,
    pub repo: String,
    /// The name of every head, so conflict candidates keep their names reserved.
    pub names: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// New task: no previous state, no move handling.
    Create,
    /// Interactive save: a changed repository is a move that clears references.
    Save,
    /// Synchronized version: validate only, never reinterpret or cascade.
    Push,
}
#[derive(Debug)]
pub(crate) struct TaskWrite {
    pub body: Value,
    /// New stored bodies of other tasks the write changes, by ID.
    pub others: BTreeMap<String, Value>,
}
#[derive(Default, Debug)]
pub(crate) struct Snapshot {
    pub tasks: BTreeMap<String, TaskState>,
    pub items: BTreeMap<String, ItemState>,
    /// Live entities outside the loaded repositories: kind and repository.
    pub foreign: BTreeMap<String, (i16, String)>,
}
struct Work {
    main: Value,
    extra: Vec<Value>,
}
impl Work {
    fn bodies(&self) -> impl Iterator<Item = &Value> {
        std::iter::once(&self.main).chain(self.extra.iter())
    }
}
fn reaches_self(
    work: &BTreeMap<String, Work>,
    start: &str,
    edges: fn(&Value) -> Vec<String>,
) -> bool {
    let mut seen = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    let first = |id: &str| -> Vec<String> {
        work.get(id)
            .map(|w| w.bodies().flat_map(edges).collect())
            .unwrap_or_default()
    };
    stack.extend(first(start));
    while let Some(node) = stack.pop() {
        if node == start {
            return true;
        }
        if seen.insert(node.clone()) {
            stack.extend(first(&node));
        }
    }
    false
}
fn parent_edges(b: &Value) -> Vec<String> {
    opt_str(b, "parentTaskId")
        .map(str::to_owned)
        .into_iter()
        .collect()
}
fn duplicate_edges(b: &Value) -> Vec<String> {
    opt_str(b, "duplicateOfTaskId")
        .map(str::to_owned)
        .into_iter()
        .collect()
}
fn dependency_edges(b: &Value) -> Vec<String> {
    strings(b, "dependsOnTaskIds")
}

impl Snapshot {
    fn live(&self, id: &str) -> Option<(i16, &str)> {
        if let Some(t) = self.tasks.get(id) {
            return Some((TASK, &t.repo));
        }
        if let Some(i) = self.items.get(id) {
            return Some((i.kind, &i.repo));
        }
        self.foreign.get(id).map(|(k, r)| (*k, r.as_str()))
    }
    fn check_item_ref(&self, id: &str, kind: i16, repo: &str) -> Result<()> {
        match self.live(id) {
            Some((k, r)) if k == kind => {
                if r == repo {
                    Ok(())
                } else {
                    Err(violation(
                        Violation::CrossRepository,
                        "the referenced item belongs to another repository",
                    ))
                }
            }
            _ => Err(violation(
                Violation::ReferenceNotFound,
                "the referenced item does not exist",
            )),
        }
    }
    /// Labels, milestone and cycle must be live items of the task's repository.
    fn validate_planning_refs(&self, body: &Value, repo: &str) -> Result<()> {
        for label in strings(body, "labelIds") {
            self.check_item_ref(&label, LABEL, repo)?;
        }
        if let Some(id) = opt_str(body, "milestoneId") {
            self.check_item_ref(id, MILESTONE, repo)?;
        }
        if let Some(id) = opt_str(body, "cycleId") {
            self.check_item_ref(id, CYCLE, repo)?;
        }
        Ok(())
    }
    /// Names are unique per repository and kind among live items, compared with simple lower
    /// casing and excluding the item itself.
    pub fn check_item(&self, id: &str, kind: i16, repo: &str, name: &str) -> Result<()> {
        if let Some(existing) = self.items.get(id)
            && (existing.kind != kind || existing.repo != repo)
        {
            return Err(invalid(
                "a planning item's kind and repository are immutable",
            ));
        }
        let wanted = simple_lower(name);
        let taken = self.items.iter().any(|(other, item)| {
            other != id
                && item.kind == kind
                && item.repo == repo
                && item.names.iter().any(|n| simple_lower(n) == wanted)
        });
        if taken {
            return Err(violation(
                Violation::DuplicateName,
                "the name is already used in this repository",
            ));
        }
        Ok(())
    }

    /// Validate a task write and compute every other task body it changes. `supersede` is true
    /// when the new body replaces every current head of the task; otherwise the new body is one
    /// more head of a conflicted task and cycle checks use the union of references.
    pub fn write_task(
        &self,
        id: &str,
        repo: &str,
        mut body: Value,
        edits: &Edits,
        mode: Mode,
        supersede: bool,
    ) -> Result<TaskWrite> {
        let previous = self.tasks.get(id);
        if mode == Mode::Save && previous.is_some_and(|p| p.repo != repo) {
            // A move discards the references sent with it and clears the stored ones.
            clear_references(&mut body);
            let mut others = BTreeMap::new();
            for (other, state) in &self.tasks {
                // A conflicted task has no single body to rewrite. Its dangling reference is
                // ignored by readers and rejected when the conflict is resolved.
                if other != id && state.bodies.len() == 1 {
                    let mut changed = state.bodies[0].clone();
                    if remove_task_reference(&mut changed, id) {
                        others.insert(other.clone(), changed);
                    }
                }
            }
            return Ok(TaskWrite { body, others });
        }
        self.validate_planning_refs(&body, repo)?;
        let no_edits = Edits::default();
        let edits = if mode == Mode::Push { &no_edits } else { edits };

        let mut links: Vec<String> = Vec::new();
        links.extend(strings(&body, "dependsOnTaskIds"));
        links.extend(strings(&body, "relatedTaskIds"));
        links.extend(opt_str(&body, "parentTaskId").map(str::to_owned));
        links.extend(opt_str(&body, "duplicateOfTaskId").map(str::to_owned));
        links.extend(edits.add_subtasks.iter().cloned());
        links.extend(edits.add_blocked.iter().cloned());
        if links.iter().any(|l| l == id) {
            return Err(violation(
                Violation::SelfLink,
                "a task cannot link to itself",
            ));
        }
        if links
            .iter()
            .any(|l| !matches!(self.live(l), Some((TASK, _))))
        {
            return Err(violation(
                Violation::ReferenceNotFound,
                "a linked task does not exist",
            ));
        }
        if links
            .iter()
            .any(|l| self.live(l).map(|(_, r)| r) != Some(repo))
        {
            return Err(violation(
                Violation::CrossRepository,
                "linked tasks must belong to the same repository",
            ));
        }

        let mut work: BTreeMap<String, Work> = self
            .tasks
            .iter()
            .filter(|(_, s)| s.repo == repo)
            .map(|(k, s)| {
                (
                    k.clone(),
                    Work {
                        main: s.bodies[0].clone(),
                        extra: s.bodies[1..].to_vec(),
                    },
                )
            })
            .collect();
        let originals: BTreeMap<String, Value> = work
            .iter()
            .map(|(k, w)| (k.clone(), w.main.clone()))
            .collect();
        // Related links are stored on both endpoints.
        let new_related = strings(&body, "relatedTaskIds");
        let previous_related = previous
            .map(|p| strings(&p.bodies[0], "relatedTaskIds"))
            .unwrap_or_default();
        for related in sorted_unique(
            previous_related
                .into_iter()
                .chain(new_related.iter().cloned())
                .collect(),
        ) {
            if let Some(other) = work.get_mut(&related) {
                let mut set = strings(&other.main, "relatedTaskIds");
                set.retain(|s| s != id);
                if new_related.contains(&related) {
                    set.push(id.to_owned());
                }
                set_strings(&mut other.main, "relatedTaskIds", sorted_unique(set));
            }
        }
        work.insert(
            id.to_owned(),
            Work {
                main: body.clone(),
                extra: if supersede {
                    Vec::new()
                } else {
                    previous.map(|p| p.bodies.clone()).unwrap_or_default()
                },
            },
        );
        for child in &edits.add_subtasks {
            if let Some(w) = work.get_mut(child) {
                set_opt(&mut w.main, "parentTaskId", Some(id));
            }
        }
        for child in &edits.remove_subtasks {
            if let Some(w) = work.get_mut(child)
                && opt_str(&w.main, "parentTaskId") == Some(id)
            {
                set_opt(&mut w.main, "parentTaskId", None);
            }
        }
        for blocked in &edits.add_blocked {
            if let Some(w) = work.get_mut(blocked) {
                let mut set = strings(&w.main, "dependsOnTaskIds");
                set.push(id.to_owned());
                set_strings(&mut w.main, "dependsOnTaskIds", sorted_unique(set));
            }
        }
        for blocked in &edits.remove_blocked {
            if let Some(w) = work.get_mut(blocked) {
                let mut set = strings(&w.main, "dependsOnTaskIds");
                set.retain(|s| s != id);
                set_strings(&mut w.main, "dependsOnTaskIds", sorted_unique(set));
            }
        }

        // A new cycle must pass through a node this write changed.
        let modified: Vec<&String> = work
            .iter()
            .filter(|(k, w)| k.as_str() == id || originals.get(*k) != Some(&w.main))
            .map(|(k, _)| k)
            .collect();
        for (edges, which) in [
            (
                parent_edges as fn(&Value) -> Vec<String>,
                Violation::ParentCycle,
            ),
            (dependency_edges, Violation::DependencyCycle),
            (duplicate_edges, Violation::DuplicateCycle),
        ] {
            if modified.iter().any(|m| reaches_self(&work, m, edges)) {
                return Err(violation(which, "the links would form a cycle"));
            }
        }
        let others = work
            .into_iter()
            .filter(|(k, w)| k != id && originals.get(k) != Some(&w.main))
            .map(|(k, w)| (k, w.main))
            .collect();
        Ok(TaskWrite { body, others })
    }

    /// Bodies of other tasks after removing every reference to a deleted task.
    pub fn delete_task(&self, id: &str) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        for (other, state) in &self.tasks {
            if other != id && state.bodies.len() == 1 {
                let mut changed = state.bodies[0].clone();
                if remove_task_reference(&mut changed, id) {
                    out.insert(other.clone(), changed);
                }
            }
        }
        out
    }
    /// Bodies of tasks after removing the assignment of a deleted label, milestone or cycle.
    pub fn delete_item(&self, id: &str) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        for (task, state) in &self.tasks {
            if state.bodies.len() != 1 {
                continue;
            }
            let mut changed = state.bodies[0].clone();
            let mut labels = strings(&changed, "labelIds");
            let before = labels.len();
            labels.retain(|l| l != id);
            let mut touched = labels.len() != before;
            if touched {
                set_strings(&mut changed, "labelIds", labels);
            }
            for key in ["milestoneId", "cycleId"] {
                if opt_str(&changed, key) == Some(id) {
                    set_opt(&mut changed, key, None);
                    touched = true;
                }
            }
            if touched {
                out.insert(task.clone(), changed);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Database access
// ---------------------------------------------------------------------------------------------

/// Load the current heads of live tasks and planning items of `repos`, and classify `refs`.
pub(crate) async fn load(
    db: &mut PgConnection,
    account: &str,
    repos: &[String],
    refs: &[String],
) -> Result<Snapshot> {
    let mut snapshot = Snapshot::default();
    let rows = sqlx::query("SELECT e.id,e.repository_id,v.envelope->'task' AS body FROM lane_entities e JOIN lane_heads h ON h.account_id=e.account_id AND h.entity_id=e.id JOIN lane_versions v ON v.account_id=h.account_id AND v.entity_id=h.entity_id AND v.version_id=h.version_id WHERE e.account_id=$1 AND e.kind=2 AND NOT e.deleted AND e.repository_id=ANY($2) ORDER BY e.id,(h.version_id=e.accepted_version_id) DESC,h.version_id")
        .bind(account).bind(repos).fetch_all(&mut *db).await?;
    for row in rows {
        let id: String = row.get("id");
        let body: Value = row.get("body");
        snapshot
            .tasks
            .entry(id)
            .or_insert_with(|| TaskState {
                repo: row
                    .get::<Option<String>, _>("repository_id")
                    .unwrap_or_default(),
                bodies: Vec::new(),
            })
            .bodies
            .push(body);
    }
    let rows = sqlx::query("SELECT e.id,e.kind,e.repository_id,coalesce(v.envelope->'label',v.envelope->'milestone',v.envelope->'cycle')->>'name' AS name FROM lane_entities e JOIN lane_heads h ON h.account_id=e.account_id AND h.entity_id=e.id JOIN lane_versions v ON v.account_id=h.account_id AND v.entity_id=h.entity_id AND v.version_id=h.version_id WHERE e.account_id=$1 AND e.kind IN (4,5,6) AND NOT e.deleted AND e.repository_id=ANY($2) ORDER BY e.id,h.version_id")
        .bind(account).bind(repos).fetch_all(&mut *db).await?;
    for row in rows {
        let id: String = row.get("id");
        let name: Option<String> = row.get("name");
        snapshot
            .items
            .entry(id)
            .or_insert_with(|| ItemState {
                kind: row.get("kind"),
                repo: row
                    .get::<Option<String>, _>("repository_id")
                    .unwrap_or_default(),
                names: Vec::new(),
            })
            .names
            .extend(name);
    }
    if !refs.is_empty() {
        let rows = sqlx::query("SELECT id,kind,repository_id FROM lane_entities WHERE account_id=$1 AND id=ANY($2) AND NOT deleted AND kind IN (2,4,5,6)")
            .bind(account).bind(refs).fetch_all(&mut *db).await?;
        for row in rows {
            snapshot.foreign.insert(
                row.get("id"),
                (
                    row.get("kind"),
                    row.get::<Option<String>, _>("repository_id")
                        .unwrap_or_default(),
                ),
            );
        }
    }
    Ok(snapshot)
}

pub(crate) struct Plan {
    pub body: Value,
    pub others: BTreeMap<String, Value>,
}
/// Validate a task write against current account state and compute the cascade. Callers hold
/// the account lock, so the checks and the following writes are serialized with every other
/// writer of the account.
pub(crate) struct TaskPlan<'a> {
    pub id: &'a str,
    /// Canonical repository the stored task currently belongs to, if it exists.
    pub current_repo: Option<&'a str>,
    pub body: Value,
    pub edits: &'a Edits,
    pub mode: Mode,
    pub supersede: bool,
}
pub(crate) async fn plan_task(
    db: &mut PgConnection,
    account: &str,
    task: TaskPlan<'_>,
) -> Result<Plan> {
    let TaskPlan {
        id,
        current_repo,
        body,
        edits,
        mode,
        supersede,
    } = task;
    let repo = e::canonical_repository(db, account, string(&body, "repositoryId")).await?;
    let mut repos = vec![repo.clone()];
    if let Some(old) = current_repo
        && old != repo
    {
        repos.push(old.to_owned());
    }
    let refs = referenced_ids(&body, edits);
    let snapshot = load(db, account, &repos, &refs).await?;
    let write = snapshot.write_task(id, &repo, body, edits, mode, supersede)?;
    if mode != Mode::Push {
        for other in write.others.keys() {
            if snapshot
                .tasks
                .get(other)
                .is_some_and(|t| t.bodies.len() > 1)
            {
                return Err(fault(4, "an affected task is conflicted and read-only"));
            }
        }
    }
    Ok(Plan {
        body: write.body,
        others: if mode == Mode::Push {
            BTreeMap::new()
        } else {
            write.others
        },
    })
}
/// Write one new version per changed task with the caller's origin and return their results in
/// ID order. The primary write and these cascades share the caller's transaction.
pub(crate) async fn write_others(
    db: &mut PgConnection,
    p: &Principal,
    others: BTreeMap<String, Value>,
) -> Result<Vec<Value>> {
    let mut results = Vec::new();
    for (id, body) in others {
        let heads = e::heads(db, &p.account_id, &id).await?;
        if heads.len() != 1 {
            return Err(fault(4, "an affected task is conflicted and read-only"));
        }
        let v = e::version(&id, TASK, &heads, Some(body), p);
        e::insert_version(db, &p.account_id, &id, &v, false).await?;
        sqlx::query("DELETE FROM lane_heads WHERE account_id=$1 AND entity_id=$2")
            .bind(&p.account_id)
            .bind(&id)
            .execute(&mut *db)
            .await?;
        e::add_head(db, &p.account_id, &id, string(&v, "versionId")).await?;
        e::project(db, &p.account_id, &id, TASK, &v).await?;
        results.push(e::emit(db, &p.account_id, &id).await?);
    }
    Ok(results)
}
/// Remove references to a task or planning item that is about to be deleted (or whose task is
/// leaving the repository) from the other tasks of its repository.
pub(crate) async fn cleanup_before_delete(
    db: &mut PgConnection,
    p: &Principal,
    id: &str,
    kind: i16,
    repo: &str,
) -> Result<()> {
    let snapshot = load(db, &p.account_id, &[repo.to_owned()], &[]).await?;
    let others = if kind == TASK {
        snapshot.delete_task(id)
    } else {
        snapshot.delete_item(id)
    };
    write_others(db, p, others).await?;
    Ok(())
}
/// Name, repository and immutability rules for a planning item version. `body` is the
/// normalized inner body.
pub(crate) async fn validate_item(
    db: &mut PgConnection,
    account: &str,
    id: &str,
    kind: i16,
    body: &Value,
) -> Result<()> {
    let repo = e::require_repository(db, account, string(body, "repositoryId")).await?;
    let snapshot = load(db, account, std::slice::from_ref(&repo), &[]).await?;
    // The stored item may live in another repository, which the repository-scoped load misses.
    if let Some(stored) = e::entity(db, account, id).await?
        && !stored.deleted
        && (stored.kind != kind || stored.repository.as_deref().is_some_and(|r| r != repo))
    {
        return Err(invalid(
            "a planning item's kind and repository are immutable",
        ));
    }
    snapshot.check_item(id, kind, &repo, string(body, "name"))
}
/// Constraint validation of a synchronized task version against the accepted server state.
pub(crate) async fn validate_pushed_task(
    db: &mut PgConnection,
    account: &str,
    id: &str,
    body: &Value,
    supersede: bool,
) -> Result<()> {
    let current = e::entity(db, account, id)
        .await?
        .and_then(|x| if x.deleted { None } else { x.repository });
    plan_task(
        db,
        account,
        TaskPlan {
            id,
            current_repo: current.as_deref(),
            body: body.clone(),
            edits: &Edits::default(),
            mode: Mode::Push,
            supersede,
        },
    )
    .await
    .map(|_| ())
}

// ---------------------------------------------------------------------------------------------
// Task list filtering, sorting and keyset pagination
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct DateRange {
    from: Option<String>,
    to: Option<String>,
    unassigned: bool,
}
#[derive(Debug, Default)]
struct PointRange {
    min: Option<u64>,
    max: Option<u64>,
    unassigned: bool,
}
#[derive(Debug, Default)]
pub(crate) struct Filter {
    priorities: Vec<String>,
    priority_unassigned: bool,
    types: Vec<String>,
    type_unassigned: bool,
    labels: Vec<String>,
    unlabeled: bool,
    start: DateRange,
    due: DateRange,
    points: PointRange,
    milestones: Vec<String>,
    no_milestone: bool,
    cycles: Vec<String>,
    no_cycle: bool,
    relationships: Vec<String>,
    no_relationships: bool,
}
fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}
fn enum_list(v: &Value, key: &str, names: &[&str]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for item in v[key].as_array().into_iter().flatten() {
        // An unspecified entry is kept: it selects nothing, exactly like the reference.
        out.push(e::enum_value(item, names, Some("UNSPECIFIED"))?);
    }
    Ok(out)
}
fn date_range(v: &Value) -> Result<DateRange> {
    Ok(DateRange {
        from: v
            .get("from")
            .filter(|x| !x.is_null())
            .and_then(Value::as_str)
            .map(str::to_owned),
        to: v
            .get("to")
            .filter(|x| !x.is_null())
            .and_then(Value::as_str)
            .map(str::to_owned),
        unassigned: flag(v, "includeUnassigned"),
    })
}
fn point_bound(v: &Value, key: &str) -> Result<Option<u64>> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid("invalid story point bound")),
        Some(Value::String(s)) => s
            .parse()
            .map(Some)
            .map_err(|_| invalid("invalid story point bound")),
        Some(_) => Err(invalid("invalid story point bound")),
    }
}
impl Filter {
    fn parse(v: &Value) -> Result<Self> {
        if v.is_null() {
            return Ok(Self::default());
        }
        let ids = |key: &str| -> Result<Vec<String>> {
            // Unknown or deleted IDs match nothing and are not an error, but malformed IDs are.
            let set = id_set(v, key)?;
            Ok(set)
        };
        Ok(Self {
            priorities: enum_list(v, "priorities", PRIORITY)?,
            priority_unassigned: flag(v, "includeUnassignedPriority"),
            types: enum_list(v, "types", TYPE)?,
            type_unassigned: flag(v, "includeUnassignedType"),
            labels: ids("labelIds")?,
            unlabeled: flag(v, "includeUnlabeled"),
            start: date_range(&v["startDate"])?,
            due: date_range(&v["dueDate"])?,
            points: PointRange {
                min: point_bound(&v["storyPoints"], "min")?,
                max: point_bound(&v["storyPoints"], "max")?,
                unassigned: flag(&v["storyPoints"], "includeUnassigned"),
            },
            milestones: ids("milestoneIds")?,
            no_milestone: flag(v, "includeNoMilestone"),
            cycles: ids("cycleIds")?,
            no_cycle: flag(v, "includeNoCycle"),
            relationships: enum_list(v, "relationships", RELATIONSHIPS)?,
            no_relationships: flag(v, "includeNoRelationships"),
        })
    }
}
/// A validated `ListTasks` request.
#[derive(Debug)]
pub(crate) struct ListQuery {
    pub repository: Option<String>,
    progress: Vec<String>,
    proposal: Vec<String>,
    search: String,
    filter: Filter,
    key: Option<String>,
    descending: bool,
}
impl ListQuery {
    pub fn parse(q: &Value, repository: Option<String>) -> Result<Self> {
        let names = |field: &str, names: &[&str]| -> Result<Vec<String>> {
            q[field]
                .as_array()
                .into_iter()
                .flatten()
                .map(|v| e::enum_value(v, names, None))
                .collect()
        };
        let key = e::enum_value(&q["sortKey"], SORT_KEYS, Some("TASK_SORT_KEY_UNSPECIFIED"))?;
        let direction = e::enum_value(
            &q["sortDirection"],
            DIRECTIONS,
            Some("SORT_DIRECTION_UNSPECIFIED"),
        )?;
        Ok(Self {
            repository,
            progress: names("progress", e::PROGRESS)?,
            proposal: names("proposal", e::PROPOSAL)?,
            search: string(q, "search").case_fold().collect(),
            filter: Filter::parse(&q["filter"])?,
            key: (key != "TASK_SORT_KEY_UNSPECIFIED").then_some(key),
            descending: direction == "SORT_DIRECTION_DESCENDING",
        })
    }
}
/// A live task as the list evaluator sees it (accepted head without prompt text).
pub(crate) struct ListTask {
    pub id: String,
    pub repo: String,
    pub body: Value,
}
/// Derived relationship views need every live task of the account.
struct Relations {
    parents: HashSet<String>,
    blockers: HashSet<String>,
    related: HashSet<String>,
}
impl Relations {
    fn new(tasks: &[ListTask]) -> Self {
        let mut r = Relations {
            parents: HashSet::new(),
            blockers: HashSet::new(),
            related: HashSet::new(),
        };
        for t in tasks {
            r.parents
                .extend(opt_str(&t.body, "parentTaskId").map(str::to_owned));
            r.blockers.extend(strings(&t.body, "dependsOnTaskIds"));
            r.related.extend(
                strings(&t.body, "relatedTaskIds")
                    .into_iter()
                    .filter(|x| *x != t.id),
            );
        }
        r
    }
    fn kinds(&self, t: &ListTask) -> HashSet<&'static str> {
        let mut kinds = HashSet::new();
        if t.body.get("parentTaskId").is_some() {
            kinds.insert("RELATIONSHIP_KIND_PARENT");
        }
        if !strings(&t.body, "dependsOnTaskIds").is_empty() {
            kinds.insert("RELATIONSHIP_KIND_DEPENDS_ON");
        }
        if !strings(&t.body, "relatedTaskIds").is_empty() || self.related.contains(&t.id) {
            kinds.insert("RELATIONSHIP_KIND_RELATED");
        }
        if t.body.get("duplicateOfTaskId").is_some() {
            kinds.insert("RELATIONSHIP_KIND_DUPLICATE_OF");
        }
        if self.parents.contains(&t.id) {
            kinds.insert("RELATIONSHIP_KIND_SUBTASKS");
        }
        if self.blockers.contains(&t.id) {
            kinds.insert("RELATIONSHIP_KIND_BLOCKS");
        }
        kinds
    }
}
/// "Selection OR unassigned flag"; no selection and a false flag never restricts.
fn selected(selection: &[String], include_unassigned: bool, value: Option<&str>) -> bool {
    if selection.is_empty() && !include_unassigned {
        return true;
    }
    match value {
        None => include_unassigned,
        Some(v) => selection.iter().any(|s| s == v),
    }
}
fn in_dates(r: &DateRange, value: Option<&str>) -> bool {
    if r.from.is_none() && r.to.is_none() && !r.unassigned {
        return true;
    }
    let Some(value) = value else {
        return r.unassigned;
    };
    if r.from.is_none() && r.to.is_none() {
        return false;
    }
    r.from.as_deref().is_none_or(|f| value >= f) && r.to.as_deref().is_none_or(|t| value <= t)
}
fn in_points(r: &PointRange, value: Option<u64>) -> bool {
    if r.min.is_none() && r.max.is_none() && !r.unassigned {
        return true;
    }
    let Some(value) = value else {
        return r.unassigned;
    };
    if r.min.is_none() && r.max.is_none() {
        return false;
    }
    r.min.is_none_or(|m| value >= m) && r.max.is_none_or(|m| value <= m)
}
fn assigned_enum<'a>(b: &'a Value, key: &str, unspecified: &str) -> Option<&'a str> {
    b[key].as_str().filter(|v| *v != unspecified)
}
impl ListQuery {
    fn matches(&self, t: &ListTask, relations: &Relations) -> bool {
        if self.repository.as_ref().is_some_and(|r| *r != t.repo) {
            return false;
        }
        let b = &t.body;
        if !self.progress.is_empty() && !self.progress.iter().any(|p| p == string(b, "progress")) {
            return false;
        }
        if !self.proposal.is_empty() && !self.proposal.iter().any(|p| p == string(b, "proposal")) {
            return false;
        }
        if !self.search.is_empty()
            && !["title", "description"].iter().any(|f| {
                string(b, f)
                    .case_fold()
                    .collect::<String>()
                    .contains(&self.search)
            })
        {
            return false;
        }
        let f = &self.filter;
        if !selected(
            &f.priorities,
            f.priority_unassigned,
            assigned_enum(b, "priority", "TASK_PRIORITY_UNSPECIFIED"),
        ) || !selected(
            &f.types,
            f.type_unassigned,
            assigned_enum(b, "type", "TASK_TYPE_UNSPECIFIED"),
        ) {
            return false;
        }
        if !f.labels.is_empty() || f.unlabeled {
            let labels = strings(b, "labelIds");
            let any = labels.iter().any(|l| f.labels.contains(l));
            if !any && !(f.unlabeled && labels.is_empty()) {
                return false;
            }
        }
        if !in_dates(&f.start, opt_str(b, "startDate"))
            || !in_dates(&f.due, opt_str(b, "dueDate"))
            || !in_points(&f.points, b["storyPoints"].as_u64())
        {
            return false;
        }
        if !selected(&f.milestones, f.no_milestone, opt_str(b, "milestoneId"))
            || !selected(&f.cycles, f.no_cycle, opt_str(b, "cycleId"))
        {
            return false;
        }
        if !f.relationships.is_empty() || f.no_relationships {
            let kinds = relations.kinds(t);
            let any = f.relationships.iter().any(|k| kinds.contains(k.as_str()));
            if !any && !(f.no_relationships && kinds.is_empty()) {
                return false;
            }
        }
        true
    }
}
/// One task's sort key. Unassigned values have `assigned == false` and always sort last.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SortValue {
    pub assigned: bool,
    pub number: i64,
    pub text: String,
}
/// Name and start date of live milestones and cycles, for the two item-based sort keys.
#[derive(Default)]
pub(crate) struct SortLookups {
    pub milestone_names: BTreeMap<String, String>,
    pub cycle_starts: BTreeMap<String, String>,
}
impl ListQuery {
    fn sort_value(&self, t: &ListTask, lookups: &SortLookups) -> SortValue {
        let text = |s: Option<&str>| match s {
            Some(s) => SortValue {
                assigned: true,
                text: s.to_owned(),
                ..Default::default()
            },
            None => SortValue::default(),
        };
        let b = &t.body;
        match self.key.as_deref() {
            Some("TASK_SORT_KEY_PRIORITY") => {
                let n = PRIORITY
                    .iter()
                    .position(|p| Some(*p) == opt_str(b, "priority"))
                    .map(|i| i as i64 + 1);
                SortValue {
                    assigned: n.is_some(),
                    number: n.unwrap_or(0),
                    ..Default::default()
                }
            }
            Some("TASK_SORT_KEY_TYPE") => {
                let n = TYPE
                    .iter()
                    .position(|p| Some(*p) == opt_str(b, "type"))
                    .map(|i| i as i64 + 1);
                SortValue {
                    assigned: n.is_some(),
                    number: n.unwrap_or(0),
                    ..Default::default()
                }
            }
            Some("TASK_SORT_KEY_START_DATE") => text(opt_str(b, "startDate")),
            Some("TASK_SORT_KEY_DUE_DATE") => text(opt_str(b, "dueDate")),
            Some("TASK_SORT_KEY_STORY_POINTS") => match b["storyPoints"].as_u64() {
                Some(n) => SortValue {
                    assigned: true,
                    number: n as i64,
                    ..Default::default()
                },
                None => SortValue::default(),
            },
            Some("TASK_SORT_KEY_MILESTONE_NAME") => text(
                opt_str(b, "milestoneId")
                    .and_then(|id| lookups.milestone_names.get(id))
                    .map(|n| simple_lower(n))
                    .as_deref(),
            ),
            Some("TASK_SORT_KEY_CYCLE_START_DATE") => text(
                opt_str(b, "cycleId")
                    .and_then(|id| lookups.cycle_starts.get(id))
                    .map(String::as_str),
            ),
            _ => SortValue::default(),
        }
    }
    pub fn needs_milestones(&self) -> bool {
        self.key.as_deref() == Some("TASK_SORT_KEY_MILESTONE_NAME")
    }
    pub fn needs_cycles(&self) -> bool {
        self.key.as_deref() == Some("TASK_SORT_KEY_CYCLE_START_DATE")
    }
}
/// Unassigned last in both directions; ties by ascending task ID.
pub(crate) fn compare(
    a: &SortValue,
    b: &SortValue,
    a_id: &str,
    b_id: &str,
    descending: bool,
) -> Ordering {
    if a.assigned != b.assigned {
        return if a.assigned {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if a.assigned {
        let mut c = if a.number != b.number {
            a.number.cmp(&b.number)
        } else {
            a.text.as_bytes().cmp(b.text.as_bytes())
        };
        if descending {
            c = c.reverse();
        }
        if c != Ordering::Equal {
            return c;
        }
    }
    a_id.cmp(b_id)
}
/// Keyset position: the sort tuple and ID of the last returned task.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Position {
    pub value: SortValue,
    pub id: String,
}
impl Position {
    pub fn to_value(&self) -> Value {
        json!({"id":self.id,"assigned":self.value.assigned,"number":self.value.number,"text":self.value.text})
    }
    pub fn from_value(v: &Value) -> Result<Self> {
        Ok(Self {
            id: v["id"]
                .as_str()
                .ok_or_else(|| invalid("invalid cursor"))?
                .to_owned(),
            value: SortValue {
                assigned: v["assigned"]
                    .as_bool()
                    .ok_or_else(|| invalid("invalid cursor"))?,
                number: v["number"]
                    .as_i64()
                    .ok_or_else(|| invalid("invalid cursor"))?,
                text: string(v, "text").to_owned(),
            },
        })
    }
}
/// Evaluate a query over every live task: the page after `after` and the position of the last
/// returned task when more remain.
pub(crate) fn evaluate(
    query: &ListQuery,
    tasks: &[ListTask],
    lookups: &SortLookups,
    after: Option<&Position>,
    size: usize,
) -> (Vec<String>, Option<Position>) {
    let relations = Relations::new(tasks);
    let mut rows: Vec<(SortValue, &str)> = tasks
        .iter()
        .filter(|t| query.matches(t, &relations))
        .map(|t| (query.sort_value(t, lookups), t.id.as_str()))
        .collect();
    rows.sort_by(|a, b| compare(&a.0, &b.0, a.1, b.1, query.descending));
    let rest: Vec<_> = rows
        .into_iter()
        .filter(|(value, id)| {
            after.is_none_or(|a| {
                compare(value, &a.value, id, &a.id, query.descending) == Ordering::Greater
            })
        })
        .collect();
    if rest.len() <= size {
        return (
            rest.into_iter().map(|(_, id)| id.to_owned()).collect(),
            None,
        );
    }
    let last = &rest[size - 1];
    let next = Position {
        value: last.0.clone(),
        id: last.1.to_owned(),
    };
    (
        rest.into_iter()
            .take(size)
            .map(|(_, id)| id.to_owned())
            .collect(),
        Some(next),
    )
}

/// Filtered, sorted and keyset-paged task list over the account's accepted heads.
pub(crate) async fn list_tasks(db: &mut PgConnection, p: &Principal, q: &Value) -> Result<Value> {
    let size = e::page_size(&q["page"]["size"])?;
    let mut scope_q = q.clone();
    if let Some(m) = scope_q.as_object_mut() {
        m.remove("page");
    }
    let scope = format!("ListTasks:{}", e::fingerprint(&scope_q)?);
    let token = string(&q["page"], "cursor");
    let after = if token.is_empty() {
        None
    } else {
        Some(Position::from_value(
            &e::cursor(db, &p.account_id, &scope, token).await?,
        )?)
    };
    let repository = match q["repositoryId"].as_str() {
        Some(r) => Some(e::canonical_repository(db, &p.account_id, r).await?),
        None => None,
    };
    let query = ListQuery::parse(q, repository)?;
    // Prompt and capture content is not needed to filter or sort.
    let rows = sqlx::query("SELECT e.id,e.repository_id,(v.envelope->'task')-'prompt'-'captures' AS body FROM lane_entities e JOIN lane_versions v ON v.account_id=e.account_id AND v.entity_id=e.id AND v.version_id=e.accepted_version_id WHERE e.account_id=$1 AND e.kind=2 AND NOT e.deleted ORDER BY e.id")
        .bind(&p.account_id).fetch_all(&mut *db).await?;
    let tasks: Vec<ListTask> = rows
        .iter()
        .map(|r| ListTask {
            id: r.get("id"),
            repo: r
                .get::<Option<String>, _>("repository_id")
                .unwrap_or_default(),
            body: r.get("body"),
        })
        .collect();
    let mut lookups = SortLookups::default();
    if query.needs_milestones() || query.needs_cycles() {
        let kind: i16 = if query.needs_milestones() {
            MILESTONE
        } else {
            CYCLE
        };
        let rows = sqlx::query("SELECT e.id,v.envelope->$3->>'name' AS name,v.envelope->$3->>'startDate' AS start FROM lane_entities e JOIN lane_versions v ON v.account_id=e.account_id AND v.entity_id=e.id AND v.version_id=e.accepted_version_id WHERE e.account_id=$1 AND e.kind=$2 AND NOT e.deleted")
            .bind(&p.account_id).bind(kind).bind(item_key(kind)).fetch_all(&mut *db).await?;
        for row in rows {
            let id: String = row.get("id");
            if kind == MILESTONE {
                lookups
                    .milestone_names
                    .insert(id, row.get::<Option<String>, _>("name").unwrap_or_default());
            } else if let Some(start) = row.get::<Option<String>, _>("start") {
                lookups.cycle_starts.insert(id, start);
            }
        }
    }
    let (ids, next) = evaluate(&query, &tasks, &lookups, after.as_ref(), size);
    let mut items = Vec::with_capacity(ids.len());
    for id in &ids {
        items.push(e::result(db, &p.account_id, id).await?);
    }
    let cursor = match next {
        Some(position) => e::new_cursor(db, &p.account_id, &scope, &position.to_value()).await?,
        None => String::new(),
    };
    Ok(json!({"items":items,"nextCursor":cursor}))
}

/// Stable entity-ID listing of labels, milestones and cycles.
pub(crate) async fn list_items(db: &mut PgConnection, p: &Principal, q: &Value) -> Result<Value> {
    let size = e::page_size(&q["page"]["size"])?;
    let mut scope_q = q.clone();
    if let Some(m) = scope_q.as_object_mut() {
        m.remove("page");
    }
    let scope = format!("ListPlanningItems:{}", e::fingerprint(&scope_q)?);
    let token = string(&q["page"], "cursor");
    let after = if token.is_empty() {
        String::new()
    } else {
        string(&e::cursor(db, &p.account_id, &scope, token).await?, "after").to_owned()
    };
    let kinds: Vec<i16> = match e::enum_value(
        &q["kind"],
        &[
            "ENTITY_KIND_REPOSITORY",
            "ENTITY_KIND_TASK",
            "ENTITY_KIND_URL_CONNECTION",
            "ENTITY_KIND_LABEL",
            "ENTITY_KIND_MILESTONE",
            "ENTITY_KIND_CYCLE",
        ],
        Some("ENTITY_KIND_UNSPECIFIED"),
    )?
    .as_str()
    {
        "ENTITY_KIND_UNSPECIFIED" => vec![LABEL, MILESTONE, CYCLE],
        "ENTITY_KIND_LABEL" => vec![LABEL],
        "ENTITY_KIND_MILESTONE" => vec![MILESTONE],
        "ENTITY_KIND_CYCLE" => vec![CYCLE],
        _ => {
            return Err(invalid(
                "kind must be label, milestone, cycle or unspecified",
            ));
        }
    };
    let repository = match q["repositoryId"].as_str() {
        Some(r) => Some(e::canonical_repository(db, &p.account_id, r).await?),
        None => None,
    };
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM lane_entities WHERE account_id=$1 AND kind=ANY($2) AND NOT deleted AND id>$3 AND ($4::text IS NULL OR repository_id=$4) ORDER BY id LIMIT $5")
        .bind(&p.account_id).bind(&kinds).bind(&after).bind(&repository).bind((size + 1) as i64)
        .fetch_all(&mut *db).await?;
    let more = ids.len() > size;
    let mut items = Vec::new();
    for id in ids.iter().take(size) {
        items.push(e::result(db, &p.account_id, id).await?);
    }
    let next = if more {
        e::new_cursor(db, &p.account_id, &scope, &json!({"after":ids[size - 1]})).await?
    } else {
        String::new()
    };
    Ok(json!({"items":items,"nextCursor":next}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use delino_protocol::lane::v1::{
        CreatePlanningItemRequest, CreateTaskRequest, ListTasksRequest, SavePlanningItemRequest,
        SaveTaskRequest,
    };

    fn fixture(name: &str) -> Value {
        let root = std::env::var("LANE_CONFORMANCE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../protos/lane/v1/fixtures")
            });
        serde_json::from_str(&std::fs::read_to_string(root.join(format!("{name}.json"))).unwrap())
            .unwrap()
    }
    /// Round-trip a fixture request through the generated message type, like the transport.
    fn through<T: serde::Serialize + serde::de::DeserializeOwned>(v: &Value) -> Value {
        serde_json::to_value(serde_json::from_value::<T>(v.clone()).unwrap()).unwrap()
    }
    fn kind_of(name: &str) -> i16 {
        match name {
            "ENTITY_KIND_REPOSITORY" => e::REPOSITORY,
            "ENTITY_KIND_TASK" => TASK,
            "ENTITY_KIND_LABEL" => LABEL,
            "ENTITY_KIND_MILESTONE" => MILESTONE,
            "ENTITY_KIND_CYCLE" => CYCLE,
            other => panic!("unexpected kind {other}"),
        }
    }
    /// Live heads of a fixture state; deleted entries are tombstones.
    fn snapshot(state: &Value) -> Snapshot {
        let mut snapshot = Snapshot::default();
        for entity in state["entities"].as_array().unwrap() {
            if entity["deleted"] == true {
                continue;
            }
            let id = string(entity, "entityId").to_owned();
            let kind = kind_of(string(entity, "kind"));
            let body = &entity[e::body_key(kind)];
            if kind == e::REPOSITORY {
                continue;
            }
            let repo = string(body, "repositoryId").to_owned();
            if kind == TASK {
                snapshot.tasks.insert(
                    id,
                    TaskState {
                        repo,
                        bodies: vec![body.clone()],
                    },
                );
            } else {
                snapshot.items.insert(
                    id,
                    ItemState {
                        kind,
                        repo,
                        names: vec![string(body, "name").to_owned()],
                    },
                );
            }
        }
        snapshot
    }
    fn code(f: &Fault) -> (i32, Option<Violation>) {
        (f.reason & 0xff, violation_of(f))
    }
    fn expect_error(expected: &Value, got: &Fault, id: &str) {
        let want_reason = match string(expected, "reason") {
            "ERROR_REASON_INVALID_INPUT" => 1,
            "ERROR_REASON_CONSTRAINT_VIOLATION" => CONSTRAINT_REASON,
            other => panic!("unexpected reason {other}"),
        };
        let want_violation = expected
            .get("violation")
            .and_then(Value::as_str)
            .map(|name| {
                (1..=10)
                    .filter_map(Violation::from_number)
                    .find(|v| v.name() == name)
                    .unwrap_or_else(|| panic!("unknown violation {name}"))
            });
        assert_eq!(code(got), (want_reason, want_violation), "{id}: {got:?}");
    }
    fn apply_task(snapshot: &Snapshot, method: &str, request: &Value) -> Result<TaskWrite> {
        let (id, body, edits) = {
            let q = if method == "SaveTask" {
                through::<SaveTaskRequest>(request)
            } else {
                through::<CreateTaskRequest>(request)
            };
            let id = string(&q, "id").to_owned();
            let edits = Edits::from_value(&q["relationshipEdits"])?;
            let body = if method == "SaveTask" {
                e::task_body(&q["task"], false)?
            } else {
                let mut task = q.clone();
                task["repositoryId"] = q["repository"]["id"].clone();
                e::task_body(&task, true)?
            };
            (id, body, edits)
        };
        let repo = string(&body, "repositoryId").to_owned();
        snapshot.write_task(
            &id,
            &repo,
            body,
            &edits,
            if method == "SaveTask" {
                Mode::Save
            } else {
                Mode::Create
            },
            true,
        )
    }
    fn expected_bodies(list: &Value) -> BTreeMap<String, Value> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|v| (string(v, "entityId").to_owned(), v["task"].clone()))
            .collect()
    }

    #[test]
    fn shared_validation_vectors() {
        let suite = fixture("planning-validation");
        for case in suite["cases"].as_array().unwrap() {
            let id = string(case, "id");
            let snapshot = snapshot(&suite["state"]);
            let method = string(&case["input"], "method");
            let request = &case["input"]["request"];
            let expected = &case["expected"];
            let outcome: Result<Value> = (|| match method {
                "CreatePlanningItem" | "SavePlanningItem" => {
                    let q = if method == "CreatePlanningItem" {
                        through::<CreatePlanningItemRequest>(request)
                    } else {
                        through::<SavePlanningItemRequest>(request)
                    };
                    let (kind, body) = planning_body(&q["item"])?;
                    snapshot.check_item(
                        string(&q, "id"),
                        kind,
                        string(&body, "repositoryId"),
                        string(&body, "name"),
                    )?;
                    Ok(json!({ e::body_key(kind): body }))
                }
                "SaveTask" => Ok(json!({"task": apply_task(&snapshot, method, request)?.body})),
                other => panic!("unknown method {other}"),
            })();
            match string(expected, "outcome") {
                "error" => expect_error(expected, &outcome.expect_err(id), id),
                _ => assert_eq!(
                    outcome.unwrap_or_else(|f| panic!("{id}: {f:?}")),
                    expected["normalized"],
                    "{id}"
                ),
            }
        }
    }
    #[test]
    fn shared_graph_vectors() {
        let suite = fixture("planning-graph");
        for case in suite["cases"].as_array().unwrap() {
            let id = string(case, "id");
            let snapshot = snapshot(&suite["state"]);
            let method = string(&case["input"], "method");
            let expected = &case["expected"];
            let outcome = apply_task(&snapshot, method, &case["input"]["request"]);
            match string(expected, "outcome") {
                "error" => expect_error(expected, &outcome.expect_err(id), id),
                _ => {
                    let write = outcome.unwrap_or_else(|f| panic!("{id}: {f:?}"));
                    assert_eq!(json!({"task": write.body}), expected["normalized"], "{id}");
                    assert_eq!(write.others, expected_bodies(&expected["affected"]), "{id}");
                }
            }
        }
    }
    #[test]
    fn shared_cleanup_vectors() {
        let suite = fixture("planning-cleanup");
        for case in suite["cases"].as_array().unwrap() {
            let id = string(case, "id");
            let snapshot = snapshot(&suite["state"]);
            let method = string(&case["input"], "method");
            let request = &case["input"]["request"];
            let expected = &case["expected"];
            let (mut changed, deleted): (BTreeMap<String, Value>, Vec<String>) = match method {
                "SaveTask" => {
                    let write = apply_task(&snapshot, method, request).unwrap();
                    let mut others = write.others;
                    others.insert(string(request, "id").to_owned(), write.body);
                    (others, vec![])
                }
                "DeleteTask" => (
                    snapshot.delete_task(string(request, "id")),
                    vec![string(request, "id").to_owned()],
                ),
                "DeletePlanningItem" => (
                    snapshot.delete_item(string(request, "id")),
                    vec![string(request, "id").to_owned()],
                ),
                other => panic!("unknown method {other}"),
            };
            let want_deleted: Vec<String> = expected["deleted"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            assert_eq!(deleted, want_deleted, "{id}");
            assert_eq!(
                std::mem::take(&mut changed),
                expected_bodies(&expected["changed"]),
                "{id}"
            );
        }
    }
    #[test]
    fn list_vectors_match_the_reference_order_and_pages() {
        let suite = fixture("planning-list");
        let snapshot = snapshot(&suite["dataset"]);
        let tasks: Vec<ListTask> = snapshot
            .tasks
            .iter()
            .map(|(id, t)| ListTask {
                id: id.clone(),
                repo: t.repo.clone(),
                body: t.bodies[0].clone(),
            })
            .collect();
        let mut lookups = SortLookups::default();
        for entity in suite["dataset"]["entities"].as_array().unwrap() {
            if entity["deleted"] == true {
                continue;
            }
            let id = string(entity, "entityId").to_owned();
            match string(entity, "kind") {
                "ENTITY_KIND_MILESTONE" => {
                    lookups
                        .milestone_names
                        .insert(id, string(&entity["milestone"], "name").to_owned());
                }
                "ENTITY_KIND_CYCLE" => {
                    lookups
                        .cycle_starts
                        .insert(id, string(&entity["cycle"], "startDate").to_owned());
                }
                _ => {}
            }
        }
        assert!(suite["cases"].as_array().unwrap().len() >= 40);
        for case in suite["cases"].as_array().unwrap() {
            let id = string(case, "id");
            let request = through::<ListTasksRequest>(&case["input"]["request"]);
            let query = ListQuery::parse(
                &request,
                request["repositoryId"].as_str().map(str::to_owned),
            )
            .unwrap_or_else(|f| panic!("{id}: {f:?}"));
            let size = request["page"]["size"].as_u64().unwrap() as usize;
            let strs = |v: &Value| -> Vec<String> {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_owned())
                    .collect()
            };
            let all = evaluate(&query, &tasks, &lookups, None, usize::MAX).0;
            assert_eq!(all, strs(&case["expected"]["taskIds"]), "{id}: order");
            let mut pages: Vec<Vec<String>> = Vec::new();
            let mut cursor: Option<Position> = None;
            loop {
                let (page, next) = evaluate(&query, &tasks, &lookups, cursor.as_ref(), size);
                pages.push(page);
                match next {
                    // A persisted cursor must round-trip losslessly.
                    Some(position) => {
                        cursor = Some(Position::from_value(&position.to_value()).unwrap())
                    }
                    None => break,
                }
            }
            let want: Vec<Vec<String>> = case["expected"]["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(strs)
                .collect();
            assert_eq!(pages, want, "{id}: pages");
        }
    }
    #[test]
    fn violations_round_trip_through_the_error_detail() {
        for n in 1..=10 {
            let v = Violation::from_number(n).unwrap();
            let f = violation(v, "x");
            assert_eq!(violation_of(&f), Some(v));
            let detail = crate::error::detail(&f, "c", "e", vec![]);
            assert_eq!(
                detail.reason,
                delino_protocol::lane::v1::ErrorReason::ConstraintViolation
            );
            assert_eq!(detail.violation.to_i32(), n);
            assert!(!detail.retryable);
        }
        assert_eq!(violation_of(&fault(1, "x")), None);
    }
    #[test]
    fn scalar_rules() {
        assert!(valid_date("2024-02-29") && valid_date("2000-02-29"));
        assert!(!valid_date("1900-02-29") && !valid_date("2026-13-01") && !valid_date("2026-1-01"));
        assert!(
            !valid_date("2026-02-30") && !valid_date("") && !valid_date("2026-01-01T00:00:00Z")
        );
        assert_eq!(simple_lower("\u{c9}CLAIR \u{130}"), "\u{e9}clair i");
    }
}
