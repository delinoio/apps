# CF Clairic

## Goal

Provide a reusable Rust client for Cloudflare Workers AI Clairic, with a self-contained crates.io package.

## Project ID

`cf-clairic`

## Domain Ownership Map

- `crates/cf-clairic`: Rust library, consumer documentation and offline tests.

## Domain Contract Documents

- [Rust client](crates-cf-clairic-client-contract.md)

## Cross-Domain Invariants

The package uses Rust as explicitly required for this client. It has no dependency on application or server packages. It calls the upstream REST endpoint directly, not a repository business RPC. Repository-owned files use Apache-2.0.

## Change Policy

Update the client contract and public crate README when public behavior changes. Update ownership instructions only when ownership or development procedures change. Record validation evidence in PRs and CI.

## References

- [Repository defaults](repository-defaults.md)
- [License contract](repository-license-contract.md)
