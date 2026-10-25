# Lynx UI foundation

## Scope

`packages/lynx-ui` owns the Apache-2.0 `@delino/lynx-ui` workspace. Read the
[project index](project-lynx-ui.md). The package supports ReactLynx consumers on
Lynx for Web, mobile Lynx hosts and desktop Clay hosts. Native hosts must register
the input element and provide keyboard/focus capabilities for keyboard users.
Linux desktop is not an upstream native target; a web host can consume web output.

## Runtime and Language

Use TypeScript and ReactLynx 0.129 with Lynx 4.x hosts (Clay 4.0 or newer for
desktop focus). Build with the pinned Rspeedy and ReactLynx plugin under Node.js 24. Source exports require the consumer's ReactLynx compiler; they are not
precompiled React DOM modules. Web examples pin `@lynx-js/web-core` 0.26.5.

## Users and Operators

Application developers compose reusable primitives into product screens. Host
maintainers register native input elements and validate platform capabilities.

## Interfaces and Contracts

Export Button, Text, Stack, Card, Badge, Separator, Field, Input, Checkbox, Switch,
Tabs, Alert, Progress and Dialog, with explicit enum variants. All visible labels,
validation messages, loading labels and accessibility values come from the app.
Import `@delino/lynx-ui/styles.css` once and wrap content in `UIProvider`.
Light and dark themes use semantic neutral colors, red errors, 10px corners,
16/24px spacing and at least 44px interactive targets. Apps own theme selection
and persistence. Tokens can be overridden without changing component source.

Stack supports wrapping rows and columns. Cards and dialogs constrain their width
to the available viewport. A 390px mobile gallery uses a single column; wider
hosts can compose rows. No component depends on hover for its action.

Controlled values and callbacks are the state contract for inputs, toggles, tabs
and dialogs. Buttons suppress actions while disabled or loading. Tabs skip
disabled items during keyboard navigation. Input supports imperative focus and
automatic focus when a single critical field is shown. Dialog supports Escape,
initial focus, focus containment and return focus. Nested dialogs are outside
the initial contract. Apps own empty, error and permission decisions; Alert and
the layout primitives present their supplied content without network requests.

### Runtime boundaries

Use Lynx elements and ReactLynx hooks, not React DOM or Radix components.
Keep platform-specific focus and accessibility integration explicit. A native
bundle alone does not establish acceptance on iOS, Android, macOS or Windows.
Do not claim upstream headless `lynx-ui` provides complete web/desktop parity.
The component gallery must compile both `.lynx.bundle` and `.web.bundle` output.

### Consumer setup

Add `@delino/lynx-ui: workspace:*` to a Lynx app's dependencies. Use full-package
imports or component subpaths such as `@delino/lynx-ui/components/button`.

```tsx
import { useState } from "@lynx-js/react";
import { Field, Input, UIProvider } from "@delino/lynx-ui";
import "@delino/lynx-ui/styles.css";

export function NameForm() {
  const [name, setName] = useState("");
  return (
    <UIProvider>
      <Field label="Name">
        <Input
          value={name}
          onValueChange={setName}
          accessibilityLabel="Name"
          autoFocus
        />
      </Field>
    </UIProvider>
  );
}
```

Use `Theme.Light` or `Theme.Dark`, `Variant` for buttons, `Tone` for badges and
alerts, `Direction` for stacks, and `TextVariant` for text. Components accept
localized labels; strings in the example are samples. `Field` lays out labels
and errors; pass an accessible name and any error context to its `Input`.
Set `loadingLabel` when a loading button needs different visible text. Progress
accepts a percentage from 0 to 100 and clamps out-of-range or non-finite values.
Tab item values must be unique; an invalid or disabled selected value renders no
panel. Callers control selection and supply a valid enabled value. On web, the
selected panel has the `tabpanel` role and uses its tab label as its accessible
name. Changing or clearing that label updates the panel's accessible name.

