// SPDX-License-Identifier: Apache-2.0
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
fn repo() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    root
}
fn run(path: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(path)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn init_preserves_files_and_requires_content_bound_overwrite() {
    let repo = repo();
    fs::write(repo.path().join("perfine.yaml"), "user data").unwrap();
    let output = run(repo.path(), &["init"]);
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 1);
    assert_eq!(response["result"]["confirmation_required"], true);
    assert_eq!(
        fs::read_to_string(repo.path().join("perfine.yaml")).unwrap(),
        "user data"
    );
    let hash = response["result"]["existing_sha256"].as_str().unwrap();
    let output = run(repo.path(), &["init", "--overwrite-sha256", hash]);
    assert!(output.status.success());
    assert!(
        fs::read_to_string(repo.path().join("perfine.yaml"))
            .unwrap()
            .contains("schema_version: 1")
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["check-ignore", ".perfine/evidence"])
            .status()
            .unwrap()
            .success()
    );
}
#[test]
fn unsupported_version_missing_limits_and_missing_action_sha_fail() {
    let repo = repo();
    fs::write(repo.path().join("perfine.yaml"), "schema_version: 2\n").unwrap();
    let output = run(repo.path(), &["doctor"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("schema major version"));
    let output = run(repo.path(), &["init", "--github"]);
    assert!(!output.status.success());
    assert!(!repo.path().join(".github/workflows/perfine.yml").exists());
    let output = run(repo.path(), &["optimize", "--experiment", "not-a-uuid"]);
    assert!(!output.status.success());
}
#[test]
fn mcp_lists_capabilities_and_uses_versioned_cli_dispatch() {
    let repo = repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(repo.path())
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for message in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"init","arguments":{"schema_version":1,"request_id":"mcp-init"}}}),
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"doctor","arguments":{"schema_version":2,"request_id":"bad-version"}}}),
        ] {
            writeln!(stdin, "{message}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 4);
    assert_eq!(
        responses.iter().find(|r| r["id"] == 2).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        11
    );
    assert_eq!(
        responses.iter().find(|r| r["id"] == 3).unwrap()["result"]["isError"],
        false
    );
    assert_eq!(
        responses.iter().find(|r| r["id"] == 4).unwrap()["result"]["isError"],
        true
    );
}

fn configuration() -> perfine_core::evidence::Config {
    serde_json::from_value(json!({
        "schema_version":1,"repository":"fixture/repository",
        "initial_base":"a".repeat(40),"incremental_base":"a".repeat(40),
        "workload_confirmation":"confirmed representative fixture",
        "limits":{"candidate_count":1,"simultaneous_candidates":1,"start_duration_seconds":300,"candidate_duration_seconds":60},
        "policy":perfine_core::policy::Policy::Ir(perfine_core::policy::IrPolicy::default()),
        "conditions":{"harness":"a".repeat(40),"workflow":"a".repeat(40),"inputs":{"input":"b".repeat(64)},"tools":{"tool":"1"},"toolchain":"1","build_options":[],"runtime_flags":[],"environment":{"os":"Linux","architecture":"x86_64","runner":"fixture","hardware":{"cpu":"fixture"},"variables":{}},"metric":{"metric":"ir","unit":"instructions"},"cases":{"case":{"family":"fixture","frequency":1,"protected":false}},"correctness_checks":["tests"]}
    })).unwrap()
}
#[test]
fn cancel_status_report_and_request_replay_share_durable_engine_state() {
    use perfine_core::{
        lifecycle::Engine,
        storage::FileStore,
        types::{RequestId, ResourceId},
    };
    for immediate in [false, true] {
        let repo = repo();
        let id = ResourceId::generate();
        let engine = Engine::new(FileStore::open(repo.path().join(".perfine/engine")).unwrap());
        engine
            .create(
                &RequestId::new("create").unwrap(),
                id.clone(),
                configuration().freeze().unwrap(),
                1,
            )
            .unwrap();
        let mut args = vec![
            "--request-id",
            "cancel-request",
            "cancel",
            "--experiment",
            id.as_str(),
        ];
        if immediate {
            args.push("--immediate");
        }
        let output = run(repo.path(), &args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let replay = run(repo.path(), &args);
        assert!(replay.status.success());
        let receipt: Value = serde_json::from_slice(&replay.stdout).unwrap();
        assert_eq!(receipt["result"]["replayed"], true);
        for command in ["status", "report"] {
            let output = run(repo.path(), &[command, "--experiment", id.as_str()]);
            assert!(output.status.success());
            let response: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                response["result"]["experiment"]["state"],
                if immediate { "cancelled" } else { "completed" }
            );
            assert_eq!(response["result"]["verdict"], "not_evaluated");
        }
    }
}

#[test]
fn mcp_cancellation_changes_the_same_state_seen_by_cli() {
    use perfine_core::{
        lifecycle::Engine,
        storage::FileStore,
        types::{RequestId, ResourceId},
    };
    let repo = repo();
    let id = ResourceId::generate();
    Engine::new(FileStore::open(repo.path().join(".perfine/engine")).unwrap())
        .create(
            &RequestId::new("create").unwrap(),
            id.clone(),
            configuration().freeze().unwrap(),
            1,
        )
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(repo.path())
        .args(["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(),"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"cancel","arguments":{"schema_version":1,"request_id":"mcp-cancel","experiment":id,"immediate":true}}})).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["isError"], false);
    let output = run(repo.path(), &["status", "--experiment", id.as_str()]);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["experiment"]["state"], "cancelled");
}

#[test]
fn evaluation_ratios_round_trip_without_changing_evidence_fingerprints() {
    use perfine_core::types::fingerprint;
    let ratios = (1..10_000)
        .map(|n| f64::from(n) / 52_432_431.0)
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&ratios).unwrap();
    let restored: Vec<f64> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        fingerprint(&ratios).unwrap(),
        fingerprint(&restored).unwrap()
    );
}

#[test]
fn graceful_cancel_during_active_attempt_is_durable_and_deduplicated() {
    use fs2::FileExt;
    use perfine_core::{
        lifecycle::Engine,
        storage::FileStore,
        types::{RequestId, ResourceId},
    };
    let repo = repo();
    let id = ResourceId::generate();
    let engine = Engine::new(FileStore::open(repo.path().join(".perfine/engine")).unwrap());
    engine
        .create(
            &RequestId::new("create").unwrap(),
            id.clone(),
            configuration().freeze().unwrap(),
            1,
        )
        .unwrap();
    let directory = repo.path().join(".perfine").join(id.as_str());
    fs::create_dir(&directory).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("runner.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let args = [
        "--request-id",
        "graceful-request",
        "cancel",
        "--experiment",
        id.as_str(),
    ];
    let output = run(repo.path(), &args);
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["active_work_allowed_to_finish"], true);
    drop(lock);
    let replay = run(repo.path(), &args);
    assert!(replay.status.success());
    let response: Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(response["result"]["replayed"], true);
    let mut conflicting = args.to_vec();
    conflicting.push("--immediate");
    assert!(!run(repo.path(), &conflicting).status.success());
    let output = run(repo.path(), &["status", "--experiment", id.as_str()]);
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        response["result"]["cancellation"]["graceful_requested"],
        true
    );
}
