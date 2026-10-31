# Project: Delino Console

## Goal

Provide the web application foundation for Delino Console at `https://console.delino.io`, with the same language coverage as the Delino website.

## Project ID

`delino-console`. The npm workspace is `@apps/delino-console`.

## Domain Ownership Map

- `apps/delino-console`: independently built browser application, language registry, catalogs and app-local validation.
- `docs/`: project index and web application contract.

## Domain Contract Documents

- [Web application](apps-delino-console-web-contract.md)
- [Shared Delino localization](apps-delino-site-localization-contract.md)

## Cross-Domain Invariants

The application uses TypeScript, React and Rsbuild instead of the default new-project Go language. Static hosting targets Cloudflare Pages. The production origin is `https://console.delino.io`.

English uses `/`. Other registered languages use `/<language>/`. The registry matches Delino Site: `en`, `ko`, `ja`, `zh-Hans`, `es`, `fr`, `de` and `pt-BR`. Translation generation and offline validation use the shared localization CLI; credentials remain outside the browser and checkout.

This scaffold contains the app identity, a preparation message and language selection. Authentication, dashboard features and API integration require separate owning contracts before implementation.

## Change Policy

Update this index and the web contract together when paths, interfaces or deployment boundaries change. Update instructions only when development procedures, ownership or development rules change. Record validation evidence and unresolved limits in PRs and CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delino Site](project-delino-site.md)
