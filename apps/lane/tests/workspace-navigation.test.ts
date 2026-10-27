// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { loadApp, type HostNode } from "./harness/app.ts";

function visibleNodes(tree: HostNode[]): HostNode[] {
  const nodes: HostNode[] = [];
  const walk = (node: HostNode | string) => {
    if (typeof node === "string" || node.props["data-active-tab"] === "false")
      return;
    nodes.push(node);
    node.children.forEach(walk);
  };
  tree.forEach(walk);
  return nodes;
}
const repository = (id: string) => ({
  version: {
    entityId: id,
    body: {
      case: "repository",
      value: { displayName: id, normalizedUrl: `github.com/test/${id}` },
    },
  },
  alternatives: [],
});

test("a composer initialized before tab reads loads every repository page itself", async () => {
  const { App, runtime } = await loadApp();
  const state = {
    window: "composer",
    shortcuts: {},
    draft: {
      title: "Captured task",
      description: "",
      repositoryId: "repo-b",
      images: [
        {
          id: "image",
          png: "",
          width: 10,
          height: 10,
          layers: [],
          metadata: {},
        },
      ],
    },
  };
  (globalThis as any).NativeModules = {
    bridge: {},
    LaneHost: {
      realqa: (_request: string, callback: (raw: string) => void) =>
        callback(JSON.stringify({ ok: true, value: state })),
    },
  };
  const cursors: string[] = [];
  const completions: ((result: any) => void)[] = [];
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      spaceId: "space",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      local: async () => ({}),
      call: async (method: any, input: any) => {
        if (method.name === "ListRepositories") {
          cursors.push(input.page.cursor);
          if (!input.page.cursor)
            return new Promise((resolve) => {
              completions.push(resolve);
            });
          return { items: [repository("repo-b")], nextCursor: "" };
        }
        return { items: [], nextCursor: "" };
      },
    },
  });
  try {
    await view.settle();
    assert.deepEqual(cursors, ["", ""]);
    assert.ok(
      !visibleNodes(view.tree()).some((n) => n.props["data-workspace-tab"]),
    );
    completions[1]({ items: [repository("repo-a")], nextCursor: "second" });
    await view.settle();
    assert.deepEqual(cursors, ["", "", "second"]);
    completions[0]({ items: [], nextCursor: "" });
    await view.settle();
    const nodes = visibleNodes(view.tree());
    assert.ok(nodes.some((n) => n.props["accessibility-label"] === "repo-b"));
    const save = nodes.find(
      (n) => n.props["accessibility-label"] === "Save to Lane",
    );
    assert.ok(save);
    assert.ok(
      !save.props.className.includes("disabled"),
      "a URL-selected repository validates after loading",
    );
  } finally {
    view.unmount();
  }
});
