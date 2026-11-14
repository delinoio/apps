# packages-delino-ui foundation contract

## Scope

This contract owns the [`delino-ui`](project-delino-ui.md) package at `packages/delino-ui/`:

- `packages/delino-ui/package.json`: package manifest, subpath exports and pinned dependencies.
- `packages/delino-ui/src/`: component source, the shared stylesheet `styles.css` and unit tests.
- `packages/delino-ui/rstest.config.ts`: unit test configuration.
- `packages/delino-ui/tsconfig.json`: TypeScript configuration for the type check.

## Runtime and Language

- Language: TypeScript with React 19 (`"jsx": "react-jsx"`).
- Component foundation: UI SDK, `@ui-sdk/core` and `@ui-sdk/react`, pinned to an exact version. UI SDK is headless: it renders unstyled parts with `data-slot` and state attributes such as `data-open`, `data-checked` and `data-pending`.
- Test runner: Rstest `@rstest/core` with the `happy-dom` environment and Testing Library.
- Runtime: the browser of the consuming app. Node.js from root `.nvmrc` runs the type check and tests.
- Distribution: TypeScript source through `package.json` `exports`. The package has no build script and writes no `dist` directory. Consumers compile it with Rsbuild or Rspress, which transpile workspace sources outside `node_modules`.
- Deviation from repository defaults: TypeScript instead of Go, because the package is consumed by React web apps.

## Users and Operators

- Maintainers of Delino web apps under `apps/` who build user interfaces.
- Users of those apps, who get consistent keyboard, focus and screen reader behavior.

## Interfaces and Contracts

Each export is a stable interface. Exported enums are the stable identifiers for variants and sizes.

- `delino-ui/button`
  - `Button`: a styled UI SDK `Button`. It accepts all UI SDK `Button` props. A Promise returned from `onClick` keeps the button pending (`data-pending`, `aria-busy`) until it settles, and further presses do nothing.
  - `ButtonVariant`: `Primary` (`primary`), `Secondary` (`secondary`), `Danger` (`danger`). The default is `Primary`.
  - `ButtonSize`: `Small` (`small`), `Medium` (`medium`), `Large` (`large`). The default is `Medium`.
  - `fullWidth`: stretches the button to the width of its container.
- `delino-ui/text-field`
  - `TextField`: a labeled text input. Props: `label` (required), `description`, `error`, and all UI SDK `Input` props such as `value`, `defaultValue`, `onValueChange`, `name`, `type`, `required` and `autoFocus`. The field connects the label, description and error to the input.
  - A non-empty `error` marks the field invalid and shows the message. Without `error`, the field shows the browser's validation message after a failed submit.
  - When the input is the single critical input of a form, the consumer must pass `autoFocus`.
- `delino-ui/switch`
  - `Switch`: a labeled on/off setting. Props: `label` (required), `description`, and all UI SDK `Switch` props such as `checked`, `defaultChecked`, `onCheckedChange`, `name` and `disabled`. It renders with role `switch` and a hidden native input for forms.
- `delino-ui/dialog`
  - `Dialog`: the modal dialog root. It accepts the UI SDK `Dialog` props except `dismissible`, which is removed so that every dialog closes with the Escape key.
  - `DialogTrigger` and `DialogClose`: buttons styled like `Button`, with `variant` and `size`. The trigger defaults to `Primary`, the close button to `Secondary`. A Promise returned from the `DialogClose` `action` keeps the dialog open until it settles.
  - `DialogContent`: the dialog panel. It also renders the backdrop.
  - `DialogTitle` (level 2 heading, names the dialog), `DialogDescription` and `DialogActions` (button row).
  - `createDialogController` and the `DialogController` type are re-exported from UI SDK for dialogs that open from outside a trigger.
- `delino-ui/styles.css`
  - Declares the cascade layer order `ui-sdk, delino-ui`. UI SDK rules are in `ui-sdk`; package rules are in `delino-ui`. Unlayered app styles override both.
  - Includes the UI SDK rule that hides closed parts with `hidden`, because statically rendered pages do not get it from UI SDK before hydration.
  - Defines design tokens as `--dui-*` custom properties on `:root`, with dark values under `prefers-color-scheme: dark`. Text tokens meet a 4.5:1 contrast ratio and control borders and focus rings meet 3:1 (WCAG 2.1 SC 1.4.3 and SC 1.4.11).
  - Uses `dui-` class names. Every component accepts `className`; the package class comes first.

## Storage

The package has no storage.

## Security

- The package contains no secrets, network calls or browser storage.
- UI SDK adds CSS at runtime. A Content Security Policy without `'unsafe-inline'` in `style-src` must give UI SDK a nonce, as the [UI SDK styling guide](https://ui-sdk.dev/docs/react/styling-and-animation) describes. Current Delino apps set no Content Security Policy.

## Logging

The package does not log. UI SDK components accept a `debug` prop that logs state changes in development builds only; do not leave it in committed code.

## Build and Test

Run from the repository root:

- `pnpm install --frozen-lockfile`
- `pnpm exec turbo run typecheck --filter=delino-ui`: TypeScript type check.
- `pnpm exec turbo run test --filter=delino-ui`: Rstest unit tests for rendering, props, keyboard and dismissal behavior.

Root `pnpm test` and `pnpm typecheck` include the package. The package has no `build` or `dev` script. CI does not run its unit tests yet; add them to the workflow and its planner when package checks enter CI.

## Dependencies and Integrations

- `@ui-sdk/core` and `@ui-sdk/react` (Apache-2.0), pinned to the same exact version in `dependencies`. UI SDK is in public beta and its APIs can change before general availability, so upgrade both packages together, read their changelogs and run the unit tests.
- `react` and `react-dom` 19 as peer dependencies.
- `@rstest/core`, `happy-dom`, `@testing-library/react`, `@testing-library/dom`, `typescript` and the React type packages as development dependencies.
- Consumers: no app uses the package yet. An app that starts using it adds `"delino-ui": "workspace:*"` to its dependencies and records the use in its own contract.

## Change Triggers

- Update [project-delino-ui.md](project-delino-ui.md) when exports, component IDs or the styling contract change.
- Update the contracts of consuming apps when an export changes in a way that breaks them.
- Update [repository-workflow-contract.md](repository-workflow-contract.md) when the package enters CI.
- Update `packages/AGENTS.md` only under the [instruction-update policy](README.md#instruction-update-policy).

## References

- [project-delino-ui.md](project-delino-ui.md)
- [repository-defaults.md](repository-defaults.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [repository-dependency-security-contract.md](repository-dependency-security-contract.md)
- [UI SDK documentation](https://ui-sdk.dev/docs)
- [UI SDK source](https://github.com/vercel-labs/ui)
- [Rstest documentation](https://rstest.rs/)
