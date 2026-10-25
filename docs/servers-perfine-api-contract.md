# Perfine hosted API and background work

## Scope

`servers/perfine-api` owns the API, PostgreSQL persistence, background jobs, explicit migrations, and OCI packaging. The server owns evidence validation and policy evaluation in `src/evaluation` and consumes local schemas in `protos/perfine/v1`. It does not depend on client-workspace crates.

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
- Actions OIDC verification uses a key in the fresh cache without waiting for an unrelated JWKS refresh. Serialize refreshes with a separate lock, recheck the cache after acquiring it, fetch and parse without holding the cache lock, and publish only a complete successful key set. A failed or cancelled refresh keeps the previous keys. Unknown key IDs have no negative cache or refresh cooldown.
- On App removal, disconnection, or loss of execution authority: block new jobs, uploads, and publication, and request cancellation of running Actions. If cancellation is no longer permitted, display failure and instructions to cancel directly. Previously retained results stay private in the original space.
- Fail closed when current authorization cannot be established. Feature availability, resource limits, and UI controls do not replace authorization checks.

Connect errors keep their transport codes and credential-safe messages while `ErrorDetail.reason` identifies the recovery condition. Report unsupported major versions, deleted experiments, inactive connections, repository binding or admin denial, fingerprint mismatch, incomplete evidence, ineligible artifacts, missing raw consent, detected secrets, and unavailable cloud execution with their corresponding v1 reasons. `PUBLICATION_UNCERTAIN` applies when reconciliation requires operator inspection; a persisted `RECONCILING` state alone is not an RPC error. Use `TRANSIENT_FAILURE` for unavailable PostgreSQL, R2, or GitHub operations and `AUTHORITY_UNAVAILABLE` for unavailable Delcore or Logto authority checks.

### Deployment and operations

Local development registers the API at `perfine-api.localhost` on fixed loopback port `46323`. The shared API launcher selects `perfine-api`; root development prepares `perfine-postgres` and passes its resolved database URL through the service-owned Turbo task after Infisical injection. The service uses `/perfine-api` development settings.

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

[Engine contract](crates-perfine-core-contract.md), [Protocol contract](protos-perfine-v1-contract.md), [Distribution contract](packages-perfine-distribution-contract.md), [Delcore](project-delcore.md), and [server workspace](servers-rust-workspace-contract.md). GitHub App/Actions execute in user repositories; the server does not host candidate source or diffs.

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

The `ENGINE_EVIDENCE` artifact contains the lossless JSON `Bundle` defined by the server's evidence module: `schema_version`, v1 `config`, two `incremental_baseline` manifests, optional `cumulative_baseline`, optional `first` and `repeat` rounds, and `review`. A round contains `base`, `head`, and optional `cumulative_base` engine manifests. Each run has a distinct UUID v7 run ID. All referenced artifact bytes are separate entries in the typed RPC manifest. The server freezes the supplied config and uses its own evidence verification, baseline validation, and policy evaluation functions. The RPC manifest checksum uses deterministic protobuf bytes with its checksum cleared.

Completion requires the typed manifest's `metrics` list to contain exactly one metric. Its name, unit, objective and engine kind must match the manifest policy metric, and its engine kind must map to the frozen engine configuration's metric. A mismatch fails the mapping precondition.

The typed policy digest is the core policy fingerprint, and the typed environment digest is the core environment fingerprint. Unknown nested engine fields are rejected at the hosted privacy boundary even though the local core permits additive fields. Environment variable maps must be empty. Non-raw artifacts must be structured JSON. Each engine run must include normalized measurement, profile-path and correctness artifacts that exactly match its observations. Native profile and command/test-log artifacts can remain additional engine references, with their original hashes and sizes. Map native profile references to `RAW_PROFILE` and measurement/correctness command records and streams to `RAW_TEST_LOG`. The upload adapter must add the three normalized JSON artifacts to each native run manifest and retain its existing raw references. Raw-only runs cannot establish a hosted pass. This is an upload-envelope integration requirement, not a change to the engine's measured observations or configuration fingerprint.

