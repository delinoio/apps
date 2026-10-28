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

test("tab commands keep a busy RealQA tab from being closed through a stale listener", async () => {
  const { App, runtime, host } = await loadApp();
  let releaseCapture!: () => void;
  const capture = new Promise<void>((resolve) => {
    releaseCapture = resolve;
  });
  (globalThis as any).NativeModules = {
    bridge: {},
    LaneHost: {
      realqa: (raw: string, callback: (value: string) => void) => {
        const input = JSON.parse(raw);
        if (input.op === "state") {
          callback(
            JSON.stringify({
              ok: true,
              value: { window: "main", draft: null, shortcuts: {} },
            }),
          );
          return;
        }
        if (input.op === "capture") {
          void capture.then(() =>
            callback(JSON.stringify({ ok: true, value: undefined })),
          );
          return;
        }
        callback(JSON.stringify({ ok: true, value: undefined }));
      },
    },
  };
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
  const find = (predicate: (node: HostNode) => boolean) =>
    nodes(view.tree()).find(predicate);
  const command = async (value: TabCommand) => {
    host.emit("lane.workspace.tabCommand", { command: value });
    await view.settle();
  };
  try {
    await view.settle();
    await command(TabCommand.New);
    find(
      (node) => node.props["accessibility-label"] === "RealQA",
    )?.props.bindtap();
    await view.settle();
    host.emit("lane.realqa.select", { mode: "region" });
    await view.settle();
    await command(TabCommand.Close);
    const before = nodes(view.tree()).filter(
      (node) => node.props["accessibility-label"] === "Close RealQA",
    );
    assert.equal(before.length, 1, "the busy RealQA tab is present");
    assert.equal(
      nodes(view.tree()).some(
        (node) => node.props["accessibility-label"] === "Close RealQA",
      ),
      true,
      "a busy RealQA tab remains open",
    );
  } finally {
    releaseCapture();
    view.unmount();
  }
});

test("closing an unselected focused tab restores focus to the selected tab", async () => {
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
  const tab = (id: string) =>
    nodes(view.tree()).find((node) => node.props["data-workspace-tab"] === id);
  const order = () =>
    nodes(view.tree())
      .filter((node) => node.props["data-workspace-tab"])
      .map((node) => node.props["data-workspace-tab"]);
  const closeButtons = () =>
    nodes(view.tree()).filter((node) =>
      String(node.props["accessibility-label"] ?? "").startsWith("Close "),
    );
  try {
    await view.settle();
    host.route?.({ kind: "repository", id: "a" });
    await view.settle();
    host.route?.({ kind: "repository", id: "b" });
    await view.settle();
    tab("repository-a")?.props.bindtap();
    await view.settle();
    tab("repository-a")?.props.bindkeydown({ key: "ArrowDown" });
    await view.settle();
    assert.equal(tab("repository-a")?.props["data-tab-focus"], "false");
    assert.equal(tab("repository-b")?.props["data-tab-focus"], "true");
    host.emit("lane.workspace.tabCommand", { command: TabCommand.MoveLater });
    await view.settle();
    assert.deepEqual(order(), ["page-Tasks", "repository-b", "repository-a"]);
    assert.equal(
      tab("repository-b")?.props["data-tab-focus"],
      "true",
      "reordering retains focus on the same tab",
    );
    closeButtons()[1]?.props.bindtap();
    await view.settle();
    assert.equal(tab("repository-b"), undefined);
    assert.equal(
      tab("repository-a")?.props["data-tab-focus"],
      "true",
      "focus returns to the selected tab after its focused neighbor closes",
    );
    assert.equal(tab("repository-a")?.props.focusable, true);
  } finally {
    view.unmount();
  }
});
