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
