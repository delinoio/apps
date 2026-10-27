// SPDX-License-Identifier: Apache-2.0
// Exercises the real URL editor and controls with deferred adapter replies.
// The Lynx runtime is a Node stand-in, not native platform acceptance.
import assert from "node:assert/strict";
import test from "node:test";
import { create } from "@bufbuild/protobuf";
import {
  EntityKind,
  EntityResultSchema,
  ResultKind,
} from "@repo/protos/lane/v1/lane_pb";
import type { LaneAdapter } from "../src/adapter.ts";
import { toUI } from "../src/serializable.ts";
import {
  loadStoreConsumers,
  type HostNode,
  type StoreConsumersBundle,
} from "./harness/store-consumers.ts";

type Mutation = {
  method: string;
  input: {
    meta: { requestId: string };
    id: string;
    expectedVersionId?: string;
    connection?: { prefix: string; repositoryId: string };
    resolved?: { body: { value: { prefix: string; repositoryId: string } } };
  };
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

const repository = (id: string, name: string) =>
  toUI(
    create(EntityResultSchema, {
      result: ResultKind.APPLIED,
      version: {
        formatVersion: 3,
        entityId: id,
        versionId: `${id}-v1`,
        kind: EntityKind.REPOSITORY,
        body: {
          case: "repository",
          value: { normalizedUrl: `example.test/${id}`, displayName: name },
        },
      },
    }),
  );
const rule = (prefix: string, repositoryId: string, versionId = "v1") => ({
  formatVersion: 3,
  entityId: "rule",
  versionId,
  kind: EntityKind.URL_CONNECTION,
  body: { case: "urlConnection" as const, value: { prefix, repositoryId } },
});
const applied = (prefix: string, repositoryId: string) =>
  toUI(
    create(EntityResultSchema, {
      result: ResultKind.APPLIED,
      version: rule(prefix, repositoryId),
    }),
  );
const conflicting = () => {
  const first = rule("https://one.example/app", "repo-one", "v1");
  const second = rule("https://two.example/app", "repo-two", "v2");
  return toUI(
    create(EntityResultSchema, {
      result: ResultKind.CONFLICT,
      version: first,
      alternatives: [second],
    }),
  );
};

function deferredAdapter(initial: ReturnType<typeof applied>[]) {
  const calls: Mutation[] = [];
  const adapter = {
    subscribe: () => () => {},
    local: async () => ({ syncEnabled: false }),
    call: (method: { name: string }, input: Mutation["input"]) => {
      if (method.name === "ListURLConnections")
        return Promise.resolve({ items: initial, nextCursor: "" });
      return new Promise((resolve, reject) =>
        calls.push({
          method: method.name,
          input: structuredClone(input),
          resolve,
          reject,
        }),
      );
    },
  } as unknown as LaneAdapter;
  return { adapter, calls };
}

const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function all(
  nodes: (HostNode | string)[],
  predicate: (n: HostNode) => boolean,
) {
  const found: HostNode[] = [];
  const walk = (node: HostNode | string) => {
    if (typeof node === "string") return;
    if (predicate(node)) found.push(node);
    node.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const isButton = (node: HostNode) =>
  /(^| )button( |$)/.test(node.props.className ?? "");

let bundle: StoreConsumersBundle;
test.before(async () => {
  bundle = await loadStoreConsumers();
});

function mount(items: ReturnType<typeof applied>[] = []) {
  const values = new Map<string, string>();
  const query: any = {
    select: (selector: string) => ({
      invoke: (call: { method: string; params?: { value?: string } }) => ({
        exec() {
          if (call.method === "setValue")
            values.set(selector, call.params?.value ?? "");
        },
      }),
    }),
  };
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  const mock = deferredAdapter(items);
  let dirty = false;
  const view = bundle.runtime.mount(bundle.URLConnections, {
    adapter: mock.adapter,
    repositories: [
      repository("repo-one", "One"),
      repository("repo-two", "Two"),
    ],
    dirty: (value: boolean) => {
      dirty = value;
    },
  });
  const buttons = (label: string) =>
    all(view.tree(), isButton).filter((node) => text(node) === label);
  const button = (label: string, index = 0) => {
    const found = buttons(label)[index];
    assert.ok(found, `${label} is rendered`);
    return found;
  };
  const input = () => {
    const found = all(view.tree(), (node) => node.type === "input")[0];
    assert.ok(found, "URL prefix input is rendered");
    return found;
  };
  const shown = () => view.tree().map(text).join("");
  const tap = async (label: string, index = 0) => {
    button(label, index).props.bindtap();
    await view.settle();
  };
  const type = async (value: string) => {
    input().props.bindinput({ detail: { value } });
    await view.settle();
  };
  return {
    ...mock,
    view,
    values,
    buttons,
    button,
    input,
    shown,
    tap,
    type,
    dirty: () => dirty,
  };
}

async function openDraft(f: ReturnType<typeof mount>, mode: "create" | "edit") {
  await f.view.settle();
  await f.tap(mode === "create" ? "Add connection" : "Edit");
  await f.type("https://submitted.example/app");
  await f.tap("One");
}

for (const mode of ["create", "edit"] as const) {
  test(`${mode} locks URL controls and acknowledges only the submitted draft`, async () => {
    const f = mount(
      mode === "edit" ? [applied("https://old.example/app", "repo-one")] : [],
    );
    await openDraft(f, mode);
    f.button("Save").props.bindtap();
    await f.view.settle();
    assert.equal(f.calls.length, 1);
    assert.equal(
      f.calls[0].method,
      mode === "create" ? "CreateURLConnection" : "SaveURLConnection",
    );
    assert.deepEqual(f.calls[0].input.connection, {
      prefix: "https://submitted.example/app",
      repositoryId: "repo-one",
    });
    assert.equal(f.input().props.disabled, true);
    assert.match(f.button("Two").props.className, /disabled/);
    assert.equal(f.button("Two").props.focusable, false);
    assert.match(f.button("Saving…").props.className, /disabled/);
    f.input().props.bindinput({
      detail: { value: "https://blocked.example/app" },
    });
    f.button("Two").props.bindtap();
    f.button("Two").props.bindkeydown({ key: "Enter" });
    f.button("Two").props.bindkeydown({ key: " " });
    await f.view.settle();
    assert.equal(
      f.values.get(`#${f.input().props.id}`),
      "https://submitted.example/app",
    );
    assert.equal(f.calls.length, 1);
    f.calls[0].resolve({ result: { result: ResultKind.APPLIED } });
    await f.view.settle();
    assert.equal(f.dirty(), false);
    assert.equal(f.buttons("Save").length, 0);
    f.view.unmount();
  });

  test(`${mode} keeps an input accepted just before Save and does not submit it`, async () => {
    const f = mount(
      mode === "edit" ? [applied("https://old.example/app", "repo-one")] : [],
    );
    await openDraft(f, mode);
    const prefix = f.input();
    const save = f.button("Save");
    const otherRepository = f.button("Two");
    prefix.props.bindinput({ detail: { value: "https://later.example/app" } });
    otherRepository.props.bindtap();
    save.props.bindtap();
    await f.view.settle();
    assert.equal(
      f.calls[0].input.connection?.prefix,
      "https://submitted.example/app",
    );
    f.calls[0].resolve({ result: { result: ResultKind.APPLIED } });
    await f.view.settle();
    assert.equal(f.dirty(), true);
    assert.equal(
      f.values.get(`#${f.input().props.id}`),
      "https://later.example/app",
    );
    assert.match(f.button("Two").props.className, /primary/);
    assert.equal(f.calls.length, 1);
    f.view.unmount();
  });

  test(`${mode} advances a retained draft to the successful reply version`, async () => {
    const f = mount(
      mode === "edit" ? [applied("https://old.example/app", "repo-one")] : [],
    );
    await openDraft(f, mode);
    f.input().props.bindinput({
      detail: { value: "https://later.example/app" },
    });
    f.button("Two").props.bindtap();
    f.button("Save").props.bindtap();
    await f.view.settle();

    const first = f.calls[0].input;
    f.calls[0].resolve({
      result: {
        result: ResultKind.APPLIED,
        version: rule("https://submitted.example/app", "repo-one", "saved-v2"),
      },
    });
    await f.view.settle();
    assert.equal(f.dirty(), true);
    assert.equal(f.calls.length, 1);

    await f.tap("Save");
    assert.equal(f.calls.length, 2);
    assert.equal(f.calls[1].method, "SaveURLConnection");
    assert.equal(f.calls[1].input.expectedVersionId, "saved-v2");
    assert.deepEqual(f.calls[1].input.connection, {
      prefix: "https://later.example/app",
      repositoryId: "repo-two",
    });
    assert.notEqual(f.calls[1].input.meta.requestId, first.meta.requestId);
    f.calls[1].resolve({
      result: {
        result: ResultKind.APPLIED,
        version: rule("https://later.example/app", "repo-two", "saved-v3"),
      },
    });
    await f.view.settle();
    assert.equal(f.dirty(), false);
    f.view.unmount();
  });

  test(`${mode} retains failed and conflicting drafts with retry identity`, async () => {
    const f = mount(
      mode === "edit" ? [applied("https://old.example/app", "repo-one")] : [],
    );
    await openDraft(f, mode);
    await f.tap("Save");
    const first = f.calls[0].input;
    f.calls[0].reject(new Error("offline"));
    await f.view.settle();
    assert.match(f.shown(), /offline/);
    assert.equal(f.dirty(), true);
    assert.equal(f.input().props.disabled, false);
    await f.tap("Retry");
    assert.deepEqual(f.calls[1].input, first);
    f.calls[1].resolve({ result: { result: ResultKind.CONFLICT } });
    await f.view.settle();
    assert.match(f.shown(), /The rule changed/);
    assert.equal(f.dirty(), true);
    await f.tap("Retry");
    assert.deepEqual(f.calls[2].input, first);
    f.calls[2].reject(new Error("offline again"));
    await f.view.settle();
    await f.type("https://changed.example/app");
    await f.tap("Retry");
    assert.notEqual(f.calls[3].input.meta.requestId, first.meta.requestId);
    assert.equal(
      f.calls[3].input.connection?.prefix,
      "https://changed.example/app",
    );
    f.calls[3].resolve({ result: { result: ResultKind.APPLIED } });
    await f.view.settle();
    f.view.unmount();
  });
}

test("discard cancellation, missing repository and URL input validation stay intact", async () => {
  const f = mount();
  await f.view.settle();
  await f.tap("Add connection");
  await f.type("ftp://invalid.example/app");
  await f.tap("Save");
  assert.equal(f.calls.length, 0);
  assert.match(f.shown(), /select a repository/);
  await f.tap("Two");
  await f.tap("Retry");
  assert.equal(f.calls.length, 1);
  f.calls[0].reject(new Error("Invalid URL prefix"));
  await f.view.settle();
  assert.match(f.shown(), /Invalid URL prefix/);
  const dialog = all(
    f.view.tree(),
    (node) => node.props.className === "dialog",
  )[0];
  assert.ok(dialog);
  const close = all([dialog], isButton).find((node) => text(node) === "Close");
  assert.ok(close);
  close.props.bindtap();
  await f.view.settle();
  await f.tap("Keep editing");
  assert.equal(f.dirty(), true);
  assert.equal(
    f.values.get(`#${f.input().props.id}`),
    "ftp://invalid.example/app",
  );
  f.view.unmount();
});

test("conflict resolution locks alternatives and retains failed retries", async () => {
  const f = mount([conflicting()]);
  await f.view.settle();
  await f.tap("Resolve conflict");
  await f.tap("Use this version");
  await f.tap("Save resolution");
  assert.equal(f.calls.length, 1);
  assert.equal(f.calls[0].method, "ResolveConflict");
  assert.equal(
    f.calls[0].input.resolved?.body.value.prefix,
    "https://one.example/app",
  );
  for (const option of f.buttons("Use this version")) {
    assert.match(option.props.className, /disabled/);
    option.props.bindtap();
    option.props.bindkeydown({ key: "Enter" });
    option.props.bindkeydown({ key: " " });
  }
  assert.match(f.button("Two").props.className, /disabled/);
  f.button("Two").props.bindtap();
  f.button("Two").props.bindkeydown({ key: "Enter" });
  await f.view.settle();
  assert.equal(f.calls.length, 1);
  f.calls[0].reject(new Error("offline"));
  await f.view.settle();
  assert.match(f.shown(), /offline/);
  assert.equal(f.dirty(), true);
  assert.doesNotMatch(f.button("Two").props.className, /disabled/);
  await f.tap("Save resolution");
  assert.deepEqual(f.calls[1].input, f.calls[0].input);
  f.calls[1].resolve({ result: { result: ResultKind.CONFLICT } });
  await f.view.settle();
  assert.match(f.shown(), /The conflict changed/);
  assert.equal(f.dirty(), true);
  await f.tap("Two");
  await f.tap("Save resolution");
  assert.notEqual(
    f.calls[2].input.meta.requestId,
    f.calls[0].input.meta.requestId,
  );
  assert.equal(f.calls[2].input.resolved?.body.value.repositoryId, "repo-two");
  f.calls[2].resolve({ result: { result: ResultKind.APPLIED } });
  await f.view.settle();
  assert.equal(f.dirty(), false);
  assert.equal(f.buttons("Save resolution").length, 0);
  f.view.unmount();
});

test("a resolution selection accepted just before Save survives the older reply", async () => {
  const f = mount([conflicting()]);
  await f.view.settle();
  await f.tap("Resolve conflict");
  await f.tap("Use this version");
  const other = f.button("Use this version", 1);
  const save = f.button("Save resolution");
  other.props.bindtap();
  save.props.bindtap();
  await f.view.settle();
  assert.equal(
    f.calls[0].input.resolved?.body.value.prefix,
    "https://one.example/app",
  );
  f.calls[0].resolve({ result: { result: ResultKind.APPLIED } });
  await f.view.settle();
  assert.equal(f.dirty(), true);
  assert.match(f.button("Use this version", 1).props.className, /primary/);
  assert.equal(f.calls.length, 1);
  f.view.unmount();
});
