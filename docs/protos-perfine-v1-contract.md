# Perfine v1 protocol and evidence

## Scope

`protos/perfine/v1` owns canonical Connect schemas and generated Go/TypeScript bindings. This contract also owns the shared version and resource rules for CLI JSON, configuration, MCP, and evidence manifests; their implementation remains in the owning component.

## Runtime and Language

Protobuf and Connect RPC. Generate server Rust bindings through the existing server protocol crate into Cargo output. Keep Go consumers in the root `github.com/delinoio/apps` module and TypeScript exports in `@delino/protos`; do not duplicate generated source in apps or servers.

## Users and Operators

CLI/MCP clients, dashboard, Action uploader, API, and external protocol consumers.

## Interfaces and Contracts

- Version configuration, JSON, evidence schemas, MCP contracts, and RPC API from v1. Add fields compatibly, reject unsupported major versions explicitly, and use a new major version with migration guidance for breaking changes.
- Connections, experiments, candidates, approvals, and uploads are distinct resources with UUID v7 IDs. Keep GitHub IDs and commit SHAs separate. Mutations carry request IDs, lists use cursor pagination, and responses/errors expose request IDs.
- Evidence manifests include schema version, source and harness SHAs, tools/environment, input hashes, policy, metric units, file hashes, and completion status. Preserve enough information to independently assess a result and reproduce its declared conditions.

Execution state is `queued`, `running`, `completed`, `failed`, or `cancelled`. Performance verdict is independently `pass`, `regression`, `inconclusive`, or `not_evaluated`. A completed execution may have no candidate. Preserve exceptions and approval requirements independently of those enums.

CLI, MCP, and hosted comparison use the same evidence and evaluation semantics from [Engine contract](crates-perfine-core-contract.md). Mutating operations cover connection, experiment lifecycle, evidence upload, consent, approval/publication, and deletion with the independent permissions in [Api contract](servers-perfine-api-contract.md).

## Storage

Manifests preserve schema version, source/harness identities, inputs, tools/environment, policy, metric units, file hashes, and completion status. Local storage follows [Cli contract](cmds-perfine-cli-contract.md); hosted evidence eligibility, retention, tombstones, and signed URLs follow [Api contract](servers-perfine-api-contract.md). Request deduplication expires 30 days after creation; retained publication and deletion records remain independent safeguards.

## Security

Never define AI credential fields or a source/diff storage API. Keep UUID v7 resource IDs distinct from GitHub IDs and commit SHAs. Server-side authorization must validate space, repository, and execution scope; request IDs do not confer authority.

## Logging

Responses and errors expose request IDs. Structured logs must not expose API keys, AI credentials, raw evidence, or user state.

## Build and Test

With schemas implemented, run root `pnpm proto:lint`, `pnpm proto:generate`, generation freshness checks, and `node protos/scripts/check-breaking.mjs "<comparison-revision>"`. Run affected Go vet/tests/consumer builds and Rust/frontend consumers under the repository boundary contract. Test unsupported majors, additive compatibility, malformed messages, cursor pagination, identifiers, immutable inputs, missing limits, manifest completeness, and separated execution/verdict state.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Engine contract](crates-perfine-core-contract.md), [Cli contract](cmds-perfine-cli-contract.md), [Api contract](servers-perfine-api-contract.md), [Web contract](apps-perfine-web-contract.md), and [repository boundary](repository-boundary-contract.md).

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)

## Canonical RPC surface

The package is `perfine.v1` in `protos/perfine/v1/perfine.proto`. Go exports use `github.com/delinoio/apps/protos/perfine/v1` and `perfinev1connect`; TypeScript consumers import `@delino/protos/perfine/v1/perfine_pb`. The server generator exports `delino_protocol::perfine::v1` through its generated include file. Root Buf generation includes this schema alongside the other repository protocols.

