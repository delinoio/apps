// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@lynx-js/rspeedy";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);
const runtime = dirname(require.resolve("@lynx-js/web-core/package.json"));
export default defineConfig({
  source: { entry: { main: "./src/index.tsx" } },
  plugins: [pluginReactLynx()],
  environments: {
    lynx: {},
    web: {
      output: {
        copy: [
          { from: resolve(runtime, "dist/client_prod"), to: "runtime" },
          { from: resolve(runtime, "LICENSE.txt"), to: "runtime/LICENSE.txt" },
          { from: resolve(runtime, "Notice.txt"), to: "runtime/Notice.txt" },
          { from: "web/index.html", to: "index.html" },
          { from: "web/host.js", to: "host.js" },
          { from: "../../LICENSE", to: "." },
          { from: "../../NOTICE", to: "." },
        ],
      },
    },
  },
  server: { host: "127.0.0.1", port: 46325, strictPort: true },
});
