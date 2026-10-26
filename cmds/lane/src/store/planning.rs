// SPDX-License-Identifier: Apache-2.0
//! Task planning rules applied inside the local write transaction.
//!
//! The functions mirror the executable specification in
//! `protos/lane/v1/planning_reference_test.go` and replay the shared fixtures: planning item
//! validation (labels, milestones, cycles), task metadata validation, relationship graph checks,
//! symmetric related links, relationship edits and the cleanup performed by task moves, task
//! deletion and planning item deletion. Everything here only reads the database; callers write
//! the resulting versions in the same transaction so a violation leaves no partial state.
use super::{
    Connection, Failure, OptionalExtension, Result, Value,
    domain::{heads, mapped},
    invalid, json, rows, strv,
};
use crate::error::Violation;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const KIND_TASK: i64 = 2;
pub const KIND_LABEL: i64 = 4;
pub const KIND_MILESTONE: i64 = 5;
pub const KIND_CYCLE: i64 = 6;
const MAX_NAME: usize = 100;
const MAX_DESCRIPTION: usize = 4000;
const MAX_LABELS: usize = 100;
const MAX_STORY_POINTS: u64 = 2_147_483_647;
const PRIORITIES: [&str; 5] = [
    "TASK_PRIORITY_UNSPECIFIED",
    "TASK_PRIORITY_URGENT",
    "TASK_PRIORITY_HIGH",
    "TASK_PRIORITY_NORMAL",
    "TASK_PRIORITY_LOW",
];
const TYPES: [&str; 6] = [
    "TASK_TYPE_UNSPECIFIED",
    "TASK_TYPE_BUG",
    "TASK_TYPE_FEATURE",
    "TASK_TYPE_IMPROVEMENT",
    "TASK_TYPE_DOCUMENTATION",
    "TASK_TYPE_MAINTENANCE",
];
/// Task fields that hold sets of IDs. They are stored sorted and unique.
const SET_FIELDS: [&str; 3] = ["labelIds", "dependsOnTaskIds", "relatedTaskIds"];

