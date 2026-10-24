# Manual GitHub Actions setup

The distributed Action is [delinoio/perfine-action](https://github.com/delinoio/perfine-action). Releases use exact `vX.Y.Z` tags and verified distribution commit SHAs. Do not use a moving major tag, a source repository commit, or an invented SHA.

## Generate the workflow

1. Select a published Action release and verify its full distribution commit SHA and release contents. Set `PERFINE_ACTION_SHA` to that verified SHA. If no release is available, continue with the local CLI; there is no default release SHA.
2. Prepare a complete v1 YAML template for your committed source and workload. Omit local `agent` configuration. Use a pullable digest-pinned image that already contains your toolchain and dependencies.
3. Generate the manual workflow:

```sh
perfine init --github --template "perfine-actions.yaml"   --action-commit "$PERFINE_ACTION_SHA"
```

Perfine verifies the commit in the distribution repository and checks the Action entry point, executable/checksum, and provenance before writing `.github/workflows/perfine.yml`. For an existing workflow, inspect the proposed change and confirm with `--overwrite-sha256` using its current content hash.

The generated file embeds the complete configuration. It does not require a separate local config file at execution time. Workload code remains a prerequisite; generation does not invent missing benchmarks or inputs.

## Run measurement explicitly

The workflow uses only `workflow_dispatch`. It declares read-only repository permissions, checks out the configured base with full history and no persisted checkout credentials, pulls the immutable image, and pins the verified Perfine Action SHA. Supply a fresh UUID v7 through the manual `experiment` input.

The Action inputs are `command: measure`, `experiment`, and the embedded YAML `config`. Limits belong to that frozen configuration, not separate mutable Action overrides. Review and commit the generated workflow before dispatching it in GitHub. Perfine does not enable push/PR triggers automatically.

## Cloud optimization is unavailable

The current Action rejects cloud `optimize` and `resume`. Supported local file authentication does not establish an officially supported unattended cloud authentication path. Do not export a local subscription token to bypass this restriction, and do not send AI credentials to Perfine hosting.

Use [local optimization](/services/perfine/cli) with your supported local AI CLI. Keep evidence locally and use the [upload interface](/services/perfine/hosting) only when you can prepare eligible evidence. Generated measurement workflows do not imply automatic upload or publication. Draft publication always needs separate exact human approval.

Action packaging tests and mocked GitHub/OIDC tests are not proof of a successful live workflow or account integration. Inspect the actual dispatched source SHA, workflow, run, and artifacts before using a measurement. Cancel a run directly on GitHub if Perfine reports that remote cancellation could not be confirmed.
