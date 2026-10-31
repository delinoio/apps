import { readFileSync } from "node:fs";
import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
import registry from "./i18n/locales.json";
import type source from "./i18n/messages/en.json";

const origin = "https://console.delino.io";
const localePath = (lang: string) =>
  lang === registry.sourceLang ? "/" : `/${lang}/`;
const escapeHtml = (text: string) =>
  text.replace(
    /[&<>"']/g,
    (character) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[character]!,
  );

export default defineConfig({
  plugins: [pluginReact()],
  source: {
    entry: Object.fromEntries(
      registry.locales.map(({ lang }) => [
        lang === registry.sourceLang ? "index" : `${lang}/index`,
        "./src/main.tsx",
      ]),
    ),
  },
  html: {
    template: "./index.html",
    templateParameters: (parameters, { entryName }) => {
      const lang =
        registry.locales.find(({ lang }) => entryName === `${lang}/index`)
          ?.lang ?? registry.sourceLang;
      const messages = JSON.parse(
        readFileSync(
          new URL(`./i18n/messages/${lang}.json`, import.meta.url),
          "utf8",
        ),
      ) as typeof source;
      return {
        ...parameters,
        lang,
        messages: Object.fromEntries(
          Object.entries(messages).map(([key, value]) => [
            key,
            escapeHtml(value),
          ]),
        ),
        canonical: `${origin}${localePath(lang)}`,
        alternates: registry.locales
          .map(
            ({ lang }) =>
              `<link rel="alternate" hreflang="${lang}" href="${origin}${localePath(lang)}" />`,
          )
          .join(""),
      };
    },
  },
  server: { host: "127.0.0.1", port: 46332, strictPort: true },
  output: { assetPrefix: "/", sourceMap: false, cleanDistPath: true },
});
