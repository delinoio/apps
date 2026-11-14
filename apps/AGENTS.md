# apps working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `apps/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- Follow root `AGENTS.md` and project-specific docs before adding or changing app code.

- Keep repository and domain rules in the appropriate `AGENTS.md` files.

- Write all source and comments in English.

## Owning contracts

- [Repository defaults](../docs/repository-defaults.md)
- [Repository workflow](../docs/repository-workflow-contract.md)
- [Authentication](../docs/repository-auth-contract.md)
- [Billing currency and precision](../docs/repository-billing-contract.md)
- `apps/delino-site/`: [project index](../docs/project-delino-site.md) and [foundation contract](../docs/apps-delino-site-foundation.md)
- `apps/delino-docs/`: [project index](../docs/project-delino-docs.md) and [foundation contract](../docs/apps-delino-docs-foundation.md)
- Register each new project and its domain contracts in [the documentation catalog](../docs/README.md) before implementation.