- `AccessService.GetAuthorization` reports the caller's current independent grants. It does not grant authority to subsequent requests.
- `ConnectionService` lists and reads connections, connects or reconnects repositories, disconnects repositories, and changes raw-artifact consent with an expected revision. Connect requires both management and GitHub repository administrator authority. Reconnect in the same space reuses the connection and history. Connecting in another space creates a new connection without transferring retained history.
- `ExperimentService` prepares complete configurations, lists and reads experiments and candidates, starts cloud experiments, resumes matching experiments, and requests graceful or immediate cancellation. `GetExperiment` is the status polling operation. Completed experiments can have an empty candidate list.
- `EvidenceService` checks eligibility, begins and completes uploads, reports upload status, lists manifests, and issues evidence download URLs. `BeginUpload` also registers local experiments without cloud dispatch. Its experiment specification must match an existing experiment exactly when appending evidence. Candidate IDs in manifests identify separate UUID v7 resources; completion registers candidate summaries from verified engine evidence, never from an unverified client verdict.
- `ManagementService` accepts experiment deletion and reports asynchronous cleanup status. The experiment ID identifies the deletion operation and its permanent tombstone.
- `PublicationService` approves exact draft content, reads approvals, publishes approved drafts, and reports publication/reconciliation status. Approval IDs identify publication attempts; retained candidate/approval/publication records and GitHub state prevent duplicates independently of request deduplication expiry.
- `CloudService.ListCloudTools` reports current eligibility, official verification references, failure reasons, and local alternatives. It carries no authentication material.

RPC comments state the required independent permission. Result and evidence reads require `READ`; upload, execution and cloud eligibility require `EXPERIMENT`; approval/publication require `PUBLISH`; connection, consent and deletion mutations require `MANAGE`. Connection metadata reads accept `READ` or `MANAGE`; `GetRawArtifactConsent` requires `MANAGE` and lets a manager obtain the revision without result-read permission. Upload-status and publication-status operations expose their own operation records under the corresponding grant. No permission implies another. Every operation rechecks current space and resource authority, even when the caller previously obtained an authorization response. Authentication uses existing transport credentials; no RPC accepts AI credentials or defines an identity/membership system.

## Validation and transport semantics

Every request and response carries `request_id`. Mutations require a caller-generated UUID v7 request ID. Read callers may omit it, in which case the server assigns one. Application failures attach `ErrorDetail` to the Connect error and expose the same ID in `x-request-id` response metadata; transport failures also expose an assigned ID in metadata where a response can be produced. Error messages and field violations must not echo credentials or raw evidence.

Deduplication binds the authenticated actor, space, RPC method, request ID, and canonical request content. Reuse with changed content returns `REQUEST_ID_CONFLICT`; retries do not extend the 30-day expiry. Tombstone checks and publication reconciliation still apply after expiry. Request IDs and resource IDs confer no authority.

Validate required messages, UUID v7 IDs, GitHub numeric IDs, full commit SHAs, SHA-256 hashes, finite numeric values, positive limits, and explicit enum values. Reject unknown/unspecified enum values where a decision is required. Version fields denote major versions and must be `1`; reject unsupported majors with `UNSUPPORTED_VERSION`. Protobuf scalar defaults do not imply approval, consent, completeness, or a valid policy. Presence of the policy, uncertainty and limits messages is checked by handlers. Required policy ratios, repetitions and uncertainty criteria are validated by the local engine and server evaluator; zero is allowed only where the declared policy explicitly permits it. `MEASUREMENTS_ONLY` cannot establish an automatic pass. The Callgrind preset retains all thresholds and exception rules in the engine contract.

All list RPCs use `PageRequest` and `PageResponse`. An empty cursor starts a list; an empty next cursor ends it. Bind cursors to the authorized space, parent resource, filters and ordering. Reject malformed, expired or mismatched cursors. Order resource history by creation time and ID descending; use a stable unique artifact/tool key for evidence/tool lists. A zero page size selects the server default; enforce the configured maximum. Recheck authorization on each page.

Start requires a new experiment UUID v7, full immutable specification, representativeness confirmation, all execution limits, and an eligible cloud tool. Resume accepts only the stored fingerprint and does not change limits, source, inputs, policy or environment. When those inputs change, create a new experiment. Graceful cancellation and the start deadline stop new candidates while active work stays bounded by its per-candidate timeout. Immediate cancellation also stops active work. Cancellation status separately reports request, confirmation or failure and a direct GitHub cancellation URL when needed.

