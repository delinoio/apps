# Project: Perfine

## Goal

Build Perfine, a local-first performance optimization tool with an optional hosted dashboard and GitHub Actions execution. Support individuals and organizations, public and private repositories, and Rust, C, C++, Zig, Go, and JavaScript in the first release. Linux x86_64 is the first officially verified execution platform.

The product turns profile-backed hypotheses into isolated code candidates and reproducible comparisons. AI proposes changes; Perfine validates evidence, correctness, and the declared performance policy. A valid outcome can be no improvement or no candidate. Do not promise an optimization for every project.

All optimization work must be possible locally without a Perfine server account or GitHub App. Hosted features are optional and free in the first release. Users pay their own AI subscription and GitHub Actions costs. There is no fixed release deadline. Implement the local engine, six-language adapters, hosting, and cloud execution in that order; release against the acceptance criteria below.

## Project ID

`perfine`. Dashboard: `https://perfine.delino.io`. API: `https://perfine.api.delino.io`. npm package: `perfine`.

## Domain Ownership Map

- `crates/perfine-core`: Local Apache-2.0 engine and adapters.
- `cmds/perfine`: Rust CLI and MCP.
- `apps/perfine`: Rsbuild web dashboard / Cloudflare Pages deployable.
- `servers/perfine-api`: FSL API/background work, migrations and OCI deployable.
- `protos/perfine/v1`: Canonical schemas and generated Go/TypeScript bindings.
- `actions/perfine`: Official Action source; separate distribution repository.
- `packages/perfine`: npm distribution and standalone CLI packaging.

Directory skeletons reserve these canonical paths before runtime implementation. Add explicit root Cargo memberships for the engine and CLI, and an explicit server-workspace membership for the API, when their Rust implementation begins. The client engine remains in the root Cargo workspace. The API owns its evidence validation and policy evaluation in `servers/perfine-api/src/evaluation`; it has no dependency on root client crates. The implementations preserve the versioned evidence format, canonical fingerprints and evaluation semantics.

## Domain Contract Documents

- [Engine contract](crates-perfine-core-contract.md)
- [Adapters contract](crates-perfine-adapters-contract.md)
- [Cli contract](cmds-perfine-cli-contract.md)
- [Api contract](servers-perfine-api-contract.md)
- [Protocol contract](protos-perfine-v1-contract.md)
- [Web contract](apps-perfine-web-contract.md)
- [Distribution contract](packages-perfine-distribution-contract.md)

