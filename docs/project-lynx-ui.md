# Project: Lynx UI

## Goal

Provide reusable ReactLynx components for web,
Android/iOS, and macOS/Windows hosts.

## Project ID

`lynx-ui`. The npm workspace is `@packages/lynx-ui`.

## Domain Ownership Map

- `packages/lynx-ui/src`: components, theme tokens and interaction utilities.
- `packages/lynx-ui/examples`: a component gallery compiled for Lynx and web.
- `packages/lynx-ui/tests`: component behavior and compatibility fixtures.
- Consuming apps own native host integration, navigation, localization and data.

## Domain Contract Documents

- [Lynx UI foundation](packages-lynx-ui-contract.md)

## Cross-Domain Invariants

Read [Lynx UI foundation](packages-lynx-ui-contract.md). TypeScript and ReactLynx
are the language/runtime exception because these components render Lynx UI.
Source exports are compiled by the consumer's ReactLynx build plugin. This package
has no service, storage, deployment or React DOM dependency. Existing React DOM
applications retain `@packages/ui`; adopting Lynx is a separate application change.

## Change Policy

Update the foundation contract with public API or host requirement changes.
Record validation and unperformed native acceptance in PR and CI evidence.

## References

- [Repository defaults](repository-defaults.md)
- [React DOM UI](project-ui.md)
