# Perfine hosted API and background work

## Scope

`servers/perfine-api` owns the API, PostgreSQL persistence, background jobs, explicit migrations, and OCI packaging. The server uses the local Apache engine in `crates/perfine-core` for evaluation and local schemas in `protos/perfine/v1`.

## Runtime and Language

Rust in the independent server Cargo workspace, under FSL-1.1-ALv2. Add an explicit `servers/Cargo.toml` member with runtime implementation. PostgreSQL is authoritative; R2 stores eligible artifacts. Serve Connect RPC at `https://perfine.api.delino.io`.

## Users and Operators

Authorized readers, experiment operators, publishers, space managers, and service operators. Logto owns identity and Delcore owns personal/organization space authorization.

## Interfaces and Contracts

### Identity, authorization, and repository connections

- Hosting uses Logto identity and Delcore personal/organization spaces. Do not create a separate Perfine organization or membership system. Local-only operation requires no login.
- Permissions are independent: read permits results/evidence viewing; experiment permits upload/start/resume/stop; publish permits approving exact PR content and publication; manage permits connections, raw-evidence consent, and deletion. Management does not implicitly grant experiment or publish permission.
- All hosted results, including public-repository results, are private to authorized space readers. Do not provide anonymous sharing.
- A repository can connect to one space at a time. Connection requires both Perfine management authority and GitHub repository administrator authority.
- Reconnection in the same space reuses history. A connection in another space starts new history; previous private data stays with its original space and does not transfer.
- Local upload uses existing Delcore personal/tenant service API keys with explicit space and Perfine grants. No CLI browser-login feature is included. CI evidence upload uses GitHub Actions OIDC with repository and execution scope validation.
- On App removal, disconnection, or loss of execution authority: block new jobs, uploads, and publication, and request cancellation of running Actions. If cancellation is no longer permitted, display failure and instructions to cancel directly. Previously retained results stay private in the original space.
- Fail closed when current authorization cannot be established. Feature availability, resource limits, and UI controls do not replace authorization checks.

### Deployment and operations

- Rust API and background work, PostgreSQL authoritative state, R2 artifacts, Connect RPC business API, Rsbuild web app on Cloudflare Pages, and server OCI image. Follow repository configuration and secret boundaries.
- Operator-required deployment settings include space storage quota, single-upload size limit, request-rate limit, concurrent cloud-execution limit, and alert thresholds/windows. Reject new work that exceeds its applicable limit; keep authorized read/delete paths available.
- Use explicit DB migrations. Recover service defects by deploying a corrected version; no automatic reverse migrations. Preserve v1 public compatibility.
- No feature flag: this is a new independent product. Release through deployment/version controls. Authorization, quotas, cancellation, and recovery remain enforced independently.
- Structured logs include request and experiment IDs. Metrics cover execution success/failure, queueing, storage, and external integration. Audit authorization, connections, raw-evidence consent, deletion, and publication without secrets or raw native evidence.
- Provide alert rules with operator-selected thresholds and runbooks for install/authentication/profilers, retry/recovery, cleanup, permissions, Actions cancellation failure, and unsupported cloud tools. No numerical SLA, dedicated support program, or separate administrator UI in this release.
- Upload failures preserve local results and permit manual resend. Persist complete hosted evidence before making a hosted performance conclusion. Show partial uploads as incomplete rather than success.

## Storage

### Hosted data, consent, retention, and deletion

- Store measurements, metadata, profiles, and correctness results. Do not store full repositories or candidate diffs on the Perfine server; users view diffs locally or on GitHub.
- Explicit exception to source exclusion: raw profiles or test logs may contain source snippets when a space manager has preapproved those artifact types for the repository. Consent applies to subsequent runs and can be revoked; revocation blocks new raw uploads. Do not interpret source consent as consent to upload credentials or secrets: remove them or reject the upload.
- Preserve original raw evidence locally/on GitHub when it is not eligible for hosted upload. Report missing evidence explicitly; incomplete evidence cannot establish a hosted pass.
- Keep local and hosted experiments and evidence until deletion, without automatic product-data expiry. Store configured space usage limits separately from retention policy.
- Operational logs, audits, and request deduplication records expire 30 days after first creation under the common contract. Retries do not extend that expiry.
- Managers delete results. Block new reads and new download URLs immediately when deletion is accepted; remove content asynchronously. Retain minimal experiment-ID/deleted-state tombstones to reject late upload or resume of deleted experiments. A new experiment uses a new ID.
- R2 downloads use directly issued signed URLs valid for 15 minutes. Existing URLs may remain usable until expiry after permission revocation or deletion; do not claim immediate revocation or recall of previously downloaded data. Recheck current authority before issuing a replacement URL.
- Disconnecting a repository does not delete retained evidence. For space deletion, start cleanup only after Delcore reports `DELETED`, not merely `BLOCKED`, `DELETING`, or `UNKNOWN`. Retain and retry failed cleanup until completion with operational visibility.

