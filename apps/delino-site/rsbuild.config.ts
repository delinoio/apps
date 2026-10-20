import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
import en from "./i18n/messages/en.json";
import { readFileSync } from "node:fs";
import registry from "./i18n/locales.json";

const entries = Object.fromEntries(
  registry.locales.flatMap(({ lang }) => [
    [
      lang === registry.sourceLang ? "index" : `${lang}/index`,
      "./src/main.tsx",
    ],
    [`${lang}/products/fix-key/index`, "./src/main.tsx"],
    ...(lang === registry.sourceLang
      ? [["products/fix-key/index", "./src/main.tsx"]]
      : []),
  ]),
);

export default defineConfig({
  plugins: [pluginReact()],
  source: { entry: entries },
  html: {
    template: "./index.html",
    templateParameters: (parameters, { entryName }) => {
      const lang =
        registry.locales.find(({ lang }) => entryName.startsWith(`${lang}/`))
          ?.lang ?? registry.sourceLang;
      const messages = JSON.parse(
        readFileSync(
          new URL(`./i18n/messages/${lang}.json`, import.meta.url),
          "utf8",
        ),
      ) as typeof en;
      const product = entryName.includes("products/fix-key");
      return {
        ...parameters,
        lang,
        title: product ? messages["fixkey.title"] : "Delino",
        description: product
          ? messages["fixkey.description"]
          : messages["delino.description"],
      };
    },
  },
  server: { host: "127.0.0.1", port: 46314, strictPort: true },
  output: { assetPrefix: "/", sourceMap: false, cleanDistPath: true },
});
