// SPDX-License-Identifier: Apache-2.0
import * as path from "node:path";
import { defineConfig } from "@rspress/core";

export default defineConfig({
  root: path.join(__dirname, "docs"),
  // Keep build output in the repository-wide ignored `dist` directory.
  outDir: "dist",
  base: "/",
  lang: "en",
  title: "Delino",
  description: "Delino",
  icon: "/icon.svg",
  logo: "/icon.svg",
  logoText: "Delino",
  // The marketing site has no documentation search; docs.delino.io owns it.
  // This type-valid option still stops Rspress from indexing pages for search,
  // but it does not hide the search UI; see `themeConfig.search` below.
  search: false,
  sitemap: { siteUrl: "https://delino.io" },
  themeConfig: {
    // Documents the intent; the visible search UI is removed by the `Search`
    // override in `theme/index.tsx` until the default theme honours this flag.
    search: false,
    nav: [
      { text: "Docs", link: "https://docs.delino.io" },
      { text: "GitHub", link: "https://github.com/delinoio" },
    ],
  },
});
