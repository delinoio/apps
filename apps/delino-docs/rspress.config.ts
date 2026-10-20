import { defineConfig } from "@rspress/core";

export default defineConfig({
  root: "docs",
  outDir: "dist",
  title: "Delino Docs",
  description: "Documentation for Delino services.",
  siteOrigin: "https://docs.delino.io",
  base: "/",
  lang: "en",
  locales: [
    {
      lang: "en",
      label: "English",
      description: "Documentation for Delino services.",
    },
    { lang: "ko", label: "한국어", description: "Delino 서비스 문서." },
  ],
  builderConfig: {
    server: { host: "127.0.0.1", port: 46315, strictPort: true },
  },
  themeConfig: {
    localeRedirect: "never",
    locales: [
      {
        lang: "ko",
        label: "한국어",
        nav: [
          { text: "가이드", link: "/ko/guide/getting-started" },
          { text: "서비스", link: "/ko/services/" },
          { text: "Delino", link: "https://delino.io/?lang=ko" },
        ],
        sidebar: {
          "/ko/guide/": [
            {
              text: "가이드",
              items: [{ text: "시작하기", link: "/ko/guide/getting-started" }],
            },
          ],
          "/ko/services/": [
            {
              text: "서비스",
              items: [{ text: "개요", link: "/ko/services/" }],
            },
          ],
        },
      },
    ],
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
