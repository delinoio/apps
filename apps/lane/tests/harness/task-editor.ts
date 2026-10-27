// SPDX-License-Identifier: Apache-2.0
// Bundles the real Task editor and shared components for Node tests, replacing
// only the ReactLynx runtime and the host-bound runtime module. The bundle
// exposes the bare TaskEditor and a small Host component that owns the draft
// state the application shell normally provides.
import { build, type Plugin } from "esbuild";
import { fileURLToPath } from "node:url";
import type { HostNode } from "./lynx-react.ts";

const lane = fileURLToPath(new URL("../..", import.meta.url));
const harness = (name: string) => fileURLToPath(new URL(name, import.meta.url));

export interface TaskEditorBundle {
  TaskEditor: (props: any) => unknown;
  Host: (props: any) => unknown;
  runtime: typeof import("./lynx-react.ts");
  captured: { draft?: any };
}

const plugin: Plugin = {
  name: "lane-task-editor-harness",
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
    b.onLoad({ filter: /[\\/]src[\\/]runtime\.ts$/ }, () => ({
      contents: `export const hostAction = () => Promise.reject(new Error("host unavailable in tests")); export const listenReauthentication = () => () => {};`,
      loader: "js",
    }));
  },
};

export async function loadTaskEditor(): Promise<TaskEditorBundle> {
  const result = await build({
    stdin: {
      contents: `import { useState } from "@lynx-js/react";
import { TaskEditor } from "./src/TaskEditor.tsx";
export { TaskEditor };
export * as runtime from "./tests/harness/lynx-react.ts";
export const captured = {};
export function Host(props) {
  const [draft, setDraft] = useState(props.initial);
  captured.draft = draft;
  return <TaskEditor {...props} draft={draft} setDraft={setDraft} />;
}`,
      resolveDir: lane,
      sourcefile: "task-editor-entry.tsx",
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
