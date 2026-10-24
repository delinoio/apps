// SPDX-License-Identifier: Apache-2.0
//! Explicit Docker integration. This test does not invoke the fixture driver.
mod common;
use common::name;
use perfine_core::{
    adapters::{
        ArtifactStore,
        environment::{DockerConfig, DockerEnvironment},
        measurement::{
            AdapterConfig, MeasurementOutput, NativeMeasurementAdapter, RECIPE_INPUT, Workload,
        },
        profile::ProfileFormat,
    },
    evidence::*,
    interfaces::*,
    lifecycle::Workspace,
    types::*,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command as Process,
};
fn cmd(exe: &str, args: &[&str]) -> Command {
    Command {
        executable: name(exe),
        arguments: args.iter().map(|s| s.to_string()).collect(),
    }
}
// Dropped capabilities also remove root's DAC override. Bind mounts must use
// the host owner's identity so CI's non-root checkout can write build output.
fn fixture_user() -> String {
    // Docker Desktop virtualizes bind ownership; its existing emulation fixture
    // uses the VM's root identity rather than the macOS account UID.
    if !cfg!(target_os = "linux") {
        return "0:0".into();
    }
    let identity = |flag| {
        let output = Process::new("id").arg(flag).output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    format!("{}:{}", identity("-u"), identity("-g"))
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = Process::new("git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@localhost",
            "-c",
            "commit.gpgsign=false",
            "-C",
        ])
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().into()
}
struct Signal;
impl CancellationSignal for Signal {
    fn state(&self) -> Cancellation {
        Cancellation::Continue
    }
}
#[test]
#[ignore = "requires PERFINE_TEST_IMAGE with cc, Valgrind, lscpu and uname; Linux x86_64 or explicitly reported emulation"]
fn c_fixture_produces_verified_manifest_from_real_commands_and_callgrind() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/c");
    for file in ["main.c", "correctness.c", "kernel.h"] {
        fs::copy(fixtures.join(file), repo.join(file)).unwrap();
    }
    fs::copy(fixtures.join("baseline.c"), repo.join("kernel.c")).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "fixture"]);
    let sha = CommitSha::new(git(&repo, &["rev-parse", "HEAD"])).unwrap();
    let store = ArtifactStore::new(tmp.path().join("evidence")).unwrap();
    let docker = DockerConfig {
        executable: std::env::var("PERFINE_TEST_DOCKER")
            .unwrap_or("/usr/local/bin/docker".into())
            .into(),
        image: std::env::var("PERFINE_TEST_IMAGE").unwrap(),
        dependency_names: vec![name("cc-valgrind")],
        user: fixture_user(),
        cpus: 1,
        memory_bytes: 1024 * 1024 * 1024,
        process_limit: 128,
        command_timeout_seconds: 60,
    };
    let environment =
        DockerEnvironment::new(&repo, tmp.path().join("jobs"), docker, store.clone()).unwrap();
    let workspace = Workspace {
        directory: name(environment.root().join("base").to_str().unwrap()),
        build_directory: name(environment.root().join("build").to_str().unwrap()),
        branch: name("perfine/base"),
    };
    let request = EnvironmentRequest {
        role: ExecutionRole::Candidate,
        base: sha.clone(),
        workspace: workspace.clone(),
        environment: BTreeMap::from([(name("LC_ALL"), "C".into())]),
        dependency_access: vec![name("cc-valgrind")],
        network_access: vec![],
    };
    environment.prepare(&request).unwrap();
    let recipe = AdapterConfig {
        schema_version: Version,
        language: Language::C,
        discovery_files: vec![name("kernel.c")],
        build: vec![
            cmd(
                "cc",
                &["-O2", "-g", "main.c", "kernel.c", "-o", "/build/workload"],
            ),
            cmd(
                "cc",
                &[
                    "-O2",
                    "-g",
                    "correctness.c",
                    "kernel.c",
                    "-o",
                    "/build/correctness",
                ],
            ),
        ],
        correctness: BTreeMap::from([(name("project-tests"), cmd("/build/correctness", &[]))]),
        workloads: BTreeMap::from([(
            name("bulk"),
            Workload {
                command: cmd(
                    "valgrind",
                    &[
                        "--tool=callgrind",
                        "--callgrind-out-file=/build/callgrind.out",
                        "/build/workload",
                        "1024",
                    ],
                ),
                profiles: vec![name("callgrind.out")],
                format: ProfileFormat::Callgrind,
                output: MeasurementOutput::Callgrind,
                repetitions: 1,
                required_paths: BTreeSet::from([name("kernel.c")]),
            },
        )]),
        tool_probes: BTreeMap::from([
            (name("cc"), cmd("cc", &["--version"])),
            (name("valgrind"), cmd("valgrind", &["--version"])),
        ]),
        source_prefixes: BTreeMap::from([("/work/".into(), "".into())]),
    };
    let signal = Signal;
    let context = ExecutionContext {
        workspace: &workspace,
        deadline_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 180,
        cancellation: &signal,
    };
    let mut cfg = common::config();
    cfg.initial_base = sha.clone();
    cfg.incremental_base = sha.clone();
    cfg.conditions.harness = sha.clone();
    cfg.conditions.workflow = sha.clone();
    cfg.conditions.cases = BTreeMap::from([(
        name("bulk"),
        Case {
            family: name("pairs"),
            frequency: Some(1.0),
            protected: false,
        },
    )]);
    cfg.conditions.inputs = BTreeMap::from([
        (name(RECIPE_INPUT), recipe.recipe_hash().unwrap()),
        (
            name("main.c"),
            digest(&fs::read(repo.join("main.c")).unwrap()),
        ),
    ]);
    let bootstrap = cfg.clone().freeze().unwrap();
    let mut adapter = NativeMeasurementAdapter {
        config: recipe,
        frozen: &bootstrap,
        environment: &environment,
        request,
    };
    cfg.conditions.tools = adapter.observe_tools(&context).unwrap();
    cfg.conditions.toolchain = cfg.conditions.tools[&name("cc")].clone();
    cfg.conditions.build_options = adapter.config.build_options().unwrap();
    cfg.conditions.runtime_flags = adapter.config.runtime_flags().unwrap();
    cfg.conditions.environment = adapter.observe_environment(&context).unwrap();
    let frozen = cfg.freeze().unwrap();
    adapter.frozen = &frozen;
    let identity = RunIdentity {
        experiment: ResourceId::generate(),
        candidate: None,
        run: ResourceId::generate(),
        external_execution: None,
        source: sha,
        configuration: frozen.fingerprint().clone(),
        conditions: fingerprint(&frozen.config().conditions).unwrap(),
    };
    let manifest = adapter
        .measure(&identity, &frozen.config().conditions, &context)
        .unwrap();
    assert!(manifest.measurements[&name("bulk")].values[0] > 0.0);
    let verified = verify(&frozen, &identity, manifest, &store).unwrap();
    assert!(verified.correctness_passed());
    assert!(verified.manifest().profile_paths[&name("bulk")].contains(&name("kernel.c")));
    // Optional capture stays outside the immutable fixture source tree.
    if let Ok(path) = std::env::var("PERFINE_CAPTURE_CALLGRIND") {
        let artifact = verified
            .manifest()
            .artifacts
            .values()
            .find(|a| a.kind == ArtifactKind::Profile)
            .unwrap();
        fs::write(path, store.get(&artifact.sha256).unwrap()).unwrap();
    }
}
