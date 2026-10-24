// SPDX-License-Identifier: Apache-2.0
use perfine_core::{
    policy::{IrPolicy, Policy},
    types::{ResourceId, digest},
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@localhost",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}
fn command(executable: &str, args: &[&str]) -> Value {
    json!({"executable":executable,"arguments":args})
}
fn setup(mode: &str) -> (tempfile::TempDir, Value) {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/perfine-core/tests/fixtures/c");
    for name in ["main.c", "kernel.h", "correctness.c"] {
        fs::copy(fixtures.join(name), repo.join(name)).unwrap();
    }
    fs::copy(fixtures.join("baseline.c"), repo.join("kernel.c")).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let docker = json!({"executable":std::env::var("PERFINE_TEST_DOCKER").unwrap_or_else(|_| "/usr/local/bin/docker".into()),"image":std::env::var("PERFINE_TEST_IMAGE").unwrap(),"dependency_names":["cc-valgrind"],"user":"0:0","memory_bytes":1_073_741_824_u64,"cpus":1,"process_limit":128,"command_timeout_seconds":60});
    let auth = temp.path().join("fixture-auth.json");
    fs::write(&auth, "{}").unwrap();
    let mut agent_docker = docker.clone();
    agent_docker["image"] = json!(std::env::var("PERFINE_TEST_AGENT_IMAGE").unwrap());
    let cfg = json!({"schema_version":1,"experiment":{"schema_version":1,"repository":"fixture/repository","initial_base":sha,"incremental_base":sha,"workload_confirmation":"Fixture owner confirms pair-count workload","limits":{"candidate_count":1,"simultaneous_candidates":1,"start_duration_seconds":300,"candidate_duration_seconds":120},"policy":Policy::Ir(IrPolicy::default()),"conditions":{"harness":sha,"workflow":sha,"inputs":{"main.c":digest(&fs::read(repo.join("main.c")).unwrap())},"tools":{"pending":"pending"},"toolchain":"pending","build_options":[],"runtime_flags":[],"environment":{"os":"pending","architecture":"pending","runner":"pending","hardware":{"pending":"pending"},"variables":{"LC_ALL":"C"}},"metric":{"metric":"ir","unit":"instructions"},"cases":{"bulk":{"family":"pairs","frequency":1,"protected":false}},"correctness_checks":["project-tests"]}},"adapter":{"schema_version":1,"language":"c","discovery_files":["kernel.c"],"build":[command("cc",&["-O2","-g","main.c","kernel.c","-o","/build/workload"]),command("cc",&["-O2","-g","correctness.c","kernel.c","-o","/build/correctness"])],"correctness":{"project-tests":command("/build/correctness",&[])},"workloads":{"bulk":{"command":command("valgrind",&["--tool=callgrind","--callgrind-out-file=/build/callgrind.out","/build/workload","1024"]),"profiles":["callgrind.out"],"format":"callgrind","output":{"kind":"callgrind"},"repetitions":1,"required_paths":["kernel.c"]}},"tool_probes":{"cc":command("cc",&["--version"]),"valgrind":command("valgrind",&["--version"])},"source_prefixes":{"/work/":""}},"docker":docker,"execution_root":temp.path().join("jobs"),"toolchain_probe":"cc","agent":{"provider":"claude","docker":agent_docker,"executable":"/usr/local/bin/mock-agent","auth_file":auth,"network":std::env::var("PERFINE_TEST_AGENT_NETWORK").unwrap(),"model":mode,"maximum_source_bytes":1_048_576,"maximum_edits":4},"action_commit":null});
    fs::write(
        repo.join("perfine.yaml"),
        serde_json::to_vec_pretty(&cfg).unwrap(),
    )
    .unwrap();
    (temp, cfg)
}
#[test]
#[ignore = "requires provisioned Linux x86_64 measurement image and fixture-only agent image/network"]
fn real_fixture_pass_regression_no_candidate_dirty_resume_and_fingerprint() {
    for mode in ["pass", "regression", "none", "inconclusive"] {
        let (temp, mut cfg) = setup(mode);
        let repo = temp.path().join("repo");
        if mode == "inconclusive" {
            cfg["experiment"]["policy"]["settings"]["near_threshold"] = json!(0.99);
            cfg["adapter"]["workloads"]["bulk"]["command"] = command(
                "/bin/sh",
                &[
                    "-c",
                    "n=1024; if grep -Fq 'counts[256]' kernel.c; then if test -e /build/repeated; then n=4096; fi; touch /build/repeated; fi; exec valgrind --tool=callgrind --callgrind-out-file=/build/callgrind.out /build/workload \"$n\"",
                ],
            );
            fs::write(repo.join("perfine.yaml"), serde_json::to_vec(&cfg).unwrap()).unwrap();
        }
        fs::write(repo.join("kernel.c"), "dirty user content\n").unwrap();
        let id = ResourceId::generate();
        let args = [
            "--request-id",
            "fixture-optimize",
            "optimize",
            "--experiment",
            id.as_str(),
        ];
        let output = run(&repo, &args);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let exp = &report["result"]["experiment"];
        assert_eq!(exp["state"], "completed");
        assert_eq!(
            fs::read_to_string(repo.join("kernel.c")).unwrap(),
            "dirty user content\n"
        );
        let candidates = exp["candidates"].as_object().unwrap();
        if mode == "none" {
            assert!(candidates.is_empty());
        } else {
            assert_eq!(candidates.len(), 1);
            assert_eq!(
                candidates.values().next().unwrap()["evaluation"]["verdict"],
                match mode {
                    "pass" => "pass",
                    "inconclusive" => "inconclusive",
                    _ => "regression",
                },
                "{report}"
            );
        }
        #[cfg(unix)]
        if mode == "pass" {
            let (candidate, record) = candidates.iter().next().unwrap();
            publication_reconciles_lost_response(
                &repo,
                id.as_str(),
                candidate,
                cfg["experiment"]["incremental_base"].as_str().unwrap(),
                record["head"].as_str().unwrap(),
            );
        }
        let duplicate = run(&repo, &args);
        assert!(duplicate.status.success());
        let resume = run(&repo, &["resume", "--experiment", id.as_str()]);
        assert!(
            resume.status.success(),
            "{}",
            String::from_utf8_lossy(&resume.stdout)
        );
        cfg["experiment"]["limits"]["candidate_count"] = json!(2);
        fs::write(repo.join("perfine.yaml"), serde_json::to_vec(&cfg).unwrap()).unwrap();
        let mismatch = run(&repo, &["resume", "--experiment", id.as_str()]);
        assert!(!mismatch.status.success());
        assert!(String::from_utf8_lossy(&mismatch.stdout).contains("fingerprint"));
    }
}

#[test]
#[ignore = "requires provisioned measurement image; exercises active container cancellation"]
fn immediate_cancel_retains_evidence_and_resume_does_not_restart() {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let (temp, mut config) = setup("none");
    let repo = temp.path().join("repo");
    config["adapter"]["build"]
        .as_array_mut()
        .unwrap()
        .insert(0, command("sleep", &["30"]));
    fs::write(
        repo.join("perfine.yaml"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let id = ResourceId::generate();
    let log = tempfile::tempfile().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(&repo)
        .args(["--json", "optimize", "--experiment", id.as_str()])
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !repo
        .join(".perfine")
        .join(id.as_str())
        .join("journal.json")
        .exists()
    {
        assert!(started.elapsed() < Duration::from_secs(90));
        assert!(child.try_wait().unwrap().is_none());
        thread::sleep(Duration::from_millis(100));
    }
    let cancelled = run(
        &repo,
        &["cancel", "--experiment", id.as_str(), "--immediate"],
    );
    assert!(cancelled.status.success());
    while child.try_wait().unwrap().is_none() {
        assert!(started.elapsed() < Duration::from_secs(105));
        thread::sleep(Duration::from_millis(100));
    }
    let resumed = run(&repo, &["resume", "--experiment", id.as_str()]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stdout)
    );
    let value: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(value["result"]["experiment"]["state"], "cancelled");
    assert!(repo.join(".perfine/artifacts").is_dir());
}

#[cfg(unix)]
fn publication_reconciles_lost_response(
    repo: &Path,
    experiment: &str,
    candidate: &str,
    base: &str,
    head: &str,
) {
    use std::os::unix::fs::PermissionsExt;
    let dir = repo.parent().unwrap().join("mock-bin");
    fs::create_dir(&dir).unwrap();
    let draft = json!({"schema_version":1,"target_repository":"fixture/upstream","base_ref":"main","head_repository":"fixture/fork","head_ref":"candidate","title":"perf: improve pair counting","body":"Exact measured candidate; fixture-only publication."});
    let path = repo.parent().unwrap().join("draft.json");
    fs::write(&path, serde_json::to_vec(&draft).unwrap()).unwrap();
    let path_text = path.to_str().unwrap();
    let args = [
        "publish",
        "--experiment",
        experiment,
        "--candidate",
        candidate,
        "--draft",
        path_text,
    ];
    let preview = run(repo, &args);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stdout)
    );
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    let token = preview["result"]["approval_sha256"].as_str().unwrap();
    let pulls = json!([{"head":{"sha":head},"base":{"sha":base},"title":draft["title"],"body":draft["body"],"draft":true,"html_url":"https://github.com/fixture/upstream/pull/1"}]);
    let script = format!(
        "#!/bin/sh\ncase \"$1 $2\" in\n'api user') printf '%s' '{{\"login\":\"fixture\"}}';;\n'api repos/fixture/fork') printf '%s' '{{\"permissions\":{{\"push\":true}}}}';;\n'api repos/fixture/upstream/commits/main') printf '%s' '{{\"sha\":\"{base}\"}}';;\n'api repos/fixture/fork/commits/candidate') printf '%s' '{{\"sha\":\"{head}\"}}';;\n'api repos/fixture/upstream/pulls?'*) printf '%s' '{pulls}';;\n'api repos/fixture/upstream/pulls') printf 'attempt\\n' >> \"{}\"; exit 1;;\n*) exit 1;;\nesac\n",
        dir.join("attempts").display()
    );
    let executable = dir.join("gh");
    fs::write(&executable, script).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let original = std::env::var("PATH").unwrap();
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_perfine"))
            .env("PATH", format!("{}:{original}", dir.display()))
            .arg("--repository")
            .arg(repo)
            .arg("--json")
            .args(args)
            .args(["--approve", token])
            .output()
            .unwrap()
    };
    let uncertain = invoke();
    assert!(
        uncertain.status.success(),
        "{}",
        String::from_utf8_lossy(&uncertain.stdout)
    );
    let uncertain: Value = serde_json::from_slice(&uncertain.stdout).unwrap();
    assert_eq!(uncertain["result"]["state"], "uncertain");
    let recovered = invoke();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stdout)
    );
    let recovered: Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(recovered["result"]["state"], "published");
    assert_eq!(
        fs::read_to_string(dir.join("attempts")).unwrap(),
        "attempt\n"
    );
}

