// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
export default defineConfig({
  plugins: [pluginReact()],
  source: { entry: { index: "./src/main.tsx" } },
  html: {
    title: "Perfine — Performance evidence",
    template: "./public/index.html",
  },
  server: { host: "127.0.0.1", port: 46324, strictPort: true },
  output: { cleanDistPath: true },
});
