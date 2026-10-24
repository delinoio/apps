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
    for mode in ["pass", "regression", "none"] {
        let (temp, mut cfg) = setup(mode);
        let repo = temp.path().join("repo");
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
                if mode == "pass" { "pass" } else { "regression" }
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
