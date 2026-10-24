// SPDX-License-Identifier: FSL-1.1-ALv2
//! Unknown-usage telemetry: every genuinely started Gateway attempt that ends without confirmed
//! totals is one `delitools_unknown_usage_total` increment and one `unknown_usage` warning;
//! attempts that never started, and attempts with confirmed totals, add none. The tests use the
//! loopback mocks only and need no PostgreSQL: no scenario here stores a billable event.

mod common;

use common::*;
use serde_json::json;
use std::{
    fmt::Write as _,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tracing::field::{Field, Visit};
use tracing_subscriber::{Layer, layer::SubscriberExt};

/// Text sent by every request; no log record may ever contain it.
const SECRET_TEXT: &str = "zz-confidential-input-zz";

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The capture is process-global, so scenarios run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fields(String);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

struct Capture;
impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        EVENTS.lock().unwrap().push(fields.0);
    }
}

/// Installs the capture once and returns the guard that serializes the calling scenario.
async fn scenario() -> tokio::sync::MutexGuard<'static, ()> {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(Capture))
            .expect("no other global subscriber in this test binary");
    });
    let guard = SERIAL.lock().await;
    EVENTS.lock().unwrap().clear();
    guard
}

/// The captured `unknown_usage` warnings, after checking that no record leaked request content.
fn warnings() -> Vec<String> {
    let events = EVENTS.lock().unwrap();
    for event in events.iter() {
        assert!(
            !event.contains(SECRET_TEXT),
            "a log record leaked input: {event}"
        );
        assert!(
            !event.contains("gw-secret") && !event.contains("svc-secret"),
            "a log record leaked a credential: {event}"
        );
    }
    events
        .iter()
        .filter(|event| event.contains("category=\"unknown_usage\""))
        .cloned()
        .collect()
}

fn body() -> serde_json::Value {
    translate_body(&[SECRET_TEXT])
}

fn unknown(h: &Harness) -> (u64, u64) {
    (h.unknown_usage("missing"), h.unknown_usage("invalid"))
}

