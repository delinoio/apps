// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { loadApp, type HostNode } from "./harness/app.ts";
import { TabCommand } from "../src/tabs.ts";

function nodes(tree: HostNode[]): HostNode[] {
  const found: HostNode[] = [];
  const visit = (node: HostNode | string) => {
    if (typeof node === "string") return;
    found.push(node);
    node.children.forEach(visit);
  };
  tree.forEach(visit);
  return found;
}

test("tab commands use displayed order and reopen explicit closes in LIFO order", async () => {
  const { App, runtime, host } = await loadApp();
  (globalThis as any).NativeModules = {};
  let taskReads = 0;
  const adapter = {
    mode: "web",
    spaceId: "space-a",
    open: async (spaceId?: string) => {
      if (spaceId) adapter.spaceId = spaceId;
    },
    close() {},
    subscribe: () => () => {},
    local: async () => ({}),
    call: async (method: { name: string }) => {
      if (method.name === "ListTasks") taskReads++;
      return { items: [], nextCursor: "" };
    },
  };
  const view = runtime.mount(App, { providedAdapter: adapter });
  const order = () =>
    nodes(view.tree())
      .filter((node) => node.props["data-workspace-tab"])
      .map((node) => node.props["data-workspace-tab"]);
  const selected = () =>
    nodes(view.tree()).find((node) => node.props.id === "lane-root")?.props[
      "data-selected-tab"
    ];
  const command = async (value: TabCommand, index?: number) => {
    host.emit("lane.workspace.tabCommand", { command: value, index });
    await view.settle();
  };
  try {
    await view.settle();
    host.route?.({ kind: "repository", id: "a" });
    await view.settle();
    host.route?.({ kind: "repository", id: "b" });
    await view.settle();
    assert.deepEqual(order(), ["page-Tasks", "repository-a", "repository-b"]);
    const readsBeforeMove = taskReads;
    await command(TabCommand.MoveEarlier);
    assert.deepEqual(order(), ["page-Tasks", "repository-b", "repository-a"]);
    assert.equal(selected(), "repository-b");
    assert.equal(
      taskReads,
      readsBeforeMove,
      "reordering retains mounted queries",
    );
    host.emit("lane.commandPalette");
    await view.settle();
    await command(TabCommand.Close);
    assert.deepEqual(order(), ["page-Tasks", "repository-b", "repository-a"]);
    host.emit("lane.commandPalette");
    await view.settle();
    await command(TabCommand.Next);
    assert.equal(selected(), "repository-a");
    await command(TabCommand.SelectIndex, 0);
    assert.equal(selected(), "page-Tasks");
    await command(TabCommand.Previous);
    assert.equal(selected(), "repository-a");
    await command(TabCommand.Close);
    assert.deepEqual(order(), ["page-Tasks", "repository-b"]);
    await command(TabCommand.Close);
    assert.deepEqual(order(), ["page-Tasks"]);
    const readsBeforeReopen = taskReads;
    await command(TabCommand.Reopen);
    assert.deepEqual(order(), ["page-Tasks", "repository-b"]);
    assert.ok(
      taskReads > readsBeforeReopen,
      "reopened panes read current data",
    );
    await command(TabCommand.Reopen);
    assert.deepEqual(order(), ["page-Tasks", "repository-b", "repository-a"]);
    assert.equal(selected(), "repository-a");
    await command(TabCommand.Close);
    host.route?.({ kind: "repository", id: "a" });
    await view.settle();
    await command(TabCommand.Reopen);
    assert.equal(order().filter((id) => id === "repository-a").length, 1);
    assert.equal(selected(), "repository-a");
    await command(TabCommand.SelectIndex, 7);
    assert.equal(selected(), "repository-a", "unavailable indexes do nothing");
    host.route?.({ kind: "repository", id: "new-space", spaceId: "space-b" });
    await view.settle();
    assert.equal(adapter.spaceId, "space-b");
    assert.deepEqual(order(), ["page-Tasks", "repository-new-space"]);
    await command(TabCommand.Reopen);
    assert.deepEqual(order(), ["page-Tasks", "repository-new-space"]);
    await command(TabCommand.Close);
    await command(TabCommand.Close);
    assert.deepEqual(order(), ["page-Tasks"]);
    await command(TabCommand.Reopen);
    assert.deepEqual(
      order(),
      ["page-Tasks"],
      "final-tab fallback is deduplicated",
    );
    await command(TabCommand.New);
    assert.deepEqual(order(), ["page-Tasks", "new-tab"]);
    await command(TabCommand.Close);
    await command(TabCommand.New);
    await command(TabCommand.Reopen);
    assert.deepEqual(order(), ["page-Tasks", "new-tab"]);
  } finally {
    view.unmount();
  }
});

test("a discarded new-task draft reopens empty with a new identity", async () => {
  const { App, runtime, host } = await loadApp();
  (globalThis as any).NativeModules = {};
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      spaceId: "space-a",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      local: async () => ({}),
      call: async () => ({ items: [], nextCursor: "" }),
    },
  });
  const find = (label: string) =>
    nodes(view.tree()).find(
      (node) => node.props["accessibility-label"] === label,
    );
  const selected = () =>
    nodes(view.tree()).find((node) => node.props.id === "lane-root")?.props[
      "data-selected-tab"
    ];
  const command = async (value: TabCommand) => {
    host.emit("lane.workspace.tabCommand", { command: value });
    await view.settle();
  };
  try {
    await view.settle();
    find("+ New task")?.props.bindtap();
    await view.settle();
    const original = selected();
    assert.match(original, /^task-/);
    find("Title")?.props.bindinput({ detail: { value: "Discard this draft" } });
    await view.settle();
    assert.ok(find("Discard this draft, Unsaved changes"));
    await command(TabCommand.MoveEarlier);
    assert.ok(find("Discard this draft, Unsaved changes"));
    assert.equal(selected(), original);
    await command(TabCommand.Close);
    assert.ok(find("Keep editing"));
    await command(TabCommand.Reopen);
    assert.equal(selected(), original, "the dialog blocks reopening");
    find("Keep editing")?.props.bindtap();
    await view.settle();
    await command(TabCommand.Close);
    find("Discard changes")?.props.bindtap();
    await view.settle();
    assert.equal(selected(), "page-Tasks");
    await command(TabCommand.Reopen);
    assert.match(selected(), /^task-/);
    assert.notEqual(selected(), original);
    assert.ok(find("New task"), "the discarded title does not return");
    const reopened = selected();
    await command(TabCommand.Reopen);
    assert.equal(selected(), reopened, "empty history is a no-op");
  } finally {
    view.unmount();
  }
});
