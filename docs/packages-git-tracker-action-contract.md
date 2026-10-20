# Git Tracker GitHub Action

## Scope

`actions/git-tracker` owns this component. See the [project index](project-git-tracker.md) for boundaries. Deployment, provisioning, Runbooks, billing, exports, non-GitHub hosting and native clients are excluded.

## Runtime and Language

Node.js 24 and JavaScript.

## Users and Operators

External individuals and organizations, CI jobs and service operators.

## Interfaces and Contracts

### Identity, spaces, authorization, and visibility

- Sign in with the shared Delino account and link a GitHub account. Logto owns identity; Delcore owns shared personal and organization tenants, memberships, and service permissions. Do not create a separate Git Tracker organization or membership system.
- Use Git Tracker service roles: View, Edit, and Manage. View permits reads; Edit permits measurement uploads and completion; Manage additionally permits repository connection, token management, and repository/dataset/execution deletion. YAML configuration is supplied with the execution; there is no separate web configuration editor.
- Authorization is space-wide. A member with Git Tracker read permission can read all connected repository measurements in that space.
- A GitHub repository can be connected to only one Delino space at a time. Initial connection requires both Delino space management authority and GitHub repository administrator authority.
- Use a GitHub App for selected repository access, repository metadata/visibility synchronization, and PR comment management.
- Public GitHub repository measurements are anonymously readable. Private repository measurements are available only to authorized space members.
- When a public-to-private change is received and applied, make historical measurements private and block anonymous and public-cache access.
- If GitHub availability or permissions prevent checking the current visibility, retain the last confirmed visibility. This intentionally means that an unobserved public-to-private change can leave anonymous reads available until synchronization succeeds.
- Repository disconnection or App removal stops uploads and makes retained data private. Authorized space members can still read or delete it.
- Support GitHub Actions OIDC and repository-scoped API tokens for uploads and completion. Managers issue and revoke tokens; an expiration date is mandatory at issuance.
- Support measurements from fork PRs for the connected upstream repository, without granting authority to change baseline-branch data or configuration. Validate execution identity and repository/PR scope; do not treat a fork's configuration as authority over the upstream baseline.
- Enforce current service permissions and tenant boundaries on protected operations. Feature availability never substitutes for authorization.

### Dataset configuration and numeric values

- A dataset is a table of items (rows) and measurement columns. Dataset, row, and column IDs are stable and separate from display names. Renaming with the same ID preserves history.
- Store configuration in a version-controlled YAML file. The Action transmits it; direct API users provide equivalent configuration. Measurement uploads use the defined JSON format.
- Columns specify units, improvement direction, and optional regression thresholds. Configuration is immutable within an execution for each dataset: the first upload establishes it, and differing later configuration is rejected.
- Allow finite integers and decimal values, including negative values. Preserve the JSON number's decimal value through parsing, transport, storage, and comparison; do not introduce a binary floating-point round trip. Display rounding does not affect decisions.
- Improvement directions: lower is better, higher is better, and neutral. Neutral columns display values and changes but never produce regressions.
- Supported units:
  - Size: B, kB, MB, GB, KiB, MiB, GiB.
  - Time: ns, μs, ms, s.
  - Count: count. Ratio: %. Unitless: unitless.
  - Throughput: ops/s, B/s, kB/s, MB/s, GB/s, KiB/s, MiB/s, GiB/s.
- Units come from this supported list, not arbitrary strings. Convert compatible units for comparison. Incompatible unit kinds are not comparable.
- Display readable scaled units, for example 1048576 B as 1 MiB, while exposing the original value and unit in details.
- Regression thresholds apply per column uniformly to all rows. Support absolute-change and percentage-change thresholds. When both exist, exceeding either is a regression; equality alone does not exceed the threshold. Evaluate only changes in the worsening direction.
- With no threshold, display the value, change, and improvement direction but do not produce a regression or CI failure.
- Apply the comparison target execution's stored configuration, including direction and thresholds, rather than retrospectively applying a later configuration.
- Percentage change is `(new - baseline) / abs(baseline) * 100`. If baseline is zero, percentage change is unavailable. Use the absolute threshold when present; with only a percentage threshold, defer judgment.

### Uploads, concurrency, execution lifecycle, and history