Raw text profiles and test logs, Node CPU-profile JSON, and gzip protobuf pprof require current repository-specific, artifact-kind consent that predates the run, even when the client sets `containsSourceSnippets` to false. Completion checks consent again. Raw UTF-8 text may begin with non-JSON bracket or brace labels, and valid line-delimited JSON is accepted after inspection of every non-empty line. A JSON-shaped first line that is truncated, too deep, or followed by malformed mixed content is rejected; plain text also receives a decoded-escape secret scan. Structured artifacts remain strict single JSON documents. All text and decoded JSON strings/keys, including duplicate keys and escaped strings, pass secret checks. The pprof privacy parser checks every string-table entry, including unused strings and entries used for source filenames, function/system names, mapping filenames/build IDs, labels, frame filters, comments and documentation URLs. It validates string indices and all nested wire fields against the public pprof schema. Source text within these raw formats is covered by raw consent; secrets are still rejected. The parser does not rewrite the artifact: SHA-256 and accepted bytes remain those of the original gzip file.

Pprof decompression is bounded to 64 MiB and wire inspection to one million fields. Reject malformed/truncated protobuf, invalid string indices/UTF-8, unknown fields, unexpected wire types, corrupt gzip checksums, trailing bytes, concatenated gzip members, and unsupported gzip extra metadata. Inspect gzip filenames/comments for secrets. Unknown binary formats remain ineligible. These are privacy/format checks, not a replacement for normalized engine observation verification. Retain rejected originals locally or on GitHub.

Uploads first write staging keys. Only bytes verified by the service are copied to accepted keys, whose PUT URLs are never returned to clients. Downloads sign accepted keys for 900 seconds. Deletion blocks reads immediately and waits until all staging PUT URLs expire before completing physical cleanup. Failed uploads release their full reservation only after staging PUT expiry and successful deletion of all staging and accepted objects. Cleanup has no retry expiry, and a failed storage operation keeps its work record. For a running imported Actions execution, deletion also retains the execution identity and retries cancellation until GitHub confirms termination; object removal does not discard that retry record.

Publication maintenance orders pending work by its persisted next-attempt time. Each claimed approval advances that time before authority or provider checks, including failed attempts, so blocked approvals cannot monopolize a batch. This scheduling metadata is independent of the durable external-dispatch attempt count.

### Operator runbooks

- **Authentication or permission failure:** verify the Logto resource and service identity, the exact Delcore capability registration, API-key tenant/grants, and current tenant state. Do not bypass a blocked or unknown state. Only `DELETED` triggers automatic space cleanup.
- **Upload failure:** retain local originals, inspect eligibility and quota responses, and resend with the same request ID and immutable manifest. Do not edit frozen conditions or replace accepted candidate evidence. Correct secrets or source eligibility locally and create a new eligible manifest.
- **Profiler failure:** run local tool diagnostics and preserve partial evidence. Opaque formats without a hosted privacy parser cannot establish a hosted pass. Missing baseline, correctness, profile coverage, or artifact hashes fails verification.
- **Cleanup backlog:** alert when `perfine_cleanup_pending` persists beyond the configured cleanup age. Check R2 credentials and connectivity. Restore access and allow maintenance to retry. Do not remove tombstones or pending jobs to clear an alert.
- **Publication uncertainty:** inspect `perfine_publication_reconciling` and the exact approved GitHub repository, refs, SHAs, title, and body. The durable dispatch intent precedes the external request. Reconciliation checks GitHub on each maintenance cycle. An empty listing does not prove that a failed create request was not delivered, so transport failures, timeouts, HTTP 408, rate limits, and server failures stay reconciling without another create request. A definitive client rejection instead fails that approval with a bounded status-only reason. Its retained attempt is marked `_createRejected` in the stored publication and excluded from candidate dispatch accounting, allowing a corrected draft to receive new approval and dispatch. Do not manually reset an uncertain attempt counter without resolving the remote outcome.
- **Actions cancellation failure:** use the returned direct GitHub run URL and cancel through GitHub with current repository authority. Keep completed evidence. An API cancellation request is not confirmation that GitHub stopped the run.
- **Cloud unavailable:** use local Perfine execution and upload. No deployment adapter is enabled merely by supplying credentials. The server never accepts AI authentication configuration.
- **Capacity and alerts:** monitor reserved storage, pending cleanup, reconciling publication, structured operation failures, and database health over `ALERT_WINDOW_SECONDS`. Apply `ALERT_FAILURE_THRESHOLD` to failed operations and `ALERT_CLEANUP_AGE_SECONDS` to the oldest pending cleanup. Configure the operator's monitoring sink to page on sustained thresholds, and verify alert delivery separately from fixture tests.

