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
