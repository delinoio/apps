# Example client instructions

- Follow root instructions and the protocol contract in `docs/protos-delcore-v1-contract.md`.
- `examples/delcore-client` owns the Go consumer example. It contains no server implementation.
- Run `go test ./...` and `go build ./examples/delcore-client` from the repository root.
- Read access tokens from the environment. Do not print tokens or raw response content.
- `examples/git-tracker-client` owns the Git Tracker Go consumer example. Read its [protocol contract](../docs/protos-git-tracker-v1-contract.md) and run `go build ./examples/git-tracker-client` from the root.
- `examples/devbase-client` and `examples/devbase-client-ts` own the DevBase Go and TypeScript consumer examples. Read the [DevBase protocol contract](../docs/protos-devbase-v1-contract.md), run `go build ./examples/devbase-client` from the root and run the TypeScript example from its package. Read credentials from the environment and never print them.
- `examples/delitools-client` owns the DeliTools Go consumer example. Read the [DeliTools protocol contract](../docs/protos-delitools-v1-contract.md), run `go build ./examples/delitools-client` from the root and read the API key or token from the environment without printing it.
