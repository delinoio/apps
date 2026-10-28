// SPDX-License-Identifier: Apache-2.0
// Mounts the real RealQA presentation with synthetic host operations.
import { build, type Plugin } from "esbuild";
import { fileURLToPath } from "node:url";
import type { HostNode } from "./lynx-react.ts";

const lane = fileURLToPath(new URL("../..", import.meta.url));
const harness = (name: string) => fileURLToPath(new URL(name, import.meta.url));
const stubs: Record<string, string> = {
  "src/realqa-host.ts": `
    export const CaptureMode = { Screen: "screen", Window: "window", Region: "region", DOM: "dom" };
    export const captureModes = [["screen", "Full screen"], ["window", "Window"], ["region", "Region"], ["dom", "Chrome element"]];
    export const host = { realqa: async () => undefined };
    export const realqa = (input) => host.realqa(input);
  `,
  "src/URLConnections.tsx": `export const URLConnections = () => null;`,
  "src/runtime.ts": `
    export const host = { openURL: async () => undefined };
    export const hostAction = (method, value) => host[method](value);
  `,
};
const plugin: Plugin = {
  name: "realqa-presentation-harness",
  setup(b) {
    b.onResolve(
      { filter: /^@lynx-js\/react(\/jsx-runtime|\/jsx-dev-runtime)?$/ },
      () => ({ path: harness("lynx-react.ts") }),
    );
    b.onLoad(
      {
        filter:
          /[\\/]src[\\/](realqa-host\.ts|URLConnections\.tsx|runtime\.ts)$/,
      },
      (args) => ({
        contents: stubs[args.path.slice(lane.length).replaceAll("\\", "/")],
        loader: "js",
      }),
    );
  },
};

export async function loadRealQA(): Promise<{
  RealQA: (props: any) => unknown;
  CaptureActions: (props: any) => unknown;
  RealQADialogs: (props: any) => unknown;
  runtime: typeof import("./lynx-react.ts");
  captureHost: { realqa: (input: Record<string, unknown>) => Promise<unknown> };
  urlHost: { openURL: (value: string) => Promise<unknown> };
}> {
  const result = await build({
    stdin: {
      contents: `
        export { RealQA, CaptureActions, RealQADialogs } from "./src/RealQA.tsx";
        export * as runtime from "./tests/harness/lynx-react.ts";
        export { host as captureHost } from "./src/realqa-host.ts";
        export { host as urlHost } from "./src/runtime.ts";
      `,
      resolveDir: lane,
      sourcefile: "realqa-presentation-entry.tsx",
      loader: "tsx",
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