## Security

Apply current independent permission checks to every hosted operation, not only UI controls. [Adapters contract](crates-perfine-adapters-contract.md) owns cloud AI eligibility and isolation; the server must never receive, proxy, or store AI credentials. [Cli contract](cmds-perfine-cli-contract.md) owns exact approval binding and draft-only publication. Follow the [environment contract](repository-environment-contract.md) for configuration and secret boundaries. Use disposable databases and mocked providers for automated integration.

## Logging

Use structured logs with request and experiment IDs. Do not log secrets, source snippets, raw profiles, or raw test evidence. Use Rust `tracing` where applicable. CLI/operator output supports ANSI color by default and a documented opt-out. Operational logs, audits, and request deduplication expire 30 days after first creation. Product data and pending deletion work retain their distinct policies above.

## Build and Test

After runtime implementation, run root `cargo test` for shared Rust changes and server `cargo fmt --manifest-path servers/Cargo.toml --all --check`, `cargo clippy --locked --manifest-path servers/Cargo.toml --workspace --all-targets -- -D warnings`, and `cargo test --locked --manifest-path servers/Cargo.toml`. Run disposable PostgreSQL integration and OCI packaging checks. Test each independent permission, tenant state, repository connection, OIDC/API-key scope, quota, source/secret exclusion, consent revocation, signed URL lifetime, tombstone, cleanup retry, and partial upload.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Engine contract](crates-perfine-core-contract.md), [Protocol contract](protos-perfine-v1-contract.md), [Web contract](apps-perfine-web-contract.md), [Distribution contract](packages-perfine-distribution-contract.md), [Delcore](project-delcore.md), and [server workspace](servers-rust-workspace-contract.md). GitHub App/Actions execute in user repositories; the server does not host candidate source or diffs.

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)

## Runtime configuration and recovery

The service binary accepts `serve`, `migrate`, and `check-schema`. Migrations are explicit and idempotent. The listener defaults to `127.0.0.1:46323`; the service development command selects the `/perfine-api` Infisical folder and the `perfine-api` Portless route. Root development owns the shared Cargo build, database handoff, and launcher registration.

Configure `DATABASE_URL`, `WEB_ORIGIN`, Delcore/Logto service credentials, the GitHub App identifier and key file, and the R2 endpoint, bucket, access key and secret using service environment injection. The service-local `.env.example` lists names and safe loopback placeholders. Register the Perfine service in Delcore with the exact independent capabilities `read`, `experiment`, `publish`, and `manage`; the service client needs the existing `authorize` scope. Do not add these permissions to a global role implication hierarchy. API-key requests use `AuthorizeApiKey` and require the returned tenant to equal the requested space. Human identity requests use `GetCurrentUser` and exact `Authorize` with `paid: false`. Both also require a current `ACTIVE` tenant state.

Repository connection sends the user's GitHub authorization in the `x-github-token` transport header. The server uses it only to verify current repository admin authority and does not persist it. The GitHub App must currently have access to the same immutable repository ID. This header is distinct from the Logto or Delcore authorization header. Actions evidence uses `Authorization: Actions <OIDC token>` with issuer `https://token.actions.githubusercontent.com`, audience `https://perfine.api.delino.io`, and exact repository, manual workflow, source, run and attempt checks against GitHub.

Operators must set positive `SPACE_STORAGE_QUOTA_BYTES`, `SINGLE_UPLOAD_LIMIT_BYTES`, `REQUESTS_PER_MINUTE`, `CONCURRENT_CLOUD_EXECUTIONS`, `ALERT_WINDOW_SECONDS`, `ALERT_FAILURE_THRESHOLD`, and `ALERT_CLEANUP_AGE_SECONDS`. Storage reservations and lifecycle changes serialize per space in PostgreSQL. Quotas do not expire retained product data. Configure the external operational log sink with a fixed 30-day retention period; server audits and request records use their initial creation time. Read and delete operations bypass new-work rate and storage limits.

### Evidence upload envelope

The `ENGINE_EVIDENCE` artifact contains the lossless JSON `Bundle` defined by the server's evidence module: `schema_version`, shared-engine `config`, two `incremental_baseline` manifests, optional `cumulative_baseline`, optional `first` and `repeat` rounds, and `review`. A round contains `base`, `head`, and optional `cumulative_base` engine manifests. Each run has a distinct UUID v7 run ID. All referenced artifact bytes are separate entries in the typed RPC manifest. The server freezes the supplied config and uses the shared engine's verification, baseline validation, and evaluation functions. The RPC manifest checksum uses deterministic protobuf bytes with its checksum cleared.

The typed policy digest is the core policy fingerprint, and the typed environment digest is the core environment fingerprint. Unknown nested engine fields are rejected at the hosted privacy boundary even though the local core permits additive fields. Environment variable maps must be empty. Non-raw artifacts must be structured JSON. Opaque binary artifacts require an explicit privacy parser; retain them locally or on GitHub until that parser is available. Raw text profile/test artifacts require current pre-run consent and still pass secret checks.

