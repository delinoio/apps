# apps/delino-site working instructions

- Follow root and parent instructions.
- This directory owns the Delino web application.
- Read the [project index](../../docs/project-delino-site.md) and [web contract](../../docs/apps-delino-site-web-contract.md).
- Keep frontend development on fixed loopback port 46314 and fail on conflicts.
- Run package-local `pnpm test` for frontend changes. Verify deployment separately.
- Follow the [localization contract](../../docs/apps-delino-site-localization-contract.md) for source catalogs, new languages and local ChatGPT translation commands.
- Namespace marketing catalog keys with the owning product ID (for example, `fixkey.*`). Use `common.*` only for site-wide text shared across products. Preserve translations and source fingerprints when renaming keys.
