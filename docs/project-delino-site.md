# Project: Delino

## Goal

Provide the Delino marketing website.

## Project ID

`delino-site`. The npm workspace is `delino-site`.

## Domain Ownership Map

- `apps/delino-site`: independently built web application served at `https://delino.io`.
- `.github/workflows/release-delino.yml`: shared `delino@vMAJOR.MINOR.PATCH` Cloudflare Pages publication with `delino-docs`. Shared version preparation follows the [workflow contract](repository-workflow-contract.md#project-release-preparation).

## Domain Contract Documents

- [Web application](apps-delino-site-web-contract.md)
- [Localization and translation tooling](apps-delino-site-localization-contract.md)

## Cross-Domain Invariants

This project owns the marketing website. DevBase and DeliTools remain separate applications with their own domains and contracts.

The frontend uses TypeScript, React and Rsbuild instead of the default new-project Go language. Static hosting targets Cloudflare Pages.

`i18n/` owns language registration, text catalogs and translation fingerprints. Shared local generation in `scripts/localization/` also serves Delino Docs. English uses `/`; Korean uses `/ko/`. Other registered languages use the same prefixed route convention.

## Change Policy

Update this index and the web contract together when paths, interfaces or deployment boundaries change. Follow the [instruction-update policy](README.md#instruction-update-policy). Record validation and unresolved limits in commits, PRs or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delcore](project-delcore.md)
