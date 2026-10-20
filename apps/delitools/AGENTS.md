# apps/delitools working instructions

- Follow root and parent instructions.
- This directory owns the DeliTools desktop, mobile and web application.
- Read the [project index](../../docs/project-delitools.md) and [application contract](../../docs/apps-delitools-foundation.md).
- Keep frontend development on fixed loopback port 46312 and fail on conflicts.
- Run package-local `pnpm test` for frontend changes and root `cargo test` for Rust changes. Verify native launch and store acceptance separately.
- Keep mobile release workflow fixtures in `scripts/mobile-workflow.test.mjs` with the app-owned mobile helpers.
- Generate mobile projects through the package mobile init scripts. Keep generated native projects and signing material untracked.
