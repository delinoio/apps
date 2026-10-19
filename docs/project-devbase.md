# Project: DevBase

## Goal

Provide a console for managing the API services that belong to DevBase.

## Project ID

`devbase`. The npm workspace is `devbase`.

## Domain Ownership Map

- `apps/devbase`: independently built web application served at `https://devbase.delino.io`.
- `.github/workflows/release-devbase.yml`: tagged Cloudflare Pages publication. Shared version preparation follows the [workflow contract](repository-workflow-contract.md#project-release-preparation).

## Domain Contract Documents

- [Web application](apps-devbase-web-contract.md)

## Cross-Domain Invariants

DevBase API services run as separate servers. They use delcore for billing; the console does not host these services or own billing. Individual API service names, server paths and protocols must be defined in their own contracts before implementation.

The frontend uses TypeScript, React and Rsbuild instead of the default new-project Go language. Static hosting targets Cloudflare Pages.

## Change Policy

Update this index and the web contract together when paths, interfaces or deployment boundaries change. Follow the [instruction-update policy](README.md#instruction-update-policy). Record validation and unresolved limits in commits, PRs or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delcore](project-delcore.md)
