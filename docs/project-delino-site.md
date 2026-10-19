# Project: Delino

## Goal

Provide the Delino marketing website.

## Project ID

`delino-site`. The npm workspace is `delino-site`.

## Domain Ownership Map

- `apps/delino-site`: independently built web application served at `https://delino.io`.
- `.github/workflows/release-delino-site.yml`: tagged Cloudflare Pages publication. Shared version preparation follows the [workflow contract](repository-workflow-contract.md#project-release-preparation).

## Domain Contract Documents

- [Web application](apps-delino-site-web-contract.md)

## Cross-Domain Invariants

This project owns the marketing website, including the FixKey product landing page at `/products/fixkey`. Product behavior remains owned by the [FixKey project](project-fixkey.md). DevBase and DeliTools remain separate applications with their own domains and contracts.

The frontend uses TypeScript, React and Rsbuild instead of the default new-project Go language. Static hosting targets Cloudflare Pages.

## Change Policy

Update this index and the web contract together when paths, interfaces or deployment boundaries change. Follow the [instruction-update policy](README.md#instruction-update-policy). Record validation and unresolved limits in commits, PRs or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delcore](project-delcore.md)
