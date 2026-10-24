// SPDX-License-Identifier: Apache-2.0
use super::{
    ArtifactStore,
    environment::DockerEnvironment,
    profile::{self, ProfileFormat},
    regular_file, relative,
};
use crate::{evidence::*, interfaces::*, lifecycle::Workspace, types::*};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MeasurementOutput {
    /// Total Ir from one complete Callgrind file, including Gungraun-produced files.
    Callgrind,
    /// Exact Go benchmark name (including its -N CPU suffix) and original unit.
    GoBenchmark { benchmark: Name, unit: Name },
    /// A single JSON object printed by the configured Node harness. No timing
    /// is inferred from profile duration; warmup belongs to the declared harness.
    NodeBenchmark { value_field: Name, unit: Name },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workload {
    pub command: Command,
    /// Relative to /build. Every repetition must produce fresh, nonempty files.
    pub profiles: Vec<Name>,
    pub format: ProfileFormat,
    pub output: MeasurementOutput,
    pub repetitions: u32,
    /// Repository-relative paths which must have positive observed profile costs.
    pub required_paths: BTreeSet<Name>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterConfig {
    pub schema_version: Version,
    pub language: Language,
    pub discovery_files: Vec<Name>,
    pub build: Vec<Command>,
    pub correctness: BTreeMap<Name, Command>,
    pub workloads: BTreeMap<Name, Workload>,
    /// Commands such as rustc --version. Exact trimmed output is sealed in tools.
    pub tool_probes: BTreeMap<Name, Command>,
    /// Explicit mappings from profiler build paths to repository-relative paths.
    /// For example /work/ -> "" and file:///work/ -> "".
    pub source_prefixes: BTreeMap<String, String>,
}
impl AdapterConfig {
    pub fn validate(&self) -> Result<()> {
        if self.language == Language::Extension
            || self.discovery_files.is_empty()
            || self.correctness.is_empty()
            || self.workloads.is_empty()
            || self.tool_probes.is_empty()
        {
            return Err(Error::Invalid("complete adapter recipe"));
        }
        for path in &self.discovery_files {
            relative(path.as_str())?;
        }
        for w in self.workloads.values() {
            if w.repetitions == 0 || w.profiles.is_empty() || w.required_paths.is_empty() {
                return Err(Error::Invalid(
                    "workload repetitions, profiles and coverage",
                ));
            }
            for path in w.profiles.iter().chain(&w.required_paths) {
                relative(path.as_str())?;
            }
            let supported = match self.language {
                Language::Rust | Language::C | Language::Cpp | Language::Zig => {
                    w.format == ProfileFormat::Callgrind
                        && matches!(w.output, MeasurementOutput::Callgrind)
                }
                Language::Go => {
                    w.format == ProfileFormat::Pprof
                        && matches!(w.output, MeasurementOutput::GoBenchmark { .. })
                }
                Language::JavaScript => {
                    w.format == ProfileFormat::NodeCpu
                        && matches!(w.output, MeasurementOutput::NodeBenchmark { .. })
                }
                Language::Extension => false,
            };
            if !supported {
                return Err(Error::Invalid("language/profile/measurement route"));
            }
        }
        if self.language == Language::JavaScript
            && self
                .build
                .iter()
                .chain(self.correctness.values())
                .chain(self.workloads.values().map(|w| &w.command))
                .flat_map(|c| &c.arguments)
                .any(|a| a.contains("--jitless") || a.contains("--no-opt"))
        {
            return Err(Error::Invalid(
                "exploratory JIT-disabled route cannot establish normal-JIT evidence",
            ));
        }
        Ok(())
    }
    /// Canonical actual argv records, sealed as the engine's build/runtime options.
    pub fn build_options(&self) -> Result<Vec<String>> {
        self.build
            .iter()
            .map(|c| Ok(serde_json::to_string(c)?))
            .collect()
    }
    pub fn runtime_flags(&self) -> Result<Vec<String>> {
        self.workloads
            .iter()
            .map(|(case, w)| Ok(serde_json::to_string(&(case, &w.command))?))
            .collect()
    }
    pub fn recipe_hash(&self) -> Result<Fingerprint> {
        self.validate()?;
        fingerprint(self)
    }
}
pub const RECIPE_INPUT: &str = "perfine.adapter.recipe";

pub struct ConfiguredLanguageAdapter {
    pub config: AdapterConfig,
    pub tools: BTreeMap<Name, Name>,
    pub metric: MetricSpec,
    pub dependencies: Vec<Name>,
}
impl LanguageAdapter for ConfiguredLanguageAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            language: self.config.language,
            level: CapabilityLevel::ProfileInterpretation,
            tools: self.tools.clone(),
            metric: self.metric.clone(),
            source_mapping: true,
            dependency_access: self.dependencies.clone(),
            network_access: vec![],
            runtime_conditions: vec![
                "offline Docker; explicit correctness checks; normal Node JIT".into(),
            ],
            verification_evidence: vec![],
        }
    }
    fn discover(&self, workspace: &Workspace) -> Result<Project> {
        self.config.validate()?;
        for path in &self.config.discovery_files {
            regular_file(Path::new(workspace.directory.as_str()), path.as_str())?;
        }
        Ok(Project {
            language: self.config.language,
            build_commands: self.config.build.clone(),
            correctness_commands: self.config.correctness.values().cloned().collect(),
            workload_commands: self
                .config
                .workloads
                .values()
                .map(|w| w.command.clone())
                .collect(),
        })
    }
}
/// Caller seals the recipe hash, observed tools/environment and input hashes in
/// FrozenConfig before either same-SHA baseline or candidate measurements.
pub struct NativeMeasurementAdapter<'a> {
    pub config: AdapterConfig,
    pub frozen: &'a FrozenConfig,
    pub environment: &'a DockerEnvironment,
    pub request: EnvironmentRequest,
}
impl NativeMeasurementAdapter<'_> {
    pub fn observe_tools(&self, context: &ExecutionContext<'_>) -> Result<BTreeMap<Name, Name>> {
        self.config
            .tool_probes
            .iter()
            .map(|(name, command)| {
                let output = self.environment.execute(command, &self.request, context)?;
                if output.exit_code != 0 {
                    return Err(Error::Incomplete("tool version probe"));
                }
                let bytes = self.environment.artifacts.get(&output.stdout_artifact)?;
                // Multiline version output is hashed rather than flattened or truncated.
                let version = std::str::from_utf8(&bytes)
                    .map_err(|_| Error::Invalid("tool output UTF-8"))?
                    .trim();
                let value = if version.chars().any(char::is_control) {
                    format!("sha256:{}", digest(&bytes).as_str())
                } else {
                    version.to_owned()
                };
                Ok((name.clone(), Name::new(value)?))
            })
            .collect()
    }
    pub fn observe_environment(&self, context: &ExecutionContext<'_>) -> Result<Environment> {
        let probe = |flag: &str| -> Result<Name> {
            let out = self.environment.execute(
                &Command {
                    executable: Name::new("/usr/bin/uname")?,
                    arguments: vec![flag.into()],
                },
                &self.request,
                context,
            )?;
            if out.exit_code != 0 {
                return Err(Error::Incomplete("uname probe"));
            }
            Name::new(
                String::from_utf8_lossy(&self.environment.artifacts.get(&out.stdout_artifact)?)
                    .trim(),
            )
        };
        let os = probe("-s")?;
        let architecture = probe("-m")?;
        if os.as_str() != "Linux" || architecture.as_str() != "x86_64" {
            return Err(Error::Invalid("Linux x86_64 execution required"));
        }
        // Kernel and container limits are observed/declared explicitly. CPU model
        // identity is supplied by lscpu's stable machine-readable output hash.
        let out = self.environment.execute(
            &Command {
                executable: Name::new("lscpu")?,
                arguments: vec!["--json".into()],
            },
            &self.request,
            context,
        )?;
        if out.exit_code != 0 {
            return Err(Error::Incomplete("lscpu hardware probe"));
        }
        let raw: serde_json::Value =
            serde_json::from_slice(&self.environment.artifacts.get(&out.stdout_artifact)?)?;
        let rows = raw["lscpu"]
            .as_array()
            .ok_or(Error::Incomplete("lscpu fields"))?;
        let mut hardware = BTreeMap::new();
        for row in rows {
            if let (Some(field), Some(value)) = (row["field"].as_str(), row["data"].as_str())
                && matches!(
                    field,
                    "Architecture:"
                        | "CPU(s):"
                        | "Model name:"
                        | "Vendor ID:"
                        | "Stepping:"
                        | "CPU family:"
                        | "Model:"
                )
            {
                hardware.insert(Name::new(field)?, Name::new(value)?);
            }
        }
        if !hardware.contains_key(&Name::new("Model name:")?) {
            return Err(Error::Incomplete("CPU model"));
        }
        hardware.insert(
            Name::new("container_cpus")?,
            Name::new(self.environment.config.cpus.to_string())?,
        );
        hardware.insert(
            Name::new("container_memory_bytes")?,
            Name::new(self.environment.config.memory_bytes.to_string())?,
        );
        hardware.insert(Name::new("kernel")?, probe("-r")?);
        Ok(Environment {
            os,
            architecture,
            runner: Name::new(&self.environment.config.image)?,
            hardware,
            variables: self
                .request
                .environment
                .iter()
                .map(|(k, v)| Ok((k.clone(), Name::new(v)?)))
                .collect::<Result<_>>()?,
        })
    }
    fn record(
        &self,
        artifacts: &mut BTreeMap<Name, Artifact>,
        kind: ArtifactKind,
        bytes: &[u8],
    ) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let sha256 = self.environment.artifacts.put(bytes)?;
        artifacts.insert(
            Name::new(sha256.as_str())?,
            Artifact {
                kind,
                sha256,
                size: bytes.len() as u64,
            },
        );
        Ok(())
    }
    fn execute_record(
        &self,
        label: &str,
        command: &Command,
        kind: ArtifactKind,
        artifacts: &mut BTreeMap<Name, Artifact>,
        context: &ExecutionContext<'_>,
    ) -> Result<CommandOutput> {
        let out = self.environment.execute(command, &self.request, context)?;
        // Include the command, exit status and exact stream hashes in provenance.
        self.record(
            artifacts,
            kind,
            &serde_json::to_vec(&(label, command, &out))?,
        )?;
        for hash in [&out.stdout_artifact, &out.stderr_artifact] {
            self.record(artifacts, kind, &self.environment.artifacts.get(hash)?)?;
        }
        Ok(out)
    }
    fn source_path(&self, source: &str) -> Result<Option<Name>> {
        let mut result = source.to_owned();
        // Longest prefix wins, avoiding ambiguous path rewrites.
        if let Some((prefix, target)) = self
            .config
            .source_prefixes
            .iter()
            .filter(|(p, _)| source.starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
        {
            result = format!("{target}{}", &source[prefix.len()..]);
        }
        if relative(&result).is_err() || result == "???" {
            return Ok(None);
        }
        if regular_file(
            Path::new(self.request.workspace.directory.as_str()),
            &result,
        )
        .is_err()
        {
            return Ok(None);
        }
        Ok(Some(Name::new(result)?))
    }
}
impl MeasurementAdapter for NativeMeasurementAdapter<'_> {
    fn measure(
        &self,
        identity: &RunIdentity,
        conditions: &Conditions,
        context: &ExecutionContext<'_>,
    ) -> Result<Manifest> {
        self.config.validate()?;
        self.frozen.validate()?;
        if conditions != &self.frozen.config().conditions
            || identity.configuration != *self.frozen.fingerprint()
            || identity.conditions != fingerprint(conditions)?
        {
            return Err(Error::Mismatch("scheduled measurement configuration"));
        }
        if identity.source != self.environment.source(context.workspace)? {
            return Err(Error::Mismatch("actual Git source"));
        }
        self.environment.require_clean(context.workspace)?;
        if conditions.inputs.get(&Name::new(RECIPE_INPUT)?) != Some(&self.config.recipe_hash()?) {
            return Err(Error::Mismatch("adapter recipe hash"));
        }
        for (name, hash) in &conditions.inputs {
            if name.as_str() != RECIPE_INPUT
                && digest(&fs::read(regular_file(
                    Path::new(context.workspace.directory.as_str()),
                    name.as_str(),
                )?)?)
                    != *hash
            {
                return Err(Error::Mismatch("actual workload input"));
            }
        }
        if conditions.build_options != self.config.build_options()?
            || conditions.runtime_flags != self.config.runtime_flags()?
            || !conditions
                .tools
                .values()
                .any(|version| version == &conditions.toolchain)
        {
            return Err(Error::Mismatch("actual argv or toolchain version"));
        }
        if conditions.tools != self.observe_tools(context)?
            || conditions.environment != self.observe_environment(context)?
        {
            return Err(Error::Mismatch("actual tools or environment"));
        }
        if conditions.cases.keys().collect::<BTreeSet<_>>()
            != self.config.workloads.keys().collect()
            || conditions
                .correctness_checks
                .iter()
                .collect::<BTreeSet<_>>()
                != self.config.correctness.keys().collect()
        {
            return Err(Error::Mismatch("recipe cases/checks"));
        }
        let mut manifest = Manifest {
            schema_version: Version,
            identity: identity.clone(),
            conditions: conditions.clone(),
            policy: self.frozen.config().policy.clone(),
            completion: Completion::Incomplete,
            measurements: BTreeMap::new(),
            correctness: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            profile_paths: BTreeMap::new(),
        };
        for cmd in &self.config.build {
            if self
                .execute_record(
                    "build",
                    cmd,
                    ArtifactKind::Measurement,
                    &mut manifest.artifacts,
                    context,
                )?
                .exit_code
                != 0
            {
                return Err(Error::Incomplete("build command failed; logs retained"));
            }
        }
        for (name, cmd) in &self.config.correctness {
            let out = self.execute_record(
                name.as_str(),
                cmd,
                ArtifactKind::Correctness,
                &mut manifest.artifacts,
                context,
            )?;
            manifest.correctness.insert(
                name.clone(),
                if out.exit_code == 0 {
                    CheckResult::Passed
                } else {
                    CheckResult::Failed
                },
            );
        }
        for (name, workload) in &self.config.workloads {
            let mut values = vec![];
            let mut coverage: Option<BTreeSet<Name>> = None;
            for _ in 0..workload.repetitions {
                for path in &workload.profiles {
                    let full = Path::new(context.workspace.build_directory.as_str())
                        .join(relative(path.as_str())?);
                    if full.exists() {
                        fs::remove_file(regular_file(
                            Path::new(context.workspace.build_directory.as_str()),
                            path.as_str(),
                        )?)?;
                    }
                }
                let out = self.execute_record(
                    name.as_str(),
                    &workload.command,
                    ArtifactKind::Measurement,
                    &mut manifest.artifacts,
                    context,
                )?;
                if out.exit_code != 0 {
                    return Err(Error::Incomplete("workload failed; logs retained"));
                }
                let mut observed = BTreeSet::new();
                let mut ir = None;
                for path in &workload.profiles {
                    let bytes = fs::read(regular_file(
                        Path::new(context.workspace.build_directory.as_str()),
                        path.as_str(),
                    )?)?;
                    let parsed = profile::parse(workload.format, &bytes)?;
                    if let Some(value) = parsed.totals.get(&Name::new("Ir")?)
                        && ir.replace(*value).is_some()
                    {
                        return Err(Error::Invalid("ambiguous multi-profile Ir total"));
                    }
                    for cost in &parsed.profile.costs {
                        if cost.cost > 0.0
                            && let Some(path) = self.source_path(cost.source.file.as_str())?
                        {
                            observed.insert(path);
                        }
                    }
                    self.record(&mut manifest.artifacts, ArtifactKind::Profile, &bytes)?;
                }
                if !workload.required_paths.is_subset(&observed) {
                    return Err(Error::Incomplete(
                        "required source path not observed in profile",
                    ));
                }
                coverage = Some(match coverage {
                    Some(previous) => previous.intersection(&observed).cloned().collect(),
                    None => observed,
                });
                let stdout = self.environment.artifacts.get(&out.stdout_artifact)?;
                let value = match &workload.output {
                    MeasurementOutput::Callgrind => {
                        if conditions.metric.metric != Metric::Ir
                            || conditions.metric.unit.as_str() != "instructions"
                        {
                            return Err(Error::Mismatch("Callgrind metric"));
                        }
                        ir.ok_or(Error::Incomplete("Ir total"))?
                    }
                    MeasurementOutput::GoBenchmark { benchmark, unit } => {
                        if &conditions.metric.unit != unit {
                            return Err(Error::Mismatch("Go measurement unit"));
                        }
                        parse_go_benchmark(&stdout, benchmark.as_str(), unit.as_str())?
                    }
                    MeasurementOutput::NodeBenchmark { value_field, unit } => {
                        if &conditions.metric.unit != unit
                            || conditions.metric.metric != Metric::Time
                        {
                            return Err(Error::Mismatch("Node measurement metric"));
                        }
                        let value: serde_json::Value = serde_json::from_slice(&stdout)?;
                        value
                            .get(value_field.as_str())
                            .and_then(serde_json::Value::as_f64)
                            .ok_or(Error::Incomplete("Node benchmark numeric field"))?
                    }
                };
                if !value.is_finite() || value <= 0.0 {
                    return Err(Error::Invalid("positive finite measurement"));
                }
                values.push(value);
            }
            manifest
                .measurements
                .insert(name.clone(), Samples { values });
            manifest
                .profile_paths
                .insert(name.clone(), coverage.unwrap_or_default());
        }
        self.environment.require_clean(context.workspace)?;
        if identity.source != self.environment.source(context.workspace)? {
            return Err(Error::Mismatch("source changed during measurement"));
        }
        manifest.completion = Completion::Complete;
        self.environment
            .artifacts
            .put(&serde_json::to_vec(&manifest)?)?;
        tracing::info!(
            run = identity.run.as_str(),
            "completed measured evidence manifest"
        );
        Ok(manifest)
    }
}
/// Reject absent, duplicate and partial benchmark rows instead of selecting the
/// first convenient number from stdout. One execution must emit one selected row.
pub fn parse_go_benchmark(bytes: &[u8], benchmark: &str, unit: &str) -> Result<f64> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Invalid("Go output UTF-8"))?;
    let mut found = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first().copied() != Some(benchmark) {
            continue;
        }
        if found.is_some()
            || fields.len() < 4
            || fields.len() % 2 != 0
            || fields[1].parse::<u64>().ok().is_none_or(|n| n == 0)
        {
            return Err(Error::Invalid("complete unique Go benchmark row"));
        }
        for pair in fields[2..].as_chunks::<2>().0 {
            if pair[1] == unit {
                if found.is_some() {
                    return Err(Error::Invalid("duplicate Go unit"));
                }
                let value: f64 = pair[0]
                    .parse()
                    .map_err(|_| Error::Invalid("Go measurement"))?;
                if !value.is_finite() || value <= 0.0 {
                    return Err(Error::Invalid("positive Go measurement"));
                }
                found = Some(value);
            }
        }
    }
    found.ok_or(Error::Incomplete("Go benchmark/unit"))
}
/// Convenience for verification with the same content-addressed store.
pub fn verify_local(
    config: &FrozenConfig,
    expected: &RunIdentity,
    manifest: Manifest,
    store: &ArtifactStore,
) -> Result<VerifiedRun> {
    verify(config, expected, manifest, store)
}