## Engine evidence and manifest mapping

`EvidenceManifest` is the typed upload envelope. It binds source/base/head and harness SHAs, experiment fingerprint, tools and environment, input hashes, policy, metric units, artifact hashes and sizes, run start time, consent revision and completion. A baseline or no-candidate manifest has no candidate ID. Candidate evidence includes its distinct candidate UUID v7 and the exact measured head SHA.

The serializable evidence produced by the shared engine is uploaded as an `ENGINE_EVIDENCE` artifact with a versioned JSON media type. `engine_artifact_name` identifies that entry in the manifest. The artifact preserves the engine's complete lossless evidence, including original metric values, same-SHA validation, incremental and cumulative comparisons, correctness, exceptions, limitations and immutable provenance. The typed `Comparison`, `CaseMeasurement`, and `CorrectnessResult` messages are read summaries derived from that artifact. They do not replace the lossless evidence consumed by the local engine and the independent server evaluator. In particular, protobuf double summaries must not be used to reconstruct exact large integer instruction counts.

Adapters map engine version to `schema_version`, source and harness identities to the SHA fields, engine fingerprint to `experiment_fingerprint_sha256`, declared policy to `PerformancePolicy`, tools/runtime/build settings to `ExecutionEnvironment`, input digests to `InputHash`, and persisted artifact bytes to manifest hashes and sizes. Preserve the engine's own fingerprint and policy digest algorithms; do not rehash a lossy RPC summary as if it were equivalent evidence. Completion verifies these mappings against the decoded engine artifact and evaluates through the shared engine. Unsupported engine versions or mismatched provenance fail explicitly. Full repositories, source files, candidate diffs and credentials are not eligible artifact kinds, including when embedded in engine JSON.

Artifact SHA-256 hashes cover the exact uploaded bytes. For `manifest_sha256`, hash deterministic protobuf serialization of the manifest with `manifest_sha256` cleared, no unknown fields, and the original repeated-field ordering. Producers and consumers must preserve that order. Reject duplicate artifact names and require exactly one referenced `ENGINE_EVIDENCE` artifact for evaluation. Transport JSON is not the hash input.

`CheckUploadEligibility` is advisory. `BeginUpload` and `CompleteUpload` recheck authority, connection, quotas, tombstones, consent and content eligibility. Only raw profiles and raw test logs can use repository-specific source-snippet consent, and the grant must predate the run. Revocation blocks new raw uploads even for previously approved runs. Client flags about removed secrets or source content are claims, not trusted validation. Completion verifies stored object bytes, digests and sizes; uploads cannot update frozen provenance or overwrite accepted evidence under a different hash. Missing required artifacts remain incomplete and cannot establish a hosted pass. Preserve ineligible originals locally or on GitHub.

Download issuance rechecks `READ` and deletion state. Signed GET URLs expire after 15 minutes; existing URLs may remain usable until expiry after deletion or permission revocation. Accepted deletion immediately blocks reads, new URLs, uploads and resume, while cleanup retries persist until completion. The permanent tombstone retains only the experiment ID and deleted state after cleanup.

## Exact publication binding

`DraftBinding` includes target and head GitHub repository identities, base ref and SHA, head ref and SHA, title, body, candidate UUID v7 and evidence manifest digest. Approval records bind all these fields to an explicit human action. Self-approval is permitted for a publisher; policy-required human judgment must be explicit. Approval is not inferred from an empty string, a successful experiment, or a publish request alone.

`PublishDraft` carries both the approval ID and expected binding. Revalidate current authority, GitHub repository/ref state, evidence completeness, measured/submitted production-source correspondence and all approved content before publication. Reject any changed binding with `APPROVAL_STALE`; prose-only changes need new approval, while changed production content also requires remeasurement. Only draft PRs are created, including fork-to-upstream publication. Uncertain outcomes enter `RECONCILING` and are checked against GitHub before another creation attempt. No RPC merges a PR.

