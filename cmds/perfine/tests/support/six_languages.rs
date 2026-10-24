// SPDX-License-Identifier: Apache-2.0
//! Actual CLI/adapter measurements; only the structured agent response is mocked.
use super::{command, git, run, setup};
use perfine_core::{
    adapters::{ArtifactStore, measurement::verify_local},
    evidence::{Manifest, validate_baseline},
    interfaces::Language,
    policy::{DeclaredPolicy, Objective, Policy, Uncertainty},
    types::{ResourceId, digest},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

#[expect(
    clippy::too_many_lines,
    reason = "Keep the six explicit language recipes together for fixture review"
)]
fn recipe(language: &str, cfg: &mut Value, repo: &Path) -> String {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/perfine-core/tests/fixtures")
        .join(language);
    // Replace the C bootstrap project before pinning a new fixture commit.
    for file in fs::read_dir(repo).unwrap().flatten() {
        if file.file_type().unwrap().is_file() {
            fs::remove_file(file.path()).unwrap();
        }
    }
    let ext = match language {
        "rust" => "rs",
        "cpp" => "cpp",
        "zig" => "zig",
        "go" => "go",
        "javascript" => "mjs",
        _ => "c",
    };
    let kernel = format!("kernel.{ext}");
    for file in fs::read_dir(&fixtures).unwrap().flatten() {
        let name = file.file_name();
        if !name.to_str().unwrap().starts_with("candidate.")
            && !name.to_str().unwrap().starts_with("baseline.")
        {
            fs::copy(file.path(), repo.join(name)).unwrap();
        }
    }
    fs::copy(fixtures.join(format!("baseline.{ext}")), repo.join(&kernel)).unwrap();
    git(repo, &["add", "-A"]);
    git(
        repo,
        &["commit", "--allow-empty", "-qm", "pin language fixture"],
    );
    let sha = git(repo, &["rev-parse", "HEAD"]);
    for key in ["initial_base", "incremental_base"] {
        cfg["experiment"][key] = json!(sha);
    }
    for key in ["harness", "workflow"] {
        cfg["experiment"]["conditions"][key] = json!(sha);
    }
    let mut inputs = serde_json::Map::new();
    for file in fs::read_dir(repo).unwrap().flatten() {
        if file.file_type().unwrap().is_file() && file.file_name() != kernel.as_str() {
            inputs.insert(
                file.file_name().to_str().unwrap().into(),
                json!(digest(&fs::read(file.path()).unwrap())),
            );
        }
    }
    cfg["experiment"]["conditions"]["inputs"] = json!(inputs);
    cfg["experiment"]["conditions"]["cases"]["protected"] =
        json!({"family":"pairs", "frequency":1, "protected":true});
    cfg["experiment"]["limits"]["start_duration_seconds"] = json!(1800);
    cfg["experiment"]["limits"]["candidate_duration_seconds"] = json!(900);
    cfg["docker"]["command_timeout_seconds"] = json!(300);
    cfg["docker"]["memory_bytes"] = json!(4_294_967_296_u64);
    cfg["adapter"]["language"] = json!(match language {
        "rust" => Language::Rust,
        "c" => Language::C,
        "cpp" => Language::Cpp,
        "zig" => Language::Zig,
        "go" => Language::Go,
        "javascript" => Language::JavaScript,
        _ => unreachable!(),
    });
    cfg["adapter"]["discovery_files"] = json!([kernel]);
    let mut build = vec![];
    let (tool, probe, correctness) = match language {
        "rust" => {
            for (source, output, test) in [
                ("main.rs", "/build/workload", false),
                ("correctness.rs", "/build/correctness", true),
            ] {
                let mut args = vec![
                    "--edition=2021",
                    "-C",
                    "opt-level=2",
                    "-C",
                    "debuginfo=2",
                    source,
                    "-o",
                    output,
                ];
                if test {
                    args.push("--test");
                }
                build.push(command("rustc", &args));
            }
            ("rustc", "--version", command("/build/correctness", &[]))
        }
        "c" | "cpp" => {
            let tool = if language == "c" { "cc" } else { "c++" };
            let standard = if language == "c" {
                "-std=c11"
            } else {
                "-std=c++17"
            };
            build.push(command(
                tool,
                &[
                    standard,
                    "-O2",
                    "-g",
                    "-c",
                    &kernel,
                    "-o",
                    "/build/kernel.o",
                ],
            ));
            for (source, output) in [
                (format!("main.{ext}"), "/build/workload"),
                (format!("correctness.{ext}"), "/build/correctness"),
            ] {
                build.push(command(
                    tool,
                    &[
                        standard,
                        "-O2",
                        "-g",
                        &source,
                        "/build/kernel.o",
                        "-o",
                        output,
                    ],
                ));
            }
            (tool, "--version", command("/build/correctness", &[]))
        }
        "zig" => {
            build.push(command(
                "zig",
                &[
                    "build-exe",
                    "main.zig",
                    "-O",
                    "ReleaseSafe",
                    "--cache-dir",
                    "/build/zig-cache",
                    "--global-cache-dir",
                    "/build/zig-global",
                    "-femit-bin=/build/workload",
                ],
            ));
            (
                "zig",
                "version",
                command(
                    "zig",
                    &[
                        "test",
                        "correctness.zig",
                        "-O",
                        "ReleaseSafe",
                        "--cache-dir",
                        "/build/zig-cache",
                        "--global-cache-dir",
                        "/build/zig-global",
                    ],
                ),
            )
        }
        "go" => (
            "go",
            "version",
            command(
                "env",
                &[
                    "GOCACHE=/build/go-cache",
                    "GOTMPDIR=/build/go-tmp",
                    "GOTOOLCHAIN=local",
                    "go",
                    "test",
                    "-count=1",
                    "-run=^Test",
                    "./...",
                ],
            ),
        ),
        "javascript" => (
            "node",
            "--version",
            command("node", &["--test", "correctness.test.mjs"]),
        ),
        _ => unreachable!(),
    };
    if language == "go" {
        // The sandbox mounts /tmp without execute permission. Go runs its test
        // binary from GOTMPDIR even with -o; keep it in this task's build mount.
        build.push(command("mkdir", &["-p", "/build/go-tmp"]));
    }
    cfg["adapter"]["build"] = json!(build);
    cfg["adapter"]["correctness"]["project-tests"] = correctness;
    cfg["adapter"]["tool_probes"] = json!({tool:command(tool, &[probe])});
    cfg["toolchain_probe"] = json!(tool);
    let timing = matches!(language, "go" | "javascript");
    if timing {
        cfg["experiment"]["conditions"]["metric"] = json!({"metric":"time", "unit":"ns/op"});
        // Fixed before any measurement. Wide fixture noise tolerance is disclosed;
        // it is not a production timing policy or a claim about native performance.
        cfg["experiment"]["policy"] = json!(Policy::Declared(DeclaredPolicy {
            objective: Objective::Minimize,
            minimum_improvement: 0.05,
            maximum_case_regression: 0.01,
            repetitions: 3,
            near_threshold: 0.001,
            uncertainty: Uncertainty {
                maximum_relative_range: 0.50,
                decision_margin: 0.01,
                repeat_tolerance: 0.25
            },
        }));
    } else {
        cfg["adapter"]["tool_probes"]["valgrind"] = command("valgrind", &["--version"]);
    }
    for (case, size) in [("bulk", "1024"), ("protected", "128")] {
        let workload = if language == "go" {
            let bench = format!("^BenchmarkPairs$/{case}$");
            json!({"command":command("env", &["GOCACHE=/build/go-cache", "GOTMPDIR=/build/go-tmp", "GOTOOLCHAIN=local", "GOMAXPROCS=1", "go", "test", "-run=^$", "-bench", &bench, "-benchtime=1s", "-count=1", "-cpuprofile=/build/cpu.pprof", "-o", "/build/pairs.test"]), "profiles":["cpu.pprof"], "format":"pprof", "output":{"kind":"go_benchmark", "benchmark":format!("BenchmarkPairs/{case}"), "unit":"ns/op"}})
        } else if language == "javascript" {
            json!({"command":command("node", &["--cpu-prof", "--cpu-prof-interval=100", "--cpu-prof-dir=/build", "--cpu-prof-name=cpu.cpuprofile", "benchmark.mjs", size]), "profiles":["cpu.cpuprofile"], "format":"node_cpu", "output":{"kind":"node_benchmark", "value_field":"ns_per_op", "unit":"ns/op"}})
        } else {
            json!({"command":command("valgrind", &["--tool=callgrind", "--callgrind-out-file=/build/callgrind.out", "/build/workload", size]), "profiles":["callgrind.out"], "format":"callgrind", "output":{"kind":"callgrind"}})
        };
        cfg["adapter"]["workloads"][case] = workload;
        cfg["adapter"]["workloads"][case]["repetitions"] = json!(if timing { 3 } else { 1 });
        cfg["adapter"]["workloads"][case]["required_paths"] = json!([kernel]);
    }
    cfg["adapter"]["source_prefixes"] = json!({"/work/":"", "file:///work/":""});
    kernel
}

