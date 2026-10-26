// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { create } from "@bufbuild/protobuf";
import {
  EntityResultSchema,
  EntityVersionSchema,
  TaskSchema,
  Progress,
  Proposal,
  ResultKind,
} from "@repo/protos/lane/v1/lane_pb";
import {
  createUITask,
  draftFor,
  editDraft,
  promptForPresence,
  type Draft,
} from "../src/model.ts";
import { fromUI } from "../src/serializable.ts";
import {
  loadTaskEditor,
  type HostNode,
  type TaskEditorBundle,
} from "./harness/task-editor.ts";

const EXACT = "Keep **exact** Markdown\n";

const draftWith = (prompt: string | undefined): Draft =>
  draftFor(
    create(EntityResultSchema, {
      version: create(EntityVersionSchema, {
        entityId: "task-1",
        versionId: "v1",
        body: {
          case: "task",
          value: create(TaskSchema, {
            title: "Fixture",
            repositoryId: "repo-1",
            description: "Description",
            prompt,
            progress: Progress.TODO,
            proposal: Proposal.REGULAR,
            captures: [{ attachmentId: "capture-1", width: 4, height: 3 }],
          }),
        },
      }),
    }),
  );

// Mirrors the editor's Prompt presence Choices handler: selecting an option
// applies the same patch the component passes to edit().
const choose = (draft: Draft, choice: string): Draft =>
  editDraft(draft, { prompt: promptForPresence(draft.task.prompt, choice) });

// Mirrors the Save payload task built by the editor before SaveTask.
const savedTask = (draft: Draft) =>
  fromUI(TaskSchema, createUITask({ ...draft.task }));

test("reselecting Include prompt keeps an exact nonempty prompt and stays clean", () => {
  const draft = draftWith(EXACT);
  const next = choose(draft, "present");
  assert.equal(next, draft);
  assert.equal(next.dirty, false);
  assert.equal(next.task.prompt, EXACT);
  const saved = savedTask(next);
  assert.equal(saved.prompt, EXACT);
  assert.equal(saved.captures.length, 1);
  assert.equal(saved.captures[0].attachmentId, "capture-1");
  assert.equal(saved.title, "Fixture");
  assert.equal(saved.repositoryId, "repo-1");
  assert.equal(saved.description, "Description");
  assert.equal(next.base, "v1");
  assert.equal(next.id, "task-1");
});

test("reselecting Include prompt keeps an unsaved Markdown edit", () => {
  const edited = editDraft(draftWith(EXACT), { prompt: "  unsaved\n\nedit  " });
  assert.equal(edited.dirty, true);
  const next = choose(edited, "present");
  assert.equal(next, edited);
  assert.equal(next.task.prompt, "  unsaved\n\nedit  ");
  assert.equal(savedTask(next).prompt, "  unsaved\n\nedit  ");
  assert.equal(next.base, "v1");
});

test("Include prompt from absence creates a present empty prompt", () => {
  const draft = draftWith(undefined);
  assert.equal(draft.task.prompt, undefined);
  const next = choose(draft, "present");
  assert.equal(next.dirty, true);
  assert.equal(next.task.prompt, "");
  const saved = savedTask(next);
  assert.equal(saved.prompt, "");
  assert.notEqual(saved.prompt, undefined);
  assert.equal(saved.captures.length, 1);
  assert.equal(saved.captures[0].attachmentId, "capture-1");
  assert.equal(next.base, "v1");
});

test("reselecting Include prompt on present empty keeps it present and distinct from absence", () => {
  const draft = draftWith("");
  const next = choose(draft, "present");
  assert.equal(next, draft);
  assert.equal(next.dirty, false);
  assert.equal(savedTask(next).prompt, "");
  assert.equal(savedTask(draftWith(undefined)).prompt, undefined);
});

test("No prompt removes presence, reselecting leaves it absent, Include then initializes empty", () => {
  const removed = choose(draftWith(EXACT), "absent");
  assert.equal(removed.dirty, true);
  assert.equal(removed.task.prompt, undefined);
  assert.equal(savedTask(removed).prompt, undefined);
  assert.equal(savedTask(removed).captures[0].attachmentId, "capture-1");
  assert.equal(removed.base, "v1");

  const again = choose(removed, "absent");
  assert.equal(again, removed);
  assert.equal(again.task.prompt, undefined);

  const included = choose(removed, "present");
  assert.equal(included.task.prompt, "");
  assert.equal(savedTask(included).prompt, "");
});

