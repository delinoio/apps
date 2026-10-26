// SPDX-License-Identifier: Apache-2.0
//! `ListTasks` filtering, sorting and keyset pagination, plus the planning item reads.
//!
//! The semantics follow `protos/lane/v1/planning_list_reference_test.go`: filter fields AND
//! together, selected values within a field OR, an `include_*` flag ORs in that field's
//! unassigned case, unassigned sort values come last in both directions and ties break by
//! ascending task ID. A cursor stores the last returned sort tuple and task ID so that
//! concatenated pages equal the unpaged order, and is scoped to the exact query.
use super::{
    Connection, Failure, Result, Store, Value,
    domain::{heads, mapped, result},
    invalid, json, planning, rows, strv,
};
use crate::protocol::fingerprint;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use caseless::Caseless;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

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
const RELATIONSHIPS: [&str; 6] = [
    "RELATIONSHIP_KIND_PARENT",
    "RELATIONSHIP_KIND_SUBTASKS",
    "RELATIONSHIP_KIND_DEPENDS_ON",
    "RELATIONSHIP_KIND_BLOCKS",
    "RELATIONSHIP_KIND_RELATED",
    "RELATIONSHIP_KIND_DUPLICATE_OF",
];

/// One task's sort key. Unassigned values have `assigned == false` and sort last.
#[derive(Clone, Default, Debug)]
struct Key {
    assigned: bool,
    number: i64,
    text: String,
}
fn compare(a: &Key, b: &Key, a_id: &str, b_id: &str, descending: bool) -> Ordering {
    if a.assigned != b.assigned {
        return if a.assigned {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if a.assigned {
        let mut order = a.number.cmp(&b.number).then_with(|| a.text.cmp(&b.text));
        if descending {
            order = order.reverse();
        }
        if order != Ordering::Equal {
            return order;
        }
    }
    a_id.cmp(b_id)
}
fn strings(v: &Value, key: &str) -> Vec<String> {
    planning::list(v, key)
}
fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}
/// "Selection OR unassigned flag"; no selection and a false flag never restricts.
fn selected(selection: &[String], include_unassigned: bool, value: Option<&str>) -> bool {
    if selection.is_empty() && !include_unassigned {
        return true;
    }
    value.map_or(include_unassigned, |v| selection.iter().any(|s| s == v))
}
/// Inclusive date range; a range with only bounds excludes unassigned values.
fn in_dates(range: &Value, value: Option<&str>) -> bool {
    let from = range.get("from").and_then(Value::as_str);
    let to = range.get("to").and_then(Value::as_str);
    let include = flag(range, "includeUnassigned");
    if from.is_none() && to.is_none() && !include {
        return true;
    }
    let Some(value) = value else { return include };
    if from.is_none() && to.is_none() {
        return false;
    }
    from.is_none_or(|f| value >= f) && to.is_none_or(|t| value <= t)
}
/// Inclusive story point range; 0 is an assigned value, absent points are unassigned.
fn in_points(range: &Value, value: Option<u64>) -> bool {
    let min = range.get("min").and_then(Value::as_u64);
    let max = range.get("max").and_then(Value::as_u64);
    let include = flag(range, "includeUnassigned");
    if min.is_none() && max.is_none() && !include {
        return true;
    }
    let Some(value) = value else { return include };
    if min.is_none() && max.is_none() {
        return false;
    }
    min.is_none_or(|m| value >= m) && max.is_none_or(|m| value <= m)
}
fn validate_filter(request: &Value) -> Result<()> {
    let filter = &request["filter"];
    let bad = |m: &str| invalid(m);
    let known = |key: &str, allowed: &[&str]| {
        filter[key].as_array().is_none_or(|a| {
            a.iter()
                .all(|v| v.as_str().is_some_and(|s| allowed.contains(&s)))
        })
    };
    if !known("priorities", &PRIORITIES)
        || !known("types", &TYPES)
        || !known("relationships", &RELATIONSHIPS)
    {
        return Err(bad("Invalid task filter value."));
    }
    for key in ["startDate", "dueDate"] {
        let range = &filter[key];
        let (from, to) = (strv(range, "from"), strv(range, "to"));
        if (range.get("from").is_some() && !planning::valid_date(from))
            || (range.get("to").is_some() && !planning::valid_date(to))
            || (range.get("from").is_some() && range.get("to").is_some() && from > to)
        {
            return Err(bad("Invalid task date range."));
        }
    }
    let points = &filter["storyPoints"];
    if let (Some(min), Some(max)) = (points["min"].as_u64(), points["max"].as_u64())
        && min > max
    {
        return Err(bad("Invalid story point range."));
    }
    let key = strv(request, "sortKey");
    let direction = strv(request, "sortDirection");
    if !matches!(
        key,
        "" | "TASK_SORT_KEY_UNSPECIFIED"
            | "TASK_SORT_KEY_PRIORITY"
            | "TASK_SORT_KEY_TYPE"
            | "TASK_SORT_KEY_START_DATE"
            | "TASK_SORT_KEY_DUE_DATE"
            | "TASK_SORT_KEY_STORY_POINTS"
            | "TASK_SORT_KEY_MILESTONE_NAME"
            | "TASK_SORT_KEY_CYCLE_START_DATE"
    ) || !matches!(
        direction,
        "" | "SORT_DIRECTION_UNSPECIFIED"
            | "SORT_DIRECTION_ASCENDING"
            | "SORT_DIRECTION_DESCENDING"
    ) {
        return Err(bad("Invalid task sort."));
    }
    Ok(())
}
/// Inverse relationship views derived from every live task.
#[derive(Default)]
struct Inverse {
    parents: HashSet<String>,
    blocking: HashSet<String>,
    related: HashSet<String>,
}
fn inverse(c: &Connection) -> Result<Inverse> {
    let mut out = Inverse::default();
    for id in rows(
        c,
        "SELECT id FROM entities WHERE kind=2 AND deleted=0 ORDER BY id",
        [],
    )? {
        for head in heads(c, &id)? {
            let task = &head["task"];
            if let Some(parent) = task["parentTaskId"].as_str() {
                out.parents.insert(parent.to_owned());
            }
            out.blocking.extend(strings(task, "dependsOnTaskIds"));
            out.related.extend(
                strings(task, "relatedTaskIds")
                    .into_iter()
                    .filter(|other| *other != id),
            );
        }
    }
    Ok(out)
}
fn fold(s: &str) -> String {
    s.chars().default_case_fold().collect()
}
struct Context<'a> {
    request: &'a Value,
    repository: Option<String>,
    search: String,
    inverse: Option<Inverse>,
}
impl Context<'_> {
    fn body_matches(&self, c: &Connection, id: &str, heads: &[Value], t: &Value) -> Result<bool> {
        if let Some(target) = &self.repository
            && mapped(c, strv(t, "repositoryId"))? != *target
        {
            return Ok(false);
        }
        for (key, value) in [("progress", &t["progress"]), ("proposal", &t["proposal"])] {
            if self.request[key]
                .as_array()
                .is_some_and(|a| !a.is_empty() && !a.contains(value))
            {
                return Ok(false);
            }
        }
        if !self.search.is_empty()
            && !fold(strv(t, "title")).contains(&self.search)
            && !fold(strv(t, "description")).contains(&self.search)
        {
            return Ok(false);
        }
        let f = &self.request["filter"];
        if !f.is_object() {
            return Ok(true);
        }
        let priority = opt_enum(t, "priority", "TASK_PRIORITY_UNSPECIFIED");
        let kind = opt_enum(t, "type", "TASK_TYPE_UNSPECIFIED");
        if !selected(
            &strings(f, "priorities"),
            flag(f, "includeUnassignedPriority"),
            priority,
        ) || !selected(&strings(f, "types"), flag(f, "includeUnassignedType"), kind)
        {
            return Ok(false);
        }
        let selected_labels = strings(f, "labelIds");
        if !selected_labels.is_empty() || flag(f, "includeUnlabeled") {
            let labels = strings(t, "labelIds");
            let any = labels.iter().any(|l| selected_labels.contains(l));
            if !(any || flag(f, "includeUnlabeled") && labels.is_empty()) {
                return Ok(false);
            }
        }
        if !in_dates(&f["startDate"], t["startDate"].as_str())
            || !in_dates(&f["dueDate"], t["dueDate"].as_str())
            || !in_points(&f["storyPoints"], t["storyPoints"].as_u64())
        {
            return Ok(false);
        }
        if !selected(
            &strings(f, "milestoneIds"),
            flag(f, "includeNoMilestone"),
            t["milestoneId"].as_str(),
        ) || !selected(
            &strings(f, "cycleIds"),
            flag(f, "includeNoCycle"),
            t["cycleId"].as_str(),
        ) {
            return Ok(false);
        }
        let wanted = strings(f, "relationships");
        if !wanted.is_empty() || flag(f, "includeNoRelationships") {
            let kinds = self.relationship_kinds(id, heads);
            let any = wanted.iter().any(|k| kinds.contains(k.as_str()));
            if !(any || flag(f, "includeNoRelationships") && kinds.is_empty()) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn relationship_kinds(&self, id: &str, heads: &[Value]) -> HashSet<&'static str> {
        let mut kinds = HashSet::new();
        for h in heads {
            let t = &h["task"];
            if t["parentTaskId"].is_string() {
                kinds.insert(RELATIONSHIPS[0]);
            }
            if !strings(t, "dependsOnTaskIds").is_empty() {
                kinds.insert(RELATIONSHIPS[2]);
            }
            if !strings(t, "relatedTaskIds").is_empty() {
                kinds.insert(RELATIONSHIPS[4]);
            }
            if t["duplicateOfTaskId"].is_string() {
                kinds.insert(RELATIONSHIPS[5]);
            }
        }
        if let Some(inverse) = &self.inverse {
            if inverse.parents.contains(id) {
                kinds.insert(RELATIONSHIPS[1]);
            }
            if inverse.blocking.contains(id) {
                kinds.insert(RELATIONSHIPS[3]);
            }
            if inverse.related.contains(id) {
                kinds.insert(RELATIONSHIPS[4]);
            }
        }
        kinds
    }
}
/// The stored enum name, or `None` when it is unassigned.
fn opt_enum<'a>(t: &'a Value, key: &str, unassigned: &str) -> Option<&'a str> {
    t.get(key)
        .and_then(Value::as_str)
        .filter(|v| *v != unassigned)
}
fn sort_key(
    c: &Connection,
    task: &Value,
    sort: &str,
    names: &mut HashMap<String, Option<String>>,
) -> Result<Key> {
    let text = |value: Option<&str>| {
        value.map_or_else(Key::default, |s| Key {
            assigned: true,
            number: 0,
            text: s.to_owned(),
        })
    };
    let referenced = |c: &Connection,
                      names: &mut HashMap<String, Option<String>>,
                      field: &str,
                      id: &str,
                      cycle: bool|
     -> Result<Key> {
        let cache_key = format!("{field}:{id}");
        if !names.contains_key(&cache_key) {
            let value = heads(c, id).ok().and_then(|h| {
                h.first().and_then(|v| {
                    let body = &v[field];
                    body.is_object().then(|| {
                        if cycle {
                            strv(body, "startDate").to_owned()
                        } else {
                            planning::simple_lowercase(strv(body, "name"))
                        }
                    })
                })
            });
            names.insert(cache_key.clone(), value);
        }
        Ok(names[&cache_key]
            .as_deref()
            .map_or_else(Key::default, |s| Key {
                assigned: true,
                number: 0,
                text: s.to_owned(),
            }))
    };
    Ok(match sort {
        "TASK_SORT_KEY_PRIORITY" => {
            let n = PRIORITIES
                .iter()
                .position(|p| Some(*p) == task["priority"].as_str())
                .unwrap_or(0);
            Key {
                assigned: n != 0,
                number: i64::try_from(n).unwrap_or(0),
                text: String::new(),
            }
        }
        "TASK_SORT_KEY_TYPE" => {
            let n = TYPES
                .iter()
                .position(|p| Some(*p) == task["type"].as_str())
                .unwrap_or(0);
            Key {
                assigned: n != 0,
                number: i64::try_from(n).unwrap_or(0),
                text: String::new(),
            }
        }
        "TASK_SORT_KEY_START_DATE" => text(task["startDate"].as_str()),
        "TASK_SORT_KEY_DUE_DATE" => text(task["dueDate"].as_str()),
        "TASK_SORT_KEY_STORY_POINTS" => {
            task["storyPoints"]
                .as_i64()
                .map_or_else(Key::default, |n| Key {
                    assigned: true,
                    number: n,
                    text: String::new(),
                })
        }
        "TASK_SORT_KEY_MILESTONE_NAME" => match task["milestoneId"].as_str() {
            Some(id) => referenced(c, names, "milestone", id, false)?,
            None => Key::default(),
        },
        "TASK_SORT_KEY_CYCLE_START_DATE" => match task["cycleId"].as_str() {
            Some(id) => referenced(c, names, "cycle", id, true)?,
            None => Key::default(),
        },
        _ => Key::default(),
    })
}
fn encode(scope: &str, key: &Key, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(
        json!({
            "Scope": scope, "Last": id,
            "Value": {"a": key.assigned, "n": key.number, "t": key.text},
        })
        .to_string(),
    )
}
/// Page size and the keyset position of the cursor, if any.
fn decode(request: &Value, scope: &str) -> Result<(usize, Option<(Key, String)>)> {
    let size = request["page"]["size"].as_u64().unwrap_or(0);
    if size > 1000 {
        return Err(invalid("Page size exceeds 1000."));
    }
    let size = if size == 0 {
        100
    } else {
        usize::try_from(size).map_err(|_| invalid("Invalid page size."))?
    };
    let cursor = strv(&request["page"], "cursor");
    if cursor.is_empty() {
        return Ok((size, None));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid("Invalid cursor."))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid cursor."))?;
    if v["Scope"] != scope {
        return Err(invalid("Cursor does not match query."));
    }
    let value = &v["Value"];
    Ok((
        size,
        Some((
            Key {
                assigned: value["a"].as_bool().unwrap_or(false),
                number: value["n"].as_i64().unwrap_or(0),
                text: strv(value, "t").to_owned(),
            },
            strv(&v, "Last").to_owned(),
        )),
    ))
}
impl Store {
    pub(super) fn list_tasks(&self, c: &Connection, request: &Value) -> Result<Value> {
        for (key, valid) in [
            (
                "progress",
                &[
                    "PROGRESS_TODO",
                    "PROGRESS_IN_PROGRESS",
                    "PROGRESS_BLOCKED",
                    "PROGRESS_DONE",
                    "PROGRESS_CANCELED",
                ][..],
            ),
            (
                "proposal",
                &[
                    "PROPOSAL_REGULAR",
                    "PROPOSAL_PENDING",
                    "PROPOSAL_ACCEPTED",
                    "PROPOSAL_REJECTED",
                ][..],
            ),
        ] {
            if let Some(a) = request[key].as_array()
                && a.iter().any(|v| !valid.contains(&v.as_str().unwrap_or("")))
            {
                return Err(invalid("Invalid state filter."));
            }
        }
        validate_filter(request)?;
        let mut query = request.clone();
        query.as_object_mut().unwrap().remove("page");
        let scope = format!(
            "{}lane.v1.ListTasksRequest{}",
            self.space,
            fingerprint(&query)
        );
        let (size, after) = decode(request, &scope)?;
        let sort = strv(request, "sortKey");
        let sorted = !matches!(sort, "" | "TASK_SORT_KEY_UNSPECIFIED");
        let descending = strv(request, "sortDirection") == "SORT_DIRECTION_DESCENDING";
        let target = strv(request, "repositoryId");
        let wants_relationships = !request["filter"]["relationships"]
            .as_array()
            .is_none_or(Vec::is_empty)
            || flag(&request["filter"], "includeNoRelationships");
        let context = Context {
            request,
            repository: if target.is_empty() {
                None
            } else {
                Some(mapped(c, target)?)
            },
            search: fold(strv(request, "search")),
            inverse: if wants_relationships {
                Some(inverse(c)?)
            } else {
                None
            },
        };
        let mut names = HashMap::new();
        let mut items: Vec<(String, Key, Vec<Value>)> = vec![];
        let start = if sorted {
            String::new()
        } else {
            after.as_ref().map_or_default(|(_, id)| id.clone())
        };
        for id in rows(
            c,
            "SELECT id FROM entities WHERE kind=2 AND deleted=0 AND id>? ORDER BY id",
            [&start],
        )? {
            let heads = heads(c, &id)?;
            let mut matched = false;
            for h in &heads {
                let task = &h["task"];
                if task.is_object() && context.body_matches(c, &id, &heads, task)? {
                    matched = true;
                    break;
                }
            }
            if !matched {
                continue;
            }
            let key = sort_key(c, &heads[0]["task"], sort, &mut names)?;
            items.push((id, key, heads));
            // Without a sort key the order is the ID order, so a page needs one extra match.
            if !sorted && items.len() > size {
                break;
            }
        }
        if sorted {
            items.sort_by(|a, b| compare(&a.1, &b.1, &a.0, &b.0, descending));
            if let Some((key, id)) = &after {
                items.retain(|item| {
                    compare(&item.1, key, &item.0, id, descending) == Ordering::Greater
                });
            }
        }
        let next = if items.len() > size {
            let (id, key, _) = &items[size - 1];
            encode(&scope, key, id)
        } else {
            String::new()
        };
        items.truncate(size);
        Ok(json!({
            "items": items.iter().map(|(_, _, heads)| result(heads)).collect::<Vec<_>>(),
            "nextCursor": next,
        }))
    }
    pub(super) fn planning_query(
        &self,
        c: &Connection,
        method: &str,
        request: &Value,
    ) -> Result<Value> {
        if method == "GetPlanningItem" {
            let h = heads(c, strv(request, "id"))?;
            if !planning::is_planning_kind(strv(&h[0], "kind")) {
                return Err(Failure::new("NOT_FOUND", "Entity does not exist."));
            }
            return Ok(json!({"result": result(&h)}));
        }
        let kind = strv(request, "kind");
        let numbers: Vec<i64> = match kind {
            "" | "ENTITY_KIND_UNSPECIFIED" => {
                vec![
                    planning::KIND_LABEL,
                    planning::KIND_MILESTONE,
                    planning::KIND_CYCLE,
                ]
            }
            "ENTITY_KIND_LABEL" | "ENTITY_KIND_MILESTONE" | "ENTITY_KIND_CYCLE" => {
                vec![planning::kind_number(kind)]
            }
            _ => {
                return Err(invalid(
                    "Only label, milestone and cycle kinds can be listed.",
                ));
            }
        };
        let mut query = request.clone();
        query.as_object_mut().unwrap().remove("page");
        let scope = format!(
            "{}lane.v1.ListPlanningItemsRequest{}",
            self.space,
            fingerprint(&query)
        );
        let (size, after) = decode(request, &scope)?;
        let last = after.map_or_default(|(_, id)| id);
        let repository = if strv(request, "repositoryId").is_empty() {
            None
        } else {
            Some(mapped(c, strv(request, "repositoryId"))?)
        };
        let mut items = vec![];
        let mut next = String::new();
        let mut previous = String::new();
        for id in rows(
            c,
            "SELECT id FROM entities WHERE deleted=0 AND id>? AND kind IN (4,5,6) ORDER BY id",
            [&last],
        )? {
            let h = heads(c, &id)?;
            let number = planning::kind_number(strv(&h[0], "kind"));
            let field = planning::body_field(strv(&h[0], "kind")).unwrap_or("label");
            if !numbers.contains(&number)
                || repository.as_ref().is_some_and(|target| {
                    !h.iter().any(|v| {
                        mapped(c, strv(&v[field], "repositoryId")).is_ok_and(|r| r == *target)
                    })
                })
            {
                continue;
            }
            if items.len() == size {
                next = encode(&scope, &Key::default(), &previous);
                break;
            }
            items.push(result(&h));
            previous = id;
        }
        Ok(json!({"items": items, "nextCursor": next}))
    }
}
