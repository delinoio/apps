# Perfine CLI and MCP

## Scope

`cmds/perfine` owns the Rust CLI, local MCP entry point, configuration, and publication orchestration. npm and standalone release packaging belong to [Distribution contract](packages-perfine-distribution-contract.md).

## Runtime and Language

Rust CLI using `crates/perfine-core`, an explicit exception to the non-server Go default. Add an explicit root Cargo workspace member with runtime implementation. Official first-release execution targets Linux x86_64.

## Users and Operators

Individual and organization developers operating locally without a hosted account or GitHub App; authorized users can opt into upload and publication.

## Interfaces and Contracts

### Commands and local contracts

- Commands: `init`, `doctor`, `measure`, `optimize`, `compare`, `status`, `resume`, `cancel`, `report`, `upload`, `publish`, and `mcp serve`.
- Provide human-readable and versioned JSON output. MCP uses the same capabilities, authorization, and decisions as the CLI.
- Local configuration is `perfine.yaml`. Store evidence, profiles, and resume records under Git-excluded `.perfine/`.
- `perfine init` creates local configuration. `perfine init --github` generates `.github/workflows/perfine.yml` with the required configuration in that single workflow file; a separate local config file is not required for this mode. Workload code remains a separate prerequisite when absent. Show changes and obtain confirmation before overwriting existing files.
- Generated workflows are manual by default. Users may add push/PR triggers themselves; the default workflow must not silently enable automatic AI optimization or publication.
- Execution state: `queued`, `running`, `completed`, `failed`, `cancelled`. Performance verdict is separate: `pass`, `regression`, `inconclusive`, `not_evaluated`. No candidate can be a normal `completed` execution. Clearly report exceptions and approval requirements alongside verdicts.

### Human approval and publication

- Support publication within the connected repository and fork-to-upstream publication, subject to authority. Keep independent candidates separate and preserve exact measured/submitted source correspondence.
- A person with publish permission can approve their own candidate; a second reviewer is not required. Bind approval to target repository, base, exact candidate SHA, and PR content. Present correctness results, comparison evidence, per-case regressions, policy exceptions, limitations, and reviewable diff before approval.
- Create a draft PR only after explicit approval. Never auto-merge. Revalidate and request new approval when its binding changes. If a rebase or conflict resolution changes production content, remeasure the actual submission source.
- Exclude temporary harnesses, fork CI infrastructure, unrelated commits, and user changes from production PRs. Confirm uncertain publication outcomes before retrying.

## Storage

Use `perfine.yaml` and Git-excluded `.perfine/` as specified above. Keep local experiments and evidence until deletion. Upload failures preserve local results and allow manual resend. Manifests and resume fingerprints follow [Protocol contract](protos-perfine-v1-contract.md) and [Engine contract](crates-perfine-core-contract.md).

## Security

Local-only operation needs no login. Upload uses Delcore personal/tenant service API keys with explicit space and Perfine grants; no CLI browser login. Publication requires current authority and explicit approval. Never publish user changes, temporary harnesses, fork CI infrastructure, or unrelated commits. [Api contract](servers-perfine-api-contract.md) owns hosted authorization and [Adapters contract](crates-perfine-adapters-contract.md) owns credential isolation.

## Logging

Use structured logs with request and experiment IDs. Do not log secrets, source snippets, raw profiles, or raw test evidence. Use Rust `tracing` where applicable. CLI/operator output supports ANSI color by default and a documented opt-out. Human-readable and versioned JSON output must keep execution state, verdict, policy exceptions, and approval requirements distinct.

## Build and Test

Run root `cargo test` after Rust changes. Test every command and its MCP equivalent, malformed input, unsupported major versions, missing limits/tools, overwrite confirmation, offline results, manual resend, graceful/immediate cancellation, resume mismatch, exact-source approval, self-approval, fork/upstream publication, and uncertain publication reconciliation.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Engine contract](crates-perfine-core-contract.md), [Adapters contract](crates-perfine-adapters-contract.md), [Protocol contract](protos-perfine-v1-contract.md), [Api contract](servers-perfine-api-contract.md), and [Distribution contract](packages-perfine-distribution-contract.md).

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)
