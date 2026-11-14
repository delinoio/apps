# Delino

Monorepo for Delino services. It contains apps, backend servers, shared packages, Rust crates, Go command tools and protocol schemas.

## Setup

Use Node.js 24 and pnpm 10.26.2.

```sh
pnpm install --frozen-lockfile
pnpm test
pnpm format:check --all
```

## Add a project

Start with [the documentation catalog](docs/README.md) and [repository instructions](AGENTS.md). Create a project index and at least one domain contract before runtime implementation. Add language workspaces and build, test and release workflows when their first owning project is introduced.

## License

Repository-owned content is licensed under [Apache-2.0](LICENSE), except backend services in `servers/`. Those use the [Functional Source License 1.1, ALv2 Future License](servers/LICENSE): each version becomes available under Apache-2.0 two years after its release. Imported skills retain their original licenses and notices.
