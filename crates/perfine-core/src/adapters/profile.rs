// SPDX-License-Identifier: Apache-2.0
//! Parsers retain raw artifacts and original units. Unknown mappings are explicit;
//! the measurement adapter requires observed paths from its configured workload.
use super::ArtifactStore;
use crate::{
    evidence::{ArtifactKind, VerifiedRun},
    interfaces::*,
    types::*,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFormat {
    Callgrind,
    Pprof,
    NodeCpu,
}
pub struct ParsedProfile {
    pub profile: Profile,
    pub totals: BTreeMap<Name, f64>,
}
pub struct NativeProfileAdapter {
    pub format: ProfileFormat,
    pub artifacts: ArtifactStore,
}
impl ProfileAdapter for NativeProfileAdapter {
    fn interpret(&self, run: &VerifiedRun) -> Result<Profile> {
        let mut profile = empty();
        for artifact in run
            .manifest()
            .artifacts
            .values()
            .filter(|a| a.kind == ArtifactKind::Profile)
        {
            let bytes = self.artifacts.get(&artifact.sha256)?;
            let parsed = parse(self.format, &bytes)?.profile;
            profile.costs.extend(parsed.costs);
            profile.edges.extend(parsed.edges);
            profile.raw_artifacts.extend(parsed.raw_artifacts);
            profile
                .missing_source_mappings
                .extend(parsed.missing_source_mappings);
        }
        if profile.costs.is_empty() {
            return Err(Error::Incomplete("profile costs"));
        }
        Ok(profile)
    }
}
fn empty() -> Profile {
    Profile {
        costs: vec![],
        edges: vec![],
        raw_artifacts: vec![],
        missing_source_mappings: vec![],
    }
}
pub fn parse(format: ProfileFormat, bytes: &[u8]) -> Result<ParsedProfile> {
    let mut parsed = match format {
        ProfileFormat::Callgrind => callgrind(bytes)?,
        ProfileFormat::Pprof => pprof(bytes)?,
        ProfileFormat::NodeCpu => node(bytes)?,
    };
    if parsed.profile.costs.is_empty() {
        return Err(Error::Incomplete("empty profile"));
    }
    parsed.profile.raw_artifacts.push(digest(bytes));
    for cost in &parsed.profile.costs {
        if cost.source.file.as_str() == "???" {
            parsed
                .profile
                .missing_source_mappings
                .push(cost.source.symbol.clone().unwrap_or(Name::new("unknown")?));
        }
    }
    Ok(parsed)
}
fn metric(name: &str, unit: &str) -> Result<MetricSpec> {
    Ok(MetricSpec {
        metric: match name {
            "Ir" => Metric::Ir,
            "cpu" | "wall" | "time" => Metric::Time,
            _ => Metric::Custom,
        },
        unit: Name::new(unit)?,
    })
}
fn number(value: &str) -> Result<u64> {
    if let Some(v) = value.strip_prefix("0x") {
        u64::from_str_radix(v, 16)
    } else {
        value.parse()
    }
    .map_err(|_| Error::Invalid("profile integer"))
}
fn exact(n: u64) -> Result<f64> {
    if n > 9_007_199_254_740_992 {
        return Err(Error::Invalid("profile integer exceeds exact f64"));
    }
    Ok(n as f64)
}
fn compressed(value: &str, table: &mut BTreeMap<String, String>) -> Result<String> {
    if let Some(rest) = value.strip_prefix('(') {
        let (id, rest) = rest
            .split_once(')')
            .ok_or(Error::Invalid("compressed profile name"))?;
        if rest.trim().is_empty() {
            return table
                .get(id)
                .cloned()
                .ok_or(Error::Incomplete("compressed name definition"));
        }
        let text = rest.trim().to_string();
        table.insert(id.into(), text.clone());
        Ok(text)
    } else {
        Ok(value.to_string())
    }
}
fn position(value: &str, previous: u64) -> Result<u64> {
    if value == "*" {
        Ok(previous)
    } else if let Some(v) = value.strip_prefix('+') {
        previous
            .checked_add(number(v)?)
            .ok_or(Error::Invalid("position overflow"))
    } else if let Some(v) = value.strip_prefix('-') {
        previous
            .checked_sub(number(v)?)
            .ok_or(Error::Invalid("negative position"))
    } else {
        number(value)
    }
}
fn location(file: &str, line: Option<u64>, symbol: &str) -> Result<SourceLocation> {
    Ok(SourceLocation {
        file: Name::new(if file.is_empty() { "???" } else { file })?,
        line: line
            .filter(|v| *v > 0)
            .map(u32::try_from)
            .transpose()
            .map_err(|_| Error::Invalid("source line overflow"))?,
        symbol: if symbol.is_empty() {
            None
        } else {
            Some(Name::new(symbol)?)
        },
    })
}
fn callgrind(bytes: &[u8]) -> Result<ParsedProfile> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Invalid("Callgrind UTF-8"))?;
    let mut profile = empty();
    let mut events: Vec<String> = vec![];
    let mut positions = vec!["line".to_owned()];
    let mut previous = vec![0];
    let mut files = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let (mut file, mut function, mut callee_file, mut callee_function) =
        (String::new(), String::new(), String::new(), String::new());
    let mut pending: Option<(u64, Option<u64>)> = None;
    let mut totals = None;
    let mut summary = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(v) = line.strip_prefix("events:") {
            if !events.is_empty() {
                return Err(Error::Invalid(
                    "multi-part Callgrind requires separate artifacts",
                ));
            }
            events = v.split_whitespace().map(str::to_string).collect();
            continue;
        }
        if let Some(v) = line.strip_prefix("positions:") {
            positions = v.split_whitespace().map(str::to_string).collect();
            previous = vec![0; positions.len()];
            continue;
        }
        if let Some(v) = line.strip_prefix("totals:") {
            totals = Some(
                v.split_whitespace()
                    .map(number)
                    .collect::<Result<Vec<_>>>()?,
            );
            continue;
        }
        if let Some(v) = line.strip_prefix("summary:") {
            summary = Some(
                v.split_whitespace()
                    .map(number)
                    .collect::<Result<Vec<_>>>()?,
            );
            continue;
        }
        if let Some((key, v)) = line.split_once('=') {
            match key {
                "fl" | "fi" | "fe" => {
                    file = compressed(v, &mut files)?;
                }
                "fn" => {
                    function = compressed(v, &mut functions)?;
                    callee_file.clear();
                    callee_function.clear();
                }
                "cfl" | "cfi" => callee_file = compressed(v, &mut files)?,
                "cfn" => callee_function = compressed(v, &mut functions)?,
                "calls" => {
                    if pending.is_some() {
                        return Err(Error::Incomplete("Callgrind call cost"));
                    }
                    let fields: Vec<_> = v.split_whitespace().collect();
                    if fields.len() != positions.len() + 1 {
                        return Err(Error::Invalid("Callgrind call positions"));
                    }
                    let target = positions
                        .iter()
                        .position(|p| p == "line")
                        .map(|i| position(fields[i + 1], previous[i]))
                        .transpose()?;
                    pending = Some((number(fields[0])?, target));
                }
                _ => (),
            }
            continue;
        }
        if line.contains(':') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if events.is_empty()
            || positions.is_empty()
            || fields.len() < positions.len()
            || fields.len() > positions.len() + events.len()
        {
            return Err(Error::Invalid("Callgrind cost columns"));
        }
        for (i, value) in fields.iter().take(positions.len()).enumerate() {
            previous[i] = position(value, previous[i])?;
        }
        let source_line = positions
            .iter()
            .position(|p| p == "line")
            .map(|i| previous[i]);
        let source = location(&file, source_line, &function)?;
        for (i, event) in events.iter().enumerate() {
            let cost = exact(
                fields
                    .get(positions.len() + i)
                    .map_or(Ok(0), |v| number(v))?,
            )?;
            let metric = metric(event, if event == "Ir" { "instructions" } else { event })?;
            if let Some((calls, target)) = pending {
                let callee = location(
                    if callee_file.is_empty() {
                        &file
                    } else {
                        &callee_file
                    },
                    target,
                    &callee_function,
                )?;
                profile.edges.push(ProfileEdge {
                    caller: source.clone(),
                    callee,
                    metric,
                    cost,
                    calls: Some(calls),
                });
            } else {
                profile.costs.push(ProfileCost {
                    source: source.clone(),
                    metric,
                    cost,
                    callees: vec![],
                });
            }
        }
        pending = None;
    }
    if pending.is_some() {
        return Err(Error::Incomplete("Callgrind final call cost"));
    }
    let counts = totals
        .or(summary)
        .ok_or(Error::Incomplete("Callgrind totals"))?;
    if counts.len() != events.len() {
        return Err(Error::Incomplete("Callgrind totals columns"));
    }
    let totals = events
        .iter()
        .zip(counts)
        .map(|(event, count)| Ok((Name::new(event)?, exact(count)?)))
        .collect::<Result<_>>()?;
    Ok(ParsedProfile { profile, totals })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CpuProfile {
    nodes: Vec<CpuNode>,
    samples: Vec<u64>,
    time_deltas: Vec<i64>,
    start_time: f64,
    end_time: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CpuNode {
    id: u64,
    call_frame: Frame,
    #[serde(default)]
    children: Vec<u64>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Frame {
    function_name: String,
    url: String,
    line_number: i64,
}
// V8 can emit samples out of timestamp order. Reconstruct the signed timeline
// and stably reorder samples, as Chrome DevTools does, rather than clamping or
// discarding negative deltas. Original profile bytes remain in the artifact store.
// https://chromium.googlesource.com/devtools/devtools-frontend/+/main/front_end/models/cpu_profile/CPUProfileDataModel.ts
fn node_timeline(samples: &[u64], deltas: &[i64]) -> Result<Vec<(u64, u64)>> {
    let mut elapsed = 0_i64;
    let mut timeline = Vec::with_capacity(samples.len());
    for (&sample, &delta) in samples.iter().zip(deltas) {
        elapsed = elapsed
            .checked_add(delta)
            .ok_or(Error::Invalid("Node timestamp overflow"))?;
        let timestamp = u64::try_from(elapsed)
            .map_err(|_| Error::Invalid("Node sample before profile start"))?;
        timeline.push((timestamp, sample));
    }
    timeline.sort_by_key(|&(timestamp, _)| timestamp);
    let mut previous = 0;
    Ok(timeline
        .into_iter()
        .map(|(timestamp, sample)| {
            let delta = timestamp - previous;
            previous = timestamp;
            (sample, delta)
        })
        .collect())
}

fn node(bytes: &[u8]) -> Result<ParsedProfile> {
    let cpu: CpuProfile = serde_json::from_slice(bytes)?;
    if cpu.samples.len() != cpu.time_deltas.len()
        || cpu.samples.is_empty()
        || !cpu.start_time.is_finite()
        || !cpu.end_time.is_finite()
        || cpu.end_time <= cpu.start_time
    {
        return Err(Error::Incomplete("Node samples and time deltas"));
    }
    let mut nodes = BTreeMap::new();
    let mut parents = BTreeMap::new();
    for n in &cpu.nodes {
        if nodes.insert(n.id, n).is_some() {
            return Err(Error::Invalid("duplicate Node node"));
        }
        for child in &n.children {
            if parents.insert(*child, n.id).is_some() {
                return Err(Error::Invalid("multiple Node parents"));
            }
        }
    }
    for n in &cpu.nodes {
        let mut current = n.id;
        let mut visited = BTreeSet::new();
        loop {
            if !nodes.contains_key(&current) || !visited.insert(current) {
                return Err(Error::Invalid("Node graph reference or cycle"));
            }
            match parents.get(&current) {
                Some(parent) => current = *parent,
                None => break,
            }
        }
    }
    for child in parents.keys() {
        if !nodes.contains_key(child) {
            return Err(Error::Incomplete("Node child"));
        }
    }
    let source = |id: &u64| -> Result<SourceLocation> {
        let frame = &nodes
            .get(id)
            .ok_or(Error::Incomplete("Node sample ID"))?
            .call_frame;
        location(
            &frame.url,
            if frame.line_number < 0 {
                None
            } else {
                Some(frame.line_number as u64 + 1)
            },
            &frame.function_name,
        )
    };
    let mut costs: BTreeMap<u64, u64> = BTreeMap::new();
    let mut edges: BTreeMap<(u64, u64), u64> = BTreeMap::new();
    for (sample, delta) in node_timeline(&cpu.samples, &cpu.time_deltas)? {
        source(&sample)?;
        let count = costs.entry(sample).or_default();
        *count = count
            .checked_add(delta)
            .ok_or(Error::Invalid("Node cost overflow"))?;
        let mut child = sample;
        while let Some(parent) = parents.get(&child) {
            let count = edges.entry((*parent, child)).or_default();
            *count = count
                .checked_add(delta)
                .ok_or(Error::Invalid("Node edge overflow"))?;
            child = *parent;
        }
    }
    let metric = metric("time", "microseconds")?;
    let mut profile = empty();
    for (id, cost) in costs {
        profile.costs.push(ProfileCost {
            source: source(&id)?,
            metric: metric.clone(),
            cost: exact(cost)?,
            callees: nodes[&id]
                .children
                .iter()
                .map(&source)
                .collect::<Result<_>>()?,
        });
    }
    for ((caller, callee), cost) in edges {
        profile.edges.push(ProfileEdge {
            caller: source(&caller)?,
            callee: source(&callee)?,
            metric: metric.clone(),
            cost: exact(cost)?,
            calls: None,
        });
    }
    Ok(ParsedProfile {
        profile,
        totals: BTreeMap::new(),
    })
}

// Wire fields are independently implemented from the public pprof schema.
// Unknown fields remain in the hashed raw protobuf artifact.
#[derive(Clone, PartialEq, prost::Message)]
struct Pprof {
    #[prost(message, repeated, tag = "1")]
    types: Vec<ValueType>,
    #[prost(message, repeated, tag = "2")]
    samples: Vec<Sample>,
    #[prost(message, repeated, tag = "4")]
    locations: Vec<Location>,
    #[prost(message, repeated, tag = "5")]
    functions: Vec<Function>,
    #[prost(string, repeated, tag = "6")]
    strings: Vec<String>,
}
#[derive(Clone, PartialEq, prost::Message)]
struct ValueType {
    #[prost(int64, tag = "1")]
    kind: i64,
    #[prost(int64, tag = "2")]
    unit: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Sample {
    #[prost(uint64, repeated, tag = "1")]
    locations: Vec<u64>,
    #[prost(int64, repeated, tag = "2")]
    values: Vec<i64>,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Location {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(message, repeated, tag = "4")]
    lines: Vec<Line>,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Line {
    #[prost(uint64, tag = "1")]
    function: u64,
    #[prost(int64, tag = "2")]
    line: i64,
}
#[derive(Clone, PartialEq, prost::Message)]
struct Function {
    #[prost(uint64, tag = "1")]
    id: u64,
    #[prost(int64, tag = "2")]
    name: i64,
    #[prost(int64, tag = "4")]
    filename: i64,
}
fn pprof(bytes: &[u8]) -> Result<ParsedProfile> {
    use prost::Message;
    let mut raw = vec![];
    flate2::read::GzDecoder::new(bytes)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut raw)?;
    if raw.len() > 64 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    let pp = Pprof::decode(raw.as_slice()).map_err(|_| Error::Invalid("pprof protobuf"))?;
    let string = |id: i64| {
        pp.strings
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .map(String::as_str)
            .ok_or(Error::Invalid("pprof string ID"))
    };
    let functions: BTreeMap<_, _> = pp.functions.iter().map(|f| (f.id, f)).collect();
    let locations: BTreeMap<_, _> = pp.locations.iter().map(|l| (l.id, l)).collect();
    if functions.len() != pp.functions.len()
        || locations.len() != pp.locations.len()
        || pp.types.is_empty()
    {
        return Err(Error::Invalid("pprof duplicate IDs or absent metrics"));
    }
    let metrics = pp
        .types
        .iter()
        .map(|t| metric(string(t.kind)?, string(t.unit)?))
        .collect::<Result<Vec<_>>>()?;
    let mut profile = empty();
    for sample in &pp.samples {
        if sample.values.len() != metrics.len() || sample.locations.is_empty() {
            return Err(Error::Incomplete("pprof sample values/stack"));
        }
        let mut stack = vec![];
        for id in &sample.locations {
            let loc = locations
                .get(id)
                .ok_or(Error::Incomplete("pprof location"))?;
            if loc.lines.is_empty() {
                stack.push(location("", None, "")?);
            }
            for line in &loc.lines {
                let f = functions
                    .get(&line.function)
                    .ok_or(Error::Incomplete("pprof function"))?;
                stack.push(location(
                    string(f.filename)?,
                    u64::try_from(line.line).ok(),
                    string(f.name)?,
                )?);
            }
        }
        for (m, value) in metrics.iter().zip(&sample.values) {
            let cost =
                exact(u64::try_from(*value).map_err(|_| Error::Invalid("negative pprof sample"))?)?;
            profile.costs.push(ProfileCost {
                source: stack[0].clone(),
                metric: m.clone(),
                cost,
                callees: vec![],
            });
            for pair in stack.windows(2) {
                profile.edges.push(ProfileEdge {
                    caller: pair[1].clone(),
                    callee: pair[0].clone(),
                    metric: m.clone(),
                    cost,
                    calls: None,
                });
            }
        }
    }
    Ok(ParsedProfile {
        profile,
        totals: BTreeMap::new(),
    })
}

/// Apply explicit profiler path prefixes before presenting a profile to an agent.
/// Raw artifacts retain the original paths. Unmapped external frames remain visible.
pub fn map_sources(profile: &mut Profile, prefixes: &BTreeMap<String, String>) -> Result<()> {
    fn map(source: &mut SourceLocation, prefixes: &BTreeMap<String, String>) -> Result<()> {
        let value = source.file.as_str();
        if let Some((prefix, target)) = prefixes
            .iter()
            .filter(|(p, _)| value.starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
        {
            source.file = Name::new(format!("{target}{}", &value[prefix.len()..]))?;
        }
        Ok(())
    }
    for cost in &mut profile.costs {
        map(&mut cost.source, prefixes)?;
        for callee in &mut cost.callees {
            map(callee, prefixes)?;
        }
    }
    for edge in &mut profile.edges {
        map(&mut edge.caller, prefixes)?;
        map(&mut edge.callee, prefixes)?;
    }
    Ok(())
}
