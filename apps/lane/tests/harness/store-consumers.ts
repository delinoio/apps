// SPDX-License-Identifier: Apache-2.0
// Bundles the real URL connections screen and capture composer for Node tests,
// replacing only the ReactLynx runtime and host-bound collaborators. Both
// components keep using the real NativeAdapter passed in by the test.
import { build, type Plugin } from "esbuild";
import { fileURLToPath } from "node:url";
import type { HostNode } from "./lynx-react.ts";

const lane = fileURLToPath(new URL("../..", import.meta.url));
const harness = (name: string) => fileURLToPath(new URL(name, import.meta.url));

export interface StoreConsumersBundle {
  URLConnections: (props: any) => unknown;
  ComposerHost: (props: any) => unknown;
  runtime: typeof import("./lynx-react.ts");
  host: { realqa: (input: Record<string, unknown>) => Promise<unknown> };
}

const stubs: Record<string, string> = {
  // The capture host is replaced; tests observe and answer its operations.
  "src/realqa-host.ts": `export const host = { realqa: async () => undefined };
export const realqa = (input) => host.realqa(input);
export const EditTool = { Crop: "crop", Draw: "draw", Arrow: "arrow", Rectangle: "rectangle", Text: "text" };
export const CaptureMode = { Screen: "screen", Window: "window", Region: "region", DOM: "dom" };
export const captureModes = [];`,
  "src/RealQA.tsx": `export const CaptureActions = () => null; export const RealQADialogs = () => null;`,
  "src/runtime.ts": `export const hostAction = () => Promise.reject(new Error("host unavailable in tests")); export const listenReauthentication = () => () => {};`,
};
const plugin: Plugin = {
  name: "lane-store-consumers-harness",
  setup(b) {
    b.onResolve(
      { filter: /^@lynx-js\/react(\/jsx-runtime|\/jsx-dev-runtime)?$/ },
      () => ({ path: harness("lynx-react.ts") }),
    );
    b.onResolve({ filter: /\?inline$/ }, (args) => ({
      path: args.path,
      namespace: "empty",
    }));
    b.onLoad({ filter: /.*/, namespace: "empty" }, () => ({
      contents: `export default "";`,
      loader: "js",
    }));
    b.onLoad(
      { filter: /[\\/]src[\\/](realqa-host\.ts|RealQA\.tsx|runtime\.ts)$/ },
      (args) => ({
        contents: stubs[args.path.slice(lane.length).replaceAll("\\", "/")],
        loader: "js",
      }),
    );
  },
};

export async function loadStoreConsumers(): Promise<StoreConsumersBundle> {
  const result = await build({
    stdin: {
      // ComposerHost keeps the draft in state like the capture controller does
      // and applies the host's updateDraft operation to it.
      contents: `import { useState } from "@lynx-js/react";
import { CaptureComposer } from "./src/CaptureComposer.tsx";
export { URLConnections } from "./src/URLConnections.tsx";
export * as runtime from "./tests/harness/lynx-react.ts";
import { host } from "./src/realqa-host.ts";
export { host };
export function ComposerHost({ adapter, repositories, initial, snapshot, captureHost }) {
  const [draft, setDraft] = useState(initial);
  host.realqa = async (input) => {
    if (captureHost) return captureHost(input);
    if (input.op === "updateDraft") setDraft((d) => ({ ...d, repositoryId: input.repositoryId }));
  };
  snapshot.draft = draft;
  snapshot.setDraft = setDraft;
  const controller = {
    state: { draft }, busy: false, error: "", discard: false, sources: undefined,
    available: true, setDiscard() {}, setSources() {}, capture() {}, refresh() {},
    run: async (action) => { await action(); },
    updateDraft: (patch) => { setDraft((d) => ({ ...d, ...patch })); },
  };
  return <CaptureComposer controller={controller} adapter={adapter} repositories={repositories} />;
}`,
      resolveDir: lane,
      sourcefile: "store-consumers-entry.tsx",
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
  // Components reach for the Lynx element API from background-only effects.
  const query: any = new Proxy(() => query, { get: () => query });
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  const code = result.outputFiles[0].text;
  return import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
}

export type { HostNode };
