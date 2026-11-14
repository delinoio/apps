# Delino

Monorepo for Delino services. It contains apps, backend servers, shared packages, Rust crates, Go command tools and protocol schemas.

## License

This repository uses two licenses. The license depends on the directory.

| Path                         | License                                                                                |
| ---------------------------- | -------------------------------------------------------------------------------------- |
| `servers/`                   | [Functional Source License 1.1, ALv2 Future License](servers/LICENSE) (`FSL-1.1-ALv2`) |
| All other repository content | [Apache License 2.0](LICENSE) (`Apache-2.0`)                                           |

The backend services in `servers/` are source-available, not Apache-2.0. You can use, change and redistribute them for any purpose other than a Competing Use, as defined in [servers/LICENSE](servers/LICENSE). Each version becomes available under Apache-2.0 two years after its release.

[LICENSE-FSL](LICENSE-FSL) is a root copy of `servers/LICENSE`. It is there so that the GitHub license summary shows both licenses. Its terms apply only to `servers/`.

Imported skills retain their original licenses and notices. See [NOTICE](NOTICE).

## Setup

Use Node.js 24, pnpm 10.26.2 and the Go toolchain selected by the root `go.mod` (Go 1.26.1 or later). The root `pnpm build`, `pnpm dev`, `pnpm lint` and `pnpm test` tasks run `servers/accounts-api`, whose scripts call `go`.

```sh
pnpm install --frozen-lockfile
pnpm test
pnpm format:check --all
```

## Add a project

Start with [the documentation catalog](docs/README.md) and [repository instructions](AGENTS.md). Create a project index and at least one domain contract before runtime implementation. Add language workspaces and build, test and release workflows when their first owning project is introduced.
