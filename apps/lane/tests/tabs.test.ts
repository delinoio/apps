// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import {
  adjacentTab,
  allTasks,
  closedTab,
  insertRestoredTab,
  moveTab,
  recordClosedTab,
  restoredTarget,
  tabAtIndex,
  tabAtOffset,
  tabCommandRequest,
  tabIdentity,
  TabCommand,
  TabKind,
} from "../src/tabs.ts";
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
test("ordered tab selection and movement preserve tab objects and clamp edges", () => {
  const tabs = ["a", "b", "c"].map((id) => ({
    id,
    target: { kind: TabKind.Repository, repositoryId: id, label: id },
  }));
  assert.equal(tabAtOffset(tabs, "c", 1), "a");
  assert.equal(tabAtOffset(tabs, "a", -1), "c");
  assert.equal(tabAtIndex(tabs, 1), "b");
  assert.equal(tabAtIndex(tabs, 9), undefined);
  assert.equal(tabAtIndex(tabs, -1), undefined);
  const moved = moveTab(tabs, "b", -9);
  assert.deepEqual(
    moved.map((tab) => tab.id),
    ["b", "a", "c"],
  );
  assert.equal(moved[0], tabs[1]);
  assert.equal(moveTab(moved, "b", 0), moved);
  assert.deepEqual(
    moveTab(moved, "b", 99).map((tab) => tab.id),
    ["a", "c", "b"],
  );
  assert.equal(moveTab(tabs, "missing", 0), tabs);
});
test("closed history retains only safe destinations and the latest twenty entries", () => {
  const draft = {
    id: "task-old",
    target: {
      kind: TabKind.NewTask,
      taskId: "task-old",
      repositoryId: "repo-a",
      label: "Unsaved title",
      prompt: "must not enter history",
    },
  };
  const entry = closedTab(draft, 3);
  assert.deepEqual(entry, {
    target: {
      kind: TabKind.NewTask,
      label: "New task",
      repositoryId: "repo-a",
    },
    position: 3,
    label: "New task",
  });
  assert.equal(
    tabIdentity(restoredTarget(entry, "task-fresh")),
    "task-task-fresh",
  );
  assert.deepEqual(closedTab(draft, 3, "task-saved").target, {
    kind: TabKind.Task,
    label: "Task",
    taskId: "task-saved",
    repositoryId: "repo-a",
  });
  const history = Array.from({ length: 21 }, (_, position) =>
    closedTab(
      {
        id: `repository-${position}`,
        target: {
          kind: TabKind.Repository,
          repositoryId: `${position}`,
          label: "Repository",
        },
      },
      position,
    ),
  ).reduce(recordClosedTab, [] as ReturnType<typeof closedTab>[]);
  assert.equal(history.length, 20);
  assert.equal(history[0].target.repositoryId, "1");
  assert.equal(history.at(-1)?.target.repositoryId, "20");
});
test("reopen inserts at the previous position or selects an existing pane", () => {
  const entry = closedTab(
    {
      id: "repository-b",
      target: {
        kind: TabKind.Repository,
        repositoryId: "b",
        label: "Repository",
      },
    },
    1,
  );
  const first = { id: "page-Tasks", target: allTasks() };
  const last = {
    id: "repository-c",
    target: { kind: TabKind.Repository, repositoryId: "c", label: "C" },
  };
  const restored = insertRestoredTab(
    [first, last],
    entry,
    restoredTarget(entry, "unused"),
  );
  assert.deepEqual(
    restored.tabs.map((tab) => tab.id),
    ["page-Tasks", "repository-b", "repository-c"],
  );
  const duplicate = insertRestoredTab(
    restored.tabs,
    entry,
    restoredTarget(entry, "unused"),
  );
  assert.equal(duplicate.tabs, restored.tabs);
  assert.equal(duplicate.selected, "repository-b");
});
test("tab commands reject invalid indexes and row moves without an owner", () => {
  assert.equal(
    tabCommandRequest({ command: TabCommand.SelectIndex, index: 8 }),
    undefined,
  );
  assert.equal(
    tabCommandRequest({ command: TabCommand.SelectIndex, index: -1 }),
    undefined,
  );
  assert.equal(tabCommandRequest({ command: TabCommand.MoveFirst }), undefined);
  assert.equal(tabCommandRequest({ command: "unknown" }), undefined);
  assert.deepEqual(
    tabCommandRequest({ command: TabCommand.SelectIndex, index: 7 }),
    {
      command: TabCommand.SelectIndex,
      index: 7,
    },
  );
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

test("one window subscription serves multiple retained panes and is released once", async () => {
  const { loadApp } = await import("./harness/app.ts");
  const { App, runtime } = await loadApp();
  let subscriptions = 0;
  let released = 0;
  let opened = 0;
  let closed = 0;
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      open: async () => {
        opened++;
      },
      close: () => {
        closed++;
      },
      subscribe: () => {
        subscriptions++;
        return () => {
          released++;
        };
      },
      call: async () => ({ items: [], nextCursor: "" }),
      local: async () => ({}),
    },
  });
  const visit = (name: string) => {
    let target: any;
    const walk = (node: any) => {
      if (typeof node === "string" || node.props["data-active-tab"] === "false")
        return;
      if (
        node.props["accessibility-label"] === name &&
        !node.props["data-workspace-tab"]
      )
        target = node;
      node.children.forEach(walk);
    };
    view.tree().forEach(walk);
    assert.ok(target, `visible navigation to ${name}`);
    target.props.bindtap();
  };
  try {
    await view.settle();
    visit("Repositories");
    await view.settle();
    visit("Events");
    await view.settle();
    visit("All tasks");
    await view.settle();
    assert.equal(opened, 1);
    assert.equal(
      subscriptions,
      1,
      "adding and selecting tabs does not multiply invalidations",
    );
    assert.equal(
      released,
      0,
      "selecting a pane must not unsubscribe or close shared storage",
    );
  } finally {
    view.unmount();
  }
  assert.equal(released, 1);
  assert.equal(closed, 1);
});
