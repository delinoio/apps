// SPDX-License-Identifier: Apache-2.0
use perfine_core::{
    adapters::{
        ArtifactStore,
        agent::{LocalAgent, parse_response},
        environment::{DockerConfig, DockerEnvironment},
        measurement::{
            AdapterConfig, ConfiguredLanguageAdapter, MeasurementOutput, Workload,
            parse_go_benchmark,
        },
        profile::{self, ProfileFormat},
    },
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
fn name(s: &str) -> Name {
    Name::new(s).unwrap()
}
fn git(path: &Path, args: &[&str]) -> String {
    let result = Process::new("git")
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
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}
fn config() -> DockerConfig {
    DockerConfig {
        executable: "/usr/local/bin/docker".into(),
        image: format!("perfine-test@sha256:{}", "1".repeat(64)),
        dependency_names: vec![name("compiler")],
        user: "1000:1000".into(),
        memory_bytes: 1024 * 1024 * 1024,
        cpus: 1,
        process_limit: 64,
        command_timeout_seconds: 10,
    }
}
fn command(executable: &str, args: &[&str]) -> Command {
    Command {
        executable: name(executable),
        arguments: args.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn callgrind_preserves_compressed_names_relative_positions_and_edges() {
    let raw = b"# callgrind format\npositions: instr line\nevents: Ir Dr\nfl=(1) /work/kernel.c\nfn=(1) main\n0x10 10 7 2\n+4 +1 3\ncfl=(1)\ncfn=(2) count_pairs\ncalls=4 0x20 22\n* * 90 3\nfn=(2)\n0x20 22 90 3\ntotals: 100 5\n";
    let parsed = profile::parse(ProfileFormat::Callgrind, raw).unwrap();
    assert_eq!(parsed.totals[&name("Ir")], 100.0);
    assert_eq!(parsed.profile.costs[2].source.line, Some(11));
    assert_eq!(parsed.profile.edges[0].calls, Some(4));
    assert_eq!(parsed.profile.edges[0].callee.file, name("/work/kernel.c"));
    assert_eq!(parsed.profile.edges[0].callee.line, Some(22));
    assert_eq!(parsed.profile.costs[1].metric.unit, name("Dr"));
    assert_eq!(parsed.profile.raw_artifacts, vec![digest(raw)]);
    assert!(profile::parse(ProfileFormat::Callgrind, b"events: Ir\nfl=a.c\nfn=f\n1 1\n").is_err());
    assert!(
        profile::parse(
            ProfileFormat::Callgrind,
            b"events: Ir\nfl=(99)\n1 1\ntotals: 1\n"
        )
        .is_err()
    );
}
#[test]
fn node_cpu_samples_preserve_microseconds_and_call_graph() {
    let raw = br#"{"nodes":[{"id":1,"callFrame":{"functionName":"root","url":"","lineNumber":-1},"children":[2]},{"id":2,"callFrame":{"functionName":"countPairs","url":"file:///work/kernel.mjs","lineNumber":4}}],"startTime":10,"endTime":410,"samples":[2,2],"timeDeltas":[100,200]}"#;
    let mut parsed = profile::parse(ProfileFormat::NodeCpu, raw).unwrap();
    assert_eq!(parsed.profile.costs[0].cost, 300.0);
    assert_eq!(parsed.profile.costs[0].metric.unit, name("microseconds"));
    assert_eq!(parsed.profile.costs[0].source.line, Some(5));
    assert_eq!(parsed.profile.edges[0].cost, 300.0);
    profile::map_sources(
        &mut parsed.profile,
        &BTreeMap::from([("file:///work/".into(), "".into())]),
    )
    .unwrap();
    assert_eq!(parsed.profile.costs[0].source.file, name("kernel.mjs"));
    let invalid = String::from_utf8(raw.to_vec())
        .unwrap()
        .replace("[100,200]", "[100]");
    assert!(profile::parse(ProfileFormat::NodeCpu, invalid.as_bytes()).is_err());
}
#[test]
fn go_benchmark_rejects_missing_duplicate_and_incomplete_rows() {
    assert_eq!(
        parse_go_benchmark(
            b"BenchmarkPairs-8 1000 45.25 ns/op 0 B/op\nPASS\n",
            "BenchmarkPairs-8",
            "ns/op"
        )
        .unwrap(),
        45.25
    );
    for text in [
        "BenchmarkPairs-8 0 1 ns/op",
        "BenchmarkPairs-8 10 NaN ns/op",
        "BenchmarkPairs-8 10 2 ns/op\nBenchmarkPairs-8 10 2 ns/op",
        "BenchmarkOther-8 10 2 ns/op",
        "BenchmarkPairs-8 10 2",
    ] {
        assert!(parse_go_benchmark(text.as_bytes(), "BenchmarkPairs-8", "ns/op").is_err());
    }
}
#[test]
fn agent_requires_completed_provider_envelope() {
    let good = b"{\"type\":\"turn.started\"}\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"hypotheses\\\":[]}\"}}\n{\"type\":\"turn.completed\"}\n";
    assert_eq!(
        parse_response(LocalAgent::Codex, good).unwrap()["hypotheses"],
        serde_json::json!([])
    );
    assert!(parse_response(LocalAgent::Codex, &good[..good.len() - 26]).is_err());
    assert!(parse_response(LocalAgent::Codex, b"{\"type\":\"turn.failed\"}").is_err());
    assert!(parse_response(LocalAgent::Claude, br#"{"type":"result","subtype":"error_max_turns","is_error":true,"structured_output":{"hypotheses":[]}}"#).is_err());
    assert!(parse_response(LocalAgent::Claude, br#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"hypotheses":[]}}"#).is_ok());
}
#[test]
fn pinned_checkout_preserves_dirty_user_files_and_does_not_share_git_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("kernel.c"), "committed").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    let base = CommitSha::new(git(&repo, &["rev-parse", "HEAD"])).unwrap();
    fs::write(repo.join("kernel.c"), "dirty user data").unwrap();
    fs::write(repo.join("untracked"), "private user data").unwrap();
    fs::write(
        repo.join(".git/hooks/post-checkout"),
        "#!/bin/sh\ntouch hook-ran\n",
    )
    .unwrap();
    let before = git(&repo, &["status", "--porcelain"]);
    let root = tmp.path().join("jobs");
    let store = ArtifactStore::new(tmp.path().join("evidence")).unwrap();
    let environment = DockerEnvironment::new(&repo, &root, config(), store).unwrap();
    let workspace = Workspace {
        directory: name(root.join("candidate").to_str().unwrap()),
        build_directory: name(root.join("build").to_str().unwrap()),
        branch: name("perfine/candidate"),
    };
    let request = EnvironmentRequest {
        role: ExecutionRole::Candidate,
        base: base.clone(),
        workspace: workspace.clone(),
        environment: BTreeMap::new(),
        dependency_access: vec![name("compiler")],
        network_access: vec![],
    };
    environment.prepare(&request).unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(workspace.directory.as_str()).join("kernel.c")).unwrap(),
        "committed"
    );
    assert!(
        Path::new(workspace.directory.as_str())
            .join(".git")
            .is_dir()
    );
    assert!(
        !Path::new(workspace.directory.as_str())
            .join("untracked")
            .exists()
    );
    assert!(!repo.join("hook-ran").exists());
    assert_eq!(git(&repo, &["status", "--porcelain"]), before);
    assert_eq!(environment.source(&workspace).unwrap(), base);
    assert!(environment.prepare(&request).is_err());
    let args = environment
        .candidate_arguments(
            &command("echo", &["$(touch injected)", ";"]),
            &request,
            "perfine-test",
        )
        .unwrap();
    assert!(args.windows(2).any(|a| a == ["--network", "none"]));
    assert!(args.contains(&"$(touch injected)".into()));
    assert!(!args.iter().any(|a| a.contains(repo.to_str().unwrap())));
    assert!(args.iter().any(|a| a.contains("dst=/work,readonly")));
    let mut invalid = request.clone();
    invalid
        .environment
        .insert(name("GITHUB_TOKEN"), "secret".into());
    assert!(
        environment
            .candidate_arguments(&command("env", &[]), &invalid, "perfine-test")
            .is_err()
    );
    invalid = request;
    invalid.role = ExecutionRole::Agent;
    assert!(
        environment
            .candidate_arguments(&command("env", &[]), &invalid, "perfine-test")
            .is_err()
    );
}
#[test]
fn configured_discovery_supports_all_fixture_routes_without_running_fixture_driver() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (language, directory, file, format, output) in [
        (
            Language::Rust,
            "rust",
            "main.rs",
            ProfileFormat::Callgrind,
            MeasurementOutput::Callgrind,
        ),
        (
            Language::C,
            "c",
            "main.c",
            ProfileFormat::Callgrind,
            MeasurementOutput::Callgrind,
        ),
        (
            Language::Cpp,
            "cpp",
            "main.cpp",
            ProfileFormat::Callgrind,
            MeasurementOutput::Callgrind,
        ),
        (
            Language::Zig,
            "zig",
            "main.zig",
            ProfileFormat::Callgrind,
            MeasurementOutput::Callgrind,
        ),
        (
            Language::Go,
            "go",
            "go.mod",
            ProfileFormat::Pprof,
            MeasurementOutput::GoBenchmark {
                benchmark: name("BenchmarkPairs-1"),
                unit: name("ns/op"),
            },
        ),
        (
            Language::JavaScript,
            "javascript",
            "benchmark.mjs",
            ProfileFormat::NodeCpu,
            MeasurementOutput::NodeBenchmark {
                value_field: name("ns_per_op"),
                unit: name("ns/op"),
            },
        ),
    ] {
        let config = AdapterConfig {
            schema_version: Version,
            language,
            discovery_files: vec![name(file)],
            build: vec![],
            correctness: BTreeMap::from([(name("check"), command("test-command", &[]))]),
            workloads: BTreeMap::from([(
                name("bulk"),
                Workload {
                    command: command("workload-command", &[]),
                    profiles: vec![name("profile.out")],
                    format,
                    output,
                    repetitions: 1,
                    required_paths: BTreeSet::from([name(file)]),
                },
            )]),
            tool_probes: BTreeMap::from([(name("tool"), command("tool", &["--version"]))]),
            source_prefixes: BTreeMap::new(),
        };
        let adapter = ConfiguredLanguageAdapter {
            config,
            tools: BTreeMap::new(),
            metric: MetricSpec {
                metric: Metric::Time,
                unit: name("ns/op"),
            },
            dependencies: vec![],
        };
        let workspace = Workspace {
            directory: name(fixture.join(directory).to_str().unwrap()),
            branch: name("unused"),
            build_directory: name("/tmp/unused"),
        };
        assert_eq!(adapter.discover(&workspace).unwrap().language, language);
        assert_eq!(
            adapter.capabilities().level,
            CapabilityLevel::ProfileInterpretation
        );
        let mut bad = adapter.config.clone();
        bad.discovery_files = vec![name("missing-profile-path")];
        assert!(
            ConfiguredLanguageAdapter {
                config: bad,
                ..adapter
            }
            .discover(&workspace)
            .is_err()
        );
    }
}

#[test]
fn captured_go_pprof_and_node_cpu_profiles_keep_hot_paths_and_original_units() {
    let go = profile::parse(
        ProfileFormat::Pprof,
        include_bytes!("adapter-data/go-darwin-arm64.pprof"),
    )
    .unwrap();
    assert!(go.profile.costs.iter().any(|c| {
        c.source
            .symbol
            .as_ref()
            .is_some_and(|s| s.as_str().ends_with("CountPairs"))
            && c.cost > 0.0
            && c.metric.unit.as_str() == "nanoseconds"
    }));
    assert!(go.profile.edges.iter().any(|e| {
        e.callee
            .symbol
            .as_ref()
            .is_some_and(|s| s.as_str().ends_with("CountPairs"))
    }));
    assert!(
        go.profile
            .costs
            .iter()
            .any(|c| c.metric.unit.as_str() == "count")
    );
    let node = profile::parse(
        ProfileFormat::NodeCpu,
        include_bytes!("adapter-data/node-darwin-arm64.cpuprofile"),
    )
    .unwrap();
    assert!(node.profile.costs.iter().any(|c| {
        c.source
            .symbol
            .as_ref()
            .is_some_and(|s| s.as_str() == "countPairs")
            && c.cost > 0.0
            && c.metric.unit.as_str() == "microseconds"
    }));
}

struct Continue;
impl CancellationSignal for Continue {
    fn state(&self) -> Cancellation {
        Cancellation::Continue
    }
}
#[test]
#[ignore = "requires PERFINE_TEST_IMAGE digest and local Docker; run explicitly"]
fn docker_enforces_filesystem_credentials_network_and_timeout_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("source"), "source").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    let secret = tmp.path().join("agent-secret");
    fs::write(&secret, "credential-sentinel").unwrap();
    let mut docker = config();
    docker.image = std::env::var("PERFINE_TEST_IMAGE").expect("set digest-pinned test image");
    docker.user = "0:0".into();
    docker.command_timeout_seconds = 2;
    let environment = DockerEnvironment::new(
        &repo,
        tmp.path().join("jobs"),
        docker,
        ArtifactStore::new(tmp.path().join("evidence")).unwrap(),
    )
    .unwrap();
    let workspace = Workspace {
        directory: name(environment.root().join("candidate").to_str().unwrap()),
        branch: name("perfine/isolation"),
        build_directory: name(environment.root().join("build").to_str().unwrap()),
    };
    let request = EnvironmentRequest {
        role: ExecutionRole::Candidate,
        base: CommitSha::new(git(&repo, &["rev-parse", "HEAD"])).unwrap(),
        workspace: workspace.clone(),
        environment: BTreeMap::new(),
        dependency_access: vec![name("compiler")],
        network_access: vec![],
    };
    environment.prepare(&request).unwrap();
    let signal = Continue;
    let context = ExecutionContext {
        workspace: &workspace,
        deadline_unix_seconds: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 60,
        cancellation: &signal,
    };
    let out = environment.execute(&command("/bin/sh", &["-c", "test -z \"$OPENAI_API_KEY$ANTHROPIC_API_KEY$GITHUB_TOKEN\" && test ! -e \"$1\" && test ! -e /var/run/docker.sock && test ! -e /root/.codex/auth.json && test ! -e /root/.aws/credentials && ! touch /work/injected && touch /build/output", "isolation", secret.to_str().unwrap()]), &request, &context).unwrap();
    assert_eq!(
        out.exit_code,
        0,
        "{}",
        String::from_utf8_lossy(&environment.artifacts.get(&out.stderr_artifact).unwrap())
    );
    assert!(
        Path::new(workspace.build_directory.as_str())
            .join("output")
            .exists()
    );
    assert!(matches!(
        environment.execute(
            &command("/bin/sh", &["-c", "sleep 60 & wait"]),
            &request,
            &context
        ),
        Err(Error::Limit)
    ));
    assert_eq!(fs::read_to_string(secret).unwrap(), "credential-sentinel");
}

