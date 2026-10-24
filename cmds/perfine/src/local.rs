// SPDX-License-Identifier: Apache-2.0
use crate::{App, config::Settings};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use perfine_core::{
    adapters::{
        ArtifactStore,
        agent::HeadlessAgentAdapter,
        environment::DockerEnvironment,
        measurement::{
            ConfiguredLanguageAdapter, NativeMeasurementAdapter, RECIPE_INPUT, verify_local,
        },
        profile::{NativeProfileAdapter, map_sources},
    },
    evidence::{FrozenConfig, Manifest, RunIdentity, VerifiedRun, validate_baseline},
    interfaces::{
        AgentAdapter, Cancellation, CancellationSignal, EnvironmentAdapter, EnvironmentRequest,
        ExecutionContext, ExecutionRole, LanguageAdapter, MeasurementAdapter, ProfileAdapter,
    },
    lifecycle::{Engine, FailureClass, Workspace},
    policy::{self, Comparison, ReviewEvidence, Round},
    storage::FileStore,
    types::{
        CommitSha, ExecutionState, Fingerprint, Name, RequestId, ResourceId, Version, digest,
        fingerprint,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command as Process,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn request() -> RequestId {
    RequestId::new(ResourceId::generate().as_str()).expect("UUID request")
}
pub fn atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("state parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Process::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "core.hooksPath=/dev/null", "-C"])
        .arg(root)
        .args(args)
        .output()?;
    ensure!(
        out.status.success(),
        "Git operation failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim_end().to_owned())
}
#[derive(Clone, Serialize, Deserialize)]
struct RunRecord {
    identity: RunIdentity,
    artifact: Fingerprint,
}
#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    schema_version: Version,
    settings: Fingerprint,
    optimize: bool,
    created_request: RequestId,
    runs: BTreeMap<String, RunRecord>,
    failure: Option<String>,
}
struct Signal {
    directory: PathBuf,
}
impl CancellationSignal for Signal {
    fn state(&self) -> Cancellation {
        if self.directory.join("immediate").exists() {
            Cancellation::Immediate
        } else if self.directory.join("graceful").exists() {
            Cancellation::GracefulStop
        } else {
            Cancellation::Continue
        }
    }
}
impl App {
    pub(crate) fn root(&self) -> Result<PathBuf> {
        // Use Git's local exclude file; never stage or rewrite the user's .gitignore.
        let exclude = git(
            &self.repository,
            &["rev-parse", "--git-path", "info/exclude"],
        )?;
        let exclude = if Path::new(&exclude).is_absolute() {
            PathBuf::from(exclude)
        } else {
            self.repository.join(exclude)
        };
        if let Some(parent) = exclude.parent() {
            fs::create_dir_all(parent)?;
        }
        let existing = fs::read_to_string(&exclude).unwrap_or_default();
        if !existing.lines().any(|l| l == "/.perfine/") {
            let mut file = OpenOptions::new().create(true).append(true).open(exclude)?;
            writeln!(file, "\n/.perfine/")?;
        }
        let root = self.repository.join(".perfine");
        ensure!(
            !fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_symlink()),
            ".perfine must not be a symlink"
        );
        fs::create_dir_all(&root)?;
        Ok(root)
    }
    pub(crate) fn engine(&self) -> Result<Engine<FileStore>> {
        Ok(Engine::new(FileStore::open(self.root()?.join("engine"))?))
    }
    fn directory(&self, id: &ResourceId) -> Result<PathBuf> {
        let p = self.root()?.join(id.as_str());
        fs::create_dir_all(&p)?;
        Ok(p)
    }
    pub(crate) fn report(&self, id: &str) -> Result<Value> {
        let id = ResourceId::new(id)?;
        let experiment = self.engine()?.status(&id)?;
        let journal = self.directory(&id)?.join("journal.json");
        let evidence = if journal.exists() {
            serde_json::from_slice::<Value>(&fs::read(journal)?)?
        } else {
            Value::Null
        };
        Ok(
            json!({"experiment":experiment,"evidence":evidence,"verdict": "not_evaluated", "verdict_note":"Candidate evaluations carry their own verdict; experiment completion is not a performance pass.","deduplication_days":30}),
        )
    }
    pub(crate) fn cancel(&self, id: &str, req: &RequestId, immediate: bool) -> Result<Value> {
        let id = ResourceId::new(id)?;
        self.engine()?.status(&id)?;
        // Cross-process signal is observed inside each adapter's process loop.
        let receipt = self.engine()?.stop(req, &id, immediate, now())?;
        atomic(
            &self
                .directory(&id)?
                .join(if immediate { "immediate" } else { "graceful" }),
            &json!({"request_id":req}),
        )?;
        Ok(serde_json::to_value(receipt)?)
    }
    pub(crate) fn doctor(&self) -> Result<Value> {
        let cfg = Settings::read(&self.config)?;
        let root = self.root()?;
        let env = DockerEnvironment::new(
            &self.repository,
            &cfg.execution_root,
            cfg.docker.clone(),
            ArtifactStore::new(root.join("artifacts"))?,
        )?;
        let request = prepare(&env, &cfg, "doctor")?;
        let signal = Signal {
            directory: root.join("doctor-signals"),
        };
        let ctx = ExecutionContext {
            workspace: &request.workspace,
            deadline_unix_seconds: now() + cfg.docker.command_timeout_seconds,
            cancellation: &signal,
        };
        let frozen = cfg.experiment.clone().freeze()?;
        let adapter = NativeMeasurementAdapter {
            config: cfg.adapter.clone(),
            frozen: &frozen,
            environment: &env,
            request: request.clone(),
        };
        let tools = adapter.observe_tools(&ctx)?;
        let environment = adapter.observe_environment(&ctx)?;
        let language = ConfiguredLanguageAdapter {
            config: cfg.adapter.clone(),
            tools: tools.clone(),
            metric: cfg.experiment.conditions.metric.clone(),
            dependencies: cfg.docker.dependency_names,
        };
        let project = language.discover(&request.workspace)?;
        Ok(
            json!({"tools":tools,"environment":environment,"project":project,"capabilities":language.capabilities(),"dirty_user_files_preserved":!git(&self.repository,&["status","--porcelain"])?.is_empty()}),
        )
    }
    #[expect(
        clippy::too_many_lines,
        reason = "Sequential durable execution transitions remain visible together"
    )]
    pub(crate) fn run(
        &self,
        id: &str,
        req: &RequestId,
        optimize: bool,
        resume: bool,
    ) -> Result<Value> {
        let id = ResourceId::new(id)?;
        let cfg = Settings::read(&self.config)?;
        let dir = self.directory(&id)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("runner.lock"))?;
        lock.try_lock_exclusive()
            .context("experiment already has an active runner")?;
        let journal_path = dir.join("journal.json");
        let engine = self.engine()?;
        if journal_path.exists() {
            let saved: Journal = serde_json::from_slice(&fs::read(&journal_path)?)?;
            ensure!(
                saved.settings == fingerprint(&cfg)?,
                "resume settings fingerprint changed; create a new experiment"
            );
            if !resume {
                ensure!(
                    saved.created_request == *req && saved.optimize == optimize,
                    "existing experiment; use resume or a new experiment ID"
                );
                return self.report(id.as_str());
            }
        }
        if resume {
            let status = engine.status(&id)?;
            if matches!(
                status.state,
                ExecutionState::Cancelled | ExecutionState::Failed
            ) {
                return self.report(id.as_str());
            }
            if now() >= status.start_deadline
                && status
                    .candidates
                    .values()
                    .all(|c| c.state != ExecutionState::Running)
            {
                engine.tick(&request(), &id, now())?;
                return self.report(id.as_str());
            }
        }
        let signal = Signal { directory: dir };
        let artifacts = ArtifactStore::new(self.root()?.join("artifacts"))?;
        let env = DockerEnvironment::new(
            &self.repository,
            &cfg.execution_root,
            cfg.docker.clone(),
            artifacts.clone(),
        )?;
        let base_request = prepare(&env, &cfg, "baseline")?;
        let bootstrap = cfg.experiment.clone().freeze()?;
        let bootstrap_ctx = ExecutionContext {
            workspace: &base_request.workspace,
            deadline_unix_seconds: now() + cfg.docker.command_timeout_seconds,
            cancellation: &signal,
        };
        let adapter = NativeMeasurementAdapter {
            config: cfg.adapter.clone(),
            frozen: &bootstrap,
            environment: &env,
            request: base_request.clone(),
        };
        let mut observed = cfg.experiment.clone();
        observed.conditions.tools = adapter.observe_tools(&bootstrap_ctx)?;
        observed.conditions.toolchain = observed
            .conditions
            .tools
            .get(&cfg.toolchain_probe)
            .context("toolchain_probe must select an observed tool")?
            .clone();
        observed.conditions.environment = adapter.observe_environment(&bootstrap_ctx)?;
        observed.conditions.build_options = cfg.adapter.build_options()?;
        observed.conditions.runtime_flags = cfg.adapter.runtime_flags()?;
        observed
            .conditions
            .inputs
            .insert(Name::new(RECIPE_INPUT)?, cfg.adapter.recipe_hash()?);
        // Declared hashes must match committed inputs. Never silently adopt changed inputs.
        for (name, hash) in &observed.conditions.inputs {
            if name.as_str() != RECIPE_INPUT {
                ensure!(
                    digest(&fs::read(
                        Path::new(base_request.workspace.directory.as_str()).join(name.as_str())
                    )?) == *hash,
                    "committed workload input hash mismatch"
                );
            }
        }
        let frozen = observed.freeze()?;
        ConfiguredLanguageAdapter {
            config: cfg.adapter.clone(),
            tools: frozen.config().conditions.tools.clone(),
            metric: frozen.config().conditions.metric.clone(),
            dependencies: cfg.docker.dependency_names.clone(),
        }
        .discover(&base_request.workspace)?;
        let mut journal = if journal_path.exists() {
            let j: Journal = serde_json::from_slice(&fs::read(&journal_path)?)?;
            ensure!(
                j.settings == fingerprint(&cfg)?,
                "resume settings fingerprint changed; create a new experiment"
            );
            ensure!(
                resume || engine.status(&id)?.config.fingerprint() == frozen.fingerprint(),
                "experiment fingerprint changed"
            );
            engine.resume(req, &id, &frozen, now())?;
            j
        } else {
            ensure!(
                !resume,
                "unknown experiment; start measure or optimize first"
            );
            engine.create(req, id.clone(), frozen.clone(), now())?;
            let j = Journal {
                schema_version: Version,
                settings: fingerprint(&cfg)?,
                optimize,
                created_request: req.clone(),
                runs: BTreeMap::new(),
                failure: None,
            };
            atomic(&journal_path, &j)?;
            j
        };
        let result = (|| -> Result<()> {
            let existing = engine.status(&id)?;
            if matches!(
                existing.state,
                ExecutionState::Completed | ExecutionState::Cancelled | ExecutionState::Failed
            ) {
                return Ok(());
            }
            // Interrupted running candidates retain their workspace and cannot supply conclusions.
            for candidate in existing
                .candidates
                .values()
                .filter(|c| c.state == ExecutionState::Running)
            {
                engine.fail_candidate(
                    &request(),
                    &id,
                    &candidate.id,
                    FailureClass::Permanent,
                    now(),
                )?;
            }
            let deadline = existing.start_deadline;
            let a = measure_step(
                &env,
                &cfg,
                &frozen,
                &id,
                None,
                &base_request,
                "baseline-a",
                deadline,
                &signal,
                &engine,
                &mut journal,
                &journal_path,
            )?;
            let b = measure_step(
                &env,
                &cfg,
                &frozen,
                &id,
                None,
                &base_request,
                "baseline-b",
                deadline,
                &signal,
                &engine,
                &mut journal,
                &journal_path,
            )?;
            let baseline = validate_baseline(&frozen, &a, &b)?;
            if !journal.optimize {
                engine.stop(&request(), &id, false, now())?;
                return Ok(());
            }
            if signal.state() != Cancellation::Continue || now() >= deadline {
                engine.tick(&request(), &id, now())?;
                return Ok(());
            }
            let profile_adapter = NativeProfileAdapter {
                format: cfg
                    .adapter
                    .workloads
                    .values()
                    .next()
                    .context("workloads")?
                    .format,
                artifacts: artifacts.clone(),
            };
            let mut profile = profile_adapter.interpret(&a)?;
            map_sources(&mut profile, &cfg.adapter.source_prefixes)?;
            let agent = HeadlessAgentAdapter::new(
                cfg.agent
                    .as_ref()
                    .context("optimize requires a local agent configuration")?
                    .local(),
                &env,
            )?;
            let ctx = ExecutionContext {
                workspace: &base_request.workspace,
                deadline_unix_seconds: deadline,
                cancellation: &signal,
            };
            let hypotheses = agent.propose(&profile, &ctx)?;
            for hypothesis in hypotheses
                .into_iter()
                .take(cfg.experiment.limits.candidate_count as usize)
            {
                let current = engine.status(&id)?;
                if current.stop_requested
                    || signal.state() != Cancellation::Continue
                    || now() >= deadline
                    || current.candidates.len() >= cfg.experiment.limits.candidate_count as usize
                {
                    break;
                }
                let candidate = ResourceId::generate();
                let candidate_request = prepare(&env, &cfg, candidate.as_str())?;
                let candidate_deadline = now() + cfg.experiment.limits.candidate_duration_seconds;
                let ctx = ExecutionContext {
                    workspace: &candidate_request.workspace,
                    deadline_unix_seconds: candidate_deadline,
                    cancellation: &signal,
                };
                let head = agent.implement(&hypothesis, &ctx)?;
                // Empty edits are not a candidate and cannot consume a performance verdict.
                if head == cfg.experiment.incremental_base {
                    continue;
                }
                ensure!(
                    now() < candidate_deadline,
                    "candidate implementation deadline reached"
                );
                engine.start_candidate(
                    &request(),
                    &id,
                    candidate.clone(),
                    head,
                    candidate_request.workspace.clone(),
                    &a,
                    &baseline,
                    &hypothesis.source_paths,
                    now(),
                )?;
                let label = format!("candidate-{}", candidate.as_str());
                let candidate_result = (|| -> Result<()> {
                    let h = measure_step(
                        &env,
                        &cfg,
                        &frozen,
                        &id,
                        Some(candidate.clone()),
                        &candidate_request,
                        &label,
                        candidate_deadline,
                        &signal,
                        &engine,
                        &mut journal,
                        &journal_path,
                    )?;
                    let round = Round {
                        incremental: Comparison {
                            base: &a,
                            head: &h,
                            baseline: &baseline,
                        },
                        cumulative: None,
                    };
                    let review = ReviewEvidence::default();
                    let evaluation = policy::evaluate(&frozen, &round, None, &review)?;
                    let repeated = if evaluation.repeat_required {
                        let rb = measure_step(
                            &env,
                            &cfg,
                            &frozen,
                            &id,
                            None,
                            &base_request,
                            &format!("{label}-repeat-base"),
                            candidate_deadline,
                            &signal,
                            &engine,
                            &mut journal,
                            &journal_path,
                        )?;
                        let rh = measure_step(
                            &env,
                            &cfg,
                            &frozen,
                            &id,
                            Some(candidate.clone()),
                            &candidate_request,
                            &format!("{label}-repeat-head"),
                            candidate_deadline,
                            &signal,
                            &engine,
                            &mut journal,
                            &journal_path,
                        )?;
                        Some((rb, rh))
                    } else {
                        None
                    };
                    let repeat = repeated.as_ref().map(|(rb, rh)| Round {
                        incremental: Comparison {
                            base: rb,
                            head: rh,
                            baseline: &baseline,
                        },
                        cumulative: None,
                    });
                    engine.finish_candidate(
                        &request(),
                        &id,
                        &candidate,
                        &round,
                        repeat.as_ref(),
                        &review,
                        now(),
                    )?;
                    Ok(())
                })();
                if let Err(error) = candidate_result {
                    if engine.status(&id)?.candidates[&candidate].state == ExecutionState::Running {
                        engine.fail_candidate(
                            &request(),
                            &id,
                            &candidate,
                            FailureClass::Permanent,
                            now(),
                        )?;
                    }
                    journal.failure = Some(error.to_string());
                    atomic(&journal_path, &journal)?;
                    if signal.state() == Cancellation::Immediate {
                        break;
                    }
                }
            }
            let status = engine.status(&id)?;
            if matches!(
                status.state,
                ExecutionState::Queued | ExecutionState::Running
            ) {
                engine.stop(&request(), &id, false, now())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            journal.failure = Some(error.to_string());
            atomic(&journal_path, &journal)?;
            return Err(error.context(format!("evidence retained for experiment {}", id.as_str())));
        }
        self.report(id.as_str())
    }
    pub(crate) fn compare(&self, id: &str, candidate: &str) -> Result<Value> {
        let id = ResourceId::new(id)?;
        let candidate = ResourceId::new(candidate)?;
        let exp = self.engine()?.status(&id)?;
        let c = exp
            .candidates
            .get(&candidate)
            .context("unknown candidate")?;
        // Reload every artifact before reporting a stored evaluation.
        let journal: Journal =
            serde_json::from_slice(&fs::read(self.directory(&id)?.join("journal.json"))?)?;
        let artifacts = ArtifactStore::new(self.root()?.join("artifacts"))?;
        for record in journal.runs.values() {
            load(&exp.config, record, &artifacts)?;
        }
        Ok(json!({"candidate":c,"configuration":exp.config.fingerprint()}))
    }
    pub(crate) fn init(
        &self,
        github: bool,
        action: Option<String>,
        template: Option<PathBuf>,
        overwrite: Option<&str>,
    ) -> Result<Value> {
        let contents = if let Some(path) = template {
            fs::read_to_string(path)?
        } else {
            include_str!("template.yaml").to_owned()
        };
        let (path, contents) = if github {
            let from_config = serde_yaml::from_str::<Value>(&contents)
                .ok()
                .and_then(|v| v["action_commit"].as_str().map(str::to_owned));
            let sha=CommitSha::new(action.or(from_config).context("init --github requires an exact verified distribution --action-commit; no release SHA is assumed")?)?;
            let metadata = crate::remote::gh(&[
                "api",
                &format!("repos/delinoio/perfine-action/commits/{}", sha.as_str()),
            ])?;
            ensure!(
                metadata["sha"] == sha.as_str(),
                "Action commit verification mismatch"
            );
            let action_file = crate::remote::gh(&[
                "api",
                &format!(
                    "repos/delinoio/perfine-action/contents/action.yml?ref={}",
                    sha.as_str()
                ),
            ])?;
            ensure!(
                action_file["type"] == "file",
                "verified commit has no distributed action.yml"
            );
            let mut indented = String::new();
            for line in contents.lines() {
                indented.push_str("            ");
                indented.push_str(line);
                indented.push('\n');
            }
            (
                self.repository.join(".github/workflows/perfine.yml"),
                format!(
                    "name: Perfine\non:\n  workflow_dispatch:\npermissions:\n  contents: read\njobs:\n  measure:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: delinoio/perfine-action@{}\n        with:\n          command: measure\n          config: |\n{}",
                    sha.as_str(),
                    indented
                ),
            )
        } else {
            (self.config.clone(), contents)
        };
        if path.exists() {
            let old = digest(&fs::read(&path)?);
            if overwrite != Some(old.as_str()) {
                return Ok(
                    json!({"confirmation_required":true,"path":path,"existing_sha256":old,"proposed_content":contents,"instruction":"Review content and repeat with --overwrite-sha256 equal to existing_sha256."}),
                );
            }
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &contents)?;
        self.root()?;
        Ok(
            json!({"path":path,"workload_code_required":true,"instruction":"Configure pinned source, image, inputs, policy, limits, tools and representative-workload confirmation before execution."}),
        )
    }
}
fn prepare(env: &DockerEnvironment, cfg: &Settings, label: &str) -> Result<EnvironmentRequest> {
    let name = format!("{}-{}", label, ResourceId::generate().as_str());
    let request = EnvironmentRequest {
        role: ExecutionRole::Candidate,
        base: cfg.experiment.incremental_base.clone(),
        workspace: Workspace {
            directory: Name::new(env.root().join(&name).to_string_lossy())?,
            build_directory: Name::new(env.root().join(format!("{name}-build")).to_string_lossy())?,
            branch: Name::new(format!("perfine/{name}"))?,
        },
        environment: cfg
            .experiment
            .conditions
            .environment
            .variables
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().to_owned()))
            .collect(),
        dependency_access: cfg.docker.dependency_names.clone(),
        network_access: vec![],
    };
    env.prepare(&request)?;
    Ok(request)
}
fn load(cfg: &FrozenConfig, record: &RunRecord, artifacts: &ArtifactStore) -> Result<VerifiedRun> {
    let manifest: Manifest = serde_json::from_slice(&artifacts.get(&record.artifact)?)?;
    Ok(verify_local(cfg, &record.identity, manifest, artifacts)?)
}
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit capability and provenance parameters"
)]
fn measure_step(
    env: &DockerEnvironment,
    cfg: &Settings,
    frozen: &FrozenConfig,
    id: &ResourceId,
    candidate: Option<ResourceId>,
    req: &EnvironmentRequest,
    label: &str,
    deadline: u64,
    signal: &Signal,
    engine: &Engine<FileStore>,
    journal: &mut Journal,
    path: &Path,
) -> Result<VerifiedRun> {
    if let Some(record) = journal.runs.get(label) {
        return load(frozen, record, &env.artifacts);
    }
    let identity = RunIdentity {
        experiment: id.clone(),
        candidate,
        run: ResourceId::generate(),
        external_execution: None,
        source: env.source(&req.workspace)?,
        configuration: frozen.fingerprint().clone(),
        conditions: fingerprint(&frozen.config().conditions)?,
    };
    let adapter = NativeMeasurementAdapter {
        config: cfg.adapter.clone(),
        frozen,
        environment: env,
        request: req.clone(),
    };
    let ctx = ExecutionContext {
        workspace: &req.workspace,
        deadline_unix_seconds: deadline,
        cancellation: signal,
    };
    let manifest = adapter.measure(&identity, &frozen.config().conditions, &ctx)?;
    let run = verify_local(frozen, &identity, manifest, &env.artifacts)?;
    let artifact = env.artifacts.put(&serde_json::to_vec(run.manifest())?)?;
    journal.runs.insert(
        label.into(),
        RunRecord {
            identity,
            artifact: artifact.clone(),
        },
    );
    atomic(path, journal)?;
    engine.record_step(&request(), id, frozen, Name::new(label)?, artifact, now())?;
    Ok(run)
}