`Button`, `Input`, `Checkbox` and `Switch` accept `focusRef`, a ReactLynx ref to
`FocusHandle` (`focus()`). Callback refs receive `null` on replacement and unmount,
followed by their returned cleanup when present. Replacing a callback ref alone
does not restart `Input` automatic focus. Dialog requires `returnFocusRef` pointing
to its trigger; set `initialFocusRef` to the sole critical input when present.
Render Dialog near the root, outside clipped or transformed containers. Use library controls inside
Dialog so its native focus scope can register them. Nested dialogs and arbitrary
third-party focusable children require a separate host integration.

### Web host bridge

In the browser host, import `installWebSupport` from `@delino/lynx-ui/web`.
Install it on the view's open shadow root before setting the bundle URL:

```ts
import "@lynx-js/web-core/client";
import { installWebSupport } from "@delino/lynx-ui/web";

await customElements.whenDefined("lynx-view");
const view = document.createElement("lynx-view");
document.body.append(view);
const dispose = installWebSupport(
  view.shadowRoot ?? view.attachShadow({ mode: "open" }),
);
view.setAttribute("url", "/gallery.web.bundle");
// Call dispose() when this host is removed permanently.
```

The bridge supplies DOM roles, checked/disabled/selected states, keyboard
activation, roving tab stops, modal background isolation and Clay-compatible
focus methods. It handles shadow-DOM keyboard retargeting in the pinned runtime.
Install once per view; retain the bridge through persisted `pagehide`/`pageshow`
cycles, and dispose on permanent unmount or non-persisted `pagehide`. It does not
patch global prototypes. This entry belongs in the browser host, never in a
background-thread bundle. Native hosts use Lynx key events and focus methods
directly, with accessibility-focus fallback when Clay `setFocus` is unavailable.

Semantic CSS variables use the `--lui-` prefix. Override them on the provider's
style (for example `--lui-primary`, `--lui-on-primary`, `--lui-focus`) while
preserving readable contrast. Theme preference and persistence belong to the app.

## Storage

No persistence or cache. Gallery artifacts under `dist` are generated and ignored.

## Security

No network, credential access or raw HTML. Applications own authorization and
data validation. The browser bridge operates only inside the supplied view.

## Logging

Report failed host methods through `UIProvider.onHostError`, or the default
structured console event. Never include entered text in diagnostics. Applications
own business events and telemetry destinations.

## Build and Test

Run `pnpm test` in `packages/lynx-ui`: type checks, ReactLynx interaction tests
and gallery builds. Cover loading/disabled buttons, controlled input changes,
toggle callbacks, disabled tab navigation, dialog dismissal/focus and both themes.
Record browser viewport checks separately from tests and native acceptance.
Remove generated `dist` after validation. Shared UI CI runs this package's tests.

Run `pnpm build` in the package to generate the gallery, then serve `dist` with a
static HTTP server for browser inspection. `dist/index.html` uses only local
runtime assets and includes license notices. `examples/gallery.tsx` exercises
all components. The package has no application development server or deployment.
Check 390px mobile and desktop widths, light/dark themes, keyboard activation,
tab wrapping, disabled states, edited values, initial modal focus, repeated Tab
and Shift+Tab containment, Escape dismissal and trigger focus restoration.
Repeat native acceptance on Android, iOS, macOS and Windows in the adopting app;
web rendering does not establish native keyboard or screen-reader behavior.

## Dependencies and Integrations

ReactLynx and Lynx types are peers. No React DOM, Radix or Tailwind runtime is
required. The pinned Rspeedy toolchain, ReactLynx testing library and Rstest own
build/test integration. The official headless Lynx UI library is not required.
Native host installation and application adoption remain with each consuming app.

## Change Triggers

Update this contract and the project index for component, platform or build
interface changes. Follow the repository instruction-update policy for ownership
and development rules. Record actual validation outcomes in PRs and CI evidence.

## References

- [Project index](project-lynx-ui.md)
- [Repository defaults](repository-defaults.md)

- [ReactLynx testing](https://lynxjs.org/react/reactlynx-testing-library.html)
- [Host integration](https://lynxjs.org/guide/start/integrate-with-existing-apps.html)
- [Lynx UI platform scope](https://lynxjs.org/next/blog/lynx-ui)