- Each CI job uploads measurements with the common execution ID, repository, commit, dataset configuration, and applicable branch/PR and comparison context. Different jobs can upload different rows of the same dataset. All columns of a row must be uploaded together.
- Repeating the same row with the same values in the same execution/dataset is an idempotent retransmission. Different values for that row are rejected, not overwritten.
- An upload request is atomic. If any row is invalid or conflicts, save none of that request and identify the invalid row and reason.
- Data appears immediately on the website as measuring/in progress. It is not yet the commit's representative result.
- The user adds a final CI job, such as `mark as done`, that explicitly calls the completion API for the commit and execution ID. It reports only that measurement is finished; it does not submit CI success/failure/cancellation or perform aggregate CI regression gating.
- Completion is idempotent. Repeated completion succeeds without changing data, completion time, or execution ordering.
- Completed executions reject further uploads; use a new execution ID for reruns. Preserve execution history.
- Permit completion with no preceding uploads when all jobs were skipped. Record an empty completed execution and create no graph values.
- Automatically mark an unfinished execution interrupted six hours after the last upload that successfully saved new measurement data. Identical retransmissions do not reset that timer. Preserve interrupted data, reject uploads and completion for that execution, and require a new execution ID.
- For each commit, the representative result is the completed execution with the latest registration order, regardless of regressions. Registration occurs on first upload, or at creation by the completion call for an empty execution. An older execution completing late does not supersede a newer registered completed execution.
- A representative result does not exclude regression-containing executions. In-progress executions remain separately inspectable.
- Support uploading historical commits through the same API. Do not order baseline eligibility by historical upload arrival in place of Git ancestry.
- Skipping means omitting uploads, without explicit skip records or reasons. Before completion, an absent row is uncollected; after completion it is missing. Missing data alone is not a regression and does not fail CI.
- Keep measurements until the user deletes them. No paid plans, usage billing, or product usage quotas are part of the first release.

### Comparison rules

- Allow explicit baseline SHA override in addition to automatic selection.
- PR automatic baseline: the merge-base commit. Pin the baseline commit and its representative completed execution at the target execution's first upload. If unavailable then, retain comparison-unavailable status for that execution; do not silently switch to a later uploaded baseline.
- Push automatic baseline: use first-parent Git ancestry, with row-level fallback for skipped measurements. Fix an eligibility cutoff at the target execution's first upload. For each row, select the nearest ancestor measurement from executions completed by that cutoff. Later-arriving jobs in the same execution use the same cutoff, not their individual upload times. Freeze the selected row baseline.
- Row-level historical fallback is specific to automatic push comparison. It does not mean using different baseline commits for a user-requested exact two-commit comparison.
- Direct comparison in the website compares only the two selected commits' representative completed executions. Do not search ancestors for missing rows.
- Missing baseline data results in stored uploads, comparison-unavailable status, and no CI failure for missing comparison alone. New rows/columns are marked new/not comparable and are not judged as regressions without a baseline.
- An incompatible unit change is not comparable. Compatible units are converted before applying the target configuration's comparison rules.
- Display values, absolute deltas, percentage deltas when defined, and regression/improvement status, with actual source commit/execution references for row-level baselines.
- If a referenced baseline execution is deleted, do not retain its values to reconstruct the comparison. Show baseline-data-deleted/not comparable.

### Website and GitHub experience

- Provide an English responsive web application with desktop and mobile access to graphs, comparisons, and management.
- The flow covers Delino sign-in, space selection, GitHub linking/App repository connection, upload setup guidance, dataset/history browsing, execution details, comparisons, and authorized token/deletion management.
- Graph controls include dataset, branch, measurement column, multiple items, date range, and commit range. A point links to its execution details.
- Use equally spaced commits on the horizontal axis, ordered along the branch's first-parent path. Dates appear in details.
- Plot actual measurements only and connect surrounding measured points across skipped commits. Do not fabricate or carry forward values.
- Provide automatic-baseline comparisons and user-selected two-commit comparisons in tables.
- Support keyboard use of primary functions, tabular alternatives to graphs, and text as well as color for improvement/regression. Follow this repository's existing UI instructions.
- Empty states explain connection/upload steps; loading states show progress; request failures offer retry and a trace/correlation ID; permission failures explain lack of access without exposing private data.
- The official Action provides logs, Job Summary, and web detail links. Each upload job judges its uploaded data. By default regressions are reported without failing CI; users can enable regression failure.
- For transient upload/completion network or server errors, retry at most three times. After retries, fail the job by default; permit configuration to warn and pass instead.
- Maintain one PR comment. Each upload updates the cumulative results of the latest registered execution on the PR's latest commit; completion updates the completion state. Older commit/execution updates must not replace the latest comment contents. Earlier results remain available through web history.
- PR comment publication/update failure produces a warning only. It does not invalidate stored measurements, change the regression result, or independently fail CI.

