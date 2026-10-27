// SPDX-License-Identifier: Apache-2.0
// Mounts the real PlanningManager with a scripted adapter and the shared
// ReactLynx test renderer, without starting a browser or native host.
import { build, type Plugin } from "esbuild";
import { fileURLToPath } from "node:url";
import type { HostNode } from "./lynx-react.ts";

const lane = fileURLToPath(new URL("../..", import.meta.url));
const harness = (name: string) => fileURLToPath(new URL(name, import.meta.url));

const plugin: Plugin = {
  name: "lane-planning-manager-harness",
  setup(build) {
    build.onResolve(
      { filter: /^@lynx-js\/react(\/jsx-runtime|\/jsx-dev-runtime)?$/ },
      () => ({ path: harness("lynx-react.ts") }),
    );
    build.onLoad({ filter: /[\\/]src[\\/]runtime\.ts$/ }, () => ({
      contents: `export const hostAction = () => Promise.reject(new Error("host unavailable in tests"));`,
      loader: "js",
    }));
  },
};

export async function loadPlanningManager(): Promise<{
  PlanningManager: (props: any) => unknown;
  runtime: typeof import("./lynx-react.ts");
}> {
  const result = await build({
    stdin: {
      contents: `export { PlanningManager } from "./src/PlanningManager.tsx";
export * as runtime from "./tests/harness/lynx-react.ts";`,
      resolveDir: lane,
      sourcefile: "planning-manager-entry.ts",
      loader: "ts",
    },
    bundle: true,
    write: false,
    format: "esm",
    platform: "node",
    target: "node24",
    jsx: "automatic",
    jsxImportSource: "@lynx-js/react",
    plugins: [plugin],
    logLevel: "silent",
  });
  const query: any = new Proxy(() => query, { get: () => query });
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  return import(
    `data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString("base64")}`
  );
}

export type { HostNode };
