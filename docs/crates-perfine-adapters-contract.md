# Perfine language and execution adapters

## Scope

`crates/perfine-core` owns adapter interfaces and shared implementations. `cmds/perfine` wires local execution; `actions/perfine` wires GitHub execution. This contract owns the adapter boundary within those components, not a separate engine.

## Runtime and Language

Rust adapter interfaces with language-specific external tools. Support Rust, C, C++, Zig, Go, and JavaScript. Official execution verification targets Linux x86_64; do not infer verification on other platforms.

## Users and Operators

Local developers, adapter authors, and GitHub Actions operators.

## Interfaces and Contracts

### Common engine and adapters

- Implement the CLI in Rust. Separate project/build/test discovery, measurement, profile interpretation, AI agent integration, and execution environment adapters. Language support must not require duplicating the experiment engine.
- Official first-release measurement integrations: Rust/C/C++/Zig through validated Gungraun/Callgrind paths; Go benchmarks and pprof; JavaScript Node CPU profiles and benchmarks. Verify supported paths per language rather than assuming binary compatibility. Provide an extension contract for other tools; do not claim all profiler integrations have been verified.
- Integrate Codex CLI and Claude Code locally and provide the common extension interface. Provide both CLI and MCP access to the same capabilities and policy enforcement.
- Preserve original metrics, units, source mapping, profile costs and call relationships where the adapter supplies them. Distinguish execution support, profile interpretation, and verified optimization support.
- Do not equate instruction-count improvement with elapsed-time improvement. Do not sum measurements with incompatible units. For JavaScript, exploratory `--jitless` or similar configurations do not establish improvement under normal JIT execution; validate claims under the declared deployment runtime conditions.
- Workload assertions do not replace the project's correctness checks. A profile must demonstrate that a workload exercises the path being optimized.

### Extension boundary

Keep discovery/build/test, measurement, profile interpretation, AI integration, and execution-environment capabilities separate. An extension declares its tools and versions, metric units, runtime conditions, input/output evidence, source mapping support, dependency/network needs, and verified capability level. It must obey the shared engine lifecycle, policy, and versioned evidence contracts. A new language must not duplicate the engine.

### Concrete local adapter API

- `adapters::measurement::AdapterConfig` is a serde v1 recipe: explicit discovery files, build argv, named independent correctness argv, workload argv, profile output paths under `/build`, repetitions, required observed source paths, tool-version probes, and profiler-to-repository path prefixes. Rust/C/C++/Zig consume Callgrind files, including explicitly configured Gungraun output paths. Go consumes an exact benchmark row/unit and gzip pprof. Node consumes a declared numeric benchmark JSON field and CPU profile; normal JIT is required. Missing paths, ambiguous Ir totals, incomplete profiles and missing required source coverage fail closed.
- `ConfiguredLanguageAdapter` performs configured file discovery without running repository code. Capability level is profile interpretation, not verified optimization. `NativeMeasurementAdapter` executes the recipe, records actual stdout/stderr and command hashes, verifies the source SHA and clean candidate tree, probes tools and Linux x86_64 environment identity, hashes workload inputs, and returns an engine `Manifest`. `ArtifactStore` implements content-addressed storage and `ArtifactReader`; `verify_local` invokes the shared verifier. `NativeProfileAdapter` parses verified profile artifacts; `profile::map_sources` applies declared prefixes before passing the profile to an agent.
- Before freezing engine conditions, include `AdapterConfig::recipe_hash()` under the reserved input name `perfine.adapter.recipe`. Other input keys are regular repository-relative workload files. Set build options with `build_options()`, runtime flags with `runtime_flags()`, tools with `observe_tools()`, and environment with `observe_environment()`. The toolchain value must match an observed tool version. Do not seal optimized production files as immutable workload inputs. Sealing precedes both baseline and candidate runs.
- `DockerEnvironment` requires a trusted digest-pinned, pre-provisioned image (repository digest or immutable local image ID), explicit dependency names, numeric uid/gid, memory/CPU/process limits and a command timeout. Candidate network access is empty; dependencies must already exist in the image. Mount only a task-owned source checkout read-only at `/work` and its independent build directory at `/build`. No user home, Docker socket, agent auth, publication credentials or evidence store is mounted. Commands must write outputs to `/build` or ephemeral `/tmp`; for Go CPU profiling, explicitly set the test binary output under `/build`. Go recipes must also create and set `GOTMPDIR` under the task-owned `/build` mount: `go test` executes temporary binaries there even with `-o`, and the sandbox temporary filesystem does not permit execution. Arbitrary environment keys and role reuse are rejected. Host command capture checks stdout and stderr against a 64 MiB per-stream threshold during execution; exceeding it terminates the command through the same cleanup path as cancellation and rejects oversized evidence.
- Preparation copies committed Git objects into an independent task-owned Git working tree and branch at the explicit base. It preserves dirty original user files, excludes them from the candidate, removes remotes, and avoids shared Git metadata. Candidate input must be clean. Git operations disable hooks, system/global configuration, credential prompts and commit signing. Separate execution/evidence roots and nonexisting candidate/build paths are mandatory.
- `HeadlessAgentAdapter` invokes Codex `exec --json --output-schema` or Claude `-p --output-format json --json-schema` in a separate container with the CLI's own locally authenticated file. It disables Codex shell, execution, delegation and extension tools and supplies an empty Claude tool set. Unsupported CLI feature flags fail closed. It mounts no repository: bounded source/profile data is supplied as prompt input, and structured proposed edits are applied by the host only to previously profile-linked regular source files. It requires a successful provider envelope, verifies hypothesis references and source hashes, and reads the actual committed candidate SHA. Empty edits do not fabricate a candidate. Construct the headless adapter with the owning `DockerEnvironment`; it rejects workspaces that this environment did not prepare. Agent authentication configuration is local-only and must not enter hosted APIs.
- The agent image needs the selected CLI, and its dedicated network needs operator-enforced outbound access. Fresh agent home directories contain only the CLI auth file; repository hooks/settings and build commands cannot execute there. File-based local Linux authentication is required; a host keychain-only login cannot be silently exported into a container. Authentication refresh may update that same local auth file. This local integration does not decide cloud eligibility.
- Docker integration tests are explicitly enabled with an operator-provisioned image. Captured profile parser tests and emulated runs remain distinct from native Linux x86_64 acceptance. The adapter does not auto-install tools, fetch dependencies, retry failed correctness, or infer a performance pass from execution completion.

