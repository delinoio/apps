// SPDX-License-Identifier: Apache-2.0
mod common;
use common::*;
use perfine_core::{lifecycle::*, policy::*, publication::*, storage::*, types::*};
fn prepared() -> (
    tempfile::TempDir,
    Engine<FileStore>,
    Fixture,
    ApprovalBinding,
) {
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    let f = Fixture::new(config(), [10000.0, 10000.0], [9000.0, 9000.0]);
    e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
        .unwrap();
    start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
    e.finish_candidate(
        &request(),
        &f.experiment,
        &f.candidate,
        &f.round(),
        None,
        &ReviewEvidence::default(),
        102,
    )
    .unwrap();
    let eval = e.status(&f.experiment).unwrap().candidates[&f.candidate]
        .evaluation
        .clone()
        .unwrap();
    let binding = ApprovalBinding {
        experiment: f.experiment.clone(),
        candidate: f.candidate.clone(),
        content: PublicationContent {
            target_repository: name("owner/repo"),
            target_base: name("main"),
            base_sha: sha('a'),
            head_sha: sha('d'),
            title: name("perf: optimize"),
            body: "Measured fixture improvement".into(),
            production_diff: digest(b"diff"),
        },
        evaluation: fingerprint(&eval).unwrap(),
    };
    (dir, e, f, binding)
}
fn auth() -> PublishAuthority {
    PublishAuthority {
        currently_authorized: true,
    }
}
#[test]
fn exact_approval_rejects_target_base_sha_title_body_diff_and_evaluation_changes() {
    let (_dir, e, f, binding) = prepared();
    let approval = ResourceId::generate();
    e.approve(
        &request(),
        approval.clone(),
        binding.clone(),
        name("publisher"),
        None,
        auth(),
        103,
    )
    .unwrap();
    let mut mutations = vec![];
    let mut b = binding.clone();
    b.content.target_repository = name("other/repo");
    mutations.push(b);
    let mut b = binding.clone();
    b.content.target_base = name("release");
    mutations.push(b);
    let mut b = binding.clone();
    b.content.base_sha = sha('e');
    mutations.push(b);
    let mut b = binding.clone();
    b.content.head_sha = sha('e');
    mutations.push(b);
    let mut b = binding.clone();
    b.content.title = name("other title");
    mutations.push(b);
    let mut b = binding.clone();
    b.content.body.push('!');
    mutations.push(b);
    let mut b = binding.clone();
    b.content.production_diff = digest(b"other diff");
    mutations.push(b);
    let mut b = binding.clone();
    b.evaluation = digest(b"other evaluation");
    mutations.push(b);
    for b in mutations {
        assert!(
            e.begin_publication(&request(), &approval, &b, auth(), 104)
                .is_err()
        );
    }
    assert_eq!(
        e.approval(&f.experiment, &approval).unwrap().state,
        PublicationState::Approved
    );
}
#[test]
fn uncertain_publication_requires_reconciliation_and_retained_records_outlive_dedup() {
    let (dir, e, f, binding) = prepared();
    let approval = ResourceId::generate();
    e.approve(
        &request(),
        approval.clone(),
        binding.clone(),
        name("publisher"),
        None,
        auth(),
        103,
    )
    .unwrap();
    let req = request();
    assert!(
        !e.begin_publication(&req, &approval, &binding, auth(), 104)
            .unwrap()
            .replayed
    );
    assert!(
        e.begin_publication(&req, &approval, &binding, auth(), 105)
            .unwrap()
            .replayed
    );
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    assert!(
        e.begin_publication(&request(), &approval, &binding, auth(), 106)
            .is_err()
    );
    e.reconcile_publication(
        &request(),
        &approval,
        &binding,
        Reconciliation::Unknown,
        auth(),
        107,
    )
    .unwrap();
    assert!(
        e.begin_publication(&request(), &approval, &binding, auth(), 108)
            .is_err()
    );
    e.reconcile_publication(
        &request(),
        &approval,
        &binding,
        Reconciliation::DraftFound {
            pull_request: name("https://github.com/owner/repo/pull/1"),
        },
        auth(),
        109,
    )
    .unwrap();
    assert!(
        e.begin_publication(&req, &approval, &binding, auth(), 104 + DEDUP_SECONDS)
            .is_err()
    );
    assert_eq!(
        e.approval(&f.experiment, &approval).unwrap().state,
        PublicationState::Published
    );
}
#[test]
fn absence_reconciliation_allows_only_one_retry_and_revoked_authority_blocks_replay() {
    let (_dir, e, _f, binding) = prepared();
    let approval = ResourceId::generate();
    e.approve(
        &request(),
        approval.clone(),
        binding.clone(),
        name("publisher"),
        None,
        auth(),
        103,
    )
    .unwrap();
    let req = request();
    e.begin_publication(&req, &approval, &binding, auth(), 104)
        .unwrap();
    assert!(
        e.begin_publication(
            &req,
            &approval,
            &binding,
            PublishAuthority {
                currently_authorized: false
            },
            105
        )
        .is_err()
    );
    e.reconcile_publication(
        &request(),
        &approval,
        &binding,
        Reconciliation::Absent,
        auth(),
        106,
    )
    .unwrap();
    e.begin_publication(&request(), &approval, &binding, auth(), 107)
        .unwrap();
    e.reconcile_publication(
        &request(),
        &approval,
        &binding,
        Reconciliation::Absent,
        auth(),
        108,
    )
    .unwrap();
    assert!(
        e.begin_publication(&request(), &approval, &binding, auth(), 109)
            .is_err()
    );
}