#[test]
fn captured_callgrind_retains_actual_c_hot_function() {
    let parsed = profile::parse(
        ProfileFormat::Callgrind,
        include_bytes!("adapter-data/c-linux-amd64-emulated.callgrind"),
    )
    .unwrap();
    assert!(parsed.totals[&name("Ir")] > 0.0);
    assert!(parsed.profile.costs.iter().any(|c| {
        c.source
            .symbol
            .as_ref()
            .is_some_and(|s| s.as_str() == "count_pairs")
            && c.cost > 0.0
    }));
    assert!(parsed.profile.edges.iter().any(|e| {
        e.callee
            .symbol
            .as_ref()
            .is_some_and(|s| s.as_str() == "count_pairs")
    }));
}

#[cfg(unix)]
#[test]
fn mock_headless_agents_apply_only_profile_linked_edits_in_owned_checkouts() {
    use perfine_core::adapters::agent::{HeadlessAgentAdapter, LocalAgentConfig};
    use std::os::unix::fs::PermissionsExt;
    for (provider, unchanged) in [
        (LocalAgent::Codex, false),
        (LocalAgent::Claude, false),
        (LocalAgent::Codex, true),
        (LocalAgent::Claude, true),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("kernel.c"), "int value(void) { return 1; }\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        let base = CommitSha::new(git(&repo, &["rev-parse", "HEAD"])).unwrap();
        let fake = tmp.path().join("mock-docker");
        let args_log = tmp.path().join("argv.json");
        let script = format!(
            r#"#!/usr/bin/python3
import sys,json
json.dump(sys.argv,open({:?},'w'))
data=json.loads(sys.stdin.read().split('\n',1)[1])
if 'profile' in data:
    result={{'hypotheses':[{{'explanation':'Profile shows the measured function','source_paths':['kernel.c'],'profile_evidence':data['profile']['raw_artifacts']}}]}}
else:
    result={{'edits':[{{'path':'kernel.c','content':'int value(void) {{ return 2; }}\n'}}]}}
if '--output-schema' in sys.argv:
    print(json.dumps({{'type':'turn.started'}}))
    print(json.dumps({{'type':'item.completed','item':{{'type':'agent_message','text':json.dumps(result)}}}}))
    print(json.dumps({{'type':'turn.completed'}}))
else:
    print(json.dumps({{'type':'result','subtype':'success','is_error':False,'structured_output':result}}))
"#,
            args_log
        );
        fs::write(
            &fake,
            if unchanged {
                script.replace("return 2;", "return 1;")
            } else {
                script
            },
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let mut docker = config();
        docker.executable = fake;
        let store = ArtifactStore::new(tmp.path().join("evidence")).unwrap();
        let environment = DockerEnvironment::new(
            &repo,
            tmp.path().join("jobs"),
            docker.clone(),
            store.clone(),
        )
        .unwrap();
        let workspace = Workspace {
            directory: name(environment.root().join("candidate").to_str().unwrap()),
            build_directory: name(environment.root().join("build").to_str().unwrap()),
            branch: name("perfine/agent"),
        };
        let request = EnvironmentRequest {
            role: ExecutionRole::Candidate,
            base: base.clone(),
            workspace: workspace.clone(),
            environment: BTreeMap::new(),
            dependency_access: vec![name("compiler")],
            network_access: vec![],
        };
        environment.prepare(&request).unwrap();
        let auth = tmp.path().join("auth.json");
        fs::write(&auth, "mock local authentication sentinel").unwrap();
        let agent_config = LocalAgentConfig {
            provider,
            docker,
            executable: "agent-cli".into(),
            auth_file: auth,
            network: "perfine-agent-egress".into(),
            model: None,
            maximum_source_bytes: 8192,
            maximum_edits: 2,
        };
        let agent = HeadlessAgentAdapter::new(agent_config.clone(), &environment).unwrap();
        let context = ExecutionContext {
            workspace: &workspace,
            deadline_unix_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 30,
            cancellation: &Continue,
        };
        let hash = store.put(b"mock profile").unwrap();
        let profile = Profile {
            costs: vec![ProfileCost {
                source: SourceLocation {
                    file: name("kernel.c"),
                    line: Some(1),
                    symbol: Some(name("value")),
                },
                metric: MetricSpec {
                    metric: Metric::Ir,
                    unit: name("instructions"),
                },
                cost: 10.0,
                callees: vec![],
            }],
            edges: vec![],
            raw_artifacts: vec![hash],
            missing_source_mappings: vec![],
        };
        let hypotheses = agent.propose(&profile, &context).unwrap();
        assert_eq!(hypotheses.len(), 1);
        drop(agent);
        let agent = HeadlessAgentAdapter::new(agent_config, &environment).unwrap();
        assert!(agent.implement(&hypotheses[0], &context).is_err());
        let mut invalid = hypotheses[0].clone();
        invalid.profile_evidence = vec![digest(b"absent profile")];
        assert!(
            agent
                .restore_hypothesis(&invalid, &profile, &context)
                .is_err()
        );
        agent
            .restore_hypothesis(&hypotheses[0], &profile, &context)
            .unwrap();
        let sha = agent.implement(&hypotheses[0], &context).unwrap();
        if unchanged {
            assert_eq!(sha, base);
        } else {
            assert_ne!(sha, base);
        }
        assert_eq!(
            fs::read_to_string(repo.join("kernel.c")).unwrap(),
            "int value(void) { return 1; }\n"
        );
        let argv: Vec<String> = serde_json::from_slice(&fs::read(args_log).unwrap()).unwrap();
        assert!(
            !argv
                .iter()
                .any(|arg| arg.contains(workspace.directory.as_str())
                    || arg.contains(repo.to_str().unwrap()))
        );
        match provider {
            LocalAgent::Codex => {
                assert!(argv.windows(2).any(|a| a == ["--disable", "shell_tool"]));
                assert!(argv.windows(2).any(|a| a == ["--disable", "unified_exec"]));
                assert!(argv.windows(2).any(|a| a == ["--disable", "multi_agent"]));
            }
            LocalAgent::Claude => assert!(argv.windows(2).any(|a| a == ["--tools", ""])),
        }
        if !unchanged {
            assert!(agent.implement(&hypotheses[0], &context).is_err());
        }
        let user_workspace = Workspace {
            directory: name(repo.to_str().unwrap()),
            ..workspace.clone()
        };
        let context = ExecutionContext {
            workspace: &user_workspace,
            ..context
        };
        assert!(agent.propose(&profile, &context).is_err());
    }
}

#[test]
fn codex_commentary_does_not_replace_the_final_schema_response() {
    let events = [
        serde_json::json!({"type":"turn.started"}),
        serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":"I will inspect the supplied profile."}}),
        serde_json::json!({"type":"item.completed","item":{"type":"agent_message","text":"{\"hypotheses\":[]}"}}),
        serde_json::json!({"type":"turn.completed"}),
    ].iter().map(serde_json::Value::to_string).collect::<Vec<_>>().join("\n");
    assert_eq!(
        parse_response(LocalAgent::Codex, events.as_bytes()).unwrap()["hypotheses"],
        serde_json::json!([])
    );
}

#[test]
fn node_signed_deltas_reorder_samples_without_losing_elapsed_cost() {
    // Synthetic parser regression for out-of-order samples seen in actual V8
    // captures. This test does not supply benchmark or acceptance measurements.
    let raw = br#"{"nodes":[{"id":1,"callFrame":{"functionName":"root","url":"","lineNumber":-1},"children":[2,3]},{"id":2,"callFrame":{"functionName":"a","url":"file:///work/a.mjs","lineNumber":0}},{"id":3,"callFrame":{"functionName":"b","url":"file:///work/b.mjs","lineNumber":0}}],"startTime":10,"endTime":410,"samples":[2,3,2],"timeDeltas":[100,-4,200]}"#;
    let parsed = profile::parse(ProfileFormat::NodeCpu, raw).unwrap();
    let costs: BTreeMap<_, _> = parsed
        .profile
        .costs
        .iter()
        .map(|cost| (cost.source.file.as_str(), cost.cost))
        .collect();
    assert_eq!(costs["file:///work/a.mjs"], 200.0);
    assert_eq!(costs["file:///work/b.mjs"], 96.0);
    assert_eq!(
        parsed
            .profile
            .edges
            .iter()
            .map(|edge| edge.cost)
            .sum::<f64>(),
        296.0
    );
    let text = std::str::from_utf8(raw).unwrap();
    for deltas in ["[100,-200,200]", "[9223372036854775807,1,1]"] {
        let invalid = text.replace("[100,-4,200]", deltas);
        assert!(profile::parse(ProfileFormat::NodeCpu, invalid.as_bytes()).is_err());
    }
}