### AI authentication and isolated execution

- Cloud AI authentication occurs entirely inside the user's repository's GitHub Actions using their configured GitHub Secrets and an officially supported unattended authentication method.
- The Perfine server must never receive, proxy, or store user AI credentials. Do not provide AI credential fields in the dashboard or server API. Local operation uses each local AI CLI's own authentication.
- Separate AI agent execution, candidate build/test/measurement, evidence upload, and GitHub publication privileges. Candidate code runs in an isolated environment without AI or GitHub publication credentials. Apply credential non-inheritance locally as well; declare required dependency/network access.
- Only enable cloud tools whose official unattended authentication/execution support is established. Unsupported tools show the reason and local alternative. If none qualify, ship cloud execution unavailable with explicit mock-only integration evidence; lack of an eligible cloud tool does not itself block this first release.
- BYOS is not permission to export a local subscription token or use an unsupported provider authentication path.

## Storage

Preserve original profile costs, call relationships, source mappings, metrics, and units when supplied. Preserve raw evidence locally or on GitHub when hosted upload is ineligible; record evidence gaps explicitly.

## Security

Keep agent execution, candidate execution, evidence upload, and publication privileges separate. The cloud runner uses repository GitHub Secrets only through officially supported unattended authentication. Local tools retain their own authentication; do not export local subscription tokens to enable unsupported cloud paths.

## Logging

Use structured logs with request and experiment IDs. Do not log secrets, source snippets, raw profiles, or raw test evidence. Use Rust `tracing` where applicable. CLI/operator output supports ANSI color by default and a documented opt-out.

## Build and Test

Run root `cargo test` after Rust changes. Each language needs representative automated fixtures covering baseline, hypotheses, candidates, correctness, comparisons, verdicts, and evidence. Test missing symbols/profile paths, incompatible workloads, absent tools, normal JIT versus exploratory modes, credential non-inheritance, and unsupported cloud tools. Verify each language-specific Gungraun/Callgrind path before claiming support. Record actual provider/native checks separately from fixtures.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Engine contract](crates-perfine-core-contract.md), [Cli contract](cmds-perfine-cli-contract.md), [Distribution contract](packages-perfine-distribution-contract.md), and [Api contract](servers-perfine-api-contract.md). Codex CLI and Claude Code are local integrations; cloud eligibility requires current official unattended execution/authentication evidence.

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)

- [Codex non-interactive execution and structured output](https://developers.openai.com/codex/noninteractive)
- [Claude headless execution and structured output](https://code.claude.com/docs/en/headless)
- [Callgrind file format](https://valgrind.org/docs/manual/cl-format.html)
- [pprof profile schema](https://github.com/google/pprof/blob/main/proto/profile.proto)
- [Node CPU profiling options](https://nodejs.org/api/cli.html#--cpu-prof)
- [Docker container execution boundary](https://docs.docker.com/reference/cli/docker/container/run/)

Each native profile file is limited to 64 MiB before parsing or copying into the artifact store. Oversized files fail the candidate with a bounded read, independently of container memory and captured-output limits.

On Unix hosts the execution root is owner-only. Each fresh, credential-free build
directory is a sticky writable mount so a configured container UID different from
the CLI user can create compiler outputs and profiles. Only that child is mounted;
source remains read-only and host credentials remain outside execution mounts.

### Interrupted workspace reattachment

The Docker adapter writes a host-owned ownership record outside candidate mounts after preparing an isolated checkout. Reattachment checks the original request and Docker configuration, direct canonical workspace paths, branch, clean source, pinned-base ancestry, absent remotes and Git alternates, and active container mounts. Reject changed or legacy workspaces without the record. Refuse reattachment while a container uses the retained source or build directory. Reattachment preserves the original checkout and build directory; it does not reset deadlines or authorize new candidate work.

A retained agent hypothesis can be restored after restart only against its original available raw profile hashes, positive-cost measured source paths, safe edit paths and clean task-owned source. The adapter binds the restored hypothesis to current source hashes before implementation. The CLI must first verify that the checkout still equals the pinned base.
