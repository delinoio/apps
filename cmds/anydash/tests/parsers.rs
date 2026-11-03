// SPDX-License-Identifier: Apache-2.0
//! Session parsers, cursors and 15-minute bucket aggregation.
mod support;
use anydash::{
    parse::{self, CodexState, Event, UNKNOWN_MODEL},
    pb::{LocalTool, ParserState, SourcePathOrigin},
    scan,
    store::{Store, Tokens},
    time,
};
use serde_json::json;
use std::{io::Write as _, path::Path};
fn now() -> i64 {
    time::parse("2026-10-31T12:00:00Z").unwrap()
}
fn store(home: &Path) -> Store {
    Store::open(&home.join("anydash/data/collector.db")).unwrap()
}
fn codex_token(
    ts: &str,
    input: u64,
    cached: u64,
    output: u64,
    rate_limits: Option<&serde_json::Value>,
) -> String {
    json!({"timestamp": ts, "type": "event_msg", "payload": {"type": "token_count",
        "info": {"total_token_usage": {"input_tokens": input, "cached_input_tokens": cached, "output_tokens": output, "reasoning_output_tokens": 3, "total_tokens": input + output},
                 "last_token_usage": {"input_tokens": 7, "cached_input_tokens": 2, "output_tokens": 1}},
        "rate_limits": rate_limits}})
    .to_string()
}
fn codex_session(lines: &[String]) -> String {
    lines.iter().fold(String::new(), |mut s, l| {
        s.push_str(l);
        s.push('\n');
        s
    })
}
fn meta(ts: &str, id: &str) -> String {
    json!({"timestamp": ts, "type": "session_meta", "payload": {"id": id, "timestamp": ts, "cwd": "/private/never-uploaded", "originator": "codex_cli_rs", "cli_version": "1"}}).to_string()
}
fn turn(ts: &str, model: &str) -> String {
    json!({"timestamp": ts, "type": "turn_context", "payload": {"model": model, "cwd": "/x"}})
        .to_string()
}
#[test]
fn codex_cumulative_deltas_and_cached_input() {
    let mut state = CodexState::default();
    let mut events = vec![];
    let cutoff = now() - time::RETENTION_SECONDS;
    for line in [
        meta("2026-10-31T10:01:00Z", "s1"),
        turn("2026-10-31T10:01:01Z", "gpt-5.5"),
        codex_token("2026-10-31T10:02:00Z", 100, 40, 10, None),
        codex_token("2026-10-31T10:20:00Z", 250, 100, 30, None),
        // A decreasing total starts a new baseline with last_token_usage as the delta.
        codex_token("2026-10-31T10:21:00Z", 50, 10, 5, None),
    ] {
        parse::codex_line(&line, &mut state, "file", cutoff, &mut events).unwrap();
    }
    let tokens: Vec<Tokens> = events
        .iter()
        .filter_map(|e| match e {
            Event::Tokens { tokens, model, .. } => {
                assert_eq!(model, "gpt-5.5");
                Some(*tokens)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        tokens,
        vec![
            Tokens {
                input: 60,
                cache_read: 40,
                cache_write: 0,
                output: 10
            },
            Tokens {
                input: 90,
                cache_read: 60,
                cache_write: 0,
                output: 20
            },
            Tokens {
                input: 5,
                cache_read: 2,
                cache_write: 0,
                output: 1
            },
        ]
    );
    let sessions: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::Session { .. }))
        .collect();
    // The session is recorded at its start and again once its model becomes known.
    assert!(
        matches!(sessions[0], Event::Session { model, at, .. } if model == UNKNOWN_MODEL && *at == time::parse("2026-10-31T10:01:00Z").unwrap())
    );
    assert!(matches!(sessions[1], Event::Session { model, .. } if model == "gpt-5.5"));
}
#[test]
fn codex_rate_limits_extraction_and_unknown_plan() {
    let mut state = CodexState::default();
    let mut events = vec![];
    let limits = json!({"limit_id": "codex", "primary": {"used_percent": 42.5, "window_minutes": 300, "resets_at": 1_793_000_000}, "secondary": {"used_percent": 10.0, "window_minutes": 10080, "resets_at": 1_793_500_000}, "credits": {"has_credits": true, "unlimited": false, "balance": "4.20"}, "plan_type": "galactic_unlimited"});
    parse::codex_line(
        &codex_token("2026-10-31T11:00:00Z", 1, 0, 1, Some(&limits)),
        &mut state,
        "f",
        0,
        &mut events,
    )
    .unwrap();
    let observed = events.iter().find_map(|e| match e {
        Event::RateLimits { at, value } => Some((*at, value.clone())),
        _ => None,
    });
    assert_eq!(
        observed,
        Some((time::parse("2026-10-31T11:00:00Z").unwrap(), limits.clone()))
    );
    let snapshot = anydash::providers::codex::map_passive(&limits, 100);
    assert_eq!(
        snapshot.source,
        anydash::pb::ObservationSource::PassiveSession as i32
    );
    let plan = snapshot
        .metrics
        .iter()
        .find(|m| m.kind == anydash::pb::MetricKind::SubscriptionPlan as i32)
        .unwrap();
    assert_eq!(
        plan.value.as_ref().unwrap().value,
        Some(anydash::pb::metric_value::Value::Text(
            "galactic_unlimited".into()
        ))
    );
    let duration = snapshot
        .metrics
        .iter()
        .find(|m| {
            m.kind == anydash::pb::MetricKind::SubscriptionWindowDuration as i32
                && m.dimension == "weekly"
        })
        .unwrap();
    assert_eq!(
        duration.value.as_ref().unwrap().value,
        Some(anydash::providers::seconds(10080 * 60))
    );
}
fn sync_dir(home: &Path, extra: &[(&str, &str)]) -> (Store, scan::SyncReport) {
    let settings = support::settings(home, extra);
    let store = store(home);
    let report = scan::sync(&settings, &store, now()).unwrap();
    (store, report)
}
#[test]
fn codex_files_jsonl_zst_archived_and_session_start_bucket() {
    let home = tempfile::tempdir().unwrap();
    let codex = home.path().join(".codex");
    support::write(
        &codex.join("sessions/2026/10/31/rollout-a.jsonl"),
        &codex_session(&[
            meta("2026-10-31T10:14:59Z", "a"),
            turn("2026-10-31T10:14:59Z", "gpt-5.5"),
            codex_token("2026-10-31T10:16:00Z", 100, 0, 10, None),
        ]),
    );
    let compressed = zstd::encode_all(
        codex_session(&[
            meta("2026-10-31T10:30:00Z", "b"),
            turn("2026-10-31T10:30:00Z", "gpt-5.5"),
            codex_token("2026-10-31T10:31:00Z", 20, 5, 2, None),
        ])
        .as_bytes(),
        0,
    )
    .unwrap();
    std::fs::create_dir_all(codex.join("archived_sessions")).unwrap();
    std::fs::write(
        codex.join("archived_sessions/rollout-b.jsonl.zst"),
        compressed,
    )
    .unwrap();
    let (store, report) = sync_dir(home.path(), &[]);
    assert_eq!(report.files_read, 2);
    let buckets = store.aggregate().unwrap();
    let at = |s: &str| time::bucket_start(time::parse(s).unwrap());
    // The session starts at 10:14:59 and counts in the 10:00 bucket; its tokens at 10:16 count in 10:15.
    let start = &buckets[&(
        LocalTool::Codex as i32,
        "gpt-5.5".into(),
        at("2026-10-31T10:00:00Z"),
    )];
    assert_eq!((start.sessions, start.tokens.total()), (1, 0));
    let tokens = &buckets[&(
        LocalTool::Codex as i32,
        "gpt-5.5".into(),
        at("2026-10-31T10:15:00Z"),
    )];
    assert_eq!(
        (tokens.sessions, tokens.tokens.input, tokens.tokens.output),
        (0, 100, 10)
    );
    let archived = &buckets[&(
        LocalTool::Codex as i32,
        "gpt-5.5".into(),
        at("2026-10-31T10:30:00Z"),
    )];
    assert_eq!(
        (
            archived.sessions,
            archived.tokens.input,
            archived.tokens.cache_read
        ),
        (1, 15, 5)
    );
    // Original session files are never modified.
    assert!(codex.join("archived_sessions/rollout-b.jsonl.zst").exists());
}
#[test]
fn moved_or_compressed_codex_rollouts_count_once() {
    let home = tempfile::tempdir().unwrap();
    let codex = home.path().join(".codex");
    let content = codex_session(&[
        meta("2026-10-31T10:00:00Z", "moved"),
        turn("2026-10-31T10:00:00Z", "gpt-5.5"),
        codex_token("2026-10-31T10:01:00Z", 100, 40, 10, None),
        codex_token("2026-10-31T10:20:00Z", 250, 100, 30, None),
    ]);
    let active = codex.join("sessions/2026/10/31/rollout-moved.jsonl");
    support::write(&active, &content);
    let settings = support::settings(home.path(), &[]);
    let store = store(home.path());
    let totals = |s: &Store| {
        s.aggregate()
            .unwrap()
            .values()
            .fold((0, 0), |(sessions, tokens), v| {
                (sessions + v.sessions, tokens + v.tokens.total())
            })
    };
    scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(totals(&store), (1, 280));
    let versions = |s: &Store| {
        s.pending_buckets(100)
            .unwrap()
            .into_iter()
            .map(|b| (b.bucket, b.version))
            .collect::<Vec<_>>()
    };
    let before = versions(&store);
    // Archiving moves the rollout; the old path's contributions remain but count once.
    let archived = codex.join("archived_sessions/rollout-moved.jsonl");
    std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
    std::fs::rename(&active, &archived).unwrap();
    let report = scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(report.files_read, 1);
    assert_eq!(totals(&store), (1, 280));
    assert_eq!(report.changed_buckets, 0);
    assert_eq!(versions(&store), before, "no bucket correction is uploaded");
    // A compressed copy of the same session also counts once.
    std::fs::write(
        codex.join("archived_sessions/rollout-moved.jsonl.zst"),
        zstd::encode_all(content.as_bytes(), 0).unwrap(),
    )
    .unwrap();
    scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(totals(&store), (1, 280));
    // A copy that keeps growing counts with its larger total, without double counting.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&archived)
        .unwrap();
    writeln!(
        f,
        "{}",
        codex_token("2026-10-31T10:40:00Z", 300, 100, 40, None)
    )
    .unwrap();
    scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(totals(&store), (1, 340));
    // A different session in another file still counts separately.
    support::write(
        &codex.join("sessions/2026/10/31/rollout-other.jsonl"),
        &codex_session(&[
            meta("2026-10-31T11:00:00Z", "other"),
            codex_token("2026-10-31T11:01:00Z", 10, 0, 1, None),
        ]),
    );
    scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(totals(&store), (2, 351));
}
#[test]
fn growth_resumes_from_offset_and_incomplete_lines_wait() {
    let home = tempfile::tempdir().unwrap();
    let file = home.path().join(".codex/sessions/2026/10/31/r.jsonl");
    support::write(
        &file,
        &codex_session(&[
            meta("2026-10-31T10:00:00Z", "g"),
            codex_token("2026-10-31T10:01:00Z", 10, 0, 1, None),
        ]),
    );
    let settings = support::settings(home.path(), &[]);
    let store = store(home.path());
    scan::sync(&settings, &store, now()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    let next = codex_token("2026-10-31T10:02:00Z", 30, 0, 3, None);
    // Half a line is not parsed and not counted as unreadable.
    write!(f, "{}", &next[..20]).unwrap();
    f.flush().unwrap();
    let report = scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(report.parsers[0].skipped_record_count, 0);
    let total = |s: &Store| {
        s.aggregate()
            .unwrap()
            .values()
            .map(|v| v.tokens.total())
            .sum::<u64>()
    };
    assert_eq!(total(&store), 11);
    writeln!(f, "{}", &next[20..]).unwrap();
    scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(total(&store), 33);
    // Truncation re-reads the file and replaces its contributions.
    support::write(
        &file,
        &codex_session(&[
            meta("2026-10-31T10:00:00Z", "g"),
            codex_token("2026-10-31T10:01:00Z", 5, 0, 1, None),
        ]),
    );
    scan::sync(&settings, &store, now()).unwrap();
    assert_eq!(total(&store), 6);
}
fn claude(
    ts: &str,
    session: &str,
    id: &str,
    request: Option<&str>,
    input: u64,
    output: u64,
) -> String {
    let mut v = json!({"type": "assistant", "timestamp": ts, "sessionId": session, "message": {"id": id, "model": "claude-opus-5-5", "role": "assistant", "content": [{"type": "text", "text": "never stored"}],
        "usage": {"input_tokens": input, "output_tokens": output, "cache_read_input_tokens": 1, "cache_creation_input_tokens": 2}}});
    if let Some(r) = request {
        v["requestId"] = r.into();
    }
    v.to_string()
}
#[test]
fn claude_deduplicates_by_message_and_request_keeping_largest_total() {
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join(".claude/projects");
    support::write(
        &projects.join("p1/a.jsonl"),
        &format!(
            "{}\n{}\n{}\n",
            json!({"type": "user", "timestamp": "2026-10-31T09:59:00Z", "sessionId": "s1", "message": {"role": "user", "content": "prompt"}}),
            claude("2026-10-31T10:00:00Z", "s1", "m1", Some("r1"), 10, 5),
            // A streamed duplicate with a larger total wins and selects the bucket.
            claude("2026-10-31T10:16:00Z", "s1", "m1", Some("r1"), 10, 50),
        ),
    );
    support::write(
        &projects.join("p2/b.jsonl"),
        &format!(
            "{}\n{}\n{}\n",
            // The same entry copied into another file of the device is deduplicated.
            claude("2026-10-31T10:00:00Z", "s1", "m1", Some("r1"), 10, 5),
            // Without requestId the key is message.id with an empty request ID.
            claude("2026-10-31T10:40:00Z", "s2", "m2", None, 7, 3),
            claude("2026-10-31T10:41:00Z", "s2", "m2", None, 7, 1),
        ),
    );
    let (store, report) = sync_dir(home.path(), &[]);
    let claude_status = report
        .parsers
        .iter()
        .find(|p| p.tool == LocalTool::ClaudeCode as i32)
        .unwrap();
    assert_eq!(claude_status.state, ParserState::Ok as i32);
    let buckets = store.aggregate().unwrap();
    let key = |s: &str| {
        (
            LocalTool::ClaudeCode as i32,
            "claude-opus-5-5".to_owned(),
            time::bucket_start(time::parse(s).unwrap()),
        )
    };
    assert!(
        !buckets.contains_key(&key("2026-10-31T10:00:00Z"))
            || buckets[&key("2026-10-31T10:00:00Z")].tokens.total() == 0
    );
    assert_eq!(buckets[&key("2026-10-31T10:15:00Z")].tokens.output, 50);
    assert_eq!(
        buckets[&key("2026-10-31T10:30:00Z")].tokens,
        Tokens {
            input: 7,
            output: 3,
            cache_read: 1,
            cache_write: 2
        }
    );
    // Sessions count once, in the bucket of their earliest entry (s1 at 09:59 with a later known model).
    let sessions: u64 = buckets.values().map(|v| v.sessions).sum();
    assert_eq!(sessions, 2);
    let s1 = buckets
        .iter()
        .find(|(k, v)| {
            k.2 == time::bucket_start(time::parse("2026-10-31T09:45:00Z").unwrap())
                && v.sessions == 1
        })
        .unwrap();
    assert_eq!(s1.0.1, "claude-opus-5-5");
}
#[test]
fn unreadable_records_produce_partial_aggregation() {
    let home = tempfile::tempdir().unwrap();
    support::write(
        &home.path().join(".claude/projects/p/a.jsonl"),
        &format!(
            "{}\nnot json\n{}\n{}\n",
            claude("2026-10-31T10:00:00Z", "s", "m1", Some("r"), 1, 1),
            json!({"type": "assistant", "timestamp": "2026-10-31T10:01:00Z", "sessionId": "s", "message": {"id": "m2", "usage": {"input_tokens": "many"}}}),
            claude("2026-10-31T10:02:00Z", "s", "m3", Some("r"), 2, 2),
        ),
    );
    let (store, report) = sync_dir(home.path(), &[]);
    let status = report
        .parsers
        .iter()
        .find(|p| p.tool == LocalTool::ClaudeCode as i32)
        .unwrap();
    assert_eq!(
        (status.state, status.skipped_record_count),
        (ParserState::Partial as i32, 2)
    );
    let total: u64 = store
        .aggregate()
        .unwrap()
        .values()
        .map(|v| v.tokens.input + v.tokens.output)
        .sum();
    assert_eq!(total, 6);
}
#[test]
fn utc_buckets_for_offset_and_dst_timestamps() {
    // +05:30 and +05:45 offsets and both sides of a DST change land in 15-minute UTC buckets.
    for (local, utc_bucket) in [
        ("2026-10-31T16:44:59+05:30", "2026-10-31T11:00:00Z"),
        ("2026-10-31T16:59:59+05:45", "2026-10-31T11:00:00Z"),
        ("2026-03-08T01:59:59-08:00", "2026-03-08T09:45:00Z"),
        ("2026-03-08T03:00:00-07:00", "2026-03-08T10:00:00Z"),
        ("2026-11-01T01:30:00-07:00", "2026-11-01T08:30:00Z"),
        ("2026-11-01T01:30:00-08:00", "2026-11-01T09:30:00Z"),
    ] {
        assert_eq!(
            time::bucket_start(time::parse(local).unwrap()),
            time::parse(utc_bucket).unwrap(),
            "{local}"
        );
    }
    assert_eq!(time::bucket_start(-1), -900);
    // Versions are microsecond times forced above the previous version.
    assert_eq!(time::next_version(0, 5), 5);
    assert_eq!(time::next_version(10, 5), 11);
}
#[test]
fn bucket_versions_increase_and_vanished_sources_become_zero() {
    let home = tempfile::tempdir().unwrap();
    let store = store(home.path());
    let cutoff = 0;
    store
        .add_contribution(
            "f",
            1,
            "m",
            900,
            &Tokens {
                input: 5,
                ..Tokens::default()
            },
        )
        .unwrap();
    assert_eq!(store.refresh_buckets(100, cutoff).unwrap(), 1);
    assert_eq!(store.refresh_buckets(200, cutoff).unwrap(), 0);
    let first = store.pending_buckets(10).unwrap();
    assert_eq!(first[0].version, 100);
    store.mark_uploaded(&first[0]).unwrap();
    assert_eq!(store.pending_buckets(10).unwrap().len(), 0);
    store.reset_file("f").unwrap();
    assert_eq!(store.refresh_buckets(50, cutoff).unwrap(), 1);
    let corrected = store.pending_buckets(10).unwrap();
    assert_eq!(
        (corrected[0].version, corrected[0].value.tokens.input),
        (101, 0)
    );
}
#[test]
fn backfill_reads_only_the_last_90_days() {
    let home = tempfile::tempdir().unwrap();
    support::write(
        &home.path().join(".claude/projects/p/old.jsonl"),
        &format!(
            "{}\n{}\n",
            claude("2026-07-01T10:00:00Z", "old", "m", None, 9, 9),
            claude("2026-10-30T10:00:00Z", "new", "n", None, 1, 1)
        ),
    );
    let (store, _) = sync_dir(home.path(), &[]);
    let buckets = store.aggregate().unwrap();
    assert!(
        buckets
            .keys()
            .all(|k| k.2 >= now() - time::RETENTION_SECONDS)
    );
    assert_eq!(buckets.values().map(|v| v.sessions).sum::<u64>(), 1);
}
#[test]
fn path_detection_environment_user_added_and_disabled() {
    let home = tempfile::tempdir().unwrap();
    let a = home.path().join("cfg-a");
    let b = home.path().join("cfg-b");
    let settings = support::settings(
        home.path(),
        &[(
            "CLAUDE_CONFIG_DIR",
            &format!("{}, {}", a.display(), b.display()),
        )],
    );
    let detected = scan::detected(&settings);
    let claude_dirs: Vec<_> = detected
        .iter()
        .filter(|d| d.0 == LocalTool::ClaudeCode)
        .collect();
    assert_eq!(claude_dirs.len(), 2);
    assert!(
        claude_dirs
            .iter()
            .all(|d| d.2 == SourcePathOrigin::Environment)
    );
    // Without CLAUDE_CONFIG_DIR, ~/.claude and $XDG_CONFIG_HOME/claude are detected.
    let xdg = home.path().join("xdg");
    let defaults = scan::detected(&support::settings(
        home.path(),
        &[("XDG_CONFIG_HOME", &xdg.to_string_lossy())],
    ));
    assert!(
        defaults.iter().any(
            |d| d.1 == home.path().join(".claude/projects") && d.2 == SourcePathOrigin::Default
        )
    );
    assert!(
        defaults
            .iter()
            .any(|d| d.1 == xdg.join("claude/projects") && d.2 == SourcePathOrigin::Environment)
    );
    let store = store(home.path());
    support::write(
        &a.join("projects/p/x.jsonl"),
        &format!("{}\n", claude("2026-10-31T10:00:00Z", "s", "m", None, 1, 1)),
    );
    let custom = home.path().join("custom");
    support::write(
        &custom.join("x.jsonl"),
        &format!("{}\n", claude("2026-10-31T10:00:00Z", "t", "n", None, 2, 2)),
    );
    let added = scan::add(
        &settings,
        &store,
        LocalTool::ClaudeCode,
        &custom.to_string_lossy(),
    )
    .unwrap();
    assert_eq!(
        (added.origin, added.enabled, added.exists),
        (SourcePathOrigin::UserAdded as i32, true, true)
    );
    scan::set_enabled(
        &settings,
        &store,
        LocalTool::ClaudeCode,
        &a.join("projects").to_string_lossy(),
        false,
    )
    .unwrap();
    scan::sync(&settings, &store, now()).unwrap();
    let sessions: u64 = store
        .aggregate()
        .unwrap()
        .values()
        .map(|v| v.sessions)
        .sum();
    assert_eq!(
        sessions, 1,
        "the disabled path is skipped and the user-added path is scanned"
    );
    assert!(scan::add(&settings, &store, LocalTool::Codex, "relative/path").is_err());
    let listed = scan::list(&settings, &store).unwrap();
    assert!(
        listed
            .iter()
            .any(|p| !p.enabled && p.path == a.join("projects").to_string_lossy())
    );
    scan::set_enabled(
        &settings,
        &store,
        LocalTool::ClaudeCode,
        &a.join("projects").to_string_lossy(),
        true,
    )
    .unwrap();
    assert!(
        scan::list(&settings, &store)
            .unwrap()
            .iter()
            .all(|p| p.enabled)
    );
}
