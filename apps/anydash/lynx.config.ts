// SPDX-License-Identifier: Apache-2.0
import { type Rspack, defineConfig } from "@lynx-js/rspeedy";
import { pluginReactLynx } from "@lynx-js/react-rsbuild-plugin";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

const require = createRequire(import.meta.url);
const runtime = dirname(require.resolve("@lynx-js/web-core/package.json"));
const { version } = JSON.parse(readFileSync("package.json", "utf8"));

/** Public origins only; credentials never enter the bundle or host page. */
function publicOrigin(name: string, fallback: string) {
  const url = new URL(process.env[name] || fallback);
  if (
    url.protocol !== "https:" &&
    !(
      url.protocol === "http:" &&
      ["127.0.0.1", "localhost"].includes(url.hostname)
    )
  )
    throw new Error(`${name} must use HTTPS or loopback HTTP.`);
  return url.origin;
}

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
  environments: {
    lynx: {},
    web: {
      tools: {
        rspack(_config, { appendPlugins, rspack }) {
          // The browser host (Logto PKCE, API calls, history) runs on the page,
          // outside the Lynx worker; bundle it beside the web runtime.
          appendPlugins([
            {
              apply(compiler: Rspack.Compiler) {
                compiler.hooks.thisCompilation.tap(
                  "anydash-web-host",
                  (compilation) => {
                    compilation.hooks.processAssets.tapPromise(
                      {
                        name: "anydash-web-host",
                        stage:
                          rspack.Compilation.PROCESS_ASSETS_STAGE_ADDITIONAL,
                      },
                      async () => {
                        const { build } = await import("esbuild");
                        const result = await build({
                          entryPoints: ["web/host.ts"],
                          bundle: true,
                          format: "esm",
                          platform: "browser",
                          target: "es2022",
                          write: false,
                          metafile: true,
                          external: ["./runtime/*"],
                        });
                        compilation.emitAsset(
                          "host.js",
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
              const replacements: Record<string, string> = {
                "%ANYDASH_API_URL%": publicOrigin(
                  "PUBLIC_ANYDASH_API_URL",
                  "https://anydash.api.delino.io",
                ),
                "%LOGTO_ENDPOINT%": process.env.PUBLIC_LOGTO_ENDPOINT
                  ? publicOrigin("PUBLIC_LOGTO_ENDPOINT", "")
                  : "",
                "%LOGTO_APP_ID%": (
                  process.env.PUBLIC_LOGTO_APP_ID || ""
                ).replace(/[^A-Za-z0-9_-]/g, ""),
                "%ANYDASH_RESOURCE%":
                  process.env.PUBLIC_ANYDASH_RESOURCE ||
                  "https://anydash.api.delino.io",
                "%ANYDASH_VERSION%": version,
                "%DELCORE_RESOURCE%":
                  process.env.PUBLIC_DELCORE_RESOURCE ||
                  "https://delcore.api.delino.io",
              };
              let text = content.toString();
              for (const [key, value] of Object.entries(replacements))
                text = text.replaceAll(key, value.replace(/"/g, ""));
              return text;
            },
          },
          { from: "web/host-module.js", to: "host-module.js" },
          { from: "web/icons", to: "icons" },
          { from: "../../LICENSE", to: "." },
          { from: "../../NOTICE", to: "." },
        ],
      },
    },
  },
  server: { host: "127.0.0.1", port: 46330, strictPort: true },
});
