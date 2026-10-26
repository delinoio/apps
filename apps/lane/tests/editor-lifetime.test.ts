// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { EditorLifetime, readTaskOwned } from "../src/editorLifetime.ts";

interface Task {
  id: string;
  title: string;
}
interface Deferred {
  resolve(value: { result?: Task }): void;
  reject(error: Error): void;
}

// A minimal stand-in for App's editor state. It uses the real lifetime helper and
// mirrors the call sites: reads begin a lifetime, accepted transitions supersede.
function editor() {
  const lifetime = new EditorLifetime();
  const state: {
    draft?: { id: string; title: string; dirty: boolean };
    current?: Task;
    page: string;
    error: string;
  } = { page: "Tasks", error: "" };
  const pending: Deferred[] = [];
  const adapterCall = () =>
    new Promise<{ result?: Task }>((resolve, reject) => {
      pending.push({ resolve, reject });
    });
  const open = (_id: string) =>
    readTaskOwned<Task>(lifetime, adapterCall, {
      opened: (task) => {
        state.current = task;
        state.draft = { id: task.id, title: task.title, dirty: false };
        state.page = "Tasks";
      },
      failed: (e) => {
        state.error = e instanceof Error ? e.message : "Task is unavailable.";
      },
    });
  // Mirrors App.navigate: a dirty draft defers the transition until confirmed.
  let prompt: (() => void) | undefined;
  const navigate = (fn: () => void) => {
    const proceed = () => {
      lifetime.supersede();
      fn();
    };
    if (state.draft?.dirty) prompt = proceed;
    else proceed();
  };
  return {
    lifetime,
    state,
    pending,
    open,
    navigate,
    confirm: () => prompt?.(),
    cancel: () => {
      prompt = undefined;
    },
    edit: (title: string) => {
      state.draft = { ...state.draft!, title, dirty: true };
    },
  };
}

test("late A success leaves edited B intact", async () => {
  const e = editor();
  const a = e.open("A");
  const b = e.open("B");
  e.pending[1].resolve({ result: { id: "B", title: "Task B" } });
  await b;
  e.edit("B unsaved");
  e.pending[0].resolve({ result: { id: "A", title: "Task A" } });
  await a;
  assert.deepEqual(e.state.draft, {
    id: "B",
    title: "B unsaved",
    dirty: true,
  });
  assert.equal(e.state.current?.id, "B");
  assert.equal(e.state.page, "Tasks");
});

test("late A failure leaves edited B and its error flow intact", async () => {
  const e = editor();
  const a = e.open("A");
  const b = e.open("B");
  e.pending[1].resolve({ result: { id: "B", title: "Task B" } });
  await b;
  e.edit("B unsaved");
  e.pending[0].reject(new Error("A unavailable"));
  await a;
  assert.equal(e.state.error, "");
  assert.equal(e.state.draft?.title, "B unsaved");
  assert.equal(e.state.draft?.dirty, true);
});

test("a read superseded by New task cannot replace the new draft", async () => {
  for (const outcome of ["resolve", "reject"] as const) {
    const e = editor();
    const a = e.open("A");
    e.navigate(() => {
      e.state.current = undefined;
      e.state.draft = { id: "new", title: "", dirty: false };
    });
    e.edit("unsaved new task");
    if (outcome === "resolve")
      e.pending[0].resolve({ result: { id: "A", title: "Task A" } });
    else e.pending[0].reject(new Error("A unavailable"));
    await a;
    assert.deepEqual(e.state.draft, {
      id: "new",
      title: "unsaved new task",
      dirty: true,
    });
    assert.equal(e.state.error, "");
    assert.equal(e.state.current, undefined);
  }
});

test("accepted page change or editor closure drops pending reads", async () => {
  for (const target of ["Settings", "closed"]) {
    const e = editor();
    const a = e.open("A");
    e.navigate(() => {
      e.state.draft = undefined;
      e.state.current = undefined;
      e.state.page = target;
    });
    e.pending[0].resolve({ result: { id: "A", title: "Task A" } });
    await a;
    assert.equal(e.state.page, target);
    assert.equal(e.state.draft, undefined);
    assert.equal(e.state.current, undefined);
  }
});

test("adapter or data-space disposal drops pending reads", async () => {
  const e = editor();
  const a = e.open("A");
  e.lifetime.supersede();
  e.pending[0].resolve({ result: { id: "A", title: "Task A" } });
  await a;
  assert.equal(e.state.draft, undefined);
  const b = e.open("B");
  e.lifetime.supersede();
  e.pending[1].reject(new Error("gone"));
  await b;
  assert.equal(e.state.error, "");
});

test("reopening the same task starts a distinct lifetime", async () => {
  const e = editor();
  const first = e.open("A");
  const b = e.open("B");
  const latest = e.open("A");
  e.pending[2].resolve({ result: { id: "A", title: "latest A" } });
  await latest;
  e.edit("latest A unsaved");
  e.pending[0].resolve({ result: { id: "A", title: "older A" } });
  await first;
  e.pending[1].resolve({ result: { id: "B", title: "Task B" } });
  await b;
  assert.deepEqual(e.state.draft, {
    id: "A",
    title: "latest A unsaved",
    dirty: true,
  });
});

test("a single current read keeps the existing success, error and missing behavior", async () => {
  const e = editor();
  const ok = e.open("A");
  e.pending[0].resolve({ result: { id: "A", title: "Task A" } });
  await ok;
  assert.equal(e.state.draft?.id, "A");
  assert.equal(e.state.draft?.dirty, false);
  e.edit("kept");
  const missing = e.open("B");
  e.pending[1].resolve({});
  await missing;
  assert.equal(e.state.draft?.title, "kept");
  assert.equal(e.state.error, "");
  const failed = e.open("B");
  e.pending[2].reject(new Error("offline"));
  await failed;
  assert.equal(e.state.error, "offline");
  assert.equal(e.state.draft?.title, "kept");
  const retry = e.open("B");
  e.pending[3].resolve({ result: { id: "B", title: "Task B" } });
  await retry;
  assert.equal(e.state.draft?.id, "B");
});

test("only an accepted discard prompt supersedes the pending read", async () => {
  // Cancelling keeps ownership of a read started before the prompt.
  const cancelled = editor();
  const read = cancelled.open("A");
  cancelled.state.draft = { id: "B", title: "B", dirty: true };
  cancelled.navigate(() => {
    cancelled.state.draft = undefined;
  });
  cancelled.cancel();
  cancelled.pending[0].resolve({ result: { id: "A", title: "Task A" } });
  await read;
  assert.equal(cancelled.state.draft?.id, "A");

  // Confirming performs the transition and obsolete reads cannot undo it.
  const confirmed = editor();
  const stale = confirmed.open("A");
  confirmed.state.draft = { id: "B", title: "B", dirty: true };
  confirmed.navigate(() => {
    confirmed.state.draft = undefined;
    confirmed.state.page = "Settings";
  });
  confirmed.confirm();
  confirmed.pending[0].resolve({ result: { id: "A", title: "Task A" } });
  await stale;
  assert.equal(confirmed.state.draft, undefined);
  assert.equal(confirmed.state.page, "Settings");
});

test("ownership is lifetime based, not task ID based", () => {
  const lifetime = new EditorLifetime();
  const first = lifetime.begin();
  lifetime.begin();
  const latest = lifetime.begin();
  assert.equal(lifetime.owns(first), false);
  assert.equal(lifetime.owns(latest), true);
  lifetime.supersede();
  assert.equal(lifetime.owns(latest), false);
});
