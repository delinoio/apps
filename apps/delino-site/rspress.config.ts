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
  search: false,
  sitemap: { siteUrl: "https://delino.io" },
  themeConfig: {
    nav: [
      { text: "Docs", link: "https://docs.delino.io" },
      { text: "GitHub", link: "https://github.com/delinoio" },
    ],
  },
});