test("promptForPresence maps every presence state", () => {
  assert.equal(promptForPresence(EXACT, "present"), EXACT);
  assert.equal(promptForPresence("", "present"), "");
  assert.equal(promptForPresence(undefined, "present"), "");
  assert.equal(promptForPresence(EXACT, "absent"), undefined);
  assert.equal(promptForPresence("", "absent"), undefined);
  assert.equal(promptForPresence(undefined, "absent"), undefined);
});

// Real TaskEditor component tests: activation goes through the rendered Choices
// buttons and Save goes through the editor's own SaveTask request.
type Call = { method: string; request: any };
let bundle: TaskEditorBundle;
test.before(async () => {
  bundle = await loadTaskEditor();
});
const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function all(
  nodes: (HostNode | string)[],
  match: (n: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (n: HostNode | string) => {
    if (typeof n === "string") return;
    if (match(n)) found.push(n);
    n.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const isButton = (n: HostNode) =>
  /(^| )button( |$)/.test(n.props.className ?? "");

async function open(
  prompt: string | undefined,
  options: {
    conflicted?: boolean;
    deleted?: boolean;
    holdSave?: Promise<unknown>;
  } = {},
) {
  const initial = draftWith(prompt);
  initial.deleted = !!options.deleted;
  const calls: Call[] = [];
  const adapter = {
    mode: "web",
    call: async (method: { name: string }, request: any) => {
      calls.push({ method: method.name, request });
      if (method.name === "ListTasks") return { items: [], nextCursor: "" };
      if (method.name === "SaveTask" && options.holdSave)
        await options.holdSave;
      return { result: undefined };
    },
    local: async () => ({}),
    subscribe: () => () => {},
    close: () => {},
    open: async () => {},
  };
  const other = (versionId: string, prompt: string | undefined) =>
    create(EntityVersionSchema, {
      entityId: "task-1",
      versionId,
      body: {
        case: "task",
        value: create(TaskSchema, {
          title: `Title ${versionId}`,
          repositoryId: "repo-1",
          prompt,
          progress: Progress.TODO,
          proposal: Proposal.REGULAR,
        }),
      },
    });
  const result = options.conflicted
    ? create(EntityResultSchema, {
        result: ResultKind.CONFLICT,
        version: other("v1", "Keep resolution text\n"),
        alternatives: [other("v2", undefined)],
      })
    : undefined;
  const props = {
    adapter,
    initial,
    current: result,
    repositories: [],
    planning: { items: [], status: "ready", error: "", retry: () => {} },
    revision: 0,
    openTask: () => {},
    onSaved: () => {},
    onClose: () => {},
  };
  const view = bundle.runtime.mount(bundle.Host, props);
  await view.settle();
  const choice = (option: string) => {
    const [found] = all(view.tree(), (n) => isButton(n) && text(n) === option);
    assert.ok(found, `${option} is rendered`);
    return found;
  };
  const tap = async (node: HostNode) => {
    node.props.bindtap();
    await view.settle();
  };
  const saveCalls = () => calls.filter((c) => c.method === "SaveTask");
  return {
    view,
    calls,
    saveCalls,
    choice,
    tap,
    draft: () => bundle.captured.draft as Draft,
    openPrompt: () => tap(choice("Prompt")),
    save: () => tap(choice("Save")),
    textarea: () => {
      const [found] = all(view.tree(), (n) => n.type === "textarea");
      assert.ok(found, "Markdown field is rendered");
      return found;
    },
    type: async (node: HostNode, value: string) => {
      node.props.bindinput({ detail: { value } });
      await view.settle();
    },
  };
}

test("editor: reselecting the active Include prompt retains the prompt through Save", async () => {
  const editor = await open(EXACT);
  await editor.openPrompt();
  assert.match(editor.choice("Include prompt").props.className, /primary/);
  await editor.tap(editor.choice("Include prompt"));
  assert.equal(editor.draft().task.prompt, EXACT);
  assert.equal(editor.draft().dirty, false);
  assert.equal(editor.saveCalls().length, 0);
  await editor.save();
  const [save] = editor.saveCalls();
  assert.ok(save, "explicit Save sends SaveTask");
  assert.equal(save.request.task.prompt, EXACT);
  assert.equal(save.request.expectedVersionId, "v1");
  assert.equal(save.request.id, "task-1");
  assert.equal(save.request.task.captures.length, 1);
  assert.equal(save.request.task.title, "Fixture");
  assert.equal(save.request.task.repositoryId, "repo-1");
  editor.view.unmount();
});

test("editor: absent prompt becomes present empty and No prompt omits the prompt", async () => {
  const absent = await open(undefined);
  await absent.openPrompt();
  await absent.tap(absent.choice("No prompt"));
  assert.equal(absent.draft().task.prompt, undefined);
  assert.equal(absent.draft().dirty, false);
  await absent.tap(absent.choice("Include prompt"));
  assert.equal(absent.draft().task.prompt, "");
  await absent.save();
  assert.equal(absent.saveCalls()[0].request.task.prompt, "");
  absent.view.unmount();

  const removed = await open(EXACT);
  await removed.openPrompt();
  await removed.tap(removed.choice("No prompt"));
  assert.equal(removed.draft().task.prompt, undefined);
  await removed.tap(removed.choice("No prompt"));
  assert.equal(removed.draft().task.prompt, undefined);
  await removed.save();
  assert.equal(removed.saveCalls()[0].request.task.prompt, undefined);
  removed.view.unmount();

  const empty = await open("");
  await empty.openPrompt();
  await empty.tap(empty.choice("Include prompt"));
  assert.equal(empty.draft().task.prompt, "");
  assert.equal(empty.draft().dirty, false);
  empty.view.unmount();
});

test("editor: deleted drafts keep the prompt control disabled and send nothing", async () => {
  const editor = await open(EXACT, { deleted: true });
  await editor.openPrompt();
  const control = editor.choice("No prompt");
  assert.match(control.props.className, /disabled/);
  await editor.tap(control);
  await editor.tap(editor.choice("Include prompt"));
  assert.equal(editor.draft().task.prompt, EXACT);
  assert.equal(editor.draft().dirty, false);
  assert.equal(editor.saveCalls().length, 0);
  editor.view.unmount();
});

test("editor: reselecting Include prompt keeps an unsaved Markdown edit through Save", async () => {
  const editor = await open(EXACT);
  await editor.openPrompt();
  const edited = "  unsaved\n\nedit  ";
  await editor.type(editor.textarea(), edited);
  assert.equal(editor.draft().task.prompt, edited);
  assert.equal(editor.draft().dirty, true);
  await editor.tap(editor.choice("Include prompt"));
  assert.equal(editor.draft().task.prompt, edited);
  await editor.save();
  const [save] = editor.saveCalls();
  assert.equal(save.request.task.prompt, edited);
  assert.equal(save.request.expectedVersionId, "v1");
  editor.view.unmount();
});

test("editor: a conflicted task keeps the prompt control disabled and sends nothing", async () => {
  const editor = await open(EXACT, { conflicted: true });
  await editor.openPrompt();
  assert.match(editor.choice("Include prompt").props.className, /disabled/);
  await editor.tap(editor.choice("No prompt"));
  await editor.tap(editor.choice("Include prompt"));
  assert.equal(editor.draft().task.prompt, EXACT);
  assert.equal(editor.draft().dirty, false);
  assert.equal(editor.calls.filter((c) => c.method === "SaveTask").length, 0);
  editor.view.unmount();
});

test("editor: a busy editor keeps the prompt control disabled while Save is in flight", async () => {
  let release!: () => void;
  const hold = new Promise<void>((resolve) => (release = resolve));
  const editor = await open(EXACT, { holdSave: hold });
  await editor.openPrompt();
  await editor.save();
  assert.equal(editor.saveCalls().length, 1);
  assert.match(editor.choice("No prompt").props.className, /disabled/);
  await editor.tap(editor.choice("No prompt"));
  assert.equal(editor.draft().task.prompt, EXACT);
  assert.equal(editor.saveCalls().length, 1);
  release();
  await editor.view.settle();
  editor.view.unmount();
});

test("editor: the conflict resolution Include prompt keeps the resolved text", async () => {
  const editor = await open(EXACT, { conflicted: true });
  await editor.tap(editor.choice("Compare conflict"));
  // Only the resolution dialog renders a prompt presence control here, because
  // the Prompt tab of the paused editor is not open.
  const include = all(
    editor.view.tree(),
    (n) => isButton(n) && text(n) === "Include prompt",
  );
  assert.equal(include.length, 1);
  assert.match(include[0].props.className, /primary/);
  await editor.tap(include[0]);
  assert.equal(editor.calls.filter((c) => c.method === "SaveTask").length, 0);
  await editor.tap(editor.choice("Save resolution"));
  const [resolve] = editor.calls.filter((c) => c.method === "ResolveConflict");
  assert.ok(resolve, "explicit Save resolution sends ResolveConflict");
  assert.equal(
    resolve.request.resolved.body.value.prompt,
    "Keep resolution text\n",
  );
  editor.view.unmount();
});