async fn eventually(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..500 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Waits until the execution task ended and released its tenant permit, then lets a late
/// increment surface.
async fn settled(h: &Harness) {
    eventually("the tenant permit release", || {
        h.server.in_flight(TENANT) == 0
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn a_failed_attempt_without_retries_is_one_missing_observation() {
    let _serial = scenario().await;
    for reply in [Reply::Status(503), Reply::Status(400)] {
        EVENTS.lock().unwrap().clear();
        let h = Harness::start_with(None, |c| c.retry.max_retries = 0).await;
        h.gateway.push(reply);
        let wire = h.translate(USER_TOKEN, body()).await;
        assert_eq!(wire.reason(), "ERROR_REASON_PROVIDER_FAILED");
        assert_eq!(h.gateway.calls(), 1);
        assert_eq!(unknown(&h), (1, 0));
        let logged = warnings();
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(logged[0].contains("reason=\"missing\"") && logged[0].contains("attempt=1"));
    }
}

#[tokio::test]
async fn each_failed_retry_attempt_is_observed_and_exhaustion_is_unchanged() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    for _ in 0..3 {
        h.gateway.push(Reply::Status(502));
    }
    let wire = h.translate(USER_TOKEN, body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PROVIDER_FAILED");
    assert_eq!(wire.code(), "unavailable");
    assert_eq!(h.gateway.calls(), 3);
    assert_eq!(unknown(&h), (3, 0));
    let logged = warnings();
    assert_eq!(logged.len(), 3, "{logged:?}");
    for (index, line) in logged.iter().enumerate() {
        assert!(line.contains(&format!("attempt={}", index + 1)), "{line}");
    }
}

#[tokio::test]
async fn failures_before_a_confirmed_completion_are_observed_but_the_completion_is_not() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    h.gateway.push(Reply::Status(503));
    h.gateway.push(Reply::RetryAfter(429, 0));
    // Known zero totals are confirmed usage: nothing to bill and nothing unknown.
    h.gateway.push(Reply::ok(r#"["ok"]"#, 0, 0));
    let wire = h.translate(USER_TOKEN, body()).await;
    assert!(wire.ok(), "{}", wire.body);
    assert_eq!(h.gateway.calls(), 3);
    assert_eq!(unknown(&h), (2, 0));
    assert_eq!(warnings().len(), 2);
}

#[tokio::test]
async fn completions_with_missing_or_invalid_usage_keep_one_observation() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    h.gateway.push(Reply::without_usage(r#"["a"]"#));
    h.gateway.push(Reply::Ok {
        content: r#"["a"]"#.into(),
        usage: json!({"prompt_tokens": "many", "completion_tokens": 3}),
    });
    for _ in 0..2 {
        assert!(h.translate(USER_TOKEN, body()).await.ok());
    }
    assert_eq!(unknown(&h), (1, 1));
    let logged = warnings();
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert!(logged[0].contains("reason=\"missing\""));
    assert!(logged[1].contains("reason=\"invalid\""));
}

#[tokio::test]
async fn a_malformed_completion_is_observed_once_and_stays_invalid_output() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    h.gateway
        .push(Reply::without_usage("not a translation array"));
    let wire = h.translate(USER_TOKEN, body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_INVALID_OUTPUT");
    assert_eq!(h.gateway.calls(), 1);
    assert_eq!(unknown(&h), (1, 0));
    assert_eq!(warnings().len(), 1);
}

#[tokio::test]
async fn known_zero_totals_add_no_observation() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    h.gateway.push(Reply::ok(r#"["a"]"#, 0, 0));
    assert!(h.translate(USER_TOKEN, body()).await.ok());
    assert_eq!(unknown(&h), (0, 0));
    assert!(warnings().is_empty());
}

#[derive(Clone, Copy, Debug)]
enum StopKind {
    Cancel,
    Timeout,
}

const STOP_KINDS: [StopKind; 2] = [StopKind::Cancel, StopKind::Timeout];

fn stop_config(stop: StopKind, c: &mut delitools_api::config::Config) {
    if let StopKind::Timeout = stop {
        c.limits.execution_timeout = Duration::from_millis(600);
    }
}

/// Sends a Translate request and ends it with `stop` once `ready` holds, then waits until the
/// execution has finished.
async fn translate_then_stop(h: &Harness, stop: StopKind, ready: impl Fn() -> bool) {
    let request = h.call_future("Translate", USER_TOKEN, body());
    match stop {
        StopKind::Timeout => {
            let response = request.await.unwrap();
            assert_eq!(response.status().as_u16(), 504);
        }
        StopKind::Cancel => {
            let pending = tokio::spawn(request);
            eventually("the stop condition", ready).await;
            pending.abort();
            let _ = pending.await;
        }
    }
    settled(h).await;
}

#[tokio::test]
async fn an_in_flight_call_stopped_by_cancellation_or_deadline_is_one_observation() {
    let _serial = scenario().await;
    for stop in STOP_KINDS {
        EVENTS.lock().unwrap().clear();
        let h = Harness::start_with(None, |c| stop_config(stop, c)).await;
        h.gateway.push(Reply::Hang);
        h.gateway.push(Reply::without_usage(r#"["never"]"#));
        translate_then_stop(&h, stop, || h.gateway.calls() == 1).await;
        assert_eq!(h.gateway.calls(), 1, "{stop:?}: no retry");
        assert_eq!(h.gateway.dropped(), 1, "{stop:?}: the call is dropped");
        assert_eq!(unknown(&h), (1, 0), "{stop:?}");
        let logged = warnings();
        assert_eq!(logged.len(), 1, "{stop:?}: {logged:?}");
        assert!(logged[0].contains("reason=\"missing\"") && logged[0].contains("attempt=1"));
    }
}

#[tokio::test]
async fn a_stop_during_backoff_keeps_only_the_failed_attempt_observation() {
    let _serial = scenario().await;
    for stop in STOP_KINDS {
        EVENTS.lock().unwrap().clear();
        let h = Harness::start_with(None, |c| {
            stop_config(stop, c);
            // The minimum jittered delay (half of 10s) outlasts the stop window.
            c.retry.base_delay = Duration::from_secs(10);
            c.retry.max_delay = Duration::from_secs(10);
        })
        .await;
        h.gateway.push(Reply::Status(503));
        h.gateway.push(Reply::without_usage(r#"["never"]"#));
        translate_then_stop(&h, stop, || h.unknown_usage("missing") == 1).await;
        assert_eq!(h.gateway.calls(), 1, "{stop:?}: no second provider call");
        assert_eq!(unknown(&h), (1, 0), "{stop:?}");
        assert_eq!(warnings().len(), 1, "{stop:?}");
    }
}

#[tokio::test]
async fn a_stop_during_the_authorization_recheck_keeps_only_the_failed_attempt_observation() {
    let _serial = scenario().await;
    for stop in STOP_KINDS {
        EVENTS.lock().unwrap().clear();
        let h = Harness::start_with(None, |c| stop_config(stop, c)).await;
        let gate = AuthorizeGate::after(1);
        h.delcore.lock().unwrap().authorize_gate = Some(gate.clone());
        h.gateway.push(Reply::Status(503));
        h.gateway.push(Reply::without_usage(r#"["never"]"#));
        translate_then_stop(&h, stop, || {
            gate.entered.load(std::sync::atomic::Ordering::SeqCst) > 0
        })
        .await;
        gate.wait_entered().await;
        gate.open();
        assert_eq!(h.gateway.calls(), 1, "{stop:?}: no second provider call");
        assert_eq!(unknown(&h), (1, 0), "{stop:?}");
        assert_eq!(warnings().len(), 1, "{stop:?}");
    }
}

#[tokio::test]
async fn a_denied_recheck_after_a_failed_attempt_adds_no_second_observation() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    let delcore = h.delcore.clone();
    h.gateway.push(Reply::Hook(
        std::sync::Arc::new(move || delcore.lock().unwrap().authorize = deny_permission()),
        Box::new(Reply::Status(503)),
    ));
    h.gateway.push(Reply::without_usage(r#"["never"]"#));
    let wire = h.translate(USER_TOKEN, body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    assert_eq!(h.gateway.calls(), 1);
    assert_eq!(unknown(&h), (1, 0));
    assert_eq!(warnings().len(), 1);
}

#[tokio::test]
async fn requests_that_never_start_a_provider_attempt_add_no_observation() {
    let _serial = scenario().await;
    let h = Harness::start(None).await;
    // Admission failures: a rejected credential and a denied permission.
    let wire = h.translate("unknown-token", body()).await;
    assert!(!wire.ok());
    h.delcore.lock().unwrap().authorize = deny_permission();
    let wire = h.translate(USER_TOKEN, body()).await;
    assert_eq!(wire.reason(), "ERROR_REASON_PERMISSION_DENIED");
    assert_eq!(h.gateway.calls(), 0);
    assert_eq!(unknown(&h), (0, 0));
    assert!(warnings().is_empty());
}
