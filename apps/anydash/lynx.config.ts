// SPDX-License-Identifier: Apache-2.0
import { defineConfig } from "@lynx-js/rspeedy";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);
const { version } = JSON.parse(readFileSync("package.json", "utf8"));

export default defineConfig({
  source: {
    entry: { main: "./src/index.tsx" },
    define: { __ANYDASH_VERSION__: JSON.stringify(version) },
  },
  plugins: [pluginReactLynx()],
  output: {
    copy: [
      {
        from: resolve(
          dirname(require.resolve("text-encoding-utf-8/package.json")),
          "LICENSE.md",
        ),
        to: "licenses/text-encoding-utf-8.txt",
      },
    ],
  },
  environments: { lynx: {} },
  server: { host: "127.0.0.1", port: 46328, strictPort: true },
});
