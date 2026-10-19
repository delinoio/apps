# Repository boundary contract

## Ownership

`https://github.com/delinoio/servers` owns server runtimes, background workers, migrations, server configuration examples and OCI packaging. `https://github.com/delinoio/apps` owns web, desktop and mobile clients, protobuf source, generated Go messages and Connect bindings, and consumer examples. Licenses and third-party notices remain with their sources.

The Delcore protocol is `protos/delcore/v1` in apps. Its Go import path is `github.com/delinoio/apps/protos/delcore/v1`, with Connect bindings in `delcorev1connect`. Servers imports the apps Go module at an immutable commit through `go.mod` and `go.sum`. Do not vendor, duplicate or generate protocol source in servers.

## Changes across repositories

Prepare the separate apps checkout from servers with `pnpm client:setup`. The default location is `../apps`; `DELINO_APPS_DIR` overrides it. New clones start at the immutable apps commit from the server dependency pin. Create a feature branch in that checkout before committing client edits. Existing checkouts must have the expected origin and Go module. Setup does not reset branches, discard edits or pull updates. Choose the required apps branch explicitly before development.

The ignored server-local `go.work` links the servers and apps modules for local development. Setup refuses to overwrite a different existing workspace. Do not commit workspace files, absolute paths or local `replace` directives. Run app commands from the owning app and schema commands from the apps root. Follow each repository's root and scoped instructions.

For protocol changes, preserve field numbers, enum values and wire behavior. Run apps protocol lint, generation, freshness, FILE breaking checks and Go tests. Publish the apps commit on a branch before pinning it in servers with `GOWORK=off go get github.com/delinoio/apps@<commit>`, then run `GOWORK=off go mod tidy`. A local workspace can mask a missing or stale pin; validate servers with `GOWORK=off go vet ./...`, `GOWORK=off go build ./servers/delcore-api/cmd/...` and `GOWORK=off go test -race ./...` before delivery. OCI builds use only the committed module dependency.

Keep client and server changes in separate commits and branches. Record both revisions, commands, results and unperformed platform/account checks in review or CI records. Server CI validates its pinned dependency; apps CI owns protocol generation and breaking validation. A protocol change requires server integration validation before adopting its pin. No release or provider credentials move between repositories.
