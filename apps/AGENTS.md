# apps working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `apps/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- Follow root `AGENTS.md` and project-specific docs before adding or changing app code.

- Keep repository and domain rules in the appropriate `AGENTS.md` files.

- Write all source and comments in English.

- Project-specific scripts and their tests live under the owning app’s `scripts/` directory; shared repository helpers remain in root `scripts/`.

## Owning contracts

- [project-delidev](../docs/project-delidev.md)
- [apps-delidev-desktop-contract](../docs/apps-delidev-desktop-contract.md)
- [project-delitools](../docs/project-delitools.md)
- [apps-delitools-foundation](../docs/apps-delitools-foundation.md)
- [repository-environment-contract](../docs/repository-environment-contract.md)
- [repository-workflow-contract](../docs/repository-workflow-contract.md)
- [project-devbase](../docs/project-devbase.md)
- [apps-devbase-web-contract](../docs/apps-devbase-web-contract.md)
- [project-delino-site](../docs/project-delino-site.md)
- [apps-delino-site-web-contract](../docs/apps-delino-site-web-contract.md)
- [project-delino-docs](../docs/project-delino-docs.md)
- [apps-delino-docs-web-contract](../docs/apps-delino-docs-web-contract.md)

- [Git Tracker web](../docs/apps-git-tracker-web-contract.md)

- Lane ownership: [project](../docs/project-lane.md); [application foundation](../docs/apps-lane-foundation.md).

- `apps/lane-chrome-extension` owns the RealQA Chrome companion and store materials. Read the [RealQA contract](../docs/apps-lane-realqa-contract.md); native installation and messaging integration remain in the existing Lane desktop host.

- `delino-console` ownership: [project](../docs/project-delino-console.md) and [web contract](../docs/apps-delino-console-web-contract.md). Use the shared localization CLI; development uses Infisical and Portless with fixed loopback port `46332` and preview port `46333`.
