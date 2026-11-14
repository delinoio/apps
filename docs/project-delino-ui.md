# Project: delino-ui

## Goal

Provide one shared set of React UI components for Delino web apps, so that each app does not rebuild buttons, form fields, switches and dialogs with its own behavior and accessibility rules. The components are built on the headless [UI SDK](https://ui-sdk.dev) from Vercel Labs, which supplies keyboard navigation, focus management, dismissal and ARIA. This package adds the Delino visual style.

## Project ID

`delino-ui`

## Domain Ownership Map

- `packages/delino-ui/`: workspace package `delino-ui` with React components, a shared stylesheet and unit tests.

The project is a library, not a deployable. It has no `apps`, `cmds`, `servers`, `crates` or `protos` paths.

## Domain Contract Documents

- [packages-delino-ui-foundation.md](packages-delino-ui-foundation.md)

## Cross-Domain Invariants

- Component IDs, which are also the package subpath exports:

| Component ID | Export                 | UI SDK parts used                                                               |
| ------------ | ---------------------- | ------------------------------------------------------------------------------- |
| `button`     | `delino-ui/button`     | `Button`                                                                        |
| `text-field` | `delino-ui/text-field` | `Field`, `FieldLabel`, `FieldDescription`, `FieldError`, `Input`                |
| `switch`     | `delino-ui/switch`     | `Field`, `FieldLabel`, `FieldDescription`, `Switch`, `SwitchThumb`              |
| `dialog`     | `delino-ui/dialog`     | `Dialog` and its trigger, backdrop, content, title, description and close parts |
| `styles`     | `delino-ui/styles.css` | None. Design tokens and component styles.                                       |

- Consumers import components only from the exports above. They must not import files under `packages/delino-ui/src/` by path.
- Apps that use the components must import `delino-ui/styles.css` before their own stylesheet.
- Behavior, focus handling and ARIA come from UI SDK. This package must not reimplement them; it only composes UI SDK parts and styles them.
- The package follows the [frontend design rules](../AGENTS.md#frontend-design-rules): dialogs always close with the Escape key, and the single critical input of a form receives focus when the form is shown.
- The package ships TypeScript source and has no build step, so it never writes a `dist` directory. Consumers compile the source with their own Rsbuild or Rspress build.
- Deviation from repository defaults: the primary language is TypeScript, not Go, because the consumers are React web apps.

## Change Policy

- Update [packages-delino-ui-foundation.md](packages-delino-ui-foundation.md) and this index together when exports, component IDs, the UI SDK version or the styling contract change.
- Update the owning app contract when an app starts or stops using the package.
- Update the [documentation catalog](README.md) when contracts are added, renamed or removed.
- Update `packages/AGENTS.md` only when development procedures, directory ownership or development rules change; follow the [instruction-update policy](README.md#instruction-update-policy).
- Record implementation status, validation results and unresolved limits in pull requests, Linear issues and CI logs/artifacts.

## References

- [repository-defaults.md](repository-defaults.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [repository-dependency-security-contract.md](repository-dependency-security-contract.md)
- [project-template.md](project-template.md)
- [UI SDK documentation](https://ui-sdk.dev/docs)
