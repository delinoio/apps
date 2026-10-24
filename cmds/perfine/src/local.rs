// SPDX-License-Identifier: Apache-2.0
use crate::{App, Command as Capability, config::Settings};
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
pub struct RunRecord {
    pub(crate) identity: RunIdentity,
    pub(crate) artifact: Fingerprint,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Journal {
    schema_version: Version,
    settings: Fingerprint,
    optimize: bool,
    created_request: RequestId,
    pub(crate) runs: BTreeMap<String, RunRecord>,
    failure: Option<String>,
    #[serde(default)]
    attempts: BTreeMap<Fingerprint, Attempt>,
}
#[derive(Serialize, Deserialize)]
struct Initialization {
    config: FrozenConfig,
    journal: Journal,
}

fn recover_initialization(
    engine: &Engine<FileStore>,
    id: &ResourceId,
    journal_path: &Path,
    settings: &Fingerprint,
) -> Result<()> {
    let intent = journal_path.with_file_name("initialization.json");
    if journal_path.exists() || !intent.exists() {
        return Ok(());
    }
    let saved: Initialization = serde_json::from_slice(&fs::read(intent)?)?;
    ensure!(
        &saved.journal.settings == settings,
        "initialization settings fingerprint changed"
    );
    match engine.status(id) {
        Ok(existing) => ensure!(
            existing.config.fingerprint() == saved.config.fingerprint(),
            "initialization config mismatch"
        ),
        Err(perfine_core::Error::State) => {
            engine.create(
                &saved.journal.created_request,
                id.clone(),
                saved.config,
                now(),
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    atomic(journal_path, &saved.journal)
}

#[derive(Clone, Serialize, Deserialize)]
struct Attempt {
    candidate: ResourceId,
    workspace: Workspace,
    deadline: u64,
    #[serde(default)]
    hypothesis: Option<perfine_core::interfaces::Hypothesis>,
    #[serde(default)]
    finished: bool,
}
#[derive(Serialize, Deserialize)]
struct RequestClaim {
    created_at: u64,
    payload: Fingerprint,
}
#[derive(Serialize, Deserialize)]
struct CancelReceipt {
    created_at: u64,
    immediate: bool,
    response: Value,
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
    pub(crate) fn claim_request(&self, request: &RequestId, command: &Capability) -> Result<()> {
        let directory = self.root()?.join("requests");
        fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("requests.lock"))?;
        lock.lock_exclusive()?;
        let payload_file = match command {
            Capability::Publish { draft, .. } => Some(draft.as_path()),
            Capability::Compare {
                review: Some(path), ..
            } => Some(path.as_path()),
            Capability::Measure { .. }
            | Capability::Optimize { .. }
            | Capability::Resume { .. } => Some(self.config.as_path()),
            _ => None,
        };
        let file_hash = payload_file
            .map(|path| fs::read(path).map(|bytes| digest(&bytes)))
            .transpose()?;
        let payload = fingerprint(&(command, file_hash))?;
        let path = directory.join(format!(
            "{}.json",
            digest(request.as_str().as_bytes()).as_str()
        ));
        if path.exists() {
            let previous: RequestClaim = serde_json::from_slice(&fs::read(&path)?)?;
            if now().saturating_sub(previous.created_at) < perfine_core::lifecycle::DEDUP_SECONDS {
                ensure!(
                    previous.payload == payload,
                    "request ID reused for different capability content"
                );
                return Ok(());
            }
        }
        atomic(
            &path,
            &RequestClaim {
                created_at: now(),
                payload,
            },
        )?;
        Ok(())
    }
    pub(crate) fn root(&self) -> Result<PathBuf> {
        ensure!(
            git(&self.repository, &["ls-files", "--", ".perfine"])?.is_empty(),
            ".perfine must not contain tracked files"
        );
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
            json!({"experiment":experiment,"evidence":evidence,"verdict": "not_evaluated", "verdict_note":"Candidate evaluations carry their own verdict; experiment completion is not a performance pass.","deduplication_days":30,"cancellation":{"graceful_requested":self.directory(&id)?.join("graceful").exists(),"immediate_requested":self.directory(&id)?.join("immediate").exists()}}),
        )
    }
    pub(crate) fn cancel(&self, id: &str, req: &RequestId, immediate: bool) -> Result<Value> {
        let id = ResourceId::new(id)?;
        let directory = self.directory(&id)?;
        let cancel_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("cancel.lock"))?;
        cancel_lock.lock_exclusive()?;
        let receipt_path = directory.join(format!(
            "cancel-{}.json",
            digest(req.as_str().as_bytes()).as_str()
        ));
        self.engine()?.status(&id)?;
        if receipt_path.exists() {
            let mut receipt: CancelReceipt = serde_json::from_slice(&fs::read(&receipt_path)?)?;
            if now().saturating_sub(receipt.created_at) < perfine_core::lifecycle::DEDUP_SECONDS {
                ensure!(
                    receipt.immediate == immediate,
                    "request ID reused for different cancellation content"
                );
                receipt.response["replayed"] = json!(true);
                return Ok(receipt.response);
            }
        }
        let status = self.engine()?.status(&id)?;
        if matches!(
            status.state,
            ExecutionState::Completed | ExecutionState::Cancelled | ExecutionState::Failed
        ) {
            return Ok(serde_json::to_value(self.engine()?.stop(
                req,
                &id,
                immediate,
                now(),
            )?)?);
        }
        // A graceful request must not invalidate an agent attempt already admitted
        // by the runner's durable journal. The runner stops after that attempt.
        let runner = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("runner.lock"))?;
        let active = runner.try_lock_exclusive().is_err();
        atomic(
            &directory.join(if immediate { "immediate" } else { "graceful" }),
            &json!({"request_id":req}),
        )?;
        if active && !immediate {
            let response = json!({"experiment":id,"request_id":req,"stop_requested":true,"active_work_allowed_to_finish":true,"replayed":false});
            atomic(
                &receipt_path,
                &CancelReceipt {
                    created_at: now(),
                    immediate,
                    response: response.clone(),
                },
            )?;
            return Ok(response);
        }
        let receipt = self.engine()?.stop(req, &id, immediate, now())?;
        let response = serde_json::to_value(receipt)?;
        atomic(
            &receipt_path,
            &CancelReceipt {
                created_at: now(),
                immediate,
                response: response.clone(),
            },
        )?;
        Ok(response)
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
        recover_initialization(&engine, &id, &journal_path, &fingerprint(&cfg)?)?;
        let mut pending_admission = false;
        if journal_path.exists() {
            let saved: Journal = serde_json::from_slice(&fs::read(&journal_path)?)?;
            ensure!(
                saved.settings == fingerprint(&cfg)?,
                "resume settings fingerprint changed; create a new experiment"
            );
            if resume {
                let status = engine.status(&id)?;
                pending_admission = saved
                    .attempts
                    .values()
                    .any(|a| !a.finished && !status.candidates.contains_key(&a.candidate));
            }
            if !resume {
                ensure!(
                    saved.created_request == *req && saved.optimize == optimize,
                    "existing experiment; use resume or a new experiment ID"
                );
                return self.report(id.as_str());
            }
        }
        if resume && dir.join("graceful").exists() {
            let status = engine.status(&id)?;
            for candidate in status
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
            let status = engine.status(&id)?;
            if matches!(
                status.state,
                ExecutionState::Queued | ExecutionState::Running
            ) {
                engine.stop(&request(), &id, false, now())?;
            }
            return self.report(id.as_str());
        }
        if resume {
            let status = engine.status(&id)?;
            if matches!(
                status.state,
                ExecutionState::Cancelled | ExecutionState::Failed
            ) {
                return self.report(id.as_str());
            }
            if !pending_admission
                && now() >= status.start_deadline
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
                engine.status(&id)?.config.fingerprint() == frozen.fingerprint(),
                "experiment fingerprint changed"
            );
            // An admitted agent attempt precedes its measured head. Do not advance the
            // engine clock past that durable admission before registering the head.
            // All frozen identities were checked above; the original deadline still applies.
            if !pending_admission {
                engine.resume(req, &id, &frozen, now())?;
            }
            j
        } else {
            ensure!(
                !resume,
                "unknown experiment; start measure or optimize first"
            );
            let j = Journal {
                schema_version: Version,
                settings: fingerprint(&cfg)?,
                optimize,
                created_request: req.clone(),
                runs: BTreeMap::new(),
                failure: None,
                attempts: BTreeMap::new(),
            };
            // Persist the original operation before the engine transaction. Recovery
            // can restore the journal after a crash between the two durable writes.
            atomic(
                &journal_path.with_file_name("initialization.json"),
                &Initialization {
                    config: frozen.clone(),
                    journal: j.clone(),
                },
            )?;
            engine.create(req, id.clone(), frozen.clone(), now())?;
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
            let baseline = validate_execution_baseline(&engine, &id, &frozen, &a, &b)?;
            let initial = if cfg.experiment.initial_base == cfg.experiment.incremental_base {
                None
            } else {
                let mut initial_settings = cfg.clone();
                initial_settings.experiment.incremental_base = cfg.experiment.initial_base.clone();
                let initial_request = prepare(&env, &initial_settings, "initial-base")?;
                let first = measure_step(
                    &env,
                    &cfg,
                    &frozen,
                    &id,
                    None,
                    &initial_request,
                    "initial-a",
                    deadline,
                    &signal,
                    &engine,
                    &mut journal,
                    &journal_path,
                )?;
                let second = measure_step(
                    &env,
                    &cfg,
                    &frozen,
                    &id,
                    None,
                    &initial_request,
                    "initial-b",
                    deadline,
                    &signal,
                    &engine,
                    &mut journal,
                    &journal_path,
                )?;
                let validation =
                    validate_execution_baseline(&engine, &id, &frozen, &first, &second)?;
                Some((initial_request, first, validation))
            };
            if !journal.optimize {
                engine.stop(&request(), &id, false, now())?;
                return Ok(());
            }
            if signal.state() != Cancellation::Continue
                || (now() >= deadline
                    && journal
                        .attempts
                        .values()
                        .all(|a| a.finished || now() >= a.deadline))
            {
                let status = engine.status(&id)?;
                if matches!(
                    status.state,
                    ExecutionState::Queued | ExecutionState::Running
                ) {
                    engine.stop(
                        &request(),
                        &id,
                        signal.state() == Cancellation::Immediate,
                        now(),
                    )?;
                }
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
                deadline_unix_seconds: journal
                    .attempts
                    .values()
                    .filter(|a| !a.finished)
                    .map(|a| a.deadline)
                    .max()
                    .unwrap_or(deadline),
                cancellation: &signal,
            };
            let mut hypotheses = if now() < deadline
                && journal.attempts.len() < cfg.experiment.limits.candidate_count as usize
            {
                agent.propose(&profile, &ctx)?
            } else {
                Vec::new()
            };
            // Retain admitted hypotheses even when a fresh proposal omits them.
            for attempt in journal.attempts.values().filter(|a| !a.finished) {
                if let Some(h) = &attempt.hypothesis
                    && !hypotheses
                        .iter()
                        .any(|v| fingerprint(v).ok() == fingerprint(h).ok())
                {
                    hypotheses.insert(0, h.clone());
                }
            }
            for hypothesis in hypotheses {
                let current = engine.status(&id)?;
                let hypothesis_hash = fingerprint(&hypothesis)?;
                let prior = journal.attempts.get(&hypothesis_hash).cloned();
                if prior.as_ref().is_some_and(|a| {
                    a.finished
                        || current
                            .candidates
                            .get(&a.candidate)
                            .is_some_and(|c| c.state != ExecutionState::Running)
                }) {
                    continue;
                }
                if current.stop_requested || signal.state() != Cancellation::Continue {
                    break;
                }
                if prior.is_none()
                    && (now() >= deadline
                        || journal.attempts.len() >= cfg.experiment.limits.candidate_count as usize)
                {
                    continue;
                }
                let (candidate, candidate_request, candidate_deadline, started_at) = if let Some(
                    attempt,
                ) = prior
                {
                    ensure!(
                        now() < attempt.deadline,
                        "interrupted candidate deadline expired; evidence retained"
                    );
                    let req = EnvironmentRequest {
                        role: ExecutionRole::Candidate,
                        base: cfg.experiment.incremental_base.clone(),
                        workspace: attempt.workspace,
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
                    env.reattach(&req)?;
                    ensure!(
                        git(
                            Path::new(req.workspace.directory.as_str()),
                            &["status", "--porcelain", "--untracked-files=all"]
                        )?
                        .is_empty(),
                        "interrupted agent left uncommitted edits; retained unchanged, start a new experiment after review"
                    );
                    (
                        attempt.candidate,
                        req,
                        attempt.deadline,
                        attempt.deadline - cfg.experiment.limits.candidate_duration_seconds,
                    )
                } else {
                    let started_at = now();
                    let candidate = ResourceId::generate();
                    let req = prepare(&env, &cfg, candidate.as_str())?;
                    let end = started_at + cfg.experiment.limits.candidate_duration_seconds;
                    journal.attempts.insert(
                        hypothesis_hash.clone(),
                        Attempt {
                            candidate: candidate.clone(),
                            workspace: req.workspace.clone(),
                            deadline: end,
                            hypothesis: Some(hypothesis.clone()),
                            finished: false,
                        },
                    );
                    atomic(&journal_path, &journal)?;
                    (candidate, req, end, started_at)
                };
                let ctx = ExecutionContext {
                    workspace: &candidate_request.workspace,
                    deadline_unix_seconds: candidate_deadline,
                    cancellation: &signal,
                };
                let observed_head = env.source(&candidate_request.workspace)?;
                let head = if let Some(c) = current.candidates.get(&candidate) {
                    ensure!(
                        observed_head == c.head && c.workspace == candidate_request.workspace,
                        "interrupted candidate identity changed; create a new experiment"
                    );
                    c.head.clone()
                } else if observed_head != cfg.experiment.incremental_base {
                    // A crash after the agent commit but before admission must not invoke the agent twice.
                    ensure!(
                        git(
                            Path::new(candidate_request.workspace.directory.as_str()),
                            &["rev-parse", "HEAD^"]
                        )? == cfg.experiment.incremental_base.as_str(),
                        "interrupted implementation ancestry mismatch"
                    );
                    observed_head
                } else {
                    // The unchanged pinned base and profile must authorize a retained proposal again.
                    agent.restore_hypothesis(&hypothesis, &profile, &ctx)?;
                    agent.implement(&hypothesis, &ctx)?
                };
                if head == cfg.experiment.incremental_base {
                    journal
                        .attempts
                        .get_mut(&hypothesis_hash)
                        .context("attempt")?
                        .finished = true;
                    atomic(&journal_path, &journal)?;
                    continue;
                }
                ensure!(
                    now() < candidate_deadline,
                    "candidate implementation deadline reached"
                );
                if !current.candidates.contains_key(&candidate) {
                engine.start_candidate_at(
                    &request(),
                    &id,
                    candidate.clone(),
                    head,
                    candidate_request.workspace.clone(),
                    &a,
                    &baseline,
                    &hypothesis.source_paths,
                    // Admission time is the durable agent-attempt start, not the time
                    // edits finish. This preserves the start deadline and total timeout.
                    started_at,
                    now(),
                )?;
                }
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
                        cumulative: initial.as_ref().map(|(_, initial_base, validation)| {
                            Comparison {
                                base: initial_base,
                                head: &h,
                                baseline: validation,
                            }
                        }),
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
                        let initial_repeat = if let Some((initial_request, _, _)) = &initial {
                            Some(measure_step(
                                &env,
                                &cfg,
                                &frozen,
                                &id,
                                None,
                                initial_request,
                                &format!("{label}-repeat-initial"),
                                candidate_deadline,
                                &signal,
                                &engine,
                                &mut journal,
                                &journal_path,
                            )?)
                        } else {
                            None
                        };
                        Some((rb, rh, initial_repeat))
                    } else {
                        None
                    };
                    let repeat = repeated.as_ref().map(|(rb, rh, initial_repeat)| Round {
                        incremental: Comparison {
                            base: rb,
                            head: rh,
                            baseline: &baseline,
                        },
                        cumulative: initial_repeat.as_ref().zip(initial.as_ref()).map(
                            |(base, (_, _, validation))| Comparison {
                                base,
                                head: rh,
                                baseline: validation,
                            },
                        ),
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
                journal
                    .attempts
                    .get_mut(&hypothesis_hash)
                    .context("attempt")?
                    .finished = true;
                atomic(&journal_path, &journal)?;
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
        let review = self
            .directory(&id)?
            .join(format!("review-{}.json", candidate.as_str()));
        let pending = review.with_extension("pending.json");
        if pending.exists() {
            ensure!(
                review.exists() && fs::read(&pending)? == fs::read(&review)?,
                "review transaction incomplete; repeat the confirmed review request before publication or upload"
            );
        }
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
            let distribution = crate::remote::gh(&[
                "api",
                &format!(
                    "repos/delinoio/perfine-action/git/trees/{}?recursive=1",
                    sha.as_str()
                ),
            ])?;
            let tree = distribution["tree"]
                .as_array()
                .context("distribution tree")?;
            ensure!(
                distribution["truncated"] != true,
                "incomplete distribution tree"
            );
            for required in [
                "action.cjs",
                "runtime/perfine",
                "runtime/SHA256SUMS",
                "provenance.json",
            ] {
                ensure!(
                    tree.iter()
                        .any(|entry| entry["path"] == required && entry["type"] == "blob"),
                    "missing distributed runtime artifact: {required}"
                );
            }
            let settings = Settings::parse(contents.as_bytes())
                .context("GitHub generation requires a complete --template configuration")?;
            ensure!(
                settings.agent.is_none(),
                "cloud optimization is unavailable; omit agent configuration and use local optimize"
            );
            ensure!(
                settings.docker.image.contains("@sha256:"),
                "GitHub needs a pullable digest-pinned image with provisioned workload dependencies"
            );
            (
                self.repository.join(".github/workflows/perfine.yml"),
                workflow(
                    &contents,
                    &sha,
                    &settings.experiment.incremental_base,
                    &settings.docker.image,
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
pub fn load(
    cfg: &FrozenConfig,
    record: &RunRecord,
    artifacts: &ArtifactStore,
) -> Result<VerifiedRun> {
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

fn validate_execution_baseline(
    engine: &Engine<FileStore>,
    id: &ResourceId,
    config: &FrozenConfig,
    first: &VerifiedRun,
    second: &VerifiedRun,
) -> Result<perfine_core::evidence::BaselineValidation> {
    match validate_baseline(config, first, second) {
        Ok(validation) => Ok(validation),
        Err(error) => {
            // These checks consume retained immutable evidence, so retrying cannot repair it.
            // Persist failure before returning; deadline processing must not report completion.
            engine.fail_baseline(&request(), id, now())?;
            Err(error.into())
        }
    }
}

fn workflow(config: &str, action: &CommitSha, base: &CommitSha, image: &str) -> String {
    let mut inline = String::new();
    for line in config.lines() {
        inline.push_str("            ");
        inline.push_str(line);
        inline.push('\n');
    }
    let image = serde_json::to_string(image).expect("string JSON serialization");
    format!(
        "name: Perfine\non:\n  workflow_dispatch:\n    inputs:\n      experiment:\n        description: Explicit UUID v7 experiment identifier\n        required: true\n        type: string\npermissions:\n  contents: read\njobs:\n  measure:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@d23441a48e516b6c34aea4fa41551a30e30af803\n        with:\n          ref: {}\n          fetch-depth: 0\n          persist-credentials: false\n      - name: Provision immutable workload toolchain and dependencies\n        env:\n          PERFINE_IMAGE: {image}\n        run: docker pull -- \"$PERFINE_IMAGE\"\n      - uses: delinoio/perfine-action@{}\n        with:\n          command: measure\n          experiment: ${{{{ inputs.experiment }}}}\n          config: |\n{inline}",
        base.as_str(),
        action.as_str()
    )
}
#[cfg(test)]
mod tests {
    use super::workflow;
    use perfine_core::types::CommitSha;
    #[test]
    fn manual_workflow_matches_distribution_inputs_and_preserves_inline_config() {
        let sha = CommitSha::new("a".repeat(40)).unwrap();
        let contents = "schema_version: 1\nagent: null\n";
        let text = workflow(contents, &sha, &sha, "registry.example/tools@sha256:1234");
        let value: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_eq!(
            value["jobs"]["measure"]["steps"][0]["with"]["persist-credentials"],
            false
        );
        let inputs = &value["jobs"]["measure"]["steps"][2]["with"];
        assert_eq!(inputs["command"], "measure");
        assert_eq!(inputs["experiment"], "${{ inputs.experiment }}");
        assert_eq!(inputs["config"], contents);
        assert!(value["on"]["push"].is_null());
        assert!(value["on"]["pull_request"].is_null());
        assert_eq!(
            value["on"]["workflow_dispatch"]["inputs"]["experiment"]["required"],
            true
        );
    }
}

#[cfg(test)]
#[path = "../../../crates/perfine-core/tests/common/mod.rs"]
mod initialization_fixtures;

#[cfg(test)]
mod initialization_tests {
    use super::*;
    #[test]
    fn rejected_baseline_stays_failed_after_reopen_resume_and_deadline() {
        use super::initialization_fixtures as f;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("engine");
        let engine = Engine::new(FileStore::open(&path).unwrap());
        let id = ResourceId::generate();
        let config = f::config().freeze().unwrap();
        engine
            .create(&request(), id.clone(), config.clone(), now())
            .unwrap();
        let mut manifest = f::manifest(&config, &id, None, f::sha('a'), [100.0, 10.0]);
        manifest.correctness.insert(
            f::name("project-tests"),
            perfine_core::evidence::CheckResult::Failed,
        );
        let first = f::verified(&config, manifest);
        let second = f::run(&config, &id, None, f::sha('a'), [100.0, 10.0]);
        assert!(validate_execution_baseline(&engine, &id, &config, &first, &second).is_err());
        drop(engine);
        let engine = Engine::new(FileStore::open(&path).unwrap());
        let deadline = engine.status(&id).unwrap().start_deadline;
        for time in [now(), deadline + 1] {
            engine.resume(&request(), &id, &config, time).unwrap();
            engine.tick(&request(), &id, time).unwrap();
            let status = engine.status(&id).unwrap();
            assert_eq!(status.state, ExecutionState::Failed);
            assert!(status.candidates.is_empty());
        }
    }

    #[test]
    fn journal_recovers_original_operation_after_engine_creation() {
        for created in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let engine = Engine::new(FileStore::open(dir.path().join("engine")).unwrap());
            let id = ResourceId::generate();
            let config = super::initialization_fixtures::config().freeze().unwrap();
            let settings = digest(b"original settings");
            let journal = Journal {
                schema_version: Version,
                settings: settings.clone(),
                optimize: false,
                created_request: request(),
                runs: BTreeMap::new(),
                failure: None,
                attempts: BTreeMap::new(),
            };
            let path = dir.path().join("journal.json");
            atomic(
                &path.with_file_name("initialization.json"),
                &Initialization {
                    config: config.clone(),
                    journal: journal.clone(),
                },
            )
            .unwrap();
            if created {
                engine
                    .create(&journal.created_request, id.clone(), config, now())
                    .unwrap();
            }
            assert!(recover_initialization(&engine, &id, &path, &digest(b"changed")).is_err());
            recover_initialization(&engine, &id, &path, &settings).unwrap();
            let restored: Journal = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(restored.created_request, journal.created_request);
            assert!(!restored.optimize);
            let deadline = engine.status(&id).unwrap().start_deadline;
            recover_initialization(&engine, &id, &path, &settings).unwrap();
            assert_eq!(engine.status(&id).unwrap().start_deadline, deadline);
        }
    }
}
