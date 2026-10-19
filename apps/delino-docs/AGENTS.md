# apps/delino-docs working instructions

- Follow root and parent instructions.
- This directory owns the public Delino documentation app.
- Read the [project index](../../docs/project-delino-docs.md) and [web contract](../../docs/apps-delino-docs-web-contract.md).
- Keep public content in this app's `docs/`; repository-root `docs/` owns internal contracts.
- Keep frontend development on fixed loopback port 46315 and fail on conflicts.
- Run package-local `pnpm test` for frontend changes. Verify deployment separately.