Repository CI ownership, six-language fixture profiling gates and Compose/Turbo development integration follow the [workflow contract](repository-workflow-contract.md#perfine-repository-validation). The frontend and API own fixed loopback ports `46324` and `46323`, respectively; configuration injection follows the [environment contract](repository-environment-contract.md).

## Cross-Domain Invariants

Rust is the explicit CLI/shared-engine language choice instead of the non-server Go default. TypeScript/React serves the browser UI; hosted Rust, PostgreSQL, Connect, R2 signed URLs, and Cloudflare Pages follow repository defaults. Local filesystem evidence is required for account-free offline operation rather than mandatory R2 storage. Repository-owned server files/distributions use FSL-1.1-ALv2; other owned files/distributions use Apache-2.0, with original third-party notices retained.

The shared crate exposes configured local language, measurement, profile, headless-agent and Docker environment implementations through the [adapter API contract](crates-perfine-adapters-contract.md#concrete-local-adapter-api). CLI workers and server-side evidence evaluators reuse the same evidence and parser boundary; local authentication configuration is not a hosted API input.

The [hosted evidence and publication contract](servers-perfine-api-contract.md#evidence-upload-envelope) defines normalized observations with retained native artifacts, raw-profile privacy checks, delegated Actions authority, and exact production commit-chain verification.

All surfaces share v1 evidence, immutable experiment policy, and evaluation rules. Execution completion is distinct from a performance pass. Hosted permissions, consent, credential isolation, exact human approval, draft-only publication, and private space ownership apply across every integration.

### Acceptance Criteria

- A Linux x86_64 user can perform the complete local optimization flow without a hosted account or App, using the Rust CLI and either supported local AI integration.
- Each of six languages has representative automated fixtures exercising baseline validation, hypothesis/candidate work, correctness, comparative measurements, verdicts, and evidence. No-candidate and inconclusive outcomes are valid and distinguishable.
- CLI, MCP, and hosted evaluation share the v1 contracts and policies. Runtime limits are mandatory, immutable experiment inputs are checked, and candidate isolation preserves user work.
- `Ir` preset and all declared exceptions behave as specified. Non-`Ir` metrics without an explicit policy cannot receive an automatic pass. Dependent candidates meet incremental and cumulative requirements.
- Hosting supports authorized personal/organization workflows for public/private repositories with independent permissions, private results, single-space connections, safe reconnection, and no cross-space history leakage.
- Cloud AI credentials never traverse the Perfine server. Candidate code does not inherit agent/publication credentials. Unsupported cloud integrations are explicitly unavailable, not represented as tested.
- Hosted artifact eligibility, source-snippet consent, secret exclusion, retention, 15-minute signed URLs, tombstones, and persistent deletion recovery follow the defined rules.
- Dashboard flow, responsive behavior, accessibility, empty/error/permission states, and approval behavior match the approved text specification.
- Publication supports same-repository and fork/upstream drafts, exact approval binding, source correspondence, and uncertain-outcome reconciliation without automatic merge.
- `init --github` produces a manual workflow containing its required configuration and pins the distributed Action SHA. Action source lives in `actions/perfine`; built output is released to `delinoio/perfine-action` with exact version tags.
- Automated packaging checks cover npm, binary/checksum, and Action distribution structure. Provide all operator configuration, telemetry, documentation, and runbooks.
- Automated verification is the first-release gate. Live npm/GitHub/App/OIDC/AI account checks are not release blockers; record them as unperformed unless actually run. Do not describe fixture tests, mocked integrations, builds, or packaging as live/native acceptance.

### Test Scenarios

- Six-language fixtures: deterministic correct candidate, regression, protected-case regression, no candidate, missing symbols/profile paths, incompatible workload, incomplete positive counts, and same-SHA comparisons.
- Policy tests: weighted score, per-case regression, supplemental geometric mean, all preset exceptions, uncertain repeated measurement, missing non-Ir policy, incremental and cumulative checks, and attempted post-result policy changes.
- CLI/MCP/RPC contracts: versioned config/JSON/manifests, unsupported major versions, malformed input, request IDs, pagination, separated execution/verdict state, missing limits, and dependency/tool diagnostics.
- Execution: uncommitted files preserved, isolated work/build directories, candidate count and five-way ceiling, start deadline, per-candidate timeout, graceful versus immediate cancellation, resume fingerprint match/mismatch, and retained partial evidence.
- Fault injection: temporary failure retried once, permanent/auth failures not retried, ambiguous dispatch/publication reconciled, local offline results preserved, incomplete uploads not accepted as a pass, and failed cleanup eventually retried without expiry.
- Authorization: every independent permission, management without execution/publication grants, revoked access, deleted/blocked/unknown tenant states, repository admin requirement, single-space binding, reconnect history, and cross-space/API-key/OIDC rejection.
- Privacy: server AI credential rejection/noncollection, isolated candidate credential environment, source exclusion, repository-specific raw consent and revocation, secret removal/rejection, no raw evidence in telemetry, and no public result access.
- Storage: quotas, deletion read/link issuance denial, existing signed URLs' 15-minute validity, deleted-ID late upload rejection, 30-day deduplication boundary, and old history not transferred on connection changes.
- GitHub integration fixtures: manual workflow, App removal/cancellation failure, same-repo and fork draft PRs, self-approval, changed SHA/base/content invalidating approval, remeasurement on changed production sources, and repeated publication avoiding duplicate PRs.
- UI tests: keyboard navigation/focus/Escape, accessible names/status, narrow and wide layouts, loading/empty/inconclusive/error/permission/cloud-unavailable recovery, external diff navigation, and no credentials/billing/source-edit controls.
- Distribution tests: immutable Action release SHA, version tags without moving major tags, standalone binaries and checksums, npm package contents, notices, and absence of tracked generated `dist` directories.
- Record source revision, commands, results, and unperformed checks in PR/CI evidence. Use disposable databases and mocked external providers for automated integration; keep actual native/provider acceptance separate.

### Out of Scope

- Official first-release execution support outside Linux x86_64.
- Verified support for every existing profiler; non-core tools use the extension contract.
- Payments, paid plans, fixed numerical SLA, dedicated support, and separate administrator UI.
- Feature flags or staged flag rollout, automatic reverse DB migrations, automatic PR merge, or a mandatory second human reviewer.
- Server receipt/proxy/storage of AI credentials, CLI browser login, server storage of whole repositories or candidate diffs, anonymous hosted sharing, and other UI/documentation languages.
- Automatic inclusion or modification of uncommitted changes, automatic default push/PR triggers, automatic alteration of cases/policies to pass, or guarantees that every project improves.
- Mandatory live-account/native acceptance as a first-release gate. Such checks remain explicitly unverified until performed.

## Change Policy

Update this index and the affected domain contracts together for ownership, interface, policy, or evidence changes. Keep public usage/API/CLI documentation separate from internal contracts and provide adapter extension, policy configuration, setup, distribution, compatibility, consent, limitations, and troubleshooting guidance. Update AGENTS.md only for ownership/procedure changes. Record implementation status, validation results, source revisions, and unresolved limits in PRs, issues, and CI artifacts, not as unsupported acceptance claims here.

## References

- [Repository defaults](repository-defaults.md)
- [Repository boundary](repository-boundary-contract.md)
- [Repository license](repository-license-contract.md)
- [Environment ownership](repository-environment-contract.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delcore](project-delcore.md)
- [Shared UI](packages-ui-foundation.md)