/// Stable numeric entity kinds shared with the SQL schema.
#[must_use]
pub fn kind_number(kind: &str) -> i64 {
    match kind {
        "ENTITY_KIND_REPOSITORY" => 1,
        "ENTITY_KIND_URL_CONNECTION" => 3,
        "ENTITY_KIND_LABEL" => KIND_LABEL,
        "ENTITY_KIND_MILESTONE" => KIND_MILESTONE,
        "ENTITY_KIND_CYCLE" => KIND_CYCLE,
        _ => KIND_TASK,
    }
}
#[must_use]
pub fn is_planning_kind(kind: &str) -> bool {
    matches!(
        kind,
        "ENTITY_KIND_LABEL" | "ENTITY_KIND_MILESTONE" | "ENTITY_KIND_CYCLE"
    )
}
/// The `EntityVersion` body field that carries a planning kind.
#[must_use]
pub fn body_field(kind: &str) -> Option<&'static str> {
    match kind {
        "ENTITY_KIND_LABEL" => Some("label"),
        "ENTITY_KIND_MILESTONE" => Some("milestone"),
        "ENTITY_KIND_CYCLE" => Some("cycle"),
        _ => None,
    }
}
fn violation(name: Violation, entity: &str) -> Failure {
    Failure::constraint(
        name,
        entity,
        &format!(
            "Planning constraint violated: {}.",
            name.as_str()
                .trim_start_matches("CONSTRAINT_VIOLATION_")
                .to_lowercase()
                .replace('_', " ")
        ),
    )
}
/// Unicode simple lowercase mapping, matching the reference evaluator (per code point, no
/// multi-character expansions). Used for case-insensitive names and milestone-name sorting.
#[must_use]
pub fn simple_lowercase(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut lower = c.to_lowercase();
            let first = lower.next().unwrap_or(c);
            if lower.next().is_some() && c == '\u{130}' {
                // U+0130 is the only code point whose full mapping expands; its simple
                // mapping is U+0069.
                'i'
            } else {
                first
            }
        })
        .collect()
}
/// A real proleptic Gregorian `YYYY-MM-DD` date.
#[must_use]
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || ![0, 1, 2, 3, 5, 6, 8, 9]
            .iter()
            .all(|&i| b[i].is_ascii_digit())
    {
        return false;
    }
    let year: u32 = s[0..4].parse().unwrap_or(0);
    let month: usize = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..10].parse().unwrap_or(0);
    if !(1..=12).contains(&month) || day < 1 {
        return false;
    }
    let mut days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) {
        days[1] = 29;
    }
    day <= days[month - 1]
}
fn opt<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}
/// Read a set-valued task field.
#[must_use]
pub fn list(v: &Value, key: &str) -> Vec<String> {
    v.get(key).and_then(Value::as_array).map_or_default(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}
fn sorted_unique(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}
fn set_optional(v: &mut Value, key: &str, value: Option<String>) {
    let object = v.as_object_mut().expect("task object");
    match value {
        Some(s) => {
            object.insert(key.to_owned(), s.into());
        }
        None => {
            object.remove(key);
        }
    }
}
fn set_list(v: &mut Value, key: &str, value: Vec<String>) {
    let object = v.as_object_mut().expect("task object");
    if value.is_empty() {
        object.remove(key);
    } else {
        object.insert(key.to_owned(), json!(value));
    }
}
/// Drop unassigned planning values so equal tasks compare and store identically.
pub fn compact(task: &mut Value) {
    let Some(object) = task.as_object_mut() else {
        return;
    };
    for key in SET_FIELDS {
        if object
            .get(key)
            .is_some_and(|v| v.as_array().is_some_and(Vec::is_empty))
        {
            object.remove(key);
        }
    }
    for (key, default) in [
        ("priority", "TASK_PRIORITY_UNSPECIFIED"),
        ("type", "TASK_TYPE_UNSPECIFIED"),
    ] {
        if object.get(key).is_some_and(|v| v == default) {
            object.remove(key);
        }
    }
    object.retain(|_, v| !v.is_null());
}
fn canon(task: &Value) -> Value {
    let mut copy = task.clone();
    compact(&mut copy);
    copy
}

/// Live head bodies of every task in a repository, keyed by task ID. A conflicted task has
/// several bodies; checks use their union of references.
pub type TaskSet = BTreeMap<String, Vec<Value>>;
pub(super) fn load_repository(c: &Connection, repository: &str) -> Result<TaskSet> {
    let mut out = TaskSet::new();
    for id in rows(
        c,
        "SELECT id FROM entities WHERE kind=2 AND deleted=0 AND repository_id=? ORDER BY id",
        [repository],
    )? {
        let bodies: Vec<Value> = heads(c, &id)?
            .into_iter()
            .filter(|v| v["task"].is_object())
            .map(|v| v["task"].clone())
            .collect();
        if !bodies.is_empty() {
            out.insert(id, bodies);
        }
    }
    Ok(out)
}
fn entity_info(c: &Connection, id: &str) -> Result<Option<(i64, Option<String>, bool)>> {
    Ok(c.query_row(
        "SELECT kind,repository_id,deleted FROM entities WHERE id=?",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()?)
}
fn check_planning_reference(c: &Connection, id: &str, kind: i64, repository: &str) -> Result<()> {
    match entity_info(c, id)? {
        Some((k, repo, false)) if k == kind => {
            if repo.as_deref() == Some(repository) {
                Ok(())
            } else {
                Err(violation(Violation::CrossRepository, id))
            }
        }
        _ => Err(violation(Violation::ReferenceNotFound, id)),
    }
}
/// Check the repository a planning item or task lives in: it must be a live, unconflicted
/// repository. Returns its canonical ID.
pub(super) fn live_repository(c: &Connection, id: &str) -> Result<String> {
    if !super::valid_id(id) {
        return Err(invalid("Invalid repository ID."));
    }
    let canonical = mapped(c, id)?;
    let h = heads(c, &canonical)?;
    if h[0]["kind"] != "ENTITY_KIND_REPOSITORY" || h[0]["deleted"] == true {
        return Err(Failure::new("DELETED", "Repository is deleted or invalid."));
    }
    if h.len() > 1 {
        return Err(Failure::new(
            "CONFLICT_READ_ONLY",
            "Repository has unresolved conflicts.",
        ));
    }
    Ok(canonical)
}

/// Normalize and validate the planning metadata of a task body in place (`repository` is its
/// canonical repository ID). Mirrors `validateTaskFields` of the reference evaluator.
pub(super) fn validate_task_fields(c: &Connection, t: &mut Value, repository: &str) -> Result<()> {
    if t.get("priority")
        .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|s| PRIORITIES.contains(&s)))
        || t.get("type")
            .is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|s| TYPES.contains(&s)))
    {
        return Err(invalid("Invalid task priority or type."));
    }
    for key in ["startDate", "dueDate"] {
        if let Some(v) = t.get(key).filter(|v| !v.is_null())
            && !v.as_str().is_some_and(valid_date)
        {
            return Err(invalid(
                "Task dates must be real YYYY-MM-DD calendar dates.",
            ));
        }
    }
    if let Some(points) = t.get("storyPoints").filter(|v| !v.is_null()) {
        match points.as_u64() {
            Some(n) if n <= MAX_STORY_POINTS => {}
            _ => {
                return Err(violation(Violation::FieldRange, ""));
            }
        }
    }
    if let (Some(start), Some(due)) = (opt(t, "startDate"), opt(t, "dueDate"))
        && start > due
    {
        return Err(violation(Violation::DateOrder, ""));
    }
    let labels = sorted_unique(list(t, "labelIds"));
    if labels.len() > MAX_LABELS {
        return Err(violation(Violation::LabelLimit, ""));
    }
    for id in &labels {
        check_planning_reference(c, id, KIND_LABEL, repository)?;
    }
    set_list(t, "labelIds", labels);
    for (key, kind) in [("milestoneId", KIND_MILESTONE), ("cycleId", KIND_CYCLE)] {
        if let Some(id) = opt(t, key).map(str::to_owned) {
            check_planning_reference(c, &id, kind, repository)?;
        }
    }
    for key in ["dependsOnTaskIds", "relatedTaskIds"] {
        let ids = sorted_unique(list(t, key));
        set_list(t, key, ids);
    }
    compact(t);
    Ok(())
}
fn clear_references(t: &mut Value) {
    for key in [
        "labelIds",
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "dependsOnTaskIds",
        "relatedTaskIds",
        "duplicateOfTaskId",
    ] {
        t.as_object_mut().expect("task object").remove(key);
    }
}
/// Remove every reference to `id` from a task body; reports whether it changed.
fn remove_task_reference(t: &mut Value, id: &str) -> bool {
    let mut changed = false;
    for key in ["parentTaskId", "duplicateOfTaskId"] {
        if opt(t, key) == Some(id) {
            set_optional(t, key, None);
            changed = true;
        }
    }
    for key in ["dependsOnTaskIds", "relatedTaskIds"] {
        let before = list(t, key);
        if before.iter().any(|x| x == id) {
            set_list(t, key, before.into_iter().filter(|x| x != id).collect());
            changed = true;
        }
    }
    changed
}
/// Result of a planned task write: the stored primary body and the new bodies of every other
/// task the write changes (relationship edits, symmetric links, cleanup), keyed by task ID.
pub struct TaskWrite {
    pub primary: Value,
    pub others: BTreeMap<String, Value>,
}
/// Compare working copies with the originals and return changed tasks. A conflicted task that
/// would change fails the whole operation (`CONFLICT_READ_ONLY`).
fn changed_others(
    work: &TaskSet,
    original: &TaskSet,
    skip: &str,
) -> Result<BTreeMap<String, Value>> {
    let mut out = BTreeMap::new();
    for (id, heads) in work {
        if id == skip {
            continue;
        }
        let before = original.get(id);
        let differs = before.is_none_or(|b| {
            b.len() != heads.len() || b.iter().zip(heads).any(|(x, y)| canon(x) != canon(y))
        });
        if differs {
            if heads.len() > 1 {
                return Err(Failure::new(
                    "CONFLICT_READ_ONLY",
                    "A related task has unresolved conflicts; resolve it first.",
                ));
            }
            out.insert(id.clone(), canon(&heads[0]));
        }
    }
    Ok(out)
}
fn union_refs(heads: &[Value], key: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for h in heads {
        if key == "dependsOnTaskIds" {
            out.extend(list(h, key));
        } else if let Some(s) = opt(h, key) {
            out.insert(s.to_owned());
        }
    }
    out.into_iter().collect()
}
/// Iterative three-colour DFS over `key` references starting from `starts`.
fn has_cycle(work: &TaskSet, key: &str, starts: &BTreeSet<String>) -> bool {
    // 1 = on the current path, 2 = fully explored.
    let mut state: HashMap<String, u8> = HashMap::new();
    for start in starts {
        if state.contains_key(start) {
            continue;
        }
        state.insert(start.clone(), 1);
        let edges = |id: &str| work.get(id).map_or_else(Vec::new, |h| union_refs(h, key));
        let mut stack: Vec<(String, Vec<String>, usize)> = vec![(start.clone(), edges(start), 0)];
        while let Some((node, targets, index)) = stack.last_mut() {
            if let Some(next) = targets.get(*index).cloned() {
                *index += 1;
                match state.get(&next) {
                    Some(1) => return true,
                    Some(_) => {}
                    None => {
                        state.insert(next.clone(), 1);
                        let e = edges(&next);
                        stack.push((next, e, 0));
                    }
                }
            } else {
                state.insert(node.clone(), 2);
                stack.pop();
            }
        }
    }
    false
}

