import { defineConfig } from "@rspress/core";

export default defineConfig({
  root: "docs",
  outDir: "dist",
  title: "Delino Docs",
  description: "Documentation for Delino services.",
  siteOrigin: "https://docs.delino.io",
  base: "/",
  lang: "en",
  builderConfig: {
    server: { host: "127.0.0.1", port: 46315, strictPort: true },
  },
  themeConfig: {
    nav: [
      { text: "Guide", link: "/guide/getting-started" },
      { text: "Services", link: "/services/" },
      { text: "Delino", link: "https://delino.io" },
    ],
    sidebar: {
      "/guide/": [
        {
          text: "Guide",
          items: [{ text: "Getting started", link: "/guide/getting-started" }],
        },
      ],
      "/services/": [
        { text: "Services", items: [{ text: "Overview", link: "/services/" }] },
      ],
    },
  },
});
