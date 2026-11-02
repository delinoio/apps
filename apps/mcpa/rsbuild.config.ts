// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";

export default defineConfig({
  plugins: [pluginReact()],
  source: { entry: { index: "./src/main.tsx" } },
  html: { template: "./index.html" },
  server: {
    host: "127.0.0.1",
    port: 46328,
    strictPort: true,
    // Console routes are client-side paths such as /tenants/<id>.
    historyApiFallback: true,
  },
  output: { assetPrefix: "/", sourceMap: false, cleanDistPath: true },
});
