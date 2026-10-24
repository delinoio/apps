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
