// SPDX-License-Identifier: Apache-2.0
use crate::{
    lifecycle::{Engine, Receipt},
    policy::Reason,
    storage::Store,
    types::*,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationContent {
    pub target_repository: Name,
    pub target_base: Name,
    pub base_sha: CommitSha,
    pub head_sha: CommitSha,
    pub title: Name,
    pub body: String,
    /// Hash of the reviewed production diff, excluding temporary harness/CI changes.
    pub production_diff: Fingerprint,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalBinding {
    pub experiment: ResourceId,
    pub candidate: ResourceId,
    pub content: PublicationContent,
    pub evaluation: Fingerprint,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    Approved,
    Dispatching,
    Uncertain,
    Published,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: ResourceId,
    pub binding: ApprovalBinding,
    pub approved_by: Name,
    pub human_judgment: Option<Name>,
    pub state: PublicationState,
    pub pull_request: Option<Name>,
    pub dispatch_attempts: u8,
}
/// Authority is deliberately checked on every operation by the host. Local callers
/// supply their current user authorization. A stored approval is not a permission grant.
#[derive(Debug, Clone, Copy)]
pub struct PublishAuthority {
    pub currently_authorized: bool,
}
impl PublishAuthority {
    fn check(self) -> Result<()> {
        if self.currently_authorized {
            Ok(())
        } else {
            Err(Error::Invalid("current publish authority"))
        }
    }
}
impl<S: Store> Engine<S> {
    #[allow(clippy::too_many_arguments)]
    pub fn approve(
        &self,
        request: &RequestId,
        approval_id: ResourceId,
        binding: ApprovalBinding,
        approved_by: Name,
        human_judgment: Option<Name>,
        authority: PublishAuthority,
        now: u64,
    ) -> Result<Receipt> {
        authority.check()?;
        self.mutate(
            request,
            &binding.experiment,
            now,
            &(
                "approve",
                &approval_id,
                &binding,
                &approved_by,
                &human_judgment,
            ),
            |db| {
                let exp = db
                    .experiments
                    .get(&binding.experiment)
                    .ok_or(Error::State)?;
                let candidate = exp.candidates.get(&binding.candidate).ok_or(Error::State)?;
                let evaluation = candidate
                    .evaluation
                    .as_ref()
                    .ok_or(Error::Incomplete("candidate evaluation"))?;
                if candidate.state != ExecutionState::Completed
                    || binding.content.head_sha != candidate.head
                    || binding.content.base_sha != exp.config.config().incremental_base
                    || binding.evaluation != fingerprint(evaluation)?
                {
                    return Err(Error::Mismatch("measured publication source"));
                }
                if evaluation.repeat_required
                    || matches!(
                        evaluation.verdict,
                        Verdict::Inconclusive | Verdict::NotEvaluated
                    )
                {
                    return Err(Error::Incomplete("publication evidence"));
                }
                if evaluation.human_judgment_required && human_judgment.is_none() {
                    return Err(Error::Incomplete("explicit human judgment"));
                }
                if evaluation.verdict == Verdict::Regression
                    && evaluation.reason != Reason::HumanJudgment
                {
                    return Err(Error::Invalid("rejected performance candidate"));
                }
                if db.approvals.contains_key(&approval_id) {
                    return Err(Error::State);
                }
                db.approvals.insert(
                    approval_id.clone(),
                    Approval {
                        id: approval_id.clone(),
                        binding: binding.clone(),
                        approved_by: approved_by.clone(),
                        human_judgment: human_judgment.clone(),
                        state: PublicationState::Approved,
                        pull_request: None,
                        dispatch_attempts: 0,
                    },
                );
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: binding.experiment.clone(),
                    candidate: Some(binding.candidate.clone()),
                    state: ExecutionState::Completed,
                })
            },
        )
    }
    /// Persist Dispatching BEFORE calling GitHub. A process crash requires reconciliation.
    /// This only authorizes creation of a draft; it never publishes or merges itself.
    pub fn begin_publication(
        &self,
        request: &RequestId,
        approval: &ResourceId,
        binding: &ApprovalBinding,
        authority: PublishAuthority,
        now: u64,
    ) -> Result<Receipt> {
        authority.check()?;
        self.mutate(
            request,
            &binding.experiment,
            now,
            &("publish", approval, binding),
            |db| {
                let attempts: u32 = db
                    .approvals
                    .values()
                    .filter(|a| {
                        a.binding.experiment == binding.experiment
                            && a.binding.candidate == binding.candidate
                    })
                    .map(|a| u32::from(a.dispatch_attempts))
                    .sum();
                if attempts >= 2 {
                    return Err(Error::Limit);
                }
                if db.approvals.values().any(|a| {
                    a.id != *approval
                        && a.binding.experiment == binding.experiment
                        && a.binding.candidate == binding.candidate
                        && a.state != PublicationState::Approved
                }) {
                    return Err(Error::State);
                }
                let a = db.approvals.get_mut(approval).ok_or(Error::State)?;
                if a.binding != *binding {
                    return Err(Error::Mismatch("exact publication approval"));
                }
                if a.state != PublicationState::Approved || a.dispatch_attempts >= 2 {
                    return Err(Error::State);
                }
                a.state = PublicationState::Dispatching;
                a.dispatch_attempts += 1;
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: binding.experiment.clone(),
                    candidate: Some(binding.candidate.clone()),
                    state: ExecutionState::Completed,
                })
            },
        )
    }
    /// GitHub reconciliation is required even after deduplication records expire.
    /// `Absent` permits one new dispatch only after the host establishes absence.
    pub fn reconcile_publication(
        &self,
        request: &RequestId,
        approval: &ResourceId,
        binding: &ApprovalBinding,
        result: Reconciliation,
        authority: PublishAuthority,
        now: u64,
    ) -> Result<Receipt> {
        authority.check()?;
        self.mutate(
            request,
            &binding.experiment,
            now,
            &("reconcile", approval, binding, &result),
            |db| {
                let a = db.approvals.get_mut(approval).ok_or(Error::State)?;
                if a.binding != *binding {
                    return Err(Error::Mismatch("reconciled publication"));
                }
                if !matches!(
                    a.state,
                    PublicationState::Dispatching | PublicationState::Uncertain
                ) {
                    return Err(Error::State);
                }
                match &result {
                    Reconciliation::Unknown => a.state = PublicationState::Uncertain,
                    Reconciliation::Absent => a.state = PublicationState::Approved,
                    Reconciliation::DraftFound { pull_request } => {
                        a.state = PublicationState::Published;
                        a.pull_request = Some(pull_request.clone());
                    }
                }
                Ok(Receipt {
                    replayed: false,
                    request_id: request.clone(),
                    experiment: binding.experiment.clone(),
                    candidate: Some(binding.candidate.clone()),
                    state: ExecutionState::Completed,
                })
            },
        )
    }
    pub fn approval(&self, experiment: &ResourceId, id: &ResourceId) -> Result<Approval> {
        self.store.transaction(|db| {
            if db.tombstones.contains_key(experiment) {
                return Err(Error::Deleted);
            }
            db.approvals
                .get(id)
                .filter(|a| &a.binding.experiment == experiment)
                .cloned()
                .ok_or(Error::State)
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reconciliation {
    Unknown,
    Absent,
    DraftFound { pull_request: Name },
}
