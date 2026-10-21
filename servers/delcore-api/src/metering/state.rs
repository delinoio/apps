// SPDX-License-Identifier: FSL-1.1-ALv2
use super::{
    definition::{match_validated, validate_definitions},
    *,
};
use crate::accounting::Decimal;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    pub meter_key: String,
    pub group_key: String,
    pub group_values: BTreeMap<String, Value>,
    pub kind: Aggregation,
    pub count: i64,
    pub sum: Decimal,
    pub value: Decimal,
    pub distinct_count: i64,
}
impl Aggregate {
    pub fn quantity(&self) -> Decimal {
        self.value.clone()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub meter_key: String,
    pub group_key: String,
    pub before: Option<Aggregate>,
    pub after: Option<Aggregate>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionKind {
    Insert,
    Replace,
    Cancel,
}
#[derive(Debug, Clone)]
pub struct Revision {
    pub kind: RevisionKind,
    pub event_id: String,
    pub event: Option<Event>,
}
#[derive(Debug, Clone)]
pub struct State {
    definitions: Vec<Definition>,
    active: BTreeMap<String, Event>,
    groups: BTreeMap<String, BTreeMap<String, Aggregate>>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupCardinality {
    pub distinct_values: i64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeterCardinality {
    pub groups: BTreeMap<String, GroupCardinality>,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    pub max_groups_per_meter: i64,
    pub max_distinct_values_per_group: i64,
}
pub fn retains_values(kind: Aggregation) -> bool {
    matches!(
        kind,
        Aggregation::Min | Aggregation::Max | Aggregation::UniqueCount
    )
}
/// Six-place half-up average. The caller must supply a positive count.
pub fn rounded_average(sum: &Decimal, count: i64) -> Decimal {
    assert!(count > 0, "average count must be positive");
    sum.div_round(count, 6)
        .expect("positive count is a valid divisor")
}
impl State {
    pub fn build(definitions: &[Definition], active: &BTreeMap<String, Event>) -> Result<Self> {
        let definitions = validate_definitions(definitions)?;
        let mut copy_active = BTreeMap::new();
        for (id, event) in active {
            if id.is_empty() {
                return Err(MeteringError::Event("event identity is required".into()));
            }
            copy_active.insert(id.clone(), event.canonical()?);
        }
        let mut groups: BTreeMap<String, BTreeMap<String, Aggregate>> = BTreeMap::new();
        let mut distinct: BTreeMap<(String, String), BTreeSet<Value>> = BTreeMap::new();
        for (id, event) in &copy_active {
            let contributions = match_validated(&definitions, event)
                .map_err(|e| MeteringError::Event(format!("event {id:?}: {e}")))?;
            for c in contributions {
                let kind = definitions
                    .iter()
                    .find(|d| d.key == c.meter_key)
                    .expect("matched definition")
                    .aggregation;
                let a = groups
                    .entry(c.meter_key.clone())
                    .or_default()
                    .entry(c.group_key.clone())
                    .or_insert_with(|| Aggregate {
                        meter_key: c.meter_key.clone(),
                        group_key: c.group_key.clone(),
                        group_values: c.group_values,
                        kind,
                        count: 0,
                        sum: Decimal::zero(),
                        value: Decimal::zero(),
                        distinct_count: 0,
                    });
                a.count = a
                    .count
                    .checked_add(1)
                    .ok_or_else(|| MeteringError::Event("aggregate count overflow".into()))?;
                if let Some(v) = c.value {
                    if retains_values(kind) {
                        distinct
                            .entry((c.meter_key, c.group_key))
                            .or_default()
                            .insert(v.clone());
                    }
                    if kind != Aggregation::UniqueCount {
                        a.sum = a.sum.add(&v.decimal()?);
                    }
                }
            }
        }
        for (meter, meter_groups) in &mut groups {
            for (key, a) in meter_groups {
                let values = distinct.get(&(meter.clone(), key.clone()));
                a.distinct_count = values.map_or(0, |v| v.len() as i64);
                a.value = match a.kind {
                    Aggregation::Count => Decimal::from_minor(a.count.into(), 0),
                    Aggregation::Sum => a.sum.clone(),
                    Aggregation::Avg => rounded_average(&a.sum, a.count),
                    Aggregation::UniqueCount => Decimal::from_minor(a.distinct_count.into(), 0),
                    Aggregation::Min | Aggregation::Max => {
                        let numbers = values
                            .expect("nonempty retained values")
                            .iter()
                            .map(Value::decimal)
                            .collect::<Result<Vec<_>>>()?;
                        if a.kind == Aggregation::Min {
                            numbers.into_iter().min()
                        } else {
                            numbers.into_iter().max()
                        }
                        .expect("nonempty group")
                    }
                };
            }
        }
        Ok(Self {
            definitions,
            active: copy_active,
            groups,
        })
    }
    pub fn aggregates(&self) -> BTreeMap<String, BTreeMap<String, Aggregate>> {
        self.groups.clone()
    }
    pub fn cardinality(&self) -> BTreeMap<String, MeterCardinality> {
        self.definitions
            .iter()
            .map(|d| {
                (
                    d.key.clone(),
                    MeterCardinality {
                        groups: self
                            .groups
                            .get(&d.key)
                            .into_iter()
                            .flat_map(|g| g.iter())
                            .map(|(key, a)| {
                                (
                                    key.clone(),
                                    GroupCardinality {
                                        distinct_values: a.distinct_count,
                                    },
                                )
                            })
                            .collect(),
                    },
                )
            })
            .collect()
    }
    pub fn check_limits(&self, limits: Limits) -> Result<()> {
        if limits.max_groups_per_meter < 0 || limits.max_distinct_values_per_group < 0 {
            return Err(MeteringError::Limit("negative limit".into()));
        }
        for (meter, groups) in &self.groups {
            if limits.max_groups_per_meter > 0 && groups.len() as i64 > limits.max_groups_per_meter
            {
                return Err(MeteringError::Limit(format!("meter {meter:?} group count")));
            }
            for (key, a) in groups {
                if limits.max_distinct_values_per_group > 0
                    && a.distinct_count > limits.max_distinct_values_per_group
                {
                    return Err(MeteringError::Limit(format!(
                        "meter {meter:?} group {key:?} distinct count"
                    )));
                }
            }
        }
        Ok(())
    }
    pub fn apply(&self, revision: Revision) -> Result<(Self, Vec<Change>)> {
        if revision.event_id.is_empty() {
            return Err(MeteringError::Revision("event identity is required".into()));
        }
        let exists = self.active.contains_key(&revision.event_id);
        if exists == (revision.kind == RevisionKind::Insert) {
            return Err(MeteringError::Revision(
                if exists {
                    "event already exists"
                } else {
                    "event does not exist"
                }
                .into(),
            ));
        }
        let mut active = self.active.clone();
        if revision.kind == RevisionKind::Cancel {
            active.remove(&revision.event_id);
        } else {
            active.insert(
                revision.event_id,
                revision
                    .event
                    .ok_or_else(|| MeteringError::Revision("event is required".into()))?,
            );
        }
        let next = Self::build(&self.definitions, &active)?;
        let mut changes = Vec::new();
        for d in &self.definitions {
            let before = self.groups.get(&d.key);
            let after = next.groups.get(&d.key);
            let keys: BTreeSet<_> = before
                .into_iter()
                .flat_map(|g| g.keys())
                .chain(after.into_iter().flat_map(|g| g.keys()))
                .collect();
            for key in keys {
                let b = before.and_then(|g| g.get(key));
                let a = after.and_then(|g| g.get(key));
                if b == a {
                    continue;
                }
                changes.push(Change {
                    meter_key: d.key.clone(),
                    group_key: key.clone(),
                    before: b.cloned(),
                    after: a.cloned(),
                });
            }
        }
        Ok((next, changes))
    }
}
