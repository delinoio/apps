# Delino

Open-source monorepo for Delino services. It contains apps, shared packages, Rust crates, Go command tools and protocol schemas.

Backend service source is proprietary. It lives in the private `delinoio/servers` repository, which is mounted at `servers/` as a Git submodule. Everything else works without it.

## Setup

Use Node.js 24 and pnpm 10.26.2.

```sh
pnpm install --frozen-lockfile
pnpm test
pnpm format:check --all
```

Maintainers with access to the private servers can initialize the submodule:

```sh
git submodule update --init servers
```

## Add a project

Start with [the documentation catalog](docs/README.md) and [repository instructions](AGENTS.md). Create a project index and at least one domain contract before runtime implementation. Add language workspaces and build, test and release workflows when their first owning project is introduced.

## License

Repository-owned content is licensed under [Apache-2.0](LICENSE). Imported skills retain their original licenses and notices. The private `servers/` submodule is proprietary and is not covered by this license.
