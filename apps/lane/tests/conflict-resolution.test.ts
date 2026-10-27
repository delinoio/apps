// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { create } from "@bufbuild/protobuf";
import {
  AttachmentState,
  EntityResultSchema,
  EntityVersionSchema,
  Progress,
  Proposal,
  ResultKind,
  TaskPriority,
  TaskSchema,
  TaskType,
} from "@repo/protos/lane/v1/lane_pb";
import { draftFor } from "../src/model.ts";
import { toUI } from "../src/serializable.ts";
import {
  loadTaskEditor,
  type HostNode,
  type TaskEditorBundle,
} from "./harness/task-editor.ts";

const u = (n: number) =>
  `019a0000-0000-7000-8000-${n.toString(16).padStart(12, "0")}`;
const taskId = u(200);
const repositoryId = u(100);
const version = (
  n: number,
  title: string,
  prompt: string | undefined,
  extra: Record<string, unknown> = {},
) =>
  create(EntityVersionSchema, {
    entityId: taskId,
    versionId: u(n),
    origin: { deviceName: `Device ${n}` },
    body: {
      case: "task",
      value: create(TaskSchema, {
        title,
        description: `Description ${n}`,
        prompt,
        repositoryId,
        progress: n === 1 ? Progress.IN_PROGRESS : Progress.BLOCKED,
        proposal: Proposal.REGULAR,
        ...extra,
      }),
    },
  });
const h1 = version(1, "First version", undefined);
const h2 = version(2, "Second version", "", {
  captures: [{ attachmentId: u(301), width: 1, height: 1 }],
});
const h3 = version(3, "Third version", "# Keep **literal** Markdown", {
  priority: TaskPriority.HIGH,
  type: TaskType.IMPROVEMENT,
  startDate: "2026-10-27",
  dueDate: "2026-10-28",
  storyPoints: 0,
  labelIds: [u(101)],
  milestoneId: u(102),
  cycleId: u(103),
  parentTaskId: u(201),
  dependsOnTaskIds: [u(202)],
  relatedTaskIds: [u(203)],
  duplicateOfTaskId: u(204),
});
const conflict = (...versions: (typeof h1)[]) =>
  toUI(
    create(EntityResultSchema, {
      result: ResultKind.CONFLICT,
      version: versions[0],
      alternatives: versions.slice(1),
    }),
  );
const content = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(content).join("");
const all = (roots: HostNode[], predicate: (node: HostNode) => boolean) => {
  const found: HostNode[] = [];
  const visit = (node: HostNode | string) => {
    if (typeof node === "string") return;
    if (predicate(node)) found.push(node);
    node.children.forEach(visit);
  };
  roots.forEach(visit);
  return found;
};
let bundle: TaskEditorBundle;
test.before(async () => {
  bundle = await loadTaskEditor();
});

function open(initial = conflict(h1, h2, h3)) {
  let updateCurrent!: (value: ReturnType<typeof conflict>) => void;
  let releaseSave: (() => void) | undefined;
  let rejectSave: ((reason: Error) => void) | undefined;
  let holdSave = false;
  const requests: { method: string; input: any }[] = [];
  const adapter = {
    mode: "web",
    call: async (method: { name: string }, input: any) => {
      requests.push({ method: method.name, input });
      if (method.name === "ListTasks") return { items: [], nextCursor: "" };
      if (method.name === "GetAttachment")
        return { state: AttachmentState.WAITING_FOR_SYNC };
      if (method.name === "ResolveConflict") {
        if (holdSave)
          await new Promise<void>((resolve, reject) => {
            releaseSave = resolve;
            rejectSave = reject;
          });
        return { result: undefined };
      }
      return {};
    },
    local: async () => ({}),
    subscribe: () => () => {},
    close: () => {},
    open: async () => {},
  };
  const initialDraft = draftFor(initial);
  function Host() {
    const [draft, setDraft] = bundle.runtime.useState(initialDraft);
    const [current, setCurrent] = bundle.runtime.useState(initial);
    updateCurrent = setCurrent;
    return bundle.runtime.jsx(bundle.TaskEditor, {
      adapter,
      draft,
      setDraft,
      current,
      repositories: [],
      planning: { items: [], status: "ready", error: "", retry: () => {} },
      revision: 0,
      openTask: () => {},
      beginCompletion: () => ({ saved: () => {}, close: () => {} }),
      onClose: () => {},
    });
  }
  const view = bundle.runtime.mount(Host, {});
  const nodes = (predicate: (node: HostNode) => boolean) =>
    all(view.tree(), predicate);
  const buttons = (label: string) =>
    nodes(
      (node) =>
        node.type === "view" &&
        /(^| )button( |$)/.test(node.props.className ?? "") &&
        content(node) === label,
    );
  const field = (label: string) => {
    const [node] = nodes(
      (item) =>
        (item.type === "input" || item.type === "textarea") &&
        item.props["accessibility-label"] === label,
    );
    assert.ok(node, `${label} is rendered`);
    return node;
  };
  const tap = async (node: HostNode) => {
    node.props.bindtap();
    await view.settle();
  };
  return {
    view,
    nodes,
    buttons,
    field,
    tap,
    requests,
    setCurrent: async (next: ReturnType<typeof conflict>) => {
      updateCurrent(next);
      await view.settle();
    },
    hold: () => (holdSave = true),
    release: () => releaseSave?.(),
    reject: (reason: Error) => {
      holdSave = false;
      rejectSave?.(reason);
    },
  };
}

