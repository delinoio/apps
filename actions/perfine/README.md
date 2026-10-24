# Perfine Action

Use an exact verified release commit from `delinoio/perfine-action`. Releases use `vX.Y.Z`; there is no moving `v1` tag. `perfine init --github --action-commit <verified-commit>` creates a manual workflow. Provision workload tools and digest-pinned Docker images before running. The Action supports Linux x86_64 runners.

Measurement uses `command: measure`, a UUID v7 `experiment` and the full v1 YAML in `config: |`. Checkout the source at the declared commit with persisted credentials disabled. Declare limits, workload inputs, correctness checks, runtime conditions and performance policy in that YAML. The output `config-sha256` identifies its exact bytes. Measurement does not publish candidates or upload evidence.

Cloud AI optimization is unavailable: current adapters support local authentication files, not the official unattended cloud credential routes. Do not export a cached local login. Use the local CLI with supported local authentication. No AI secret is an Action input or a Perfine server setting.

Evidence upload is a separate job on a fresh runner. Give only that job `id-token: write`; do not give it AI or publication secrets or run candidate code there. Download only explicitly screened evidence from the measurement job. Use `command: upload`, `experiment`, `envelope`, `artifacts`, `space`, `connection`, and a stable `request-id`. The envelope contains the v1 Connect `specification` and `manifest`; `artifacts` contains regular files named by SHA-256. The Action binds OIDC to the current repository ID, run, attempt, source and workflow SHA. The service verifies the signature and current authorization. The configured endpoint is `https://perfine.api.delino.io`. Source-bearing raw artifacts remain local; this upload path rejects them.

An upload failure retains local evidence. Inspect the failure, then resend with the same request ID and unchanged evidence. An incomplete upload never establishes a hosted pass. Publication is a separate explicit CLI approval flow; this Action cannot publish or merge. If cancellation cannot stop a hosted run, cancel it directly in GitHub Actions and inspect retained partial evidence.
