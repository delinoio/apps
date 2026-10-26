# Project: DeliDev

## Goal

Keep an executable desktop shell with an empty native window. Existing product features, mobile targets, CLI, server, Worker, RPC schemas and API clients are removed.

## Project ID

`delidev`. The npm workspace is `@apps/delidev`; the native executable is `delidev-desktop`.

## Domain Ownership Map

- `apps/delidev`: React/Rsbuild frontend and Tauri/CEF native desktop host.

## Domain Contract Documents

- [Desktop shell](apps-delidev-desktop-contract.md)

## Cross-Domain Invariants

Shared UI primitives are owned by [the UI package](project-ui.md) at `packages/ui`.

The shell has no product backend or API. It does not read, migrate or delete previous DeliDev account data or credentials. Shared repository tooling remains owned by its existing projects.

## Change Policy

Keep the desktop contract and applicable instructions aligned with changes to the shell. Preserve historical validation records; they describe the retired implementation, not current acceptance.
