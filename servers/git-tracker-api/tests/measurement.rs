// SPDX-License-Identifier: FSL-1.1-ALv2
use chrono::{Duration, Utc};
use delino_protocol::gittracker::v1::{ComparisonStatus::*, Direction::*, ExecutionState::*, *};
use git_tracker_api::{decimal, measurement::State};
fn column(unit: &str) -> Column {
    Column {
        id: "size".into(),
        unit: unit.into(),
        direction: DIRECTION_LOWER.into(),
        absolute_threshold: "0".into(),
        ..Default::default()
    }
}
fn dataset() -> Dataset {
    Dataset {
        id: "dataset".into(),
        name: "Dataset".into(),
        columns: vec![column("B")],
        ..Default::default()
    }
}
fn context(id: &str, commit: &str) -> Context {
    Context {
        repository_id: "repo".into(),
        execution_id: id.into(),
        commit: commit.into(),
        branch: "main".into(),
        ..Default::default()
    }
}
fn row(id: &str, value: &str) -> Row {
    Row {
        id: id.into(),
        values: [("size".into(), value.into())].into_iter().collect(),
        ..Default::default()
    }
}
#[test]
fn exact_decimals_thresholds_units_and_bounds() {
    assert_eq!(
        decimal::parse("1e3").unwrap(),
        decimal::parse("1000.000").unwrap()
    );
    for bad in [
        "NaN", "Infinity", "01", "1/2", "1e4097", "1e-4097", "+1", ".5", "1.",
    ] {
        assert!(decimal::parse(bad).is_err(), "{bad}")
    }
    let c = column("B");
    let v = decimal::compare(
        &c,
        Some(&c),
        "9007199254740993.00000000000001",
        "9007199254740993",
    );
    assert_eq!(v.status, COMPARISON_STATUS_REGRESSION);
    assert_eq!(v.delta, "0.00000000000001");
    assert_eq!(
        decimal::compare(&column("KiB"), Some(&c), "1", "1024").status,
        COMPARISON_STATUS_UNCHANGED
    );
    assert_eq!(
        decimal::compare(&column("ms"), Some(&c), "1", "1").status,
        COMPARISON_STATUS_INCOMPATIBLE
    );
    let mut c = column("B");
    c.absolute_threshold = "10".into();
    assert_eq!(
        decimal::compare(&c, Some(&c), "110", "100").status,
        COMPARISON_STATUS_CHANGED
    );
    assert_eq!(
        decimal::compare(&c, Some(&c), "110.0001", "100").status,
        COMPARISON_STATUS_REGRESSION
    );
    c.absolute_threshold.clear();
    c.percentage_threshold = "1".into();
    assert_eq!(
        decimal::compare(&c, Some(&c), "100", "0").status,
        COMPARISON_STATUS_CHANGED
    );
    c.direction = DIRECTION_NEUTRAL.into();
    assert_eq!(
        decimal::compare(&c, Some(&c), "1000", "1").status,
        COMPARISON_STATUS_CHANGED
    );
}
#[test]
fn atomic_upload_retry_lifecycle_and_registration_order() {
    let mut s = State::default();
    let at = Utc::now();
    let c = context("first", "a");
    let d = dataset();
    s.upload(&c, &d, &[row("one", "1")], at, vec![], String::new())
        .unwrap();
    assert!(
        s.upload(
            &c,
            &d,
            &[row("two", "2"), row("one", "3")],
            at,
            vec![],
            String::new()
        )
        .is_err()
    );
    assert_eq!(s.records["first"].execution.measurements[0].rows.len(), 1);
    s.upload(
        &c,
        &d,
        &[row("one", "1.00")],
        at + Duration::hours(5),
        vec![],
        String::new(),
    )
    .unwrap();
    assert_eq!(s.records["first"].last_data, at);
    let newer = context("second", "a");
    s.complete(&newer, at + Duration::minutes(1)).unwrap();
    s.complete(&c, at + Duration::minutes(2)).unwrap();
    assert_eq!(
        s.representative("a", None, false)
            .unwrap()
            .execution
            .context
            .execution_id,
        "second"
    );
    let completed = s.records["first"].execution.completed_at.clone();
    s.complete(&c, at + Duration::hours(9)).unwrap();
    assert_eq!(s.records["first"].execution.completed_at, completed);
    assert!(
        s.upload(&c, &d, &[row("one", "1")], at, vec![], String::new())
            .is_err()
    );
    let c = context("interrupted", "b");
    s.upload(&c, &d, &[row("one", "1")], at, vec![], String::new())
        .unwrap();
    assert_eq!(s.interrupt(at + Duration::hours(6)), 1);
    assert_eq!(
        s.records["interrupted"].execution.state,
        EXECUTION_STATE_INTERRUPTED
    );
    assert!(s.complete(&c, at + Duration::hours(6)).is_err());
}
#[test]
fn frozen_row_baselines_exclude_forks_and_survive_deletion() {
    let mut s = State::default();
    let at = Utc::now();
    let d = dataset();
    let base = context("base", "a");
    s.upload(
        &base,
        &d,
        &[row("one", "10"), row("two", "20")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    s.complete(&base, at).unwrap();
    let mut fork = context("fork", "a");
    fork.pull_request = 1;
    s.upload(&fork, &d, &[row("one", "10000")], at, vec![], String::new())
        .unwrap();
    s.complete(&fork, at).unwrap();
    let target = context("target", "b");
    s.upload(
        &target,
        &d,
        &[row("one", "11")],
        at + Duration::seconds(1),
        vec!["a".into()],
        String::new(),
    )
    .unwrap();
    let later = context("late", "a");
    s.upload(
        &later,
        &d,
        &[row("two", "999")],
        at + Duration::seconds(2),
        vec![],
        String::new(),
    )
    .unwrap();
    s.complete(&later, at + Duration::seconds(2)).unwrap();
    s.upload(
        &target,
        &d,
        &[row("two", "21")],
        at + Duration::seconds(3),
        vec!["a".into()],
        String::new(),
    )
    .unwrap();
    let comparisons = s.comparisons(&s.records["target"], None);
    assert!(
        comparisons
            .iter()
            .all(|v| v.baseline_execution_id == "base")
    );
    s.records.remove("base");
    assert!(
        s.comparisons(&s.records["target"], None)
            .iter()
            .all(|v| v.status == COMPARISON_STATUS_BASELINE_DELETED)
    );
    s.deleted.insert("target".into(), true);
    s.records.remove("target");
    assert!(
        s.upload(&target, &d, &[row("one", "11")], at, vec![], String::new())
            .is_err()
    );
    assert!(s.complete(&target, at).is_err());
}
#[test]
fn current_storage_rows_round_trip() {
    let raw = serde_json::json!({"next":1,"records":{"run":{"execution":{"context":{"repositoryId":"r","executionId":"run","commit":"a","branch":"main","pullRequest":"3"},"state":"EXECUTION_STATE_MEASURING","registeredAt":"2026-01-01T00:00:00Z","registrationOrder":"00000000000000000001","measurements":[{"dataset":{"id":"d","name":"D","columns":[{"id":"size","unit":"B","direction":"DIRECTION_LOWER"}]},"rows":[{"id":"r","values":{"CamelKey":"1"}}]}]},"last_data":"2026-01-01T00:00:00Z","cutoff":"2026-01-01T00:00:00Z","ancestors":[],"removed":{},"base":{"execution":"","commit":""},"push":false,"baselines":{}}}});
    let s: State = serde_json::from_value(raw).unwrap();
    let saved = serde_json::to_value(&s).unwrap();
    let e = &saved["records"]["run"]["execution"];
    assert_eq!(e["context"]["pullRequest"], "3");
    assert_eq!(e["state"], "EXECUTION_STATE_MEASURING");
    assert_eq!(
        e["measurements"][0]["dataset"]["columns"][0]["direction"],
        "DIRECTION_LOWER"
    );
    assert_eq!(e["measurements"][0]["rows"][0]["values"]["CamelKey"], "1");
    let restored: State = serde_json::from_value(saved).unwrap();
    assert_eq!(
        restored.records["run"].execution,
        s.records["run"].execution
    );
}
fn named(id: &str) -> Dataset {
    Dataset {
        id: id.into(),
        name: id.into(),
        columns: vec![column("B")],
        ..Default::default()
    }
}
#[test]
fn dataset_deletion_marks_only_executions_that_held_the_dataset() {
    let mut s = State::default();
    let at = Utc::now();
    // X holds dataset A only; Y holds A and D.
    let x = context("x", "bx");
    s.upload(
        &x,
        &named("a"),
        &[row("one", "10")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    s.complete(&x, at).unwrap();
    let y = context("y", "by");
    s.upload(
        &y,
        &named("a"),
        &[row("one", "10")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    s.upload(
        &y,
        &named("d"),
        &[row("one", "10")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    s.complete(&y, at).unwrap();

    // M is still measuring and holds A only.
    let m = context("m", "bm");
    s.upload(
        &m,
        &named("a"),
        &[row("one", "10")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();

    assert_eq!(s.delete_dataset("d").unwrap(), 1);
    assert!(s.records["x"].removed.is_empty());
    assert!(s.records["m"].removed.is_empty());
    assert_eq!(s.records["y"].removed.get("d"), Some(&true));
    // Rows are removed from the holder; the other dataset stays.
    let held: Vec<_> = s.records["y"]
        .execution
        .measurements
        .iter()
        .map(|m| m.dataset.id.as_str())
        .collect();
    assert_eq!(held, ["a"]);
    // No execution holds D any more, so a second deletion is not found and changes nothing.
    assert!(s.delete_dataset("d").is_err());
    assert!(s.delete_dataset("never").is_err());
    assert!(s.records["x"].removed.is_empty());

    // A delayed retry into the holder is still rejected.
    assert!(
        s.upload(
            &y,
            &named("d"),
            &[row("one", "10")],
            at,
            vec![],
            String::new()
        )
        .is_err()
    );
    // Pull requests pinned to X and Y upload D afterwards: X never held it, so its
    // comparison is new; Y held it, so its comparison reports the deleted baseline.
    for (id, pr, base, expected) in [
        ("p", 1, "bx", COMPARISON_STATUS_NEW),
        ("q", 2, "by", COMPARISON_STATUS_BASELINE_DELETED),
    ] {
        let mut c = context(id, "head");
        c.pull_request = pr;
        s.upload(
            &c,
            &named("d"),
            &[row("one", "11")],
            at,
            vec![],
            base.into(),
        )
        .unwrap();
        let comparisons = s.comparisons(&s.records[id], None);
        assert_eq!(comparisons.len(), 1);
        assert_eq!(comparisons[0].status, expected, "execution {id}");
    }
    // An execution that never held the dataset keeps accepting it.
    s.upload(
        &m,
        &named("d"),
        &[row("one", "10")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    assert!(s.records["m"].removed.is_empty());
}
#[test]
fn deletion_of_unknown_targets_is_not_found_and_leaves_state_unchanged() {
    let mut s = State::default();
    let at = Utc::now();
    let c = context("run", "a");
    s.upload(
        &c,
        &dataset(),
        &[row("one", "1")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    assert!(s.delete_execution("typo").is_err());
    assert!(s.deleted.is_empty());
    // The mistyped ID is still usable.
    let typo = context("typo", "a");
    s.upload(
        &typo,
        &dataset(),
        &[row("one", "1")],
        at,
        vec![],
        String::new(),
    )
    .unwrap();
    // A real deletion is applied once and then reported as already done.
    assert!(s.delete_execution("run").unwrap());
    assert!(!s.delete_execution("run").unwrap());
    assert!(
        s.upload(
            &c,
            &dataset(),
            &[row("one", "1")],
            at,
            vec![],
            String::new()
        )
        .is_err()
    );
    // Repository deletion reports how many executions it removed.
    assert_eq!(s.delete_repository(), 1);
    assert_eq!(s.delete_repository(), 0);
    assert_eq!(s.deleted.len(), 2);
}
