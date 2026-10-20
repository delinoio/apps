# Git Tracker web

- Follow root and apps instructions and the [project](../../docs/project-git-tracker.md) and [web contract](../../docs/apps-git-tracker-web-contract.md).
- Run `pnpm dev` here on `127.0.0.1:46320`; fail on port conflicts.
- Consume generated bindings from the internal `@delino/protos` workspace. Run `pnpm proto:generate` from the repository root to regenerate them; do not edit generated files manually.
- Run `pnpm test` here and remove generated `dist` output after validation.