### Integration constraints

The service-owned `delcore-service.example.yaml` provides the catalog entry; operators supply the actual Logto client ID. Generate monitoring rules with `perfine-api alerts` after setting the three alert variables. The service does not install monitoring configuration or deliver live alerts itself.

Actions upload authority follows this principal chain: a human with Delcore `manage` and GitHub repository-admin authority creates the connection; the connection retains that human's Delcore subject; each upload operation requires that subject's current independent Delcore `experiment` grant in an `ACTIVE` space and current GitHub App access to the connected immutable repository ID. Connection creation alone does not grant execution. A verified GitHub Actions OIDC job is the delegated execution principal, scoped to that repository, exact manual workflow path/ref/SHA, source SHA, run and attempt. The signed actor ID must match GitHub's run actor, but need not match the connection creator. This permits another authorized organization repository actor to dispatch the workflow without treating their GitHub identity as a Delcore user. Current grant revocation or connection/App loss blocks uploads.

Validate the signed repository name/ID, full source/workflow SHAs, workflow path and ref, manual event, run and attempt against the request and GitHub's exact attempt response. Require the remote head repository to be the connected repository; a foreign fork run is not that delegated principal. Begin also binds the frozen workflow SHA and manifest head to that execution. Completion and status use the stored execution identity. The server does not infer Delcore membership from a GitHub actor or allow Actions to approve publication. Bearer API keys register local uploads; they do not bypass Actions OIDC checks. Hosted approval and publication dispatch require a Logto human subject. The worker rechecks both the approver and the publisher before each external action. API keys are not evidence of a human approval. API-key publication dispatch is rejected because queued publication cannot revalidate a key after revocation without a Delcore key-ID authorization interface.

Publication permits a linear chain of isolated commits between the exact measured base and head. GitHub's comparison must have that exact base and merge base, a complete ordered commit list ending at the measured head, and only paths retained in measured profile coverage. Fetch every commit independently and verify its SHA, single parent and changed paths. This rejects temporary infrastructure added and then reverted within the chain, even when absent from the final diff. Reject renamed files, infrastructure paths, unmeasured paths, merge histories and potentially truncated responses. Bound comparisons below 250 commits and 300 changed files, and each commit below the endpoint's 30-file default page boundary. These conservative bounds fail closed; commit count alone is not a single-commit restriction.

The binding uses server-verified GitHub objects and the already verified engine base/head/profile scope, with no new client assertion or RPC field. Broader production changes outside measured profile paths, history rewriting and separating production changes from a mixed harness history still require a shared production-change manifest plus CLI generation and independent server verification. They remain unsupported; PR prose is not such a manifest. Prose changes require new approval; source changes require new measured evidence.

The current deployment declares both cloud tools unavailable. Start/resume fail with a local alternative; no credentials can enable an unverified adapter. For imported Actions runs, immediate cancellation uses GitHub and maintenance confirms the resulting run state. Graceful cloud control remains unavailable until a workflow control channel can stop candidate admission without terminating active candidates. Disconnection and detected authority loss request immediate cancellation and retain failures for recovery.

Configuration preparation uses `ExperimentService.PrepareExperiment` with the independent experiment grant. The typed input contains the complete public configuration and no calculated fingerprints. The API validates and freezes it through its server-owned evaluator, returns the immutable specification and canonical core configuration JSON, and retains the prepared specification for 30 days. Preparation does not create an experiment or change a started experiment. Start accepts only an exact, unexpired prepared specification and still requires an eligible cloud tool. Local/Actions uploads may use a CLI-frozen specification; registration independently reconstructs the complete configuration and verifies all three engine fingerprints before creating an experiment.

