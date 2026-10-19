# Example client instructions

- Follow root instructions and the protocol contract in `docs/protos-delcore-v1-contract.md`.
- `examples/delcore-client` owns the Go consumer example. It contains no server implementation.
- Run `go test ./...` and `go build ./examples/delcore-client` from the repository root.
- Read access tokens from the environment. Do not print tokens or raw response content.
