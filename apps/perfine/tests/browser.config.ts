// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
export default defineConfig({
  plugins: [pluginReact()],
  source: { entry: { index: "./tests/browser.tsx" } },
  html: {
    title: "Perfine — mocked RPC verification",
    template: "./public/index.html",
  },
  server: { host: "127.0.0.1", port: 46324, strictPort: true },
  output: { distPath: { root: "/tmp/perfine-browser-fixture" } },
});
