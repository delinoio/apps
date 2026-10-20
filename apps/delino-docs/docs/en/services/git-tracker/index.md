# Git Tracker

Track numeric measurements for Git commits at [git-tracker.delino.io](https://git-tracker.delino.io). Git Tracker is free. It supports public and private GitHub repositories, including fork pull requests. Use it for file sizes, timings, throughput, counts and other scalar measurements.

## Connect a repository

1. Sign in with your Delino account and select your personal or organization space.
2. Ask a space owner to grant Git Tracker View, Edit or Manage permission through the space's groups. View permits reads. Edit adds uploads and completion. Manage adds connection, token management and permanent deletion.
3. Install the Git Tracker GitHub App for the repository.
4. Open **Manage repositories**. Enter `owner/repository` and a GitHub access token for your linked GitHub account with repository administrator access. The service verifies your GitHub authority and does not retain that access token.
5. Connect the repository. You also need Delino space Owner/Admin authority. A repository can be connected to only one space at a time. Copy its connection ID from repository management for Action and API calls. After disconnection, another authorized space can connect it under a new connection ID; the old space’s retained history stays private.

Public repository measurements are readable without signing in. Private measurements require space membership and Git Tracker read permission. Space access applies to all connected repositories in that space.

Disconnecting or removing the App stops uploads and makes retained history private. Authorized members can still read it. A confirmed public-to-private change makes historical data private. If GitHub cannot confirm current visibility, Git Tracker keeps the last confirmed visibility until synchronization succeeds.

## Define your dataset

Keep a YAML configuration in your repository. IDs are stable; names can change without losing history. All columns in a row must be uploaded together.

```yaml
id: file-size
name: JavaScript file sizes
columns:
  - id: original
    name: Original size
    unit: B
    direction: lower
    absoluteThreshold: 1024
  - id: minified
    name: Minified size
    unit: B
    direction: lower
    percentageThreshold: 1
  - id: gzip
    name: Gzipped size
    unit: B
    direction: lower
    absoluteThreshold: 128
    percentageThreshold: 2
```

Upload JSON rows, for example an SWC-style file-size table:

```json
{
  "rows": [
    { "id": "jquery", "name": "jQuery", "values": { "original": 285314, "minified": 87533, "gzip": 30321 } },
    { "id": "three", "name": "Three.js", "values": { "original": 1321000, "minified": 675000, "gzip": 170000 } }
  ]
}
```

The Action preserves JSON decimal spelling without a binary floating-point conversion. Direct Connect callers send decimal values as strings, such as `"9007199254740993.0000000000000001"`. Negative numbers and finite decimals are supported. Display rounding never changes regression decisions.

Supported units are `B`, `kB`, `MB`, `GB`, `KiB`, `MiB`, `GiB`; `ns`, `μs`, `ms`, `s`; `count`, `%`, `unitless`; and `ops/s`, `B/s`, `kB/s`, `MB/s`, `GB/s`, `KiB/s`, `MiB/s`, `GiB/s`. Compatible units are converted before comparison. Incompatible kinds are not comparable. For example, 1048576 B displays as 1 MiB; exact values remain visible in tables.

Directions are `lower`, `higher` and `neutral`. Neutral columns never produce regressions. Thresholds apply to all rows in the column. With both absolute and percentage thresholds, exceeding either is a regression; equality is not. With no threshold, changes are displayed but do not cause regression failures. Only changes in the worsening direction are judged.

Percentage change is `(new - baseline) / abs(baseline) * 100`. A zero baseline has no percentage change. An absolute threshold can still decide; a percentage-only rule defers judgment. Comparisons use the target execution's stored configuration, not later configuration.

## Use the official GitHub Action

Pin `delinoio/apps/actions/git-tracker` to a reviewed full commit SHA. Replace the placeholders below with your Git Tracker connection ID, checkout revision and Action revision. Grant `id-token: write` for OIDC. A repository upload token is an alternative; store it as a GitHub secret and pass `token`.

```yaml
permissions:
  contents: read
  id-token: write
jobs:
  measure:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@<reviewed-checkout-commit-sha>
      - run: ./measure.sh
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          repository-id: '019b0000-0000-7000-8000-000000000001'
          configuration: measurements.yaml
          measurements: measurements.json
          fail-on-regression: 'false'
  mark-as-done:
    needs: measure
    if: always()
    runs-on: ubuntu-latest
    steps:
      - uses: delinoio/apps/actions/git-tracker@<reviewed-action-commit-sha>
        with:
          operation: complete
          repository-id: '019b0000-0000-7000-8000-000000000001'
```

Independent jobs share the default execution ID, `repository_id/run_id/run_attempt`, and can upload disjoint rows. Use a new execution ID for reruns. Actions OIDC requires the default identity; custom execution IDs are for repository-token callers. Fork OIDC uploads are limited to the PR head and cannot supply baseline overrides or write upstream push history.

Each upload job judges only its own uploaded rows. Regressions are reported without failing CI by default. Set `fail-on-regression: 'true'` to fail that job on regressions. Missing comparisons do not fail CI. The completion job only says measurement is finished; it does not submit CI success/failure/cancellation or perform aggregate gating. An empty completion is valid when all jobs were skipped.

Network errors and transient API failures retry at most three times. The job fails after retries by default. Set `warn-on-api-error: 'true'` to warn and pass instead. PR comment failures only warn; they do not invalidate saved data or independently fail CI. The Action logs a detail link and writes a Job Summary.

One PR comment reflects cumulative results for the latest registered execution on the PR's latest commit. Older runs remain available in web history.

## Call the Connect API

The API origin is `https://git-tracker.api.delino.io`. Connect JSON requests use `Content-Type: application/json` and `Connect-Protocol-Version: 1`. Managers can issue repository tokens with a mandatory future expiration and revoke them from repository management.

```sh
curl --fail-with-body \
  'https://git-tracker.api.delino.io/gittracker.v1.GitTrackerService/Upload' \
  -H "Authorization: Bearer ${GIT_TRACKER_TOKEN}" \
  -H 'Content-Type: application/json' \
  -H 'Connect-Protocol-Version: 1' \
  --data-binary @upload.json
```

`upload.json` contains `context`, `dataset` and `rows`:

```json
{
  "context": {
    "repositoryId": "019b0000-0000-7000-8000-000000000001",
    "executionId": "my-historical-run",
    "commit": "0123456789012345678901234567890123456789",
    "branch": "main"
  },
  "dataset": {
    "id": "size",
    "name": "Size",
    "columns": [{ "id": "bytes", "name": "Bytes", "unit": "B", "direction": "DIRECTION_LOWER", "absoluteThreshold": "1024" }]
  },
  "rows": [{ "id": "bundle", "name": "Bundle", "values": { "bytes": "1048576" } }]
}
```

Call `/gittracker.v1.GitTrackerService/Complete` with the same `context`. For PR uploads include `pullRequest` and use the PR base branch and head commit. `baselineCommit` selects an explicit full baseline SHA. Historical commits use the same upload API. Do not convert decimal strings through JavaScript `Number` before sending them.

## Execution and comparison rules

The first upload freezes each dataset's configuration for that execution. Different later configuration is rejected. Repeating an existing row with identical numeric values is idempotent. Conflicting values are rejected. A batch is atomic: an invalid or conflicting row saves none of that request.

Data appears immediately as **Measuring**. It becomes representative only after completion. The representative result for a commit is its latest registered completed execution, including executions with regressions. An older run finishing late does not replace a newer registered completed run. Repeated completion changes neither completion time nor ordering.

After six hours without newly saved measurement data, an unfinished execution becomes **Interrupted**. Identical retransmissions do not reset that timer. Completed and interrupted executions reject further uploads; interrupted executions also reject completion. Start a new execution instead. Data stays until you delete it.

Automatic PR comparisons freeze the merge-base commit and its representative completed execution on first upload. A missing baseline stays unavailable even if uploaded later. Automatic push comparisons walk first-parent ancestry separately for each row and use the nearest measured ancestor completed by the first-upload cutoff. Later jobs use that same cutoff. The table identifies each actual source commit and execution. Explicit baseline SHA comparisons do not use row-level ancestor fallback.

The website's two-commit comparison uses only the selected commits' representative completed executions. New rows/columns and missing or incompatible data are not comparable and do not cause regression failure. Deleted baseline data is not reconstructed. Before completion an absent row is uncollected; after completion it is missing.

Graphs use equally spaced commits in first-parent order. They connect actual measured points across skipped commits and never carry forward or invent values. Filter by dataset, branch, column, items, dates and commit range. Use the data table for exact values and keyboard-accessible execution links.

## Deletion and troubleshooting

Managers can permanently delete a repository's measurements, a dataset or one execution after confirmation. A comparison whose baseline was deleted becomes unavailable. When Delcore finalizes space deletion, Git Tracker deletes the space's connections, tokens, configuration and history.

For a permission error, verify the selected space and your current Git Tracker role. For upload conflicts, confirm execution identity, dataset configuration and complete row values. For interrupted or completed runs, use a new execution ID. For network/server errors, retry and retain the response correlation ID when asking for help. Never share tokens, credentials or private measurements in public reports.

PR execution measurements remain inspectable, but do not supply upstream branch graph points or automatic baselines. This prevents fork measurements from replacing baseline-branch data or configuration for a shared commit SHA.
