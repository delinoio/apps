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
  let ruleReads = 0;
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
        if (method.name === "ListURLConnections") ruleReads++;
        return { items: [], nextCursor: "" };
      },
    },
  });
  try {
    await view.settle();
    assert.deepEqual(cursors, ["", ""]);
    assert.equal(
      ruleReads,
      1,
      "URL rules load while repository choices are still pending",
    );
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

function tap(tree: HostNode[], label: string) {
  const target = visibleNodes(tree).find(
    (n) =>
      n.props["accessibility-label"] === label &&
      n.props.bindtap &&
      !n.props["data-workspace-tab"],
  );
  assert.ok(target, `visible action ${label}`);
  target.props.bindtap();
}
async function realqaMain() {
  const bundle = await loadApp();
  (globalThis as any).NativeModules = {
    bridge: {},
    LaneHost: {
      realqa: (_request: string, callback: (raw: string) => void) =>
        callback(
          JSON.stringify({
            ok: true,
            value: { window: "main", shortcuts: {}, draft: null },
          }),
        ),
    },
  };
  const view = bundle.runtime.mount(bundle.App, {
    providedAdapter: {
      mode: "web",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      call: async () => ({ items: [], nextCursor: "" }),
      local: async () => ({}),
    },
  });
  await view.settle();
  tap(view.tree(), "New tab");
  await view.settle();
  tap(view.tree(), "RealQA");
  await view.settle();
  tap(view.tree(), "Assign");
  await view.settle();
  return Object.assign(view, { host: bundle.host });
}

test("utility section targets are applied only after accepting RealQA discard", async () => {
  const view = await realqaMain();
  const app = () =>
    visibleNodes(view.tree()).find((n) => n.props.id === "lane-root");
  try {
    tap(view.tree(), "Account & sync");
    await view.settle();
    assert.equal(app()!.props["data-app"], "RealQA");
    tap(view.tree(), "Keep editing");
    await view.settle();
    assert.equal(app()!.props["data-app"], "RealQA");
    tap(view.tree(), "App settings");
    await view.settle();
    tap(view.tree(), "Discard changes");
    await view.settle();
    assert.equal(app()!.props["data-app"], "Lane");
    const target = visibleNodes(view.tree()).find(
      (n) => n.props["data-settings-target"],
    );
    assert.equal(
      target?.props["data-settings-target"],
      "notifications:1",
      "cancel does not consume or apply an earlier section request",
    );
  } finally {
    view.unmount();
  }
});

test("open-only host routes ask before leaving an edited RealQA view", async () => {
  const view = await realqaMain();
  const selectedApp = () =>
    visibleNodes(view.tree()).find((n) => n.props.id === "lane-root")?.props[
      "data-app"
    ];
  try {
    view.host.route!({ kind: "open" });
    await view.settle();
    assert.equal(selectedApp(), "RealQA");
    tap(view.tree(), "Keep editing");
    await view.settle();
    assert.equal(selectedApp(), "RealQA");
    assert.ok(
      visibleNodes(view.tree()).some(
        (n) => n.props["accessibility-label"] === "Assign keyboard shortcut",
      ),
    );
    view.host.route!({ kind: "open" });
    await view.settle();
    tap(view.tree(), "Discard changes");
    await view.settle();
    assert.equal(selectedApp(), "Lane");
  } finally {
    view.unmount();
  }
});

test("mobile repository scope opens the destination tab without changing the original identity", async () => {
  const { App, runtime } = await loadApp();
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      local: async () => ({}),
      call: async (method: any) => ({
        items:
          method.name === "ListRepositories"
            ? [repository("repo-a"), repository("repo-b")]
            : [],
        nextCursor: "",
      }),
    },
  });
  const nodes = () => visibleNodes(view.tree());
  const selected = () =>
    nodes().find((n) => n.props.id === "lane-root")?.props["data-selected-tab"];
  const selectRepositoryTab = async (id: string) => {
    const count = nodes().filter((n) => n.props["data-workspace-tab"]).length;
    if (!count) {
      const toggle = nodes().find((n) =>
        n.props["accessibility-label"]?.startsWith("Tabs ("),
      );
      toggle!.props.bindtap();
      await view.settle();
    }
    const tab = nodes().find(
      (n) => n.props["data-workspace-tab"] === `repository-${id}`,
    );
    assert.ok(tab);
    tab.props.bindtap();
    await view.settle();
  };
  try {
    await view.settle();
    tap(view.tree(), "repo-a");
    await view.settle();
    nodes()
      .find((n) => n.props.id === "lane-root")!
      .props.bindlayoutchange({ detail: { width: 390 } });
    await view.settle();
    tap(view.tree(), "repo-b");
    await view.settle();
    assert.equal(selected(), "repository-repo-b");
    await selectRepositoryTab("repo-a");
    assert.ok(
      nodes().some(
        (n) =>
          n.props["accessibility-label"] === "repo-a" &&
          n.props.className?.includes("primary"),
      ),
    );
    await selectRepositoryTab("repo-b");
    tap(view.tree(), "All repositories");
    await view.settle();
    assert.equal(selected(), "page-Tasks");
    await selectRepositoryTab("repo-a");
    tap(view.tree(), "+ New task");
    await view.settle();
    assert.ok(
      nodes().some(
        (n) =>
          n.props["accessibility-label"] === "Repository / move task: repo-a",
      ),
      "the original tab still creates tasks in repository A",
    );
  } finally {
    view.unmount();
  }
});

