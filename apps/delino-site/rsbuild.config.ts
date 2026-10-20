import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
import registry from "./i18n/locales.json";

const entries = Object.fromEntries(
  registry.locales.map(({ lang }) => [
    lang === registry.sourceLang ? "index" : `${lang}/index`,
    "./src/main.tsx",
  ]),
);

export default defineConfig({
  plugins: [pluginReact()],
  source: { entry: entries },
  html: {
    template: "./index.html",
    templateParameters: (parameters, { entryName }) => ({
      ...parameters,
      lang:
        entryName === "index" ? registry.sourceLang : entryName.split("/")[0],
    }),
  },
  server: { host: "127.0.0.1", port: 46314, strictPort: true },
  output: { assetPrefix: "/", sourceMap: false, cleanDistPath: true },
});
