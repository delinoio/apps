# Perfine CLI and MCP

## Scope

`cmds/perfine` owns the Rust CLI, local MCP entry point, configuration, and publication orchestration. npm and standalone release packaging belong to [Distribution contract](packages-perfine-distribution-contract.md).

## Runtime and Language

Rust CLI using `crates/perfine-core`, an explicit exception to the non-server Go default. Add an explicit root Cargo workspace member with runtime implementation. CLI hosts follow the four-target [minimum-version policy](project-perfine.md#supported-cli-hosts). Measurement and optimization remain Linux x86_64 Docker execution.

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

### Host portability and recovery

Preserve every command, MCP capability, versioned interface, six-language adapter and approval boundary on supported hosts. Resolve host executables and paths without Unix-only assumptions. Support spaces, non-ASCII paths and Windows path syntax; preserve argv, streams and exit status. Diagnose missing tools, unavailable Docker, unsuitable container architecture, inaccessible mounts and permission failures with recovery guidance. Docker accepts an absolute executable or a bare tool name resolved from absolute PATH entries. Windows host tools must be `.exe` files; shell wrappers are unsupported. Reject UNC/device mounts and mount paths containing commas, quotes or newlines with relocation guidance. Do not install prerequisites implicitly.

Preserve bounded capture, deadlines, subprocess cleanup, graceful and immediate cancellation, dirty user files and retained partial evidence. Windows cancellation must use supported host process controls; do not promise Unix signal semantics on Windows. Keep local authentication isolated from candidate execution. Keychain-only credentials must not be exported implicitly; unsupported authentication arrangements fail explicitly and remain unverified until tested.

Keep `perfine.yaml`, Git-excluded `.perfine/`, evidence versions, identifiers, retention and deletion unchanged. No data migration or cross-host resume guarantee is introduced. Reject changed immutable execution conditions and incompatible retained workspaces. Upload/provider failures must preserve local results and existing retry/reconciliation rules.

### Rust command interface

The root Cargo package and executable are `perfine`, version `0.1.0`. Global options are `--repository`, `--config`, `--request-id`, `--json`, and `--no-color`; `NO_COLOR` also disables ANSI. Stdout carries the v1 response, including the request ID. Stderr carries structured tracing events without raw profiles, source, or authentication data.

`perfine.yaml` contains `schema_version: 1`, `experiment` (the core configuration including explicit commit SHAs, workload confirmation, conditions, immutable policy and all limits), `adapter` (the concrete v1 recipe), `docker` (operator-provisioned image and resource boundary), `execution_root`, `toolchain_probe`, optional local-only `agent`, and optional `action_commit`. The execution root is outside the repository and evidence directory. Tool and environment probes replace bootstrap observation fields before freezing; declared committed workload hashes must match. `init` emits an incomplete template that requires operator input, not runnable invented SHAs, credentials, workloads or limits.

- `measure --experiment <uuid-v7>` creates two same-SHA measurements and validates the baseline. Invalid retained baseline evidence makes execution terminally failed. Resume and deadline processing preserve that failure; they cannot turn it into a completed no-candidate result.
- `optimize --experiment <uuid-v7>` performs baseline validation, profile interpretation, agent hypotheses/edits, correctness, candidate measurement and shared policy evaluation. It never calls publication. Independent candidates run serially in separate directories; the configured simultaneous count remains an upper bound, never more than five. Different initial and incremental bases require separate same-SHA validations and both cumulative and incremental comparisons, including repeat evidence.
- `status --experiment <uuid-v7>` and `report --experiment <uuid-v7>` expose durable execution, candidate verdicts and retained run identities. `compare --experiment <uuid-v7> --candidate <uuid-v7>` verifies retained artifact bytes before returning the candidate evaluation.
- `resume --experiment <uuid-v7>` checks configuration and observed execution fingerprints. It reuses verified completed measurements without resetting deadlines. Interrupted admitted candidates retain their UUID, branch, checkout, build directory, head and deadline. Reattachment requires the host-owned ownership record, matching immutable configuration, clean source, pinned ancestry, and no active container using that workspace. A committed implementation is not dispatched again. Uncommitted interrupted edits, missing legacy ownership records, changed identities or expired limits require explicit recovery or a new experiment; they cannot establish a successful candidate. Completed and declined hypotheses are not repeated. Serial execution respects the declared concurrency ceiling.
- `cancel --experiment <uuid-v7>` stops new candidates; `--immediate` also signals active adapter processes. Cancellation files are local cross-process signals, not authority for hosted operations.
- `publish --experiment <uuid-v7> --candidate <uuid-v7> --draft <json-file>` returns exact content, diff, evaluation and an approval fingerprint. The draft file contains v1, target/head repositories and refs, title and body. Repeating with `--approve <fingerprint>` binds explicit approval. Policy judgment, when required, uses `--human-judgment`. The remote head must already contain the exact measured SHA; pushing is explicit and separate. Draft creation uses `gh`; uncertain attempts require reconciliation.
- `upload --experiment <uuid-v7> [--candidate <uuid-v7>] --endpoint <https-url> --space <id> --connection <id> [--consent-revision <revision>]` constructs the hosted specification and engine bundle directly from durable local evidence. Select a candidate explicitly when candidates exist; baseline/no-candidate results need no candidate ID. The local mapping preserves the exact frozen engine configuration, tools, policy, environment, inputs, limits and timestamps. New journals retain the adapter language, so upload does not require the current configuration file. Legacy journals require the original configuration fingerprint before using its language label. Generate client bindings from the canonical local schema into Cargo output and compute the manifest checksum from protobuf bytes with the checksum cleared. No user-authored envelope or JSON substitute checksum is required.
- Upload adds normalized measurement, profile-path and correctness observations alongside hash-bound original artifacts; original local manifests and files remain unchanged. Environment variables cannot be removed from an immutable configuration to make it eligible; such experiments remain local-only. Raw evidence requires preexisting manager consent and its matching revision. The client screens known prohibited content before transfer; the server independently inspects retained bytes and enforces consent. Missing runs, rejected content, denied consent or missing upload targets cannot establish complete hosted evidence. Local evidence remains available after every failure.
- Hosted API authentication uses `DELCORE_API_KEY` as bearer authorization only at the Connect endpoint. HTTPS is required except for literal loopback HTTP addresses used by local integration fixtures. Storage PUTs receive no API credentials and carry the exact content length. Before dispatch, retain an upload binding, envelope, UUID v7 completion request and attempt journal. Reuse the original `--request-id` after failure; changed endpoint/space/connection/evidence bindings are rejected. Reconcile known upload IDs through `GetUpload`, retain receipts, and retry temporary storage failures at most once. Authentication and permanent storage failures are not automatically retried. A completed upload is not dispatched again.
- `compare --experiment <uuid-v7> --candidate <uuid-v7> --review <json-file>` previews a post-run review binding. The v1 input contains `schema_version`, a nonempty `statement`, and core `review` fields: optional retained artifact fingerprints `equally_simple_or_simpler`, `regression_explanation`, `no_family_recurrence`, and boolean `profile_aggregate_conflict`. Review assertions must reference artifacts already retained in the verified runs. Repeat with `--confirm-review <fingerprint>` to preserve the attestation and reevaluate identical measurements through the shared engine. No measurement or policy changes are allowed. Attach review before publication approval; existing approvals block reevaluation. Required but absent repeat measurements remain inconclusive.
- `mcp serve` uses newline-delimited JSON-RPC over stdio. The eleven command capabilities accept `schema_version: 1`, `request_id`, and the corresponding command arguments; both transports call the same dispatch. Concurrent calls permit a cancel request while optimization executes.

### Generated Action input interface

`init --github --action-commit <full-sha>` verifies that the supplied commit exists in `delinoio/perfine-action` and contains `action.yml`, `action.cjs`, the runtime binary/checksum, and distribution provenance. The supplied SHA must come from distribution release verification; source-monorepo SHAs are not release SHAs. An `action_commit` in the input template is also accepted. No default release SHA exists. `--template <yaml-file>` supplies the required complete embedded configuration. It must omit agent configuration and select a pullable digest-pinned image that already contains the declared toolchain and workload dependencies. Existing files require review of proposed content and confirmation with `--overwrite-sha256 <existing-content-hash>`.

The generated `.github/workflows/perfine.yml` has only `workflow_dispatch`, read-only repository permissions, and an exact `delinoio/perfine-action@<verified-sha>` reference. A required manual `experiment` input supplies the UUID v7. Checkout uses the repository-pinned checkout Action, the declared incremental base, full history, and `persist-credentials: false`. A separate step pulls the immutable measurement image before candidate execution. Its Action inputs are:

- `command: measure`: the explicit CLI capability selected by the workflow. The current Action rejects cloud `optimize` and `resume` because local auth-file adapters do not establish supported unattended cloud authentication. Use local optimization; publication is never implicit.
- `experiment: ${{ inputs.experiment }}`: the manually supplied UUID v7, validated by the Action and CLI.
- `config: |`: the entire v1 YAML document described above, including workload confirmation, pinned source, adapter recipe, policy and all execution limits. No separate local configuration file is required. Missing workload source remains a separate prerequisite.

The Action must materialize the embedded configuration in a task-owned file and invoke the selected CLI capability with an explicit experiment UUID v7 and request ID. Distribution owns supported command validation, executable installation and runtime setup. Limits are configuration fields, not independently mutable Action input overrides.

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

Approved local publication creates drafts through the [GitHub REST pull-request endpoint](https://docs.github.com/en/rest/pulls/pulls#create-a-pull-request), with an explicit head repository for personal and organization-owned forks. Request bodies preserve the exact approved text; successful responses still require exact SHA and content reconciliation.
The official Action passes only non-secret `PERFINE_GITHUB_RUN_ID` and `PERFINE_GITHUB_RUN_ATTEMPT` execution context to the measurement CLI. Both must be canonical positive integers. The journal retains that identity, every measured run includes it, and resume rejects a changed or omitted identity. These values are provenance, not credentials; hosted authorization independently verifies Actions OIDC and the exact GitHub execution.

Resume processes retained candidates before requesting fresh hypotheses. New proposal generation uses the experiment admission deadline and cannot delay recovery within a retained candidate’s original timeout. Fresh proposals are requested only if admission remains open, capacity remains and cancellation has not been requested.