/// Validate and plan a task create or save. `previous` is the task's current single-head body
/// for a save, which enables move handling; resolutions and creations pass `None`.
///
/// A `stale` write is preserved as an alternative head next to the current heads and never
/// edits other tasks, so it is validated against exactly the graph that will be persisted:
/// the other tasks unchanged, the task with its existing heads plus the alternative. Its
/// relationship edits and move cleanup are discarded rather than applied during validation.
pub(super) fn write_task(
    c: &Connection,
    id: &str,
    mut t: Value,
    edits: &Value,
    previous: &[&Value],
    resolving: bool,
    stale: bool,
) -> Result<TaskWrite> {
    let no_edits = Value::Null;
    let edits = if stale { &no_edits } else { edits };
    let repository = live_repository(c, strv(&t, "repositoryId"))?;
    // Repositories the task leaves. A save names the repository of its single parent head. A
    // resolution leaves the repository the entity is projected in, where other tasks still
    // reference it, plus the repository of every resolved-away head; it is a move only when the
    // projection differs from the resolved repository, so keeping the accepted repository keeps
    // the resolved references.
    let mut sources = BTreeSet::new();
    let projected = if resolving {
        entity_info(c, id)?.and_then(|(_, projected, _)| projected)
    } else {
        None
    };
    if !resolving || projected.as_ref().is_some_and(|p| *p != repository) {
        sources.extend(projected);
        for prev in previous {
            sources.insert(mapped(c, strv(prev, "repositoryId"))?);
        }
        sources.remove(&repository);
    }
    if !sources.is_empty() {
        // A move discards the references sent with it, clears the stored ones and removes
        // every other task's reference to the moved task.
        clear_references(&mut t);
        validate_task_fields(c, &mut t, &repository)?;
        if stale {
            // The alternative is stored without removing other tasks' references.
            return Ok(TaskWrite {
                primary: t,
                others: BTreeMap::new(),
            });
        }
        let mut others = BTreeMap::new();
        for source in &sources {
            let mut work = load_repository(c, source)?;
            work.remove(id);
            let before = work.clone();
            for heads in work.values_mut() {
                for h in heads {
                    remove_task_reference(h, id);
                }
            }
            others.extend(changed_others(&work, &before, id)?);
        }
        return Ok(TaskWrite { primary: t, others });
    }
    validate_task_fields(c, &mut t, &repository)?;
    let mut work = load_repository(c, &repository)?;
    let original = work.clone();
    // Collect every task this write names so each reference check runs once.
    let mut links: Vec<String> = vec![];
    links.extend(list(&t, "dependsOnTaskIds"));
    links.extend(list(&t, "relatedTaskIds"));
    for key in ["parentTaskId", "duplicateOfTaskId"] {
        links.extend(opt(&t, key).map(str::to_owned));
    }
    let edit = |key: &str| sorted_unique(list(edits, key));
    for key in ["addSubtaskIds", "addBlockedTaskIds"] {
        links.extend(edit(key));
    }
    if let Some(link) = links.iter().find(|l| *l == id) {
        return Err(violation(Violation::SelfLink, link));
    }
    for link in &links {
        let live_elsewhere = !work.contains_key(link)
            && entity_info(c, link)?
                .is_some_and(|(kind, _, deleted)| kind == KIND_TASK && !deleted);
        if !work.contains_key(link) && !live_elsewhere {
            return Err(violation(Violation::ReferenceNotFound, link));
        }
    }
    for link in &links {
        if !work.contains_key(link) {
            return Err(violation(Violation::CrossRepository, link));
        }
    }
    // Related links are stored on both endpoints.
    // Previously stored links (of every head, for a resolution) are unlinked or re-linked too.
    let mut related = list(&t, "relatedTaskIds");
    for h in work.get(id).into_iter().flatten() {
        related.extend(list(h, "relatedTaskIds"));
    }
    let related_now = list(&t, "relatedTaskIds");
    // Symmetric related links of a stale alternative are not written to the other tasks.
    for r in sorted_unique(related).into_iter().filter(|_| !stale) {
        if let Some(heads) = work.get_mut(&r) {
            for h in heads.iter_mut() {
                let mut ids: Vec<String> = list(h, "relatedTaskIds")
                    .into_iter()
                    .filter(|x| x != id)
                    .collect();
                if related_now.contains(&r) {
                    ids.push(id.to_owned());
                }
                set_list(h, "relatedTaskIds", sorted_unique(ids));
            }
        }
    }
    if stale {
        work.entry(id.to_owned()).or_default().push(t.clone());
    } else {
        work.insert(id.to_owned(), vec![t.clone()]);
    }
    for r in edit("addSubtaskIds") {
        for h in work.get_mut(&r).into_iter().flatten() {
            set_optional(h, "parentTaskId", Some(id.to_owned()));
        }
    }
    for r in edit("removeSubtaskIds") {
        for h in work.get_mut(&r).into_iter().flatten() {
            if opt(h, "parentTaskId") == Some(id) {
                set_optional(h, "parentTaskId", None);
            }
        }
    }
    for r in edit("addBlockedTaskIds") {
        for h in work.get_mut(&r).into_iter().flatten() {
            let mut ids = list(h, "dependsOnTaskIds");
            ids.push(id.to_owned());
            set_list(h, "dependsOnTaskIds", sorted_unique(ids));
        }
    }
    for r in edit("removeBlockedTaskIds") {
        for h in work.get_mut(&r).into_iter().flatten() {
            let ids = list(h, "dependsOnTaskIds");
            set_list(
                h,
                "dependsOnTaskIds",
                ids.into_iter().filter(|x| x != id).collect(),
            );
        }
    }
    let others = changed_others(&work, &original, id)?;
    let mut starts: BTreeSet<String> = others.keys().cloned().collect();
    starts.insert(id.to_owned());
    // Only chains that start at a written task can contain a new cycle.
    if has_cycle(&work, "parentTaskId", &starts) {
        return Err(violation(Violation::ParentCycle, id));
    }
    if has_cycle(&work, "dependsOnTaskIds", &starts) {
        return Err(violation(Violation::DependencyCycle, id));
    }
    if has_cycle(&work, "duplicateOfTaskId", &starts) {
        return Err(violation(Violation::DuplicateCycle, id));
    }
    Ok(TaskWrite { primary: t, others })
}
/// Cleanup for a deleted task: remove every incoming reference from other tasks.
pub(super) fn delete_task_cleanup(
    c: &Connection,
    id: &str,
    repository: &str,
) -> Result<BTreeMap<String, Value>> {
    let original = load_repository(c, repository)?;
    let mut work = original.clone();
    work.remove(id);
    for heads in work.values_mut() {
        for h in heads {
            remove_task_reference(h, id);
        }
    }
    changed_others(&work, &original, id)
}
/// Cleanup for a deleted label, milestone or cycle: detach it from every task referencing it.
pub(super) fn delete_planning_cleanup(
    c: &Connection,
    id: &str,
    repository: &str,
) -> Result<BTreeMap<String, Value>> {
    let original = load_repository(c, repository)?;
    let mut work = original.clone();
    for heads in work.values_mut() {
        for h in heads {
            let labels: Vec<String> = list(h, "labelIds")
                .into_iter()
                .filter(|x| x != id)
                .collect();
            set_list(h, "labelIds", labels);
            for key in ["milestoneId", "cycleId"] {
                if opt(h, key) == Some(id) {
                    set_optional(h, key, None);
                }
            }
        }
    }
    changed_others(&work, &original, "")
}

