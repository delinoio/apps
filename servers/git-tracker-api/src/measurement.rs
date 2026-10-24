// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::decimal;
use chrono::{DateTime, Utc};
use delino_protocol::gittracker::v1::{ComparisonStatus::*, Direction::*, ExecutionState::*, *};
use num_traits::Signed;
use serde::{Deserialize, Serialize};
use server_common::{Fault, Result};
use std::collections::{BTreeMap, HashSet};
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Baseline {
    pub execution: String,
    pub commit: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub removed: BTreeMap<String, bool>,
    pub execution: Execution,
    pub last_data: DateTime<Utc>,
    pub cutoff: DateTime<Utc>,
    pub ancestors: Vec<String>,
    pub base: Baseline,
    pub push: bool,
    pub baselines: BTreeMap<String, Baseline>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub deleted: BTreeMap<String, bool>,
    pub next: u64,
    pub records: BTreeMap<String, Record>,
}
pub fn key(dataset: &str, row: &str) -> String {
    format!("{}:{dataset}{row}", dataset.len())
}
fn find<'a>(
    r: Option<&'a Record>,
    dataset: &str,
    row: &str,
) -> (Option<&'a Dataset>, Option<&'a Row>) {
    if let Some(r) = r {
        for m in &r.execution.measurements {
            if m.dataset.id == dataset {
                return (Some(&m.dataset), m.rows.iter().find(|r| r.id == row));
            }
        }
    }
    (None, None)
}
pub fn validate(dataset: &Dataset, rows: &[Row]) -> Result<()> {
    if dataset.id.is_empty() || dataset.name.is_empty() || dataset.columns.is_empty() {
        return Err(Fault::precondition(
            "dataset id, name and columns are required",
        ));
    }
    let mut cols = HashSet::new();
    for c in &dataset.columns {
        if c.id.is_empty() || !cols.insert(&c.id) {
            return Err(Fault::precondition(
                "column IDs must be unique and nonempty",
            ));
        }
        if !decimal::supported(&c.unit) {
            return Err(Fault::precondition("unsupported column unit"));
        }
        if ![
            DIRECTION_LOWER.into(),
            DIRECTION_HIGHER.into(),
            DIRECTION_NEUTRAL.into(),
        ]
        .contains(&c.direction)
        {
            return Err(Fault::precondition("invalid direction"));
        }
        for s in [&c.absolute_threshold, &c.percentage_threshold] {
            if !s.is_empty() && decimal::parse(s)?.is_negative() {
                return Err(Fault::precondition(
                    "threshold must be a nonnegative decimal",
                ));
            }
        }
    }
    let mut seen = HashSet::new();
    for row in rows {
        if row.id.is_empty() || !seen.insert(&row.id) {
            return Err(Fault::precondition("row IDs must be unique and nonempty"));
        }
        if row.values.len() != cols.len() {
            return Err(Fault::precondition(format!(
                "row {}: all columns are required",
                row.id
            )));
        }
        for (k, v) in &row.values {
            if !cols.contains(k) {
                return Err(Fault::precondition(format!(
                    "row {}: unknown column",
                    row.id
                )));
            }
            decimal::parse(v)
                .map_err(|_| Fault::precondition(format!("row {}: invalid decimal", row.id)))?;
        }
    }
    Ok(())
}
fn equal(a: &Row, b: &Row) -> bool {
    a.values.len() == b.values.len()
        && a.values.iter().all(|(k, v)| {
            b.values
                .get(k)
                .is_some_and(|b| match (decimal::parse(v), decimal::parse(b)) {
                    (Ok(a), Ok(b)) => a == b,
                    _ => false,
                })
        })
}
impl State {
    pub fn interrupt(&mut self, now: DateTime<Utc>) -> usize {
        let mut n = 0;
        for r in self.records.values_mut() {
            if r.execution.state == EXECUTION_STATE_MEASURING
                && now >= r.last_data + chrono::Duration::hours(6)
            {
                r.execution.state = EXECUTION_STATE_INTERRUPTED.into();
                n += 1
            }
        }
        n
    }
    pub fn representative(
        &self,
        commit: &str,
        cutoff: Option<DateTime<Utc>>,
        branch: bool,
    ) -> Option<&Record> {
        self.records
            .values()
            .filter(|r| {
                r.execution.context.commit == commit
                    && r.execution.state == EXECUTION_STATE_COMPLETED
                    && (!branch || r.execution.context.pull_request == 0)
                    && cutoff.is_none_or(|c| {
                        DateTime::parse_from_rfc3339(&r.execution.completed_at)
                            .is_ok_and(|t| t <= c)
                    })
            })
            .max_by_key(|r| &r.execution.registration_order)
    }
    fn register(&mut self, c: &Context, now: DateTime<Utc>, ancestors: Vec<String>, base: String) {
        self.next += 1;
        let push = c.pull_request == 0 && c.baseline_commit.is_empty();
        let mut reference = Baseline::default();
        if !push {
            reference.commit = base.clone();
            if let Some(r) = self.representative(&base, Some(now), true) {
                reference.execution = r.execution.context.execution_id.clone()
            }
        }
        self.records.insert(
            c.execution_id.clone(),
            Record {
                execution: Execution {
                    context: Some(c.clone()).into(),
                    state: EXECUTION_STATE_MEASURING.into(),
                    registered_at: now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
                    registration_order: format!("{:020}", self.next),
                    ..Default::default()
                },
                last_data: now,
                cutoff: now,
                ancestors,
                base: reference,
                push,
                baselines: BTreeMap::new(),
                removed: BTreeMap::new(),
            },
        );
    }
    fn baseline(&self, r: &Record, dataset: &str, row: &str) -> Baseline {
        if !r.push {
            return r.base.clone();
        }
        for sha in &r.ancestors {
            let b = self.representative(sha, Some(r.cutoff), true);
            if find(b, dataset, row).1.is_some() {
                return Baseline {
                    execution: b.unwrap().execution.context.execution_id.clone(),
                    commit: sha.clone(),
                };
            }
        }
        Baseline::default()
    }
    pub fn retry(&self, c: &Context, d: &Dataset, rows: &[Row]) -> bool {
        let Some(r) = self.records.get(&c.execution_id) else {
            return false;
        };
        if rows.is_empty() || *r.execution.context != *c {
            return false;
        }
        rows.iter().all(|row| {
            let (old_d, old) = find(Some(r), &d.id, &row.id);
            old_d == Some(d) && old.is_some_and(|old| equal(old, row))
        })
    }
    pub fn upload(
        &mut self,
        c: &Context,
        d: &Dataset,
        rows: &[Row],
        now: DateTime<Utc>,
        ancestors: Vec<String>,
        base: String,
    ) -> Result<()> {
        if self.deleted.get(&c.execution_id) == Some(&true) {
            return Err(Fault::precondition("execution was deleted"));
        }
        validate(d, rows)?;
        if rows.is_empty() {
            return Err(Fault::precondition("upload requires at least one row"));
        }
        let r = self.records.get(&c.execution_id);
        if let Some(r) = r {
            if r.removed.get(&d.id) == Some(&true) {
                return Err(Fault::precondition(
                    "dataset was deleted from this execution",
                ));
            }
            if *r.execution.context != *c {
                return Err(Fault::precondition("execution context conflicts"));
            }
            if r.execution.state != EXECUTION_STATE_MEASURING
                || now >= r.last_data + chrono::Duration::hours(6)
            {
                return Err(Fault::precondition("execution is closed"));
            }
            if ancestors.len() > r.ancestors.len() && !ancestors.starts_with(&r.ancestors) {
                return Err(Fault::precondition("Git ancestry changed"));
            }
        }
        let mut added = false;
        for row in rows {
            let (old_d, old) = find(r, &d.id, &row.id);
            if old_d.is_some_and(|old| old != d) {
                return Err(Fault::precondition("dataset configuration conflicts"));
            }
            if let Some(old) = old {
                if !equal(old, row) {
                    return Err(Fault::precondition(format!(
                        "row {}: values conflict",
                        row.id
                    )));
                }
            } else {
                added = true
            }
        }
        if r.is_none() {
            self.register(c, now, ancestors.clone(), base)
        }
        let mut r = self.records.remove(&c.execution_id).unwrap();
        if ancestors.len() > r.ancestors.len() {
            r.ancestors = ancestors
        }
        let mut m = r
            .execution
            .measurements
            .iter()
            .position(|m| m.dataset.id == d.id);
        if m.is_none() {
            r.execution.measurements.push(Measurement {
                dataset: Some(d.clone()).into(),
                ..Default::default()
            });
            m = Some(r.execution.measurements.len() - 1)
        }
        for row in rows {
            if find(Some(&r), &d.id, &row.id).1.is_none() {
                r.baselines
                    .insert(key(&d.id, &row.id), self.baseline(&r, &d.id, &row.id));
                r.execution.measurements[m.unwrap()].rows.push(row.clone())
            }
        }
        if added {
            r.last_data = now
        }
        self.records.insert(c.execution_id.clone(), r);
        Ok(())
    }
    pub fn complete(&mut self, c: &Context, now: DateTime<Utc>) -> Result<()> {
        if self.deleted.get(&c.execution_id) == Some(&true) {
            return Err(Fault::precondition("execution was deleted"));
        }
        if !self.records.contains_key(&c.execution_id) {
            self.register(c, now, vec![], String::new())
        }
        let r = self.records.get_mut(&c.execution_id).unwrap();
        if *r.execution.context != *c {
            return Err(Fault::precondition("execution context conflicts"));
        }
        if r.execution.state == EXECUTION_STATE_COMPLETED {
            return Ok(());
        }
        if r.execution.state == EXECUTION_STATE_INTERRUPTED
            || now >= r.last_data + chrono::Duration::hours(6)
        {
            return Err(Fault::precondition("execution is interrupted"));
        }
        r.execution.state = EXECUTION_STATE_COMPLETED.into();
        r.execution.completed_at = now.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        Ok(())
    }
    pub fn comparisons(&self, r: &Record, exact: Option<Option<&Record>>) -> Vec<Comparison> {
        let mut out = Vec::new();
        for m in &r.execution.measurements {
            for row in &m.rows {
                let mut reference = r
                    .baselines
                    .get(&key(&m.dataset.id, &row.id))
                    .cloned()
                    .unwrap_or_default();
                let mut baseline = self.records.get(&reference.execution);
                if let Some(exact) = exact {
                    baseline = exact;
                    reference = exact
                        .map(|r| Baseline {
                            execution: r.execution.context.execution_id.clone(),
                            commit: r.execution.context.commit.clone(),
                        })
                        .unwrap_or_default()
                }
                let (bd, br) = find(baseline, &m.dataset.id, &row.id);
                for c in &m.dataset.columns {
                    let bc = bd.and_then(|d| d.columns.iter().find(|bc| bc.id == c.id));
                    let bv = br
                        .and_then(|r| r.values.get(&c.id))
                        .map(String::as_str)
                        .unwrap_or("");
                    let mut v = decimal::compare(
                        c,
                        if br.is_some() { bc } else { None },
                        row.values.get(&c.id).map(String::as_str).unwrap_or(""),
                        bv,
                    );
                    if baseline.is_some_and(|b| b.removed.get(&m.dataset.id) == Some(&true)) {
                        v.status = COMPARISON_STATUS_BASELINE_DELETED.into()
                    }
                    if baseline.is_none() {
                        v.status = if reference.execution.is_empty() {
                            COMPARISON_STATUS_UNAVAILABLE
                        } else {
                            COMPARISON_STATUS_BASELINE_DELETED
                        }
                        .into()
                    }
                    v.dataset_id = m.dataset.id.clone();
                    v.row_id = row.id.clone();
                    v.baseline_commit = reference.commit.clone();
                    v.baseline_execution_id = reference.execution.clone();
                    out.push(v);
                }
            }
        }
        out.sort_by_key(|v| format!("{}{}", key(&v.dataset_id, &v.row_id), v.column_id));
        out
    }
}
