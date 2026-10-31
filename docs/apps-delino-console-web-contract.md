# Delino Console web contract

## Scope

`apps/delino-console` owns the Delino Console web scaffold at `https://console.delino.io`.

## Runtime and Language

Use Node.js 24, pnpm, TypeScript, React and Rsbuild. The browser application is a documented exception to the default Go language. Static hosting targets Cloudflare Pages, with `dist/` as generated output.

## Users and Operators

Visitors can view the preparation screen and select a language. Maintainers own translation review, build validation and hosting configuration.

## Interfaces and Contracts

The single screen contains the `delino console` wordmark, `Delino Console` heading, a localized preparation message and a labeled native language selector. Use the approved minimal layout: white background, dark text, subtle header border and generous spacing. At narrow viewports the header wraps and content padding decreases. Support widths of 320 CSS pixels and above.

English uses `/`; targets use `/ko/`, `/ja/`, `/zh-Hans/`, `/es/`, `/fr/`, `/de/` and `/pt-BR/`. Each route has its own generated HTML document, localized description and HTML language tag, canonical URL at the production origin and alternate language links. Language selection navigates to the selected root route, retaining query parameters and fragments. The registry drives entries, catalogs and options without runtime language switch statements.

All visible prose, accessibility labels, metadata descriptions and recovery messages belong to catalogs. Keep the Delino Console product name unchanged. Load only the selected catalog at runtime. Render localized loading and no-JavaScript guidance in the initial HTML. If catalog loading fails, show a localized recovery panel and reload control without requiring the failed chunk.

Use semantic header/main elements, a skip link and a native select with a visible label. Preserve visible keyboard focus, keyboard language selection and readable contrast. The scaffold has no forms, dialogs, private data, authentication, dashboard, API calls or permission-dependent states.

## Storage

No persisted entities, browser storage or uploads are introduced. Language selection is represented by the URL. Catalogs and source fingerprints are committed under `i18n/`.

## Security

The browser receives only public static content. Translation credentials remain in the shared local CLI. Builds and tests require no Infisical or ChatGPT authentication. Hosting account configuration, DNS, TLS and live custom-domain acceptance are operator responsibilities; this scaffold does not provision them.

## Logging

Catalog failures emit a structured `locale_load_failed` console event with the language identifier. Exclude credentials, URL parameters and user state.

## Build and Test

Run `pnpm install` from the worktree root. App-local `pnpm dev` injects Infisical development variables through the root project configuration, then registers `delino-console` with Portless at `https://delino-console.localhost`. Rsbuild binds `127.0.0.1:46332` and fails on conflicts. Preview uses `127.0.0.1:46333`. Root `pnpm dev` discovers the workspace through Turborepo.

App-local `pnpm test` checks translation freshness, TypeScript, production builds and generated language routes, localized recovery content, metadata and referenced assets. `pnpm i18n:check` and `pnpm i18n:translate` participate in the root localization commands. Translation text requires meaning review in addition to structural checks.

The affected Ubuntu CI job runs the app test and preserves validation evidence. App, owning contract and shared localization changes select it. Manual CI remains affected-only. Remove generated `dist/` after validation. Browser interaction, Infisical/Portless startup and live-domain acceptance are separate from build checks.

## Dependencies and Integrations

React and Rsbuild use repository-pinned versions. Shared local translation tooling is owned by `scripts/localization/` under the [localization contract](apps-delino-site-localization-contract.md). The scaffold has no backend dependency. Production upload automation is outside this scaffold; configure the Cloudflare Pages custom domain separately before claiming deployment acceptance.

## Change Triggers

Update this contract and the [project index](project-delino-console.md) when routes, paths, language coverage, ports or hosting boundaries change. Update the shared localization contract when translation rules change. Keep ownership and development procedure instructions aligned under the repository instruction-update policy.

## References

- [Project index](project-delino-console.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Environment ownership](repository-environment-contract.md)