The versioned uncertainty and frozen configuration mapping is defined in the [protocol contract](protos-perfine-v1-contract.md#configuration-preparation-and-engine-v1-mapping). Confidence intervals are not calculated or inferred from sample ranges. API clients submit typed configuration values and obtain fingerprints from preparation; callers never need to calculate a hash to prepare a configuration.

Confirmed deleted spaces release active repository bindings even when they contain no experiments or preparations. Connection records remain available for cancellation and object-cleanup retries. Blocked spaces retain their bindings, including while execution maintenance cancels queued or running Actions. Authority loss alone does not release the repository reservation; raw-artifact consent updates preserve it. Explicit disconnection or confirmed deletion owns that transition.

Structured uploads reject duplicate object keys at every depth before decoding observations. Structured uploads accept only the exact engine bundle and normalized artifacts inspected through its verified run references. Unreferenced structured files are rejected, including for baseline-only uploads; declaring that a file contains no source snippets does not bypass this check.

Execution maintenance selects at most 100 connections by their persisted last-check time, oldest first. Unavailable Delcore, Logto, or GitHub authority checks advance the last-check time and retry on a later tick without changing connection state or cancelling runs. A definitive tenant, grant, or repository denial still marks authority lost and requests cancellation. Connections with unavailable access retain their run identities but yield later batches to other connections so authority checks and cancellation are not starved.

Execution reconciliation reads the stored GitHub run attempt and verifies both run ID and attempt number before accepting terminal status or cancellation confirmation. A later rerun cannot replace the original experiment’s outcome.

Publication verifies the connected head repository with its scoped installation
token. For a different target repository, resolve that repository's installation
using App authentication and mint a separate token restricted to its exact ID.
Verify its current ID and full name before approval or dispatch. Target commit
reads, comparisons and PR operations use the target token; head commit reads use
the head token. These credentials are ephemeral and never retained in approvals.

Affected CI registers Perfine in the shared Rust server matrix. API, protocol,
and server-owned evaluator changes select its Clippy, unit, and disposable PostgreSQL checks.
The existing matrix result gate rejects failed, cancelled, or skipped selected jobs;
ordinary and affected manual validation remain on Ubuntu.

Actions upload admission reloads the connection delegate and rechecks its current
experiment grant after acquiring the space lock. That check and upload mutation
share one transaction, including deduplicated retries; a concurrent reconnect
cannot substitute a different delegate after authorization.

Content-inspection rejection durably marks that immutable upload failed in the
same transaction as rejected-object cleanup. Begin retries report the failed state
without issuing PUT URLs or extending expiry, and completion retries are rejected.
The failure remains terminal after cleanup finishes, so expired rejected-object
records cannot be used to obtain fresh write URLs.

Structural evidence rejection, including unreferenced or unmodeled structured content, makes the immutable upload terminally failed and queues all staged objects for durable deletion. Repeated registration cannot renew its upload URLs.

Successful GitHub draft creation responses must match the exact approved base/head SHAs, title, body, and draft state. A mismatch is a failed publication with the created PR link retained for inspection and a requirement for new approval.
When GitHub returns or later exposes a matching approved draft, report `PUBLISHED` with its PR identity and an empty failure reason. Keep the failure reason for `FAILED` and `RECONCILING` states.

Deletion batches process eligible jobs by earliest retry time, with experiment ID as a stable tie-breaker. A full batch of blocked cleanup jobs must yield to older unvisited work while retaining every pending cancellation and storage reservation.

A bundle may reuse an identical validated baseline manifest as a comparison base. Bind each run ID to the complete manifest fingerprint and reject changed observations under the same ID. Same-SHA validation and repeat measurement identity rules remain enforced by the server-owned evaluator.

### Independent evaluation implementation

The server evaluator preserves the v1 JSON field order, sorted map keys, float round-trip parsing, configuration fingerprints, manifest verification, same-SHA baseline requirements and policy thresholds used by local clients. It owns only evidence types, limits validation and policy evaluation; local execution, filesystem storage and adapters remain client concerns. Server-owned policy and evidence regression suites cover threshold boundaries, repeats, exceptions and invalid evidence. Golden client-format fixtures pin canonical hashes across the boundary. Server tests keep raw profiler fixtures under `tests/fixtures` and must compile from `servers/` and `protos/` alone. Update compatibility fixtures and both implementations when changing versioned semantics.