fn scalars(s: &str) -> usize {
    s.chars().count()
}
/// Validate and normalize a planning item body (`item` is the inner `label`, `milestone` or
/// `cycle` object). `existing` is the item's current version when this is a save or resolution.
/// Returns the entity kind name and the normalized body.
pub(super) fn write_planning(
    c: &Connection,
    id: &str,
    item: &Value,
    existing: Option<&Value>,
) -> Result<(&'static str, Value)> {
    let present: Vec<&str> = ["label", "milestone", "cycle"]
        .into_iter()
        .filter(|k| item.get(*k).is_some_and(Value::is_object))
        .collect();
    let [field] = present[..] else {
        return Err(invalid(
            "Exactly one of label, milestone or cycle is required.",
        ));
    };
    let (kind, number) = match field {
        "label" => ("ENTITY_KIND_LABEL", KIND_LABEL),
        "milestone" => ("ENTITY_KIND_MILESTONE", KIND_MILESTONE),
        _ => ("ENTITY_KIND_CYCLE", KIND_CYCLE),
    };
    let mut body = item[field].clone();
    if strv(&body, "repositoryId").is_empty() {
        return Err(invalid("A repository ID is required."));
    }
    let repository = live_repository(c, strv(&body, "repositoryId"))?;
    if let Some(old) = existing {
        let old_field = body_field(strv(old, "kind"));
        if old_field != Some(field) || mapped(c, strv(&old[field], "repositoryId"))? != repository {
            return Err(invalid(
                "A planning item's kind and repository are immutable.",
            ));
        }
    }
    let name = strv(&body, "name").trim().to_owned();
    if !(1..=MAX_NAME).contains(&scalars(&name)) {
        return Err(invalid(
            "Names must contain 1 to 100 characters after trimming.",
        ));
    }
    if scalars(strv(&body, "description")) > MAX_DESCRIPTION {
        return Err(invalid("Descriptions are limited to 4000 characters."));
    }
    body["name"] = name.clone().into();
    match field {
        "label" => {
            let color = strv(&body, "color");
            let bytes = color.as_bytes();
            if bytes.len() != 7 || bytes[0] != b'#' || !bytes[1..].iter().all(u8::is_ascii_hexdigit)
            {
                return Err(invalid("Label color must be #RRGGBB."));
            }
            body["color"] = color.to_ascii_uppercase().into();
        }
        "milestone" => {
            if let Some(date) = body.get("targetDate").filter(|v| !v.is_null())
                && !date.as_str().is_some_and(valid_date)
            {
                return Err(invalid("The target date must be a real YYYY-MM-DD date."));
            }
            if body.get("targetDate").is_some_and(Value::is_null) {
                body.as_object_mut().expect("body").remove("targetDate");
            }
        }
        _ => {
            let (start, end) = (strv(&body, "startDate"), strv(&body, "endDate"));
            if !valid_date(start) || !valid_date(end) {
                return Err(invalid(
                    "Cycle start and end dates must be real YYYY-MM-DD dates.",
                ));
            }
            if start > end {
                return Err(violation(Violation::DateOrder, id));
            }
        }
    }
    let wanted = simple_lowercase(&name);
    for other in rows(
        c,
        "SELECT id FROM entities WHERE kind=? AND deleted=0 AND repository_id=? AND id<>? ORDER BY id",
        rusqlite::params![number, repository, id],
    )? {
        for head in heads(c, &other)? {
            if simple_lowercase(strv(&head[field], "name")) == wanted {
                return Err(violation(Violation::DuplicateName, id));
            }
        }
    }
    Ok((kind, body))
}