#[test]
fn a_new_approval_does_not_reset_publication_retry_budget() {
    let (_dir, e, _f, binding) = prepared();
    let first = ResourceId::generate();
    e.approve(
        &request(),
        first.clone(),
        binding.clone(),
        name("publisher"),
        None,
        auth(),
        103,
    )
    .unwrap();
    for now in [104, 106] {
        e.begin_publication(&request(), &first, &binding, auth(), now)
            .unwrap();
        e.reconcile_publication(
            &request(),
            &first,
            &binding,
            Reconciliation::Absent,
            auth(),
            now + 1,
        )
        .unwrap();
    }
    let second = ResourceId::generate();
    e.approve(
        &request(),
        second.clone(),
        binding.clone(),
        name("publisher"),
        None,
        auth(),
        108,
    )
    .unwrap();
    assert!(
        e.begin_publication(&request(), &second, &binding, auth(), 109)
            .is_err()
    );
}
#[test]
fn protected_regression_requires_explicit_judgment_before_approval() {
    let mut cfg = config();
    cfg.conditions
        .cases
        .get_mut(&name("small"))
        .unwrap()
        .protected = true;
    let f = Fixture::new(cfg, [10000.0, 100.0], [8000.0, 102.0]);
    let e = Engine::new(MemoryStore::default());
    e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
        .unwrap();
    start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
    e.finish_candidate(
        &request(),
        &f.experiment,
        &f.candidate,
        &f.round(),
        None,
        &ReviewEvidence::default(),
        102,
    )
    .unwrap();
    let eval = e.status(&f.experiment).unwrap().candidates[&f.candidate]
        .evaluation
        .clone()
        .unwrap();
    assert!(eval.human_judgment_required);
    let binding = ApprovalBinding {
        experiment: f.experiment.clone(),
        candidate: f.candidate.clone(),
        content: PublicationContent {
            target_repository: name("owner/repo"),
            target_base: name("main"),
            base_sha: sha('a'),
            head_sha: sha('d'),
            title: name("perf: reviewed tradeoff"),
            body: "Explicit protected-case tradeoff".into(),
            production_diff: digest(b"diff"),
        },
        evaluation: fingerprint(&eval).unwrap(),
    };
    assert!(
        e.approve(
            &request(),
            ResourceId::generate(),
            binding.clone(),
            name("publisher"),
            None,
            auth(),
            103
        )
        .is_err()
    );
    e.approve(
        &request(),
        ResourceId::generate(),
        binding,
        name("publisher"),
        Some(name("accept protected regression after review")),
        auth(),
        103,
    )
    .unwrap();
}