Uploads first write staging keys. Only bytes verified by the service are copied to accepted keys, whose PUT URLs are never returned to clients. Downloads sign accepted keys for 900 seconds. Deletion blocks reads immediately and waits until all staging PUT URLs expire before completing physical cleanup. Cleanup has no retry expiry, and a failed storage operation keeps its work record.

### Operator runbooks

- **Authentication or permission failure:** verify the Logto resource and service identity, the exact Delcore capability registration, API-key tenant/grants, and current tenant state. Do not bypass a blocked or unknown state. Only `DELETED` triggers automatic space cleanup.
- **Upload failure:** retain local originals, inspect eligibility and quota responses, and resend with the same request ID and immutable manifest. Do not edit frozen conditions or replace accepted candidate evidence. Correct secrets or source eligibility locally and create a new eligible manifest.
- **Profiler failure:** run local tool diagnostics and preserve partial evidence. Opaque formats without a hosted privacy parser cannot establish a hosted pass. Missing baseline, correctness, profile coverage, or artifact hashes fails verification.
- **Cleanup backlog:** alert when `perfine_cleanup_pending` persists beyond the configured cleanup age. Check R2 credentials and connectivity. Restore access and allow maintenance to retry. Do not remove tombstones or pending jobs to clear an alert.
- **Publication uncertainty:** inspect `perfine_publication_reconciling` and the exact approved GitHub repository, refs, SHAs, title, and body. The durable dispatch intent precedes the external request. Reconciliation checks GitHub before another attempt and allows only one retry. Do not manually reset the attempt counter without resolving the remote outcome.
- **Actions cancellation failure:** use the returned direct GitHub run URL and cancel through GitHub with current repository authority. Keep completed evidence. An API cancellation request is not confirmation that GitHub stopped the run.
- **Cloud unavailable:** use local Perfine execution and upload. No deployment adapter is enabled merely by supplying credentials. The server never accepts AI authentication configuration.
- **Capacity and alerts:** monitor reserved storage, pending cleanup, reconciling publication, structured operation failures, and database health over `ALERT_WINDOW_SECONDS`. Apply `ALERT_FAILURE_THRESHOLD` to failed operations and `ALERT_CLEANUP_AGE_SECONDS` to the oldest pending cleanup. Configure the operator's monitoring sink to page on sustained thresholds, and verify alert delivery separately from fixture tests.

### Integration constraints

The service-owned `delcore-service.example.yaml` provides the catalog entry; operators supply the actual Logto client ID. Generate monitoring rules with `perfine-api alerts` after setting the three alert variables. The service does not install monitoring configuration or deliver live alerts itself.

Actions upload authority is tied to the connecting human's GitHub actor ID and current Delcore `experiment` permission. A workflow started by another repository actor needs an explicit identity/authority binding before it can upload. Bearer API keys register local uploads; they do not bypass Actions OIDC checks. Hosted approval requires a Logto human subject. API keys are not evidence of a human approval, and queued publication cannot revalidate a key after revocation without a Delcore key-ID authorization interface.

The current publication verifier permits one isolated commit containing only paths retained in the measured profile coverage. It rejects renamed files, infrastructure paths, unrelated commits, and truncated comparisons. Broader candidate diffs need a shared, verified production-change manifest that identifies the measured production diff and its allowed files. The API does not infer that manifest from PR prose. Prose changes require new approval; source changes require new measured evidence.

The current deployment declares both cloud tools unavailable. Start/resume fail with a local alternative; no credentials can enable an unverified adapter. For imported Actions runs, immediate cancellation uses GitHub and maintenance confirms the resulting run state. Graceful cloud control remains unavailable until a workflow control channel can stop candidate admission without terminating active candidates. Disconnection and detected authority loss request immediate cancellation and retain failures for recovery.

Configuration preparation uses `ExperimentService.PrepareExperiment` with the independent experiment grant. The typed input contains the complete public configuration and no calculated fingerprints. The API validates and freezes it through `perfine-core`, returns the immutable specification and canonical core configuration JSON, and retains the prepared specification for 30 days. Preparation does not create an experiment or change a started experiment. Start accepts only an exact, unexpired prepared specification and still requires an eligible cloud tool. Local/Actions uploads may use a CLI-frozen specification; registration independently reconstructs the complete configuration and verifies all three engine fingerprints before creating an experiment.

The versioned uncertainty and frozen configuration mapping is defined in the [protocol contract](protos-perfine-v1-contract.md#configuration-preparation-and-engine-v1-mapping). Confidence intervals are not calculated or inferred from sample ranges. The dashboard edits typed configuration values and obtains fingerprints from preparation; callers never need to calculate a hash to prepare a configuration.
