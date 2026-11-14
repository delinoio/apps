// SPDX-License-Identifier: Apache-2.0
import * as path from "node:path";
import { defineConfig } from "@rspress/core";

export default defineConfig({
  root: path.join(__dirname, "docs"),
  // Keep build output in the repository-wide ignored `dist` directory.
  outDir: "dist",
  base: "/",
  lang: "en",
  title: "Delino Docs",
  description: "Delino documentation",
  icon: "/icon.svg",
  logo: "/icon.svg",
  logoText: "Delino Docs",
  sitemap: { siteUrl: "https://docs.delino.io" },
  themeConfig: {
    nav: [
      { text: "Guide", link: "/guide/introduction", activeMatch: "/guide/" },
      { text: "delino.io", link: "https://delino.io" },
    ],
    socialLinks: [
      { icon: "github", mode: "link", content: "https://github.com/delinoio" },
    ],
  },
});
