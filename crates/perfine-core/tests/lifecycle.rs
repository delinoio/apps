// SPDX-License-Identifier: Apache-2.0
mod common;
use common::*;
use perfine_core::{lifecycle::*, policy::*, storage::*, types::*};
use std::{collections::BTreeSet, sync::Arc};

fn fixture() -> Fixture {
    Fixture::new(config(), [10000.0, 10000.0], [9000.0, 9000.0])
}
fn engine(f: &Fixture) -> Engine<MemoryStore> {
    let e = Engine::new(MemoryStore::default());
    e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
        .unwrap();
    e
}
#[test]
fn limits_are_required_and_concurrency_six_is_rejected() {
    for (count, concurrency, start, candidate) in [
        (0, 1, 1, 1),
        (1, 0, 1, 1),
        (10, 6, 1, 1),
        (1, 1, 0, 1),
        (1, 1, 1, 0),
    ] {
        let mut c = config();
        c.limits = Limits {
            candidate_count: count,
            simultaneous_candidates: concurrency,
            start_duration_seconds: start,
            candidate_duration_seconds: candidate,
        };
        assert!(c.freeze().is_err());
    }
    let f = fixture();
    let e = engine(&f);
    for n in 0..5 {
        start(&e, &f, ResourceId::generate(), n, 101).unwrap();
    }
    assert!(start(&e, &f, ResourceId::generate(), 6, 101).is_err());
    assert_eq!(e.status(&f.experiment).unwrap().candidates.len(), 5);
}
#[test]
fn candidate_count_is_not_reset_by_failure_or_cancellation() {
    let mut cfg = config();
    cfg.limits.candidate_count = 1;
    let f = Fixture::new(cfg, [100.0, 100.0], [90.0, 90.0]);
    let e = engine(&f);
    start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
    e.fail_candidate(
        &request(),
        &f.experiment,
        &f.candidate,
        FailureClass::Permanent,
        102,
    )
    .unwrap();
    assert!(start(&e, &f, ResourceId::generate(), 1, 103).is_err());
}
#[test]
fn same_workspace_branch_or_nested_build_is_rejected() {
    let f = fixture();
    let e = engine(&f);
    start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
    assert!(start(&e, &f, ResourceId::generate(), 0, 101).is_err());
    let mut w = workspace(1);
    w.build_directory = name("/tmp/perfine/candidate-0/build");
    assert!(
        e.start_candidate(
            &request(),
            &f.experiment,
            ResourceId::generate(),
            sha('d'),
            w,
            &f.base,
            &f.baseline,
            &BTreeSet::from([name("src/hot.rs")]),
            101
        )
        .is_err()
    );
}
#[test]
fn deadline_only_stops_new_candidates_and_individual_timeout_is_separate() {
    let f = fixture();
    let e = engine(&f);
    start(&e, &f, f.candidate.clone(), 0, 199).unwrap();
    e.tick(&request(), &f.experiment, 200).unwrap();
    assert_eq!(
        e.status(&f.experiment).unwrap().candidates[&f.candidate].state,
        ExecutionState::Running
    );
    assert!(start(&e, &f, ResourceId::generate(), 1, 200).is_err());
    e.tick(&request(), &f.experiment, 229).unwrap();
    assert_eq!(
        e.status(&f.experiment).unwrap().candidates[&f.candidate].state,
        ExecutionState::Cancelled
    );
    assert!(
        e.finish_candidate(
            &request(),
            &f.experiment,
            &f.candidate,
            &f.round(),
            None,
            &ReviewEvidence::default(),
            229
        )
        .is_err()
    );
}
#[test]
fn graceful_stop_allows_finish_immediate_cancel_preserves_prior_evidence() {
    for immediate in [false, true] {
        let f = fixture();
        let e = engine(&f);
        start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
        e.record_step(
            &request(),
            &f.experiment,
            &f.cfg,
            name("baseline"),
            f.base.digest().clone(),
            102,
        )
        .unwrap();
        e.stop(&request(), &f.experiment, immediate, 103).unwrap();
        assert!(start(&e, &f, ResourceId::generate(), 1, 104).is_err());
        let result = e.finish_candidate(
            &request(),
            &f.experiment,
            &f.candidate,
            &f.round(),
            None,
            &ReviewEvidence::default(),
            105,
        );
        assert_eq!(result.is_ok(), !immediate);
        let status = e.status(&f.experiment).unwrap();
        assert_eq!(status.completed_steps.len(), 1);
        assert_eq!(
            status.candidates[&f.candidate].workspace.directory,
            workspace(0).directory
        );
        assert_eq!(
            status.state,
            if immediate {
                ExecutionState::Cancelled
            } else {
                ExecutionState::Completed
            }
        );
    }
}
#[test]
fn no_candidate_is_a_normal_completed_execution() {
    let f = fixture();
    let e = engine(&f);
    e.stop(&request(), &f.experiment, false, 101).unwrap();
    let state = e.status(&f.experiment).unwrap();
    assert_eq!(state.state, ExecutionState::Completed);
    assert!(state.candidates.is_empty());
}
#[test]
fn only_transient_failure_retries_once_without_extending_deadline() {
    for failure in [
        FailureClass::TemporaryNetwork,
        FailureClass::TemporaryService,
        FailureClass::Authentication,
        FailureClass::Authorization,
        FailureClass::Correctness,
        FailureClass::Performance,
        FailureClass::InvalidEvidence,
        FailureClass::Permanent,
        FailureClass::UncertainPublication,
    ] {
        let f = fixture();
        let e = engine(&f);
        start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
        e.fail_candidate(&request(), &f.experiment, &f.candidate, failure, 102)
            .unwrap();
        let r = e.retry_candidate(&request(), &f.experiment, &f.candidate, 103);
        assert_eq!(r.is_ok(), failure.retryable());
        if r.is_ok() {
            e.fail_candidate(&request(), &f.experiment, &f.candidate, failure, 104)
                .unwrap();
            assert!(
                e.retry_candidate(&request(), &f.experiment, &f.candidate, 105)
                    .is_err()
            );
        }
        assert_eq!(
            e.status(&f.experiment).unwrap().candidates[&f.candidate].deadline,
            131
        );
    }
}
#[test]
fn dedup_horizon_is_from_first_creation_and_conflicts_fail() {
    let e = Engine::new(MemoryStore::default());
    let f = fixture();
    let req = request();
    assert!(
        !e.create(&req, f.experiment.clone(), f.cfg.clone(), 100)
            .unwrap()
            .replayed
    );
    assert!(
        e.create(
            &req,
            f.experiment.clone(),
            f.cfg.clone(),
            100 + DEDUP_SECONDS - 1
        )
        .unwrap()
        .replayed
    );
    assert!(matches!(
        e.create(
            &req,
            ResourceId::generate(),
            f.cfg.clone(),
            100 + DEDUP_SECONDS - 1
        ),
        Err(Error::RequestConflict)
    ));
    // Replaying does not move the original expiry. A new resource can use the ID at the boundary.
    assert!(
        !e.create(
            &req,
            ResourceId::generate(),
            f.cfg.clone(),
            100 + DEDUP_SECONDS
        )
        .unwrap()
        .replayed
    );
}
#[test]
fn durable_restart_reuses_exact_steps_and_rejects_changed_inputs() {
    let dir = tempfile::tempdir().unwrap();
    let f = fixture();
    {
        let e = Engine::new(FileStore::open(dir.path()).unwrap());
        e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
            .unwrap();
        e.record_step(
            &request(),
            &f.experiment,
            &f.cfg,
            name("baseline"),
            f.base.digest().clone(),
            101,
        )
        .unwrap();
    }
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    e.resume(&request(), &f.experiment, &f.cfg, 102).unwrap();
    assert_eq!(e.status(&f.experiment).unwrap().completed_steps.len(), 1);
    let mut cfg = f.cfg.config().clone();
    cfg.conditions.runtime_flags.push("--jitless".into());
    assert!(
        e.resume(&request(), &f.experiment, &cfg.freeze().unwrap(), 103)
            .is_err()
    );
    assert!(
        e.record_step(
            &request(),
            &f.experiment,
            &f.cfg,
            name("baseline"),
            digest(b"different"),
            103
        )
        .is_err()
    );
    assert!(e.tick(&request(), &f.experiment, 99).is_err());
}
#[test]
fn corrupted_snapshot_fails_closed_and_does_not_reset_state() {
    let dir = tempfile::tempdir().unwrap();
    let f = fixture();
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
        .unwrap();
    let path = dir.path().join("state-v1.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["database"]["last_time"] = 900.into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(e.status(&f.experiment).is_err());
}
#[test]
fn tombstones_survive_dedup_expiry_and_cleanup_failure() {
    let dir = tempfile::tempdir().unwrap();
    let f = fixture();
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
        .unwrap();
    start(&e, &f, f.candidate.clone(), 0, 101).unwrap();
    let hypothesis = e.status(&f.experiment).unwrap().candidates[&f.candidate]
        .hypothesis
        .clone();
    e.delete(&request(), &f.experiment, 102).unwrap();
    assert!(
        e.cleanup_result(&f.experiment, false)
            .unwrap()
            .cleanup_evidence
            .contains(&hypothesis)
    );
    assert!(matches!(e.status(&f.experiment), Err(Error::Deleted)));
    for _ in 0..3 {
        assert!(
            e.cleanup_result(&f.experiment, false)
                .unwrap()
                .cleanup_pending
        );
    }
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    assert!(matches!(
        e.create(
            &request(),
            f.experiment.clone(),
            f.cfg.clone(),
            DEDUP_SECONDS + 103
        ),
        Err(Error::Deleted)
    ));
    let t = e.cleanup_result(&f.experiment, true).unwrap();
    assert!(!t.cleanup_pending);
    assert_eq!(t.cleanup_failures, 4);
    assert!(t.cleanup_evidence.is_empty());
    assert!(t.cleanup_workspaces.is_empty());
    assert!(matches!(
        e.resume(&request(), &f.experiment, &f.cfg, DEDUP_SECONDS + 104),
        Err(Error::Deleted)
    ));
}
#[test]
fn competing_writers_cannot_exceed_five_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let f = Arc::new(fixture());
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    e.create(&request(), f.experiment.clone(), f.cfg.clone(), 100)
        .unwrap();
    let joins: Vec<_> = (0..12)
        .map(|n| {
            let path = dir.path().to_path_buf();
            let f = f.clone();
            std::thread::spawn(move || {
                let e = Engine::new(FileStore::open(path).unwrap());
                start(&e, &f, ResourceId::generate(), n, 101).is_ok()
            })
        })
        .collect();
    assert_eq!(
        joins
            .into_iter()
            .filter(|j| j.thread().id() != std::thread::current().id())
            .map(|j| j.join().unwrap() as usize)
            .sum::<usize>(),
        5
    );
    assert_eq!(e.status(&f.experiment).unwrap().candidates.len(), 5);
}
#[test]
fn evidence_from_another_candidate_cannot_complete_a_running_candidate() {
    let f = fixture();
    let e = engine(&f);
    let other = ResourceId::generate();
    start(&e, &f, other.clone(), 0, 101).unwrap();
    assert!(
        e.finish_candidate(
            &request(),
            &f.experiment,
            &other,
            &f.round(),
            None,
            &ReviewEvidence::default(),
            102
        )
        .is_err()
    );
    assert_eq!(
        e.status(&f.experiment).unwrap().candidates[&other].state,
        ExecutionState::Running
    );
}

