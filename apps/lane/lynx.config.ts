// SPDX-License-Identifier: Apache-2.0
import { type Rspack, defineConfig } from "@lynx-js/rspeedy";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);
const runtime = dirname(require.resolve("@lynx-js/web-core/package.json"));
export default defineConfig({
  source: { entry: { main: "./src/index.tsx" } },
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
  environments: {
    lynx: {},
    web: {
      tools: {
        rspack(_config, { appendPlugins, rspack }) {
          appendPlugins([
            {
              apply(compiler: Rspack.Compiler) {
                compiler.hooks.thisCompilation.tap(
                  "lane-online-host",
                  (compilation) => {
                    compilation.hooks.processAssets.tapPromise(
                      {
                        name: "lane-online-host",
                        stage:
                          rspack.Compilation.PROCESS_ASSETS_STAGE_ADDITIONAL,
                      },
                      async () => {
                        const { build } = await import("esbuild");
                        const result = await build({
                          entryPoints: ["web/online.ts"],
                          bundle: true,
                          format: "esm",
                          platform: "browser",
                          target: "es2022",
                          write: false,
                          metafile: true,
                        });
                        compilation.emitAsset(
                          "online.js",
                          new rspack.sources.RawSource(
                            Buffer.from(result.outputFiles[0].contents),
                          ),
                        );
                        for (const path of Object.keys(result.metafile.inputs))
                          compilation.fileDependencies.add(resolve(path));
                      },
                    );
                  },
                );
              },
            },
          ]);
        },
      },
      output: {
        copy: [
          { from: resolve(runtime, "dist/client_prod"), to: "runtime" },
          { from: resolve(runtime, "LICENSE.txt"), to: "runtime/LICENSE.txt" },
          { from: resolve(runtime, "Notice.txt"), to: "runtime/Notice.txt" },
          {
            from: "web/index.html",
            to: "index.html",
            transform(content) {
              const api =
                process.env.PUBLIC_LANE_API_URL || "https://lane.api.delino.io";
              const url = new URL(api);
              if (
                url.protocol !== "https:" &&
                !(
                  url.protocol === "http:" &&
                  ["127.0.0.1", "localhost"].includes(url.hostname)
                )
              )
                throw new Error(
                  "PUBLIC_LANE_API_URL must use HTTPS or loopback HTTP.",
                );
              return content
                .toString()
                .replace("https://lane.api.delino.io", url.origin);
            },
          },
          { from: "web/host.js", to: "host.js" },
          { from: "web/accessibility.js", to: "accessibility.js" },
          { from: "web/online-module.js", to: "online-module.js" },
          { from: "web/host-module.js", to: "host-module.js" },
          { from: "web/push-worker.js", to: "push-worker.js" },
          { from: "../../LICENSE", to: "." },
          { from: "../../NOTICE", to: "." },
        ],
      },
    },
  },
  server: { host: "127.0.0.1", port: 46325, strictPort: true },
});