#[expect(
    clippy::too_many_lines,
    reason = "Keep lifecycle and retained-evidence assertions in one acceptance flow"
)]
#[expect(
    clippy::print_stderr,
    reason = "Ignored integration tests emit per-scenario progress for CI diagnostics"
)]
fn acceptance(language: &str) {
    for mode in ["pass", "reverse", "none", "inconclusive"] {
        let (temp, mut cfg) = setup(mode);
        let repo = temp.path().join("repo");
        let kernel = recipe(language, &mut cfg, &repo);
        if mode == "reverse" {
            let ext = kernel.split('.').next_back().unwrap();
            fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../crates/perfine-core/tests/fixtures")
                    .join(language)
                    .join(format!("candidate.{ext}")),
                repo.join(&kernel),
            )
            .unwrap();
            git(&repo, &["add", &kernel]);
            git(&repo, &["commit", "-qm", "pin reverse fixture"]);
            let sha = git(&repo, &["rev-parse", "HEAD"]);
            for key in ["initial_base", "incremental_base"] {
                cfg["experiment"][key] = json!(sha);
            }
        }

        fs::write(
            repo.join("perfine.yaml"),
            serde_json::to_vec_pretty(&cfg).unwrap(),
        )
        .unwrap();
        fs::write(repo.join(&kernel), "dirty user content\n").unwrap();
        let id = ResourceId::generate();
        let mut output = run(&repo, &["optimize", "--experiment", id.as_str()]);
        if mode == "inconclusive" && output.status.success() {
            inject_review_conflict(&repo, &id, &output.stdout);
            output = run(&repo, &["report", "--experiment", id.as_str()]);
        }
        // Preserve failed runs too: failures must remain reviewable, never become skips.
        let destination = std::env::var("PERFINE_SIX_EVIDENCE").ok().map(|root| {
            let path = Path::new(&root).join(format!("{language}-{mode}"));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("stdout.json"), &output.stdout).unwrap();
            fs::write(path.join("stderr.log"), &output.stderr).unwrap();
            fs::write(
                path.join("settings.json"),
                serde_json::to_vec_pretty(&cfg).unwrap(),
            )
            .unwrap();
            copy_tree(&repo.join(".perfine"), &path.join("evidence"));
            path
        });
        assert!(
            output.status.success(),
            "{language}/{mode}: {}\n{}; evidence {destination:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(repo.join(&kernel)).unwrap(),
            "dirty user content\n"
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        let exp = &report["result"]["experiment"];
        assert_eq!(exp["state"], "completed", "{report}");
        let candidates = exp["candidates"].as_object().unwrap();
        let journal: Value = serde_json::from_slice(
            &fs::read(repo.join(".perfine").join(id.as_str()).join("journal.json")).unwrap(),
        )
        .unwrap();
        let store = ArtifactStore::new(repo.join(".perfine/artifacts")).unwrap();
        let frozen: perfine_core::evidence::FrozenConfig =
            serde_json::from_value(exp["config"].clone()).unwrap();
        let mut verified = std::collections::BTreeMap::new();
        for (label, record) in journal["runs"].as_object().unwrap() {
            let hash = serde_json::from_value(record["artifact"].clone()).unwrap();
            let manifest: Manifest = serde_json::from_slice(&store.get(&hash).unwrap()).unwrap();
            let v = verify_local(&frozen, &manifest.identity, manifest.clone(), &store).unwrap();
            assert!(v.correctness_passed());
            assert_eq!(manifest.measurements.len(), 2);
            for paths in manifest.profile_paths.values() {
                assert!(paths.iter().any(|p| p.as_str() == kernel));
            }
            // Fault injection into a copy of actual evidence. These are verifier
            // regressions, never replacement profiler output or measured claims.
            let mut missing_profile = manifest.clone();
            missing_profile
                .profile_paths
                .values_mut()
                .next()
                .unwrap()
                .clear();
            assert!(verify_local(&frozen, &manifest.identity, missing_profile, &store).is_err());
            let mut incomplete = manifest.clone();
            incomplete
                .measurements
                .values_mut()
                .next()
                .unwrap()
                .values
                .clear();
            assert!(verify_local(&frozen, &manifest.identity, incomplete, &store).is_err());
            let mut incompatible = manifest.clone();
            incompatible.conditions.inputs.clear();
            assert!(verify_local(&frozen, &manifest.identity, incompatible, &store).is_err());
            let mut missing_bytes = manifest.clone();
            let artifact = missing_bytes.artifacts.values_mut().next().unwrap();
            artifact.sha256 = digest(b"deliberately absent fixture artifact");
            assert!(verify_local(&frozen, &manifest.identity, missing_bytes, &store).is_err());
            verified.insert(label.clone(), v);
        }
        validate_baseline(&frozen, &verified["baseline-a"], &verified["baseline-b"]).unwrap();
        if mode == "none" {
            assert!(candidates.is_empty());
            assert_eq!(verified.len(), 2, "no fabricated candidate measurements");
        } else {
            assert_eq!(candidates.len(), 1);
            let (cid, candidate) = candidates.iter().next().unwrap();
            assert_ne!(candidate["head"], cfg["experiment"]["initial_base"]);
            let evaluation = &candidate["evaluation"];
            assert_eq!(
                evaluation["verdict"],
                match mode {
                    "pass" => "pass",
                    "reverse" => "regression",
                    _ => "inconclusive",
                },
                "{language}/{mode}: {evaluation}"
            );
            if mode == "inconclusive" {
                assert_eq!(evaluation["reason"], "repeat_required");
            }
            assert_ne!(
                evaluation["evidence_fingerprints"]
                    .as_array()
                    .unwrap()
                    .as_slice(),
                [] as [Value; 0]
            );
            assert_eq!(
                evaluation["incremental"]["cases"]
                    .as_object()
                    .unwrap()
                    .len(),
                2
            );
            let attempt = journal["attempts"]
                .as_object()
                .unwrap()
                .values()
                .next()
                .unwrap();
            assert!(
                attempt["hypothesis"]["source_paths"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(kernel))
            );
            assert_ne!(
                attempt["hypothesis"]["profile_evidence"]
                    .as_array()
                    .unwrap()
                    .as_slice(),
                [] as [Value; 0]
            );
            assert!(verified.contains_key(&format!("candidate-{cid}")));
            let comparison = run(
                &repo,
                &["compare", "--experiment", id.as_str(), "--candidate", cid],
            );
            assert!(
                comparison.status.success(),
                "{}",
                String::from_utf8_lossy(&comparison.stdout)
            );
        }
        eprintln!("six-language acceptance: {language}/{mode} verified; evidence {destination:?}");
    }
}
fn copy_tree(source: &Path, destination: &Path) {
    if !source.is_dir() {
        return;
    }
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap().flatten() {
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
        }
    }
}
macro_rules! language_test {
    ($name:ident, $language:literal) => {
        #[test]
        #[ignore = "requires immutable six-language Linux x86_64 image and deterministic fixture-agent image/network"]
        fn $name() { acceptance($language); }
    };
}
language_test!(six_language_rust, "rust");
language_test!(six_language_c, "c");
language_test!(six_language_cpp, "cpp");
language_test!(six_language_zig, "zig");
language_test!(six_language_go, "go");
language_test!(six_language_javascript, "javascript");