#[test]
fn deleted_workspaces_stay_reserved_until_cleanup_succeeds() {
    let old = fixture();
    let next = fixture();
    let e = engine(&old);
    start(&e, &old, old.candidate.clone(), 0, 101).unwrap();
    e.delete(&request(), &old.experiment, 102).unwrap();
    e.create(&request(), next.experiment.clone(), next.cfg.clone(), 103)
        .unwrap();
    for failure in [false, false] {
        e.cleanup_result(&old.experiment, failure).unwrap();
        for overlap in [
            workspace(0),
            Workspace {
                branch: workspace(0).branch,
                ..workspace(1)
            },
            Workspace {
                build_directory: workspace(0).build_directory,
                ..workspace(2)
            },
        ] {
            assert!(
                e.start_candidate(
                    &request(),
                    &next.experiment,
                    ResourceId::generate(),
                    sha('d'),
                    overlap,
                    &next.base,
                    &next.baseline,
                    &BTreeSet::from([name("src/hot.rs")]),
                    104
                )
                .is_err()
            );
        }
    }
    e.cleanup_result(&old.experiment, true).unwrap();
    start(&e, &next, next.candidate.clone(), 0, 105).unwrap();
}

#[test]
fn fractional_configuration_survives_durable_json_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config();
    cfg.conditions.cases.values_mut().next().unwrap().frequency = Some(0.9176884786531495);
    let frozen = cfg.freeze().unwrap();
    let id = ResourceId::generate();
    let e = Engine::new(FileStore::open(dir.path()).unwrap());
    e.create(&request(), id.clone(), frozen.clone(), 100)
        .unwrap();
    drop(e);
    let reopened = Engine::new(FileStore::open(dir.path()).unwrap());
    assert_eq!(
        reopened.status(&id).unwrap().config.fingerprint(),
        frozen.fingerprint()
    );
    reopened.resume(&request(), &id, &frozen, 101).unwrap();
}
