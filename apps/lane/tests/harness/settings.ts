// SPDX-License-Identifier: Apache-2.0
// Bundles the real Settings screen and shared components for Node tests,
// replacing only the ReactLynx runtime and host-bound collaborators.
import { build, type Plugin } from "esbuild";
import { fileURLToPath } from "node:url";
import type { HostNode } from "./lynx-react.ts";

const lane = fileURLToPath(new URL("../..", import.meta.url));
const harness = (name: string) => fileURLToPath(new URL(name, import.meta.url));

export interface UpdateProps {
  settingsDirty: () => boolean;
  settingsBusy: boolean;
  onBusyChange: (busy: boolean) => void;
}
export interface SettingsBundle {
  Settings: (props: any) => unknown;
  runtime: typeof import("./lynx-react.ts");
  captured: { updates?: UpdateProps };
}

const stubs: Record<string, string> = {
  // AppUpdates is exercised by its own tests; expose its props so tests can
  // observe the Settings dirty flag and drive the update-busy boundary.
  "src/AppUpdates.tsx": `export const captured = {}; export function AppUpdates(props) { captured.updates = props; return null; }`,
  "src/ResetDevelopment.tsx": `export function ResetDevelopmentDialog() { return null; }`,
  "src/runtime.ts": `export const hostAction = () => Promise.reject(new Error("host unavailable in tests")); export const listenReauthentication = () => () => {};`,
};
const plugin: Plugin = {
  name: "lane-settings-harness",
  setup(b) {
    b.onResolve(
      { filter: /^@lynx-js\/react(\/jsx-runtime|\/jsx-dev-runtime)?$/ },
      () => ({
        path: harness("lynx-react.ts"),
      }),
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
      { filter: /\/src\/(AppUpdates\.tsx|ResetDevelopment\.tsx|runtime\.ts)$/ },
      (args) => {
        const key = args.path.slice(lane.length);
        return { contents: stubs[key], loader: "js" };
      },
    );
  },
};

export async function loadSettings(): Promise<SettingsBundle> {
  const result = await build({
    stdin: {
      contents: `export { Settings } from "./src/Settings.tsx";
export * as runtime from "./tests/harness/lynx-react.ts";
export { captured } from "./src/AppUpdates.tsx";`,
      resolveDir: lane,
      sourcefile: "settings-entry.ts",
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
  // Components reach for the Lynx element API from background-only effects.
  const query: any = new Proxy(() => query, { get: () => query });
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  const code = result.outputFiles[0].text;
  return import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
}

export type { HostNode };
