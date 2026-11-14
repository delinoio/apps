# servers working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `servers/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Licensing

- Source under `servers/` is licensed under FSL-1.1-ALv2 (`servers/LICENSE`), not the repository-wide Apache-2.0 license. Each version becomes available under Apache-2.0 on the second anniversary of its release.
- Use `SPDX-License-Identifier: FSL-1.1-ALv2` in server source headers and `FSL-1.1-ALv2` in server package manifests that declare a license.
- Root `LICENSE-FSL` is a scoped copy of `servers/LICENSE`. When `servers/LICENSE` changes, update the text after the `LICENSE-FSL` scope statement in the same change.
- Code shared with apps or packages belongs outside `servers/` under Apache-2.0. Do not move server code into Apache-2.0 directories without an explicit relicensing decision. Follow `docs/repository-license-contract.md`.

## Development and validation

- Follow root `AGENTS.md` and the owning project/domain contracts.

- Write Go code and comments in English; use `log/slog` structured logging and never log secrets or sensitive payloads.
- Server Go code uses the root Go module `github.com/delinoio/mono`; do not add a `go.mod` under `servers/`. Each server has a workspace `package.json` whose scripts wrap the Go toolchain. Run `gofmt`, `go vet ./...` and `go test ./...` from the repository root before finishing.

## Owning contracts

- [Repository defaults](../docs/repository-defaults.md)
- [Repository workflow](../docs/repository-workflow-contract.md)
- [Authentication](../docs/repository-auth-contract.md)
- [Billing currency and precision](../docs/repository-billing-contract.md)
- [Repository licensing](../docs/repository-license-contract.md)
- `servers/accounts-api/`: [project index](../docs/project-delino-accounts.md) and [API server foundation contract](../docs/servers-accounts-api-foundation.md)
- Register each new project and its domain contracts in [the documentation catalog](../docs/README.md) before implementation.
