# Shared UI Foundation

## Scope

`packages/ui` owns `@delino/ui`, shared by DevBase, Delino Site, Delino Docs, DeliDev and DeliTools.

## Runtime and Language

React 19 and TypeScript source modules are compiled by the consuming Rsbuild or Rspress application. Node.js 24 runs tooling. Tailwind CSS 4 is processed by the exported PostCSS configuration. No generated package output is required before app development or builds.

## Users and Operators

Application developers compose shared primitives into product screens.

## Interfaces and Contracts

Import components through `@delino/ui/components/<name>` and `cn` through `@delino/ui/lib/utils`. Components are button, badge, card, input, textarea, native-select, label, separator, checkbox, switch and dialog. `NativeSelect` preserves native option, change-event, keyboard and form behavior. `Card` supports `asChild` to preserve consumer section and article semantics. `components.json` routes shadcn additions into this package with the `new-york` style and neutral tokens.

Every app has a workspace dependency and a PostCSS configuration. Styles remain opt-in: import `@delino/ui/styles.css` once in the application entry, or in the Rspress custom theme entry. The stylesheet includes Tailwind Preflight, light/dark semantic tokens and component utilities. Review existing app styles before enabling this reset. Add `.dark` to the document root for dark mode. Apps own theme preference and persistence.

```tsx
import { Button } from "@delino/ui/components/button";
import "@delino/ui/styles.css";

export function SaveButton() {
  return <Button type="submit">Save</Button>;
}
```

For application-specific Tailwind utilities, import the shared stylesheet in an app CSS file and register the app's source directory relative to that file:

```css
@import "@delino/ui/styles.css";
@source "./";
```

The shared stylesheet explicitly scans package components. It does not scan unrelated applications. Consumers can override semantic CSS variables after the shared stylesheet. The package does not import app CSS or persist theme settings.

Dialog uses Radix focus management, supports Escape dismissal and restores trigger focus. Provide a title and description, localized close-button text, and accessible labels. A single critical input must receive focus when its form opens; use `autoFocus` or the dialog's `onOpenAutoFocus`. Apps own validation and localized messages. Follow Toss Design Guidelines when composing screens.

## Storage

No persistence, cache, remote files or credentials.

## Security

Components accept React props and do not render raw HTML or make network requests. Apps own authorization and data validation.

## Logging

Primitives do not log user input. Apps own structured business and failure events.

## Build and Test

Run `pnpm test` in `packages/ui` for type checks, interaction tests and CSS compilation checks. Run `pnpm test` in each affected app. CI validates the shared package in the affected `shared-ui` job and selects all app checks for shared package changes. Builds and jsdom tests do not establish native or browser acceptance.

## Dependencies and Integrations

React and React DOM are peer dependencies. Radix UI provides accessible interaction primitives; Tailwind CSS, class-variance-authority, clsx, tailwind-merge, lucide-react and tw-animate-css provide styling and icons. `@delino/ui/postcss` exports the shared Tailwind PostCSS configuration. All consumers use source exports through bundler module resolution. The `@delino/ui/package.json` export supports build-time dependency and license inventory collection.

Imported shadcn/ui component code retains its MIT license in `packages/ui/LICENSE.shadcn`; repository additions use Apache-2.0. Preserve both licenses in distributions that include the components.

## Change Triggers

Update this contract and the project index for package interface changes. Update consumer contracts when adoption changes product behavior. Update scoped instructions only for ownership or development-rule changes.

## References

- [Project index](project-ui.md)
- [Repository defaults](repository-defaults.md)
- [License contract](repository-license-contract.md)
- [shadcn monorepo setup](https://ui.shadcn.com/docs/monorepo)
- [shadcn Tailwind 4](https://ui.shadcn.com/docs/tailwind-v4)
