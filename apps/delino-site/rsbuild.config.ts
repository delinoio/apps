import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";

export default defineConfig({
  plugins: [pluginReact()],
  source: { entry: { index: "./src/main.tsx", fixkey: "./src/main.tsx" } },
  html: {
    template: ({ entryName }) =>
      entryName === "fixkey" ? "./fixkey.html" : "./index.html",
  },
  tools: {
    htmlPlugin(config, { entryName }) {
      if (entryName === "fixkey")
        config.filename = "products/fixkey/index.html";
    },
  },
  server: { host: "127.0.0.1", port: 46314, strictPort: true },
  output: { assetPrefix: "/", sourceMap: false, cleanDistPath: true },
});
