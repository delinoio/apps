// SPDX-License-Identifier: Apache-2.0
use crate::{
    evidence::*,
    policy::{self, Evaluation, ReviewEvidence, Round},
    storage::Store,
    types::*,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEDUP_SECONDS: u64 = 30 * 24 * 60 * 60;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub candidate_count: u32,
    pub simultaneous_candidates: u8,
    pub start_duration_seconds: u64,
    pub candidate_duration_seconds: u64,
}
impl Limits {
    pub fn validate(&self) -> Result<()> {
        if self.candidate_count == 0
            || !(1..=5).contains(&self.simultaneous_candidates)
            || self.start_duration_seconds == 0
            || self.candidate_duration_seconds == 0
        {
            return Err(Error::Invalid("mandatory execution limits"));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub directory: Name,
    pub branch: Name,
    pub build_directory: Name,
}
impl Workspace {
    fn validate(&self) -> Result<()> {
        for path in [&self.directory, &self.build_directory] {
            let p = std::path::Path::new(path.as_str());
            if !p.is_absolute()
                || p.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
            {
                return Err(Error::Invalid("canonical task workspace"));
            }
        }
        if self.directory == self.build_directory {
            return Err(Error::Invalid("separate build directory"));
        }
        Ok(())
    }
    fn overlaps(&self, other: &Self) -> bool {
        let paths = [&self.directory, &self.build_directory];
        let others = [&other.directory, &other.build_directory];
        self.branch == other.branch
            || paths.iter().any(|a| {
                others.iter().any(|b| {
                    std::path::Path::new(a.as_str()).starts_with(b.as_str())
                        || std::path::Path::new(b.as_str()).starts_with(a.as_str())
                })
            })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    TemporaryNetwork,
    TemporaryService,
    Authentication,
    Authorization,
    Correctness,
    Performance,
    InvalidEvidence,
    Permanent,
    UncertainPublication,
}
impl FailureClass {
    pub fn retryable(self) -> bool {
        matches!(self, Self::TemporaryNetwork | Self::TemporaryService)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: ResourceId,
    pub head: CommitSha,
    pub workspace: Workspace,
    pub state: ExecutionState,
    pub started_at: u64,
    pub deadline: u64,
    pub retries: u8,
    pub failure: Option<FailureClass>,
    pub hypothesis: Fingerprint,
    pub evaluation: Option<Evaluation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: ResourceId,
    pub config: FrozenConfig,
    pub state: ExecutionState,
    pub created_at: u64,
    pub start_deadline: u64,
    pub stop_requested: bool,
    pub candidates: BTreeMap<ResourceId, Candidate>,
    pub completed_steps: BTreeMap<Name, Fingerprint>,
}
impl Experiment {
    fn active(&self) -> usize {
        self.candidates
            .values()
            .filter(|c| c.state == ExecutionState::Running)
            .count()
    }
    fn settle(&mut self, now: u64) {
        if self.active() == 0
            && !self.candidates.values().any(|c| {
                c.state == ExecutionState::Failed
                    && c.retries == 0
                    && c.failure.is_some_and(FailureClass::retryable)
                    && now < c.deadline
            })
            && (self.stop_requested
                || self.candidates.len() >= self.config.config().limits.candidate_count as usize)
            && !matches!(
                self.state,
                ExecutionState::Cancelled | ExecutionState::Failed
            )
        {
            self.state = ExecutionState::Completed;
        }
    }
    fn tick(&mut self, now: u64) {
        if now >= self.start_deadline {
            self.stop_requested = true;
        }
        for candidate in self.candidates.values_mut() {
            if candidate.state == ExecutionState::Running && now >= candidate.deadline {
                candidate.state = ExecutionState::Cancelled;
            }
        }
        self.settle(now);
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tombstone {
    pub cleanup_pending: bool,
    pub cleanup_failures: u64,
    pub cleanup_workspaces: Vec<Workspace>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    /// A replay must never dispatch external execution or publication again.
    pub replayed: bool,
    pub request_id: RequestId,
    pub experiment: ResourceId,
    pub candidate: Option<ResourceId>,
    pub state: ExecutionState,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Dedup {
    created_at: u64,
    payload: Fingerprint,
    receipt: Receipt,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub(crate) schema_version: Version,
    pub(crate) last_time: u64,
    pub(crate) experiments: BTreeMap<ResourceId, Experiment>,
    pub(crate) tombstones: BTreeMap<ResourceId, Tombstone>,
    pub(crate) requests: BTreeMap<RequestId, Dedup>,
    pub(crate) approvals: BTreeMap<ResourceId, crate::publication::Approval>,
}
impl Default for Database {
    fn default() -> Self {
        Self {
            schema_version: Version,
            last_time: 0,
            experiments: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            requests: BTreeMap::new(),
            approvals: BTreeMap::new(),
        }
    }
}
impl Database {
    pub fn validate(&self) -> Result<()> {
        for (id, exp) in &self.experiments {
            exp.config.validate()?;
            if id != &exp.id
                || self.tombstones.contains_key(id)
                || exp.candidates.len() > exp.config.config().limits.candidate_count as usize
                || exp.active() > usize::from(exp.config.config().limits.simultaneous_candidates)
            {
                return Err(Error::Invalid("persisted lifecycle"));
            }
        }
        Ok(())
    }
}
/// All mutations run in the store's exclusive transaction, including deduplication.
pub struct Engine<S: Store> {
    pub(crate) store: S,
}
impl<S: Store> Engine<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
    pub(crate) fn mutate(
        &self,
        request: &RequestId,
        id: &ResourceId,
        now: u64,
        payload: &impl Serialize,
        action: impl FnOnce(&mut Database) -> Result<Receipt>,
    ) -> Result<Receipt> {
        let payload = fingerprint(payload)?;
        let receipt = self.store.transaction(|db| {
            if db.tombstones.contains_key(id) {
                return Err(Error::Deleted);
            }
            if now < db.last_time {
                return Err(Error::Invalid("clock moved backwards"));
            }
            if let Some(previous) = db
                .requests
                .get(request)
                .filter(|r| now.saturating_sub(r.created_at) < DEDUP_SECONDS)
            {
                if previous.payload != payload {
                    return Err(Error::RequestConflict);
                }
                let mut receipt = previous.receipt.clone();
                receipt.replayed = true;
                return Ok(receipt);
            }
            let receipt = action(db)?;
            db.last_time = now;
            db.requests
                .retain(|_, r| now.saturating_sub(r.created_at) < DEDUP_SECONDS);
            db.requests.insert(
                request.clone(),
                Dedup {
                    created_at: now,
                    payload,
                    receipt: receipt.clone(),
                },
            );
            Ok(receipt)
        })?;
        tracing::info!(request_id = request.as_str(), experiment_id = id.as_str(), state = ?receipt.state, replayed = receipt.replayed, "perfine transition committed");
        Ok(receipt)
    }
    pub fn create(
        &self,
        request: &RequestId,
        id: ResourceId,
        config: FrozenConfig,
        now: u64,
    ) -> Result<Receipt> {
        config.validate()?;
        self.mutate(request, &id, now, &("create", &id, &config), |db| {
            if db.experiments.contains_key(&id) {
                return Err(Error::State);
            }
            let deadline = now
                .checked_add(config.config().limits.start_duration_seconds)
                .ok_or(Error::Invalid("deadline overflow"))?;
            db.experiments.insert(
                id.clone(),
                Experiment {
                    id: id.clone(),
                    config: config.clone(),
                    state: ExecutionState::Queued,
                    created_at: now,
                    start_deadline: deadline,
                    stop_requested: false,
                    candidates: BTreeMap::new(),
                    completed_steps: BTreeMap::new(),
                },
            );
            Ok(Receipt {
                replayed: false,
                request_id: request.clone(),
                experiment: id.clone(),
                candidate: None,
                state: ExecutionState::Queued,
            })
        })
    }
    /// The profile is verified before admitting a hypothesis. The environment adapter
    /// must reserve these canonical paths and enforce credential isolation before execution.
    #[allow(clippy::too_many_arguments)]
    pub fn start_candidate(
        &self,
        request: &RequestId,
        id: &ResourceId,
        candidate: ResourceId,
        head: CommitSha,
        workspace: Workspace,
        profile: &VerifiedRun,
        baseline: &BaselineValidation,
        optimized_paths: &std::collections::BTreeSet<Name>,
        now: u64,
    ) -> Result<Receipt> {
        workspace.validate()?;
        self.mutate(
            request,
            id,
            now,
            &(
                "start",
                id,
                &candidate,
                &head,
                &workspace,
                profile.digest(),
                baseline.runs(),
                optimized_paths,
            ),
            |db| {
                if db
                    .experiments
                    .values()
                    .flat_map(|e| e.candidates.values())
                    .any(|c| c.workspace.overlaps(&workspace))
                {
                    return Err(Error::Invalid("workspace already reserved"));
                }
                let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
                let cfg = exp.config.config();
                if exp.stop_requested
                    || now >= exp.start_deadline
                    || !matches!(exp.state, ExecutionState::Queued | ExecutionState::Running)
                    || exp.candidates.len() >= cfg.limits.candidate_count as usize
                    || exp.active() >= usize::from(cfg.limits.simultaneous_candidates)
                {
                    return Err(Error::Limit);
                }
                if exp.candidates.contains_key(&candidate)
                    || exp
                        .candidates
                        .values()
                        .any(|c| c.workspace.overlaps(&workspace))
                {
                    return Err(Error::Invalid("candidate isolation"));
                }
                let p = profile.manifest();
                if !baseline.matches(&exp.config, &cfg.incremental_base, id)
                    || p.identity.configuration != *exp.config.fingerprint()
                    || p.identity.source != cfg.incremental_base
                    || p.identity.experiment != *id
                    || !profile.correctness_passed()
                    || optimized_paths.is_empty()
                    || !optimized_paths
                        .iter()
                        .all(|path| p.profile_paths.values().any(|paths| paths.contains(path)))
                {
                    return Err(Error::Incomplete("profile-backed hypothesis"));
                }
                let deadline = now
                    .checked_add(cfg.limits.candidate_duration_seconds)
                    .ok_or(Error::Invalid("candidate deadline overflow"))?;
                exp.candidates.insert(
                    candidate.clone(),
                    Candidate {
                        id: candidate.clone(),
                        head: head.clone(),
                        workspace: workspace.clone(),
                        state: ExecutionState::Running,
                        started_at: now,
                        deadline,
                        retries: 0,
                        failure: None,
                        hypothesis: profile.digest().clone(),
                        evaluation: None,
                    },
                );
                exp.state = ExecutionState::Running;
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: id.clone(),
                    candidate: Some(candidate.clone()),
                    state: ExecutionState::Running,
                })
            },
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn finish_candidate(
        &self,
        request: &RequestId,
        id: &ResourceId,
        candidate: &ResourceId,
        round: &Round<'_>,
        repeat: Option<&Round<'_>>,
        review: &ReviewEvidence,
        now: u64,
    ) -> Result<Receipt> {
        // Evaluation is recalculated inside the transaction against persisted configuration.
        let run_digests = |r: &Round<'_>| {
            let mut v = vec![r.incremental.base.digest(), r.incremental.head.digest()];
            if let Some(c) = &r.cumulative {
                v.push(c.base.digest());
            }
            v.into_iter().cloned().collect::<Vec<_>>()
        };
        self.mutate(
            request,
            id,
            now,
            &(
                "finish",
                id,
                candidate,
                run_digests(round),
                repeat.map(run_digests),
                review,
            ),
            |db| {
                let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
                let c = exp.candidates.get_mut(candidate).ok_or(Error::State)?;
                if c.state != ExecutionState::Running || now >= c.deadline {
                    return Err(Error::Limit);
                }
                let identity = &round.incremental.head.manifest().identity;
                if identity.experiment != *id
                    || identity.candidate.as_ref() != Some(candidate)
                    || identity.source != c.head
                {
                    return Err(Error::Mismatch("candidate evidence"));
                }
                c.evaluation = Some(policy::evaluate(&exp.config, round, repeat, review)?);
                c.state = ExecutionState::Completed;
                exp.tick(now);
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: id.clone(),
                    candidate: Some(candidate.clone()),
                    state: ExecutionState::Completed,
                })
            },
        )
    }
    pub fn fail_candidate(
        &self,
        request: &RequestId,
        id: &ResourceId,
        candidate: &ResourceId,
        failure: FailureClass,
        now: u64,
    ) -> Result<Receipt> {
        self.mutate(request, id, now, &("fail", id, candidate, failure), |db| {
            let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
            let c = exp.candidates.get_mut(candidate).ok_or(Error::State)?;
            if c.state != ExecutionState::Running {
                return Err(Error::State);
            }
            c.state = ExecutionState::Failed;
            c.failure = Some(failure);
            exp.tick(now);
            Ok(Receipt {
                replayed: false,
                request_id: request.clone(),
                experiment: id.clone(),
                candidate: Some(candidate.clone()),
                state: ExecutionState::Failed,
            })
        })
    }
    pub fn retry_candidate(
        &self,
        request: &RequestId,
        id: &ResourceId,
        candidate: &ResourceId,
        now: u64,
    ) -> Result<Receipt> {
        self.mutate(request, id, now, &("retry", id, candidate), |db| {
            let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
            if exp.state == ExecutionState::Cancelled
                || exp.active() >= usize::from(exp.config.config().limits.simultaneous_candidates)
            {
                return Err(Error::Limit);
            }
            let c = exp.candidates.get_mut(candidate).ok_or(Error::State)?;
            if c.state != ExecutionState::Failed
                || !c.failure.is_some_and(FailureClass::retryable)
                || c.retries != 0
                || now >= c.deadline
            {
                return Err(Error::Limit);
            }
            c.retries = 1;
            c.state = ExecutionState::Running;
            c.failure = None;
            exp.state = ExecutionState::Running;
            Ok(Receipt {
                replayed: false,
                request_id: request.clone(),
                experiment: id.clone(),
                candidate: Some(candidate.clone()),
                state: ExecutionState::Running,
            })
        })
    }
    /// Graceful stop completes normally, including the valid no-candidate outcome.
    pub fn stop(
        &self,
        request: &RequestId,
        id: &ResourceId,
        immediate: bool,
        now: u64,
    ) -> Result<Receipt> {
        self.mutate(request, id, now, &("stop", id, immediate), |db| {
            let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
            if matches!(
                exp.state,
                ExecutionState::Completed | ExecutionState::Failed | ExecutionState::Cancelled
            ) {
                return Err(Error::State);
            }
            exp.stop_requested = true;
            if immediate {
                exp.state = ExecutionState::Cancelled;
                for c in exp
                    .candidates
                    .values_mut()
                    .filter(|c| c.state == ExecutionState::Running)
                {
                    c.state = ExecutionState::Cancelled;
                }
            }
            exp.tick(now);
            Ok(Receipt {
                replayed: false,
                request_id: request.clone(),
                experiment: id.clone(),
                candidate: None,
                state: exp.state,
            })
        })
    }
    pub fn tick(&self, request: &RequestId, id: &ResourceId, now: u64) -> Result<Receipt> {
        self.mutate(request, id, now, &("tick", id, now), |db| {
            let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
            exp.tick(now);
            Ok(Receipt {
                replayed: false,
                request_id: request.clone(),
                experiment: id.clone(),
                candidate: None,
                state: exp.state,
            })
        })
    }
    /// Reuse completed steps only under the exact frozen source/inputs/tools/policy.
    /// Resume does not reset time limits or restart cancelled candidate processes.
    pub fn resume(
        &self,
        request: &RequestId,
        id: &ResourceId,
        config: &FrozenConfig,
        now: u64,
    ) -> Result<Receipt> {
        config.validate()?;
        self.mutate(
            request,
            id,
            now,
            &("resume", id, config.fingerprint()),
            |db| {
                let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
                if exp.config.fingerprint() != config.fingerprint() {
                    return Err(Error::Mismatch(
                        "resume fingerprint; create a new experiment",
                    ));
                }
                exp.tick(now);
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: id.clone(),
                    candidate: None,
                    state: exp.state,
                })
            },
        )
    }
    pub fn record_step(
        &self,
        request: &RequestId,
        id: &ResourceId,
        config: &FrozenConfig,
        step: Name,
        artifact: Fingerprint,
        now: u64,
    ) -> Result<Receipt> {
        config.validate()?;
        self.mutate(
            request,
            id,
            now,
            &("step", id, config.fingerprint(), &step, &artifact),
            |db| {
                let exp = db.experiments.get_mut(id).ok_or(Error::State)?;
                if exp.config.fingerprint() != config.fingerprint() {
                    return Err(Error::Mismatch("step provenance"));
                }
                if exp
                    .completed_steps
                    .get(&step)
                    .is_some_and(|v| v != &artifact)
                {
                    return Err(Error::Mismatch("immutable completed step"));
                }
                exp.completed_steps.insert(step.clone(), artifact.clone());
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: id.clone(),
                    candidate: None,
                    state: exp.state,
                })
            },
        )
    }
    pub fn status(&self, id: &ResourceId) -> Result<Experiment> {
        self.store.transaction(|db| {
            if db.tombstones.contains_key(id) {
                return Err(Error::Deleted);
            }
            db.experiments.get(id).cloned().ok_or(Error::State)
        })
    }
    pub fn delete(&self, request: &RequestId, id: &ResourceId, now: u64) -> Result<Receipt> {
        self.mutate(request, id, now, &("delete", id), |db| {
            let exp = db.experiments.remove(id).ok_or(Error::State)?;
            db.tombstones.insert(
                id.clone(),
                Tombstone {
                    cleanup_pending: true,
                    cleanup_failures: 0,
                    cleanup_workspaces: exp.candidates.into_values().map(|c| c.workspace).collect(),
                },
            );
            Ok(Receipt {
                replayed: false,
                request_id: request.clone(),
                experiment: id.clone(),
                candidate: None,
                state: ExecutionState::Cancelled,
            })
        })
    }
    /// Deletion retries intentionally have no 30-day expiry or one-retry ceiling.
    pub fn cleanup_result(&self, id: &ResourceId, succeeded: bool) -> Result<Tombstone> {
        self.store.transaction(|db| {
            let t = db.tombstones.get_mut(id).ok_or(Error::State)?;
            if t.cleanup_pending {
                if succeeded {
                    t.cleanup_pending = false;
                    t.cleanup_workspaces.clear();
                    db.approvals.retain(|_, a| &a.binding.experiment != id);
                } else {
                    t.cleanup_failures = t.cleanup_failures.saturating_add(1);
                    tracing::warn!(
                        experiment_id = id.as_str(),
                        failures = t.cleanup_failures,
                        "perfine cleanup remains pending"
                    );
                }
            }
            Ok(t.clone())
        })
    }
}
