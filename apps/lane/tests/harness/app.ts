// SPDX-License-Identifier: Apache-2.0
// Bundles the real Lane App for Node tests, replacing only the ReactLynx
// runtime, styles and the host bridge. Tests supply a scripted adapter through
// the App's `providedAdapter` prop.
import { build, type Plugin } from "esbuild";
import { fileURLToPath } from "node:url";
import type { HostNode } from "./lynx-react.ts";

const lane = fileURLToPath(new URL("../..", import.meta.url));
const harness = (name: string) => fileURLToPath(new URL(name, import.meta.url));

export interface AppBundle {
  App: (props: any) => unknown;
  runtime: typeof import("./lynx-react.ts");
  host: { route?: (route: any) => void };
}

// The host bridge needs native modules. The App only reaches it for the
// adapter (provided by tests), host event subscriptions and shortcuts.
const hostStub = `export const getAdapter = () => { throw new Error("host unavailable in tests"); };
export const host = { route: undefined };
export const listenHost = (route) => { host.route = route; return () => { host.route = undefined; }; };
export const hostAction = () => Promise.reject(new Error("host unavailable in tests"));
export const listenReauthentication = () => () => {};`;

const plugin: Plugin = {
  name: "lane-app-harness",
  setup(b) {
    b.onResolve(
      { filter: /^@lynx-js\/react(\/jsx-runtime|\/jsx-dev-runtime)?$/ },
      () => ({ path: harness("lynx-react.ts") }),
    );
    b.onResolve({ filter: /(\?inline$|\.css$)/ }, (args) => ({
      path: args.path,
      namespace: "empty",
    }));
    b.onLoad({ filter: /.*/, namespace: "empty" }, () => ({
      contents: `export default "";`,
      loader: "js",
    }));
    b.onLoad({ filter: /[\\/]src[\\/]runtime\.ts$/ }, () => ({
      contents: hostStub,
      loader: "js",
    }));
  },
};

export async function loadApp(): Promise<AppBundle> {
  const result = await build({
    stdin: {
      contents: `export { App } from "./src/App.tsx";
export * as runtime from "./tests/harness/lynx-react.ts";
export { host } from "./src/runtime.ts";`,
      resolveDir: lane,
      sourcefile: "app-entry.ts",
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
  (globalThis as any).lynx = {
    createSelectorQuery: () => query,
    getJSModule: () => ({ addListener() {}, removeListener() {} }),
  };
  // The web host is the simplest surface: no desktop capture bridge and no
  // global shortcut handling.
  (globalThis as any).NativeModules = { LaneOnline: {} };
  const code = result.outputFiles[0].text;
  return import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
}

export type { HostNode };