test("comparison keeps every source head and the selected whole-version payload", async () => {
  const editor = open();
  await editor.view.settle();
  await editor.tap(editor.buttons("Compare conflict")[0]);
  const cards = editor.nodes((node) =>
    /(^| )conflict-head( |$)/.test(node.props.className ?? ""),
  );
  assert.equal(cards.length, 3);
  assert.deepEqual(
    cards.map((card) => content(card).match(/Version ID: (.{36})/)?.[1]),
    [u(1), u(2), u(3)],
  );
  assert.match(content(cards[0]), /✓ Selected/);
  assert.match(content(cards[0]), /Prompt: Not set/);
  assert.match(content(cards[1]), /Prompt: Empty/);
  assert.match(content(cards[2]), /# Keep \*\*literal\*\* Markdown/);
  assert.match(content(cards[2]), /Priority High/);
  assert.match(content(cards[2]), /0 points/);
  assert.match(content(cards[2]), /Parent: \(unavailable task/);
  assert.match(content(cards[1]), /Capture 1 of 1/);
  assert.equal(
    editor.requests.filter((r) => r.method === "ResolveConflict").length,
    0,
  );
  await editor.tap(editor.buttons("Use this version")[1]);
  assert.match(
    content(
      editor.nodes((n) =>
        /(^| )conflict-head( |$)/.test(n.props.className ?? ""),
      )[1],
    ),
    /✓ Selected/,
  );
  assert.equal(editor.field("Resolved title").props.disabled, false);
  editor.view.unmount();
});

test("resolution retains prompt presence, blocks edits during save, and pins reviewed heads", async () => {
  const editor = open(conflict(h1, h2));
  await editor.view.settle();
  await editor.tap(editor.buttons("Compare conflict")[0]);
  await editor.tap(editor.buttons("Use this version")[1]);
  await editor.tap(editor.buttons("No prompt")[0]);
  editor
    .field("Resolved prompt")
    .props.bindinput({ detail: { value: "**Keep source**" } });
  await editor.view.settle();
  assert.match(
    content(editor.view.tree()[0]),
    /An included empty prompt is different/,
  );
  await editor.setCurrent(conflict(h2, h1));
  editor.hold();
  const save = editor.buttons("Save resolution")[0];
  save.props.bindtap();
  save.props.bindtap();
  await editor.view.settle();
  const calls = editor.requests.filter((r) => r.method === "ResolveConflict");
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].input.expectedHeadIds, [u(1), u(2)]);
  assert.equal(calls[0].input.resolved.body.value.prompt, "**Keep source**");
  assert.equal(
    calls[0].input.resolved.body.value.captures[0].attachmentId,
    u(301),
  );
  assert.equal(editor.field("Resolved title").props.disabled, true);
  editor
    .field("Resolved title")
    .props.bindinput({ detail: { value: "Lost edit" } });
  await editor.view.settle();
  editor.reject(new Error("Storage full"));
  await editor.view.settle();
  assert.match(content(editor.view.tree()[0]), /Storage full/);
  assert.equal(editor.field("Resolved title").props.disabled, false);
  assert.equal(editor.buttons("Save resolution").length, 1);
  await editor.tap(editor.buttons("Save resolution")[0]);
  const retried = editor.requests.filter((r) => r.method === "ResolveConflict");
  assert.equal(retried.length, 2);
  assert.deepEqual(retried[1].input.resolved, calls[0].input.resolved);
  editor.view.unmount();
});

test("changed heads retain edits and require an explicit new review", async () => {
  const editor = open(conflict(h1, h2));
  await editor.view.settle();
  await editor.tap(editor.buttons("Compare conflict")[0]);
  editor
    .field("Resolved title")
    .props.bindinput({ detail: { value: "Local resolution" } });
  await editor.view.settle();
  await editor.setCurrent(conflict(h1, h3));
  assert.equal(
    editor.buttons("Save resolution")[0].props.className.includes("disabled"),
    true,
  );
  assert.equal(
    editor.buttons("Use this version")[0].props.className.includes("disabled"),
    true,
  );
  await editor.tap(editor.buttons("Review current versions")[0]);
  assert.match(
    content(editor.view.tree()[0]),
    /Retained resolution from an earlier review/,
  );
  assert.equal(
    editor.buttons("Save resolution")[0].props.className.includes("disabled"),
    true,
  );
  await editor.tap(editor.buttons("Use retained resolution")[0]);
  assert.equal(editor.field("Resolved title").props.disabled, false);
  assert.equal(
    editor.nodes((n) =>
      /(^| )conflict-head selected/.test(n.props.className ?? ""),
    ).length,
    0,
  );
  assert.match(content(editor.view.tree()[0]), /Local resolution/);
  await editor.setCurrent(conflict(h3));
  assert.match(content(editor.view.tree()[0]), /resolved elsewhere/);
  assert.equal(editor.buttons("Review current versions").length, 0);
  editor.view.unmount();
});
