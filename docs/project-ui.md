# Project: Shared UI

## Goal

Provide reusable shadcn/ui primitives and theme tokens for every application.

## Project ID

`ui`. The npm workspace is `@delino/ui`.

## Domain Ownership Map

- `packages/ui`: React components, styling, PostCSS configuration and component tests.
- `apps/*`: consumers that own screens, localization, navigation and product behavior.

## Domain Contract Documents

- [Shared UI foundation](packages-ui-foundation.md)

## Cross-Domain Invariants

TypeScript and React replace the default Go language because this package renders UI in existing React applications. The package has no deployment, native bindings, backend or application state. All remaining apps depend on the same workspace package. Delino Site forms, calls to action and cards use shared primitives. Empty application roots and Rspress-owned theme controls retain their existing rendering. Apps retain ownership of screen composition and behavior.

## Change Policy

Keep the package contract and consumer contracts aligned with interface changes. Follow the [instruction-update policy](README.md#instruction-update-policy). Record validation in commits and CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