// Fault-inject a review conflict through the public confirmation flow. The raw
// measurements are unchanged. An absent required repeat must be inconclusive.
fn inject_review_conflict(repo: &Path, id: &ResourceId, bytes: &[u8]) {
    let result: Value = serde_json::from_slice(bytes).unwrap();
    let cid = result["result"]["experiment"]["candidates"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap();
    let review = repo.parent().unwrap().join("fixture-review.json");
    fs::write(&review, serde_json::to_vec(&json!({"schema_version":1,"statement":"Test-only injected profile/aggregate conflict; no additional measurements are supplied", "review":{"equally_simple_or_simpler":null,"regression_explanation":null,"no_family_recurrence":null,"profile_aggregate_conflict":true}})).unwrap()).unwrap();
    let mut args = vec![
        "compare",
        "--experiment",
        id.as_str(),
        "--candidate",
        cid,
        "--review",
        review.to_str().unwrap(),
    ];
    let preview = run(repo, &args);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stdout)
    );
    let value: Value = serde_json::from_slice(&preview.stdout).unwrap();
    args.extend([
        "--confirm-review",
        value["result"]["confirm_review"].as_str().unwrap(),
    ]);
    let confirmed = run(repo, &args);
    assert!(
        confirmed.status.success(),
        "{}",
        String::from_utf8_lossy(&confirmed.stdout)
    );
}