test("planning operation reports survive manager unmount until every request settles", async () => {
  const { App, runtime } = await loadApp();
  const label = {
    version: {
      entityId: "label",
      versionId: "version",
      body: {
        case: "label",
        value: { repositoryId: "repo-a", name: "Backend", color: "#075AFF" },
      },
    },
    alternatives: [],
  };
  const pending: {
    resolve: (value: any) => void;
    reject: (error: Error) => void;
  }[] = [];
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      local: async () => ({}),
      call: async (method: any) => {
        if (method.name === "SavePlanningItem")
          return new Promise((resolve, reject) =>
            pending.push({ resolve, reject }),
          );
        return {
          items:
            method.name === "ListRepositories"
              ? [repository("repo-a")]
              : method.name === "ListPlanningItems"
                ? [label]
                : [],
          nextCursor: "",
        };
      },
    },
  });
  const busy = () =>
    visibleNodes(view.tree())
      .find((n) => n.props["accessibility-label"] === "Close Repositories")
      ?.props.className.includes("disabled");
  const startAndLeave = async () => {
    tap(view.tree(), "Labels, milestones and cycles");
    await view.settle();
    tap(view.tree(), "Edit label Backend");
    await view.settle();
    tap(view.tree(), "Save");
    await view.settle();
    tap(view.tree(), "Back to repositories");
    await view.settle();
    assert.equal(
      busy(),
      true,
      "the hidden request still blocks closing the repository tab",
    );
  };
  try {
    await view.settle();
    tap(view.tree(), "Repositories");
    await view.settle();
    await startAndLeave();
    await startAndLeave();
    assert.equal(pending.length, 2);
    pending[0].resolve({ result: { result: 1, version: label.version } });
    await view.settle();
    assert.equal(
      busy(),
      true,
      "finishing an older manager cannot release a newer operation",
    );
    pending[1].reject(new Error("Simulated save failure"));
    await view.settle();
    assert.equal(
      busy(),
      false,
      "failure also releases the last operation after navigation",
    );
  } finally {
    view.unmount();
  }
});

test("mini-app chooser shares tabs and reuses destinations", async () => {
  const { App, runtime } = await loadApp();
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      call: async () => ({ items: [], nextCursor: "" }),
      local: async () => ({}),
    },
  });
  const nodes = () => visibleNodes(view.tree());
  const selected = () =>
    nodes().find((n) => n.props.id === "lane-root")?.props["data-selected-tab"];
  const tabIds = () =>
    nodes()
      .filter((n) => n.props["data-workspace-tab"])
      .map((n) => n.props["data-workspace-tab"]);
  try {
    await view.settle();
    nodes()
      .find(
        (n) => n.props["accessibility-label"] === "Search title or description",
      )!
      .props.bindinput({ detail: { value: "retained" } });
    tap(view.tree(), "New tab");
    await view.settle();
    assert.equal(selected(), "new-tab");
    assert.deepEqual(
      nodes()
        .filter((n) => n.props.className === "button miniapp-card")
        .map((n) => n.props["accessibility-label"]),
      ["Lane", "Pace", "RealQA"],
    );
    tap(view.tree(), "Pace");
    await view.settle();
    assert.equal(selected(), "app-Pace");
    assert.deepEqual(tabIds(), ["page-Tasks", "app-Pace"]);
    tap(view.tree(), "Back to Lane");
    await view.settle();
    assert.equal(selected(), "page-Tasks");
    tap(view.tree(), "New tab");
    await view.settle();
    tap(view.tree(), "Pace");
    await view.settle();
    assert.deepEqual(
      tabIds(),
      ["page-Tasks", "app-Pace"],
      "reopening an app consumes the chooser without duplicating the app",
    );
    tap(view.tree(), "Close Pace");
    await view.settle();
    assert.equal(selected(), "page-Tasks");
    const search = nodes().find(
      (n) => n.props["accessibility-label"] === "Search title or description",
    );
    assert.ok(search, "the retained Lane pane is visible again");
    tap(view.tree(), "New tab");
    await view.settle();
    tap(view.tree(), "Lane");
    await view.settle();
    assert.deepEqual(tabIds(), ["page-Tasks"]);
  } finally {
    view.unmount();
  }
});

test("closing the selected RealQA tab asks before discarding its edits", async () => {
  const view = await realqaMain();
  const app = () =>
    visibleNodes(view.tree()).find((n) => n.props.id === "lane-root");
  try {
    tap(view.tree(), "Close RealQA");
    await view.settle();
    assert.equal(app()?.props["data-selected-tab"], "app-RealQA");
    tap(view.tree(), "Keep editing");
    await view.settle();
    assert.equal(app()?.props["data-selected-tab"], "app-RealQA");
    tap(view.tree(), "Close RealQA");
    await view.settle();
    tap(view.tree(), "Discard changes");
    await view.settle();
    assert.equal(app()?.props["data-selected-tab"], "page-Tasks");
    assert.ok(
      !visibleNodes(view.tree()).some(
        (n) => n.props["data-workspace-tab"] === "app-RealQA",
      ),
    );
  } finally {
    view.unmount();
  }
});