## Configuration preparation and engine v1 mapping

`ExperimentService.PrepareExperiment` takes `request_id`, `space_id`, `connection_id`, and typed `ExperimentConfiguration`. It requires `EXPERIMENT`, an active connection, current tenant authority and GitHub App access. The response contains `specification`, `engine_config_json` and `prepared_at`; the JSON is canonical `perfine_core::evidence::Config`, contains public reproducibility data only, and can be consumed by CLI/Action adapters. Preparation does not start execution or mutate an existing experiment. Edit configuration and prepare again before start; started experiments remain immutable. The API retains exact prepared specifications for 30 days. `StartExperiment` accepts only an exact current prepared specification; cloud availability remains an independent gate.

Configuration provides version 1, base/initial-base/harness/workflow SHAs, language, environment, workload cases, global input hashes, correctness checks, policy, explicit workload confirmation text, parent candidate IDs, and limits. Preparation limits use positive whole-second `start_duration` and `candidate_timeout` (at most one year), positive candidate count and simultaneous count 1–5, bounded by candidate count. Preparation calculates the start deadline from server time plus the duration. The returned specification retains the workflow SHA, global inputs, checks, confirmation and duration so registration can independently reconstruct the exact configuration. No client-supplied policy or environment fingerprint is accepted during preparation. The API returns all fingerprints calculated by the shared core, using its struct serialization and ordered maps.

`Metric.engine_kind` selects `IR`, `TIME`, `THROUGHPUT`, `MEMORY`, or `CUSTOM` without guessing from display names. The display name and language label do not change the core algorithm; unit and engine kind do. `ExecutionEnvironment.hardware` supplies the required public hardware map. `runtime` maps exactly to the core toolchain string; include its version in that string and in the declared tools where applicable. The separate `runtime_version` and tool `configuration_sha256` fields must be empty because core v1 has no corresponding fields. Build options, runtime flags, tools and public hardware map are preserved exactly. Environment variables and credentials are forbidden. Case input references must match declared global input hashes; core v1 models a global input set. Declared frequencies map to explicit values; assumed-one frequencies must equal 1 and map to the core's absent frequency.

`PerformancePolicy.schema_version` and `UncertaintyCriteria.engine_schema_version` are 1. New uncertainty ratios use protobuf presence so an explicit zero remains distinct from omission. The existing minimum-improvement and maximum-case-regression scalars retain their wire-compatible zero default; zero is a valid core threshold, not a request for the IR preset. The deprecated confidence-level, interval-width and near-threshold-margin fields must be zero or absent; the engine does not implement confidence-interval inference. Exact mappings are:

| Policy              | Required settings                                                                                                                                       | Core mapping                                                                                                                                                                               |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `CALLGRIND_IR`      | IR metric, `instructions` unit, minimize objective, repetitions 1, minimum improvement, maximum case regression, near threshold, repeat tolerance       | `IrPolicy` fields with identical ratio values. The two supported exception enum values map to `allow_simple_exception` and `allow_single_case_exception`; omitted exceptions are disabled. |
| `DECLARED`          | Non-IR metric, objective, repetitions >=2, both policy ratios, maximum relative range, decision margin, repeat tolerance, near threshold; no exceptions | `DeclaredPolicy` and `Uncertainty` fields with identical values. Relative range is `(max-min)/mean`; decision margin is a conservative adjustment, not a confidence interval.              |
| `MEASUREMENTS_ONLY` | Explicit metric; no evaluation thresholds, repetitions, exceptions or uncertainty settings                                                              | `MeasurementsOnly`; no automatic pass.                                                                                                                                                     |

For IR, maximum-relative-range and decision-margin fields must be absent. All applicable ratio values are finite and in `[0,1)`. The documented IR preset is minimum improvement 0.01, maximum case regression 0.01, near threshold 0.001, repeat tolerance 0.001, with both exceptions enabled; clients submit these explicit values when selecting that preset. Preparation never silently substitutes the preset for user settings. Evidence registration and completion enforce the same mapping and verify the complete configuration against the lossless engine artifact.