/// Whether a task body carries any planning field (such a task needs entity format 3).
#[must_use]
pub fn has_planning_fields(t: &Value) -> bool {
    [
        "milestoneId",
        "cycleId",
        "parentTaskId",
        "duplicateOfTaskId",
        "startDate",
        "dueDate",
        "storyPoints",
    ]
    .iter()
    .any(|k| t.get(*k).is_some_and(|v| !v.is_null()))
        || ["labelIds", "dependsOnTaskIds", "relatedTaskIds"]
            .iter()
            .any(|k| !list(t, k).is_empty())
        || ["priority", "type"].iter().any(|k| {
            t.get(*k)
                .and_then(Value::as_str)
                .is_some_and(|v| !v.ends_with("_UNSPECIFIED"))
        })
}
/// Database-independent checks for a task body received from another peer.
pub fn check_task_shape(t: &Value) -> Result<()> {
    if t.get("priority")
        .is_some_and(|v| !v.as_str().is_some_and(|s| PRIORITIES.contains(&s)))
        || t.get("type")
            .is_some_and(|v| !v.as_str().is_some_and(|s| TYPES.contains(&s)))
    {
        return Err(invalid("Invalid task priority or type."));
    }
    for key in ["startDate", "dueDate"] {
        if let Some(v) = t.get(key).filter(|v| !v.is_null())
            && !v.as_str().is_some_and(valid_date)
        {
            return Err(invalid("Invalid task date."));
        }
    }
    if t.get("storyPoints")
        .filter(|v| !v.is_null())
        .is_some_and(|v| v.as_u64().is_none_or(|n| n > MAX_STORY_POINTS))
        || list(t, "labelIds").len() > MAX_LABELS
        || matches!((opt(t, "startDate"), opt(t, "dueDate")), (Some(a), Some(b)) if a > b)
    {
        return Err(invalid("Invalid task planning fields."));
    }
    Ok(())
}
/// Database-independent checks for a label, milestone or cycle body received from a peer.
pub fn check_planning_shape(field: &str, body: &Value) -> Result<()> {
    if !body.is_object()
        || !super::valid_id(strv(body, "repositoryId"))
        || !(1..=MAX_NAME).contains(&scalars(strv(body, "name")))
        || scalars(strv(body, "description")) > MAX_DESCRIPTION
    {
        return Err(invalid("Invalid planning item."));
    }
    let valid = match field {
        "label" => {
            let color = strv(body, "color").as_bytes();
            color.len() == 7 && color[0] == b'#' && color[1..].iter().all(u8::is_ascii_hexdigit)
        }
        "milestone" => body
            .get("targetDate")
            .filter(|v| !v.is_null())
            .is_none_or(|d| d.as_str().is_some_and(valid_date)),
        _ => {
            let (start, end) = (strv(body, "startDate"), strv(body, "endDate"));
            valid_date(start) && valid_date(end) && start <= end
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid("Invalid planning item."))
    }
}

/// Every other entity an entity version refers to: its repository and, for a task, its
/// planning items and related tasks. Used to order dependent synchronization uploads.
#[must_use]
pub fn referenced_ids(version: &Value) -> Vec<String> {
    let mut out = vec![];
    for field in ["task", "urlConnection", "label", "milestone", "cycle"] {
        let body = &version[field];
        if !body.is_object() {
            continue;
        }
        out.extend(opt(body, "repositoryId").map(str::to_owned));
        for key in ["labelIds", "dependsOnTaskIds", "relatedTaskIds"] {
            out.extend(list(body, key));
        }
        for key in [
            "milestoneId",
            "cycleId",
            "parentTaskId",
            "duplicateOfTaskId",
        ] {
            out.extend(opt(body, key).map(str::to_owned));
        }
    }
    out
}
