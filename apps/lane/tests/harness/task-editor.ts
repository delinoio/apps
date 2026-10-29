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
  LaneError: typeof import("../../src/adapter.ts").LaneError;
  runtime: typeof import("./lynx-react.ts");
  captured: { draft?: any; value?: any; selected?: string[] };
  SelectHost: (props: any) => unknown;
  LabelsHost: (props: any) => unknown;
  RelationshipsHost: (props: any) => unknown;
  RelationshipsTransitionHost: (props: any) => unknown;
  focusCalls: { selector: string; method: string }[];
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
import { TaskSelect, TaskLabelSelect } from "./src/TaskControls.tsx";
import { TaskRelationships } from "./src/TaskRelationships.tsx";
export { LaneError } from "./src/adapter.ts";
export { TaskEditor };
export * as runtime from "./tests/harness/lynx-react.ts";
export const captured = {};
export function SelectHost(props) {
  const [value, setValue] = useState(props.value);
  const [openControl, setOpenControl] = useState();
  captured.value = value;
  return <TaskSelect {...props} value={value} openControl={openControl} setOpenControl={setOpenControl} onChange={setValue} />;
}
export function LabelsHost(props) {
  const [selected, setSelected] = useState(props.selected);
  const [openControl, setOpenControl] = useState();
  captured.selected = selected;
  return <TaskLabelSelect {...props} selected={selected} openControl={openControl} setOpenControl={setOpenControl} onToggle={(id) => setSelected((ids) => ids.includes(id) ? ids.filter((item) => item !== id) : [...ids, id])} />;
}
export function RelationshipsHost(props) {
  const [draft, setDraft] = useState(props.initial);
  const [openControl, setOpenControl] = useState();
  captured.draft = draft;
  return <TaskRelationships {...props} draft={draft} setDraft={setDraft} edit={(patch) => setDraft({ ...draft, task: { ...draft.task, ...patch }, dirty: true })} openControl={openControl} setOpenControl={setOpenControl} />;
}
export function RelationshipsTransitionHost(props) {
  const [draft, setDraft] = useState(props.initial);
  const [openControl, setOpenControl] = useState(props.openControl);
  captured.draft = draft;
  return <>
    <TaskSelect
      control="priority"
      openControl={openControl}
      setOpenControl={setOpenControl}
      label="Priority"
      value={""}
      options={[["", "None"], ["high", "High"]]}
      onChange={() => {}}
    />
    <TaskRelationships {...props} draft={draft} setDraft={setDraft} edit={(patch) => setDraft({ ...draft, task: { ...draft.task, ...patch }, dirty: true })} openControl={openControl} setOpenControl={setOpenControl} />
  </>;
}
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
  const focusCalls: { selector: string; method: string }[] = [];
  let selector = "";
  const query: any = new Proxy(() => query, {
    get: (_target, property) => {
      if (property === "select")
        return (value: string) => {
          selector = value;
          return query;
        };
      if (property === "invoke")
        return (value: { method: string }) => {
          focusCalls.push({ selector, method: value.method });
          return query;
        };
      return query;
    },
  });
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  const code = result.outputFiles[0].text;
  const module = await import(
    `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
  return { ...module, focusCalls };
}

export type { HostNode };