#[test]
#[ignore = "requires provisioned measurement and mock-agent Docker images"]
fn interrupted_candidate_resumes_same_workspace_without_duplicate_candidate() {
    interrupted_candidate_resume(false);
}
#[test]
#[ignore = "requires provisioned measurement and mock-agent Docker images"]
fn interrupted_implementation_resumes_original_admission() {
    interrupted_candidate_resume(true);
}
fn interrupted_candidate_resume(before_admission: bool) {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    let (temp, mut config) = setup("pass");
    let repo = temp.path().join("repo");
    config["experiment"]["limits"]["candidate_duration_seconds"] = json!(240);
    config["adapter"]["build"].as_array_mut().unwrap().insert(
        0,
        command(
            "/bin/sh",
            &["-c", "if grep -Fq 'counts[256]' kernel.c; then sleep 5; fi"],
        ),
    );
    fs::write(
        repo.join("perfine.yaml"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let id = ResourceId::generate();
    let log_path = temp.path().join("child.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_perfine"))
        .arg("--repository")
        .arg(&repo)
        .args(["--json", "optimize", "--experiment", id.as_str()])
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .unwrap();
    let started = Instant::now();
    let candidate = loop {
        assert!(started.elapsed() < Duration::from_secs(120));
        assert!(
            child.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&log_path).unwrap()
        );
        if before_admission {
            let path = repo.join(".perfine").join(id.as_str()).join("journal.json");
            if let Ok(bytes) = fs::read(path) {
                let journal: Value = serde_json::from_slice(&bytes).unwrap();
                if let Some(attempt) = journal["attempts"]
                    .as_object()
                    .and_then(|a| a.values().next())
                {
                    break (
                        attempt["candidate"].as_str().unwrap().to_owned(),
                        json!({"workspace":attempt["workspace"],"head":config["experiment"]["incremental_base"]}),
                    );
                }
            }
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        let status = run(&repo, &["status", "--experiment", id.as_str()]);
        if let Ok(v) = serde_json::from_slice::<Value>(&status.stdout)
            && let Some(candidates) = v["result"]["experiment"]["candidates"].as_object()
            && let Some((id, c)) = candidates.iter().next()
            && c["state"] == "running"
        {
            break (id.clone(), c.clone());
        }
        thread::sleep(Duration::from_millis(50));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    // The prior container finishes before reattachment; no overlapping measurements.
    thread::sleep(Duration::from_secs(7));
    let resumed = run(&repo, &["resume", "--experiment", id.as_str()]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stdout)
    );
    let v: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    let candidates = v["result"]["experiment"]["candidates"].as_object().unwrap();
    assert_eq!(candidates.len(), 1);
    let completed = &candidates[&candidate.0];
    assert_eq!(completed["state"], "completed", "{v}");
    assert_eq!(completed["workspace"], candidate.1["workspace"]);
    if before_admission {
        assert_ne!(completed["head"], candidate.1["head"]);
    } else {
        assert_eq!(completed["head"], candidate.1["head"]);
    }
    assert_eq!(completed["evaluation"]["verdict"], "pass");
}

#[path = "support/six_languages.rs"]
mod six_languages;