### Deletion, telemetry, operations, and security

- After confirmation, managers can permanently delete an entire repository's measurements, a dataset, or an execution. Preserve the baseline-deletion behavior above.
- After Delcore finalizes space deletion, permanently delete that space's Git Tracker repository connections, tokens, configuration, and measurement history.
- Provide structured Go `log/slog` logs, request/execution correlation, bounded error categories, health/readiness interfaces, Prometheus metrics, and OpenTelemetry traces.
- Audit authorization changes, repository connection changes, token issuance/revocation, and data deletion. Exclude secret values and raw measurements from audit payloads. Make audit retention an operator-required setting and delete expired audit records.
- Supply Prometheus alert rules for API server error rate, upload/completion persistence failures, GitHub integration failures, and automatically interrupted executions. Require operator-defined thresholds and observation windows. Operators connect receiving routes through Alertmanager or equivalent; the service does not send alert messages itself.
- Implement a threat model and automated security tests for tenancy, OIDC/token validation, fork PR restrictions, webhook verification, visibility changes, and secret redaction. Follow repository secret/configuration rules; never commit credentials or expose them in telemetry.
- No feature flag is required because this is a new independent service. Authorization and data-access rules remain enforced independently.
- Deployment, deployment rollback, infrastructure provisioning, and Runbooks are outside this PRD. These exclusions do not remove the in-scope runtime state, retention, telemetry, and alert-rule behavior described above.

### Documentation and support

- Provide repository connection instructions, YAML/JSON format guidance, Action and Connect API examples, permissions/deletion documentation, error troubleshooting, and a SWC-style table example.
- Explain completion versus CI result, execution identity, row-level push baselines, missing comparisons, historical uploads, unit conversion, and precision preservation.
- Keep owning project/domain contracts and documentation catalogs current during implementation. Public documentation contains public behavior and interfaces, not internal server details.
- Do not create a Runbook or a separate support administration UI.

## Storage

PostgreSQL is authoritative for server state. Clients do not persist credentials or measurement history. Configuration and measurements retain stable IDs independently of display names.

## Security

Check current space permissions on protected operations. Verify GitHub signatures and Actions OIDC identity; reject cross-repository and cross-space access. Public responses must not be cached across visibility changes. Never expose secrets or raw measurements in logs or audits.

## Logging

Use correlation IDs and bounded error categories. Server logs use log/slog; Action logs and summaries describe failures without credentials.

## Build and Test

Run the owning repository format checks, Go tests, protocol generation/lint/breaking checks and frontend/Action tests as applicable. Test concurrency, exact decimals, frozen baselines, visibility changes, token revocation and failure paths with mocked providers and PostgreSQL integration. Automated fixtures do not establish live-account acceptance.

## Dependencies and Integrations

Logto, Delcore, GitHub App, GitHub Actions OIDC, PostgreSQL and the apps-owned Connect protocol. Client and server contributors use this checkout and require no production credentials for fixture-based validation.

## Change Triggers

Update the project index and affected protocol/client/server contracts when behavior or ownership changes.

## References

- [Project](project-git-tracker.md)
- [Defaults](repository-defaults.md)

## Distribution interface

`actions/git-tracker/action.yml` runs `action.cjs` with Node.js 24. The bundle is generated from `main.mjs` by the package-local build command and committed outside any `dist` directory. Preserve dependency license notices. CI checks source tests and bundle freshness. Inputs are operation, repository-id, execution-id, configuration, measurements, token, api-url, baseline, fail-on-regression and warn-on-api-error. The default OIDC execution ID is `repository_id/run_id/run_attempt`; custom identities require repository tokens.
