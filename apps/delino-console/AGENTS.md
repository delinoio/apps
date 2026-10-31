# Delino Console working instructions

- Follow root and `apps/AGENTS.md` instructions.
- Read [the project index](../../docs/project-delino-console.md) and [web contract](../../docs/apps-delino-console-web-contract.md) before changing this app.
- Keep app-specific validation under `scripts/`. Shared translation tooling remains in root `scripts/localization/`.
- Use the shared localization CLI and [localization contract](../../docs/apps-delino-site-localization-contract.md). Keep runtime text in catalogs and translation credentials outside the app.
- Run app-local `pnpm test` after frontend changes and remove generated `dist/` before finishing.
