// SPDX-License-Identifier: Apache-2.0
import "./index.css";

export * from "@rspress/core/theme-original";

// Workaround: the marketing site has no documentation search (docs.delino.io
// owns it), but the Rspress 2.1.3 default theme `Nav` mounts `<Search />`
// unconditionally and ignores `themeConfig.search`. Exporting `Search` here
// takes precedence over the re-exported theme-original `Search`, so the nav
// renders no search button and no Ctrl/Cmd+K handler is registered.
// Scope: delino-site only; delino-docs keeps the built-in search.
// Remove when the pinned `@rspress/core` honours `themeConfig.search: false`
// in the default `Nav`.
export function Search() {
  return null;
}
