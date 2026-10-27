// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { adjacentTab, allTasks, tabIdentity, TabKind } from "../src/tabs.ts";
import { LanePage } from "../src/navigation.ts";
// @ts-expect-error The native lifecycle module is shipped as JavaScript.
import { WorkspaceExit } from "../native/desktop/workspace-exit.mjs";

test("new and saved task destinations share identity without merging repository pages", () => {
  assert.equal(
    tabIdentity({ kind: TabKind.NewTask, taskId: "task-a", label: "New task" }),
    tabIdentity({ kind: TabKind.Task, taskId: "task-a", label: "Saved title" }),
  );
  assert.notEqual(
    tabIdentity({
      kind: TabKind.Repository,
      repositoryId: "repo-a",
      label: "Same title",
    }),
    tabIdentity({
      kind: TabKind.Repository,
      repositoryId: "repo-b",
      label: "Same title",
    }),
  );
  assert.notEqual(
    tabIdentity(allTasks()),
    tabIdentity({ kind: TabKind.Page, page: LanePage.Events, label: "Events" }),
  );
});
test("closing a selected tab chooses right, then left, then the empty fallback", () => {
  const tabs = ["a", "b", "c"].map((id) => ({ id, target: allTasks() }));
  assert.equal(adjacentTab(tabs, "b"), "c");
  assert.equal(adjacentTab(tabs, "c"), "b");
  assert.equal(adjacentTab(tabs.slice(0, 1), "a"), undefined);
});
test("native close and quit require explicit approval and never approve a busy operation", () => {
  const gate = new WorkspaceExit();
  assert.equal(gate.request("close"), true);
  gate.update(JSON.stringify({ dirty: true, busy: false, exit: false }));
  assert.equal(gate.request("quit"), false);
  // Saving an editor is not permission to execute an earlier exit request.
  assert.equal(
    gate.update(JSON.stringify({ dirty: false, busy: false, exit: false })),
    undefined,
  );
  assert.equal(
    gate.update(JSON.stringify({ dirty: false, busy: true, exit: true })),
    undefined,
  );
  assert.equal(
    gate.update(JSON.stringify({ dirty: false, busy: false, exit: true })),
    "quit",
  );
  assert.equal(
    gate.update(JSON.stringify({ dirty: false, busy: false, exit: true })),
    undefined,
  );
  assert.throws(() => gate.update('{"dirty":"false"}'));
});
