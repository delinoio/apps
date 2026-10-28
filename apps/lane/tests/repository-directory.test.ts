// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { ResultKind } from "@repo/protos/lane/v1/lane_pb";
import { loadApp, type HostNode } from "./harness/app.ts";

type Reply = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  done: boolean;
};

function visible(tree: HostNode[]): HostNode[] {
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

function words(node: HostNode | string): string {
  return typeof node === "string"
    ? node
    : node.children.map(words).join(" ").replace(/\s+/g, " ");
}

const version = (id: string, name: string, url: string) => ({
  version: {
    entityId: id,
    versionId: `${id}-version`,
    body: {
      case: "repository",
      value: { displayName: name, normalizedUrl: url },
    },
  },
  alternatives: [],
});

test("repository directory distinguishes load, empty and retained-row refresh failure", async () => {
  const { App, runtime } = await loadApp();
  const reads: Reply[] = [];
  let invalidate = () => {};
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      spaceId: "fixture",
      open: async () => {},
      close() {},
      subscribe: (callback: () => void) => {
        invalidate = callback;
        return () => {};
      },
      local: async () => ({}),
      call: (method: { name: string }) => {
        if (method.name !== "ListRepositories")
          return Promise.resolve({ items: [], nextCursor: "" });
        return new Promise((resolve, reject) =>
          reads.push({ resolve, reject, done: false }),
        );
      },
    },
  });
  const nodes = () => visible(view.tree());
  const text = () => nodes().map(words).join(" ");
  const tap = (label: string) => {
    const action = nodes().find(
      (node) =>
        node.props["accessibility-label"] === label && node.props.bindtap,
    );
    assert.ok(action, label);
    action.props.bindtap();
  };
  const settleReads = (reply: unknown, failure = false) => {
    for (const read of reads.filter((item) => !item.done)) {
      read.done = true;
      if (failure) read.reject(new Error("Repository read failed"));
      else read.resolve(reply);
    }
  };
  try {
    await view.settle();
    tap("Repositories");
    await view.settle();
    assert.match(text(), /Loading repositories/);
    assert.doesNotMatch(text(), /No repositories yet/);
    settleReads({ items: [], nextCursor: "" });
    await view.settle();
    assert.match(text(), /No repositories yet/);

    invalidate();
    await view.settle();
    assert.match(text(), /Loading repositories/);
    assert.doesNotMatch(text(), /No repositories yet/);
    settleReads({
      items: [
        version("website", "Website", "github.com/example/website"),
        version("mobile", "Mobile app", "github.com/example/mobile"),
        version("docs", "", "github.com/example/docs"),
      ],
      nextCursor: "",
    });
    await view.settle();
    assert.match(text(), /Website/);
    assert.match(text(), /Mobile app/);
    assert.match(text(), /github.com\/example\/docs/);
    for (const action of [
      "View tasks",
      "Labels, milestones and cycles",
      "Rename or change URL",
      "Delete repository",
    ])
      assert.ok(
        nodes().some((node) => node.props["accessibility-label"] === action),
      );

    invalidate();
    await view.settle();
    settleReads(undefined, true);
    await view.settle();
    assert.match(text(), /Existing rows remain available/);
    assert.match(text(), /Website/);
    tap("Retry loading repositories");
    await view.settle();
    settleReads({
      items: [version("website", "Website", "github.com/example/website")],
      nextCursor: "",
    });
    await view.settle();
    assert.doesNotMatch(text(), /could not be refreshed/);
  } finally {
    view.unmount();
  }
});

test("a successful refresh clears its error without clearing task-open errors", async () => {
  const { App, runtime } = await loadApp();
  let invalidate = () => {};
  let failTaskRefresh = false;
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      spaceId: "fixture",
      open: async () => {},
      close() {},
      subscribe: (callback: () => void) => {
        invalidate = callback;
        return () => {};
      },
      local: async () => ({}),
      call: (method: { name: string }) => {
        if (method.name === "ListRepositories")
          return Promise.resolve({
            items: [
              version("website", "Website", "github.com/example/website"),
            ],
            nextCursor: "",
          });
        if (method.name === "ListTasks" && failTaskRefresh)
          return Promise.reject(new Error("Task refresh failed"));
        return Promise.resolve({ items: [], nextCursor: "" });
      },
    },
  });
  const nodes = () => visible(view.tree());
  const text = () => nodes().map(words).join(" ");
  const action = (label: string) => {
    const found = nodes().find(
      (node) =>
        node.props["accessibility-label"] === label && node.props.bindtap,
    );
    assert.ok(found, label);
    return found;
  };
  try {
    await view.settle();
    action("Repositories").props.bindtap();
    await view.settle();
    assert.match(text(), /Website/);

    failTaskRefresh = true;
    invalidate();
    await view.settle();
    assert.match(text(), /Task refresh failed/);

    failTaskRefresh = false;
    action("Retry").props.bindtap();
    await view.settle();
    assert.doesNotMatch(text(), /Task refresh failed/);
  } finally {
    view.unmount();
  }
});

test("submitted repository save locks the form and cannot close a newer editor", async () => {
  const { App, runtime } = await loadApp();
  const saves: { input: any; reply: Reply }[] = [];
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      spaceId: "fixture",
      open: async () => {},
      close() {},
      subscribe: () => () => {},
      local: async () => ({}),
      call: (method: { name: string }, input: any) => {
        if (method.name === "CreateRepository")
          return new Promise((resolve, reject) =>
            saves.push({ input, reply: { resolve, reject, done: false } }),
          );
        return Promise.resolve({ items: [], nextCursor: "" });
      },
    },
  });
  const nodes = () => visible(view.tree());
  const action = (label: string) => {
    const found = nodes().find(
      (node) =>
        node.props["accessibility-label"] === label && node.props.bindtap,
    );
    assert.ok(found, label);
    return found;
  };
  const field = (label: string) => {
    const found = nodes().find(
      (node) =>
        node.type === "input" &&
        node.props["accessibility-label"]?.startsWith(label),
    );
    assert.ok(found, label);
    return found;
  };
  try {
    await view.settle();
    action("Repositories").props.bindtap();
    await view.settle();
    action("Add repository").props.bindtap();
    await view.settle();
    assert.equal(field("Repository URL").props["data-dialog-default"], "true");
    field("Repository URL").props.bindinput({
      detail: { value: "https://github.com/example/website.git" },
    });
    field("Display name").props.bindinput({ detail: { value: "Website" } });
    await view.settle();
    const save = action("Save");
    save.props.bindtap();
    save.props.bindtap();
    await view.settle();
    assert.equal(saves.length, 1);
    assert.equal(field("Repository URL").props.disabled, true);
    assert.equal(field("Display name").props.disabled, true);
    assert.ok(
      nodes().some((node) => node.props["accessibility-label"] === "Saving..."),
    );
    action("Close").props.bindtap();
    await view.settle();
    action("Add repository").props.bindtap();
    await view.settle();
    field("Repository URL").props.bindinput({
      detail: { value: "https://github.com/example/new" },
    });
    await view.settle();
    saves[0].reply.resolve({ result: { result: ResultKind.APPLIED } });
    await view.settle();
    assert.ok(
      nodes().some((node) => node.props["accessibility-label"] === "Save"),
    );
    action("Save").props.bindtap();
    await view.settle();
    assert.equal(saves.length, 2);
    assert.equal(saves[1].input.url, "https://github.com/example/new");
    saves[1].reply.reject(new Error("offline"));
    await view.settle();
    assert.match(nodes().map(words).join(" "), /offline/);
    assert.ok(
      nodes().some((node) => node.props["accessibility-label"] === "Save"),
    );
  } finally {
    view.unmount();
  }
});

test("lost repository access blocks editing and conflict alternatives stay explicit", async () => {
  const { App, runtime } = await loadApp();
  let invalidate = () => {};
  let rows: any[] = [
    version("website", "Website", "github.com/example/website"),
  ];
  const view = runtime.mount(App, {
    providedAdapter: {
      mode: "web",
      spaceId: "fixture",
      open: async () => {},
      close() {},
      subscribe: (callback: () => void) => {
        invalidate = callback;
        return () => {};
      },
      local: async () => ({}),
      call: (method: { name: string }) =>
        Promise.resolve({
          items: method.name === "ListRepositories" ? rows : [],
          nextCursor: "",
        }),
    },
  });
  const nodes = () => visible(view.tree());
  const action = (label: string) => {
    const found = nodes().find(
      (node) =>
        node.props["accessibility-label"] === label && node.props.bindtap,
    );
    assert.ok(found, label);
    return found;
  };
  try {
    await view.settle();
    action("Repositories").props.bindtap();
    await view.settle();
    action("Rename or change URL").props.bindtap();
    await view.settle();
    rows = [];
    invalidate();
    await view.settle();
    assert.match(
      nodes().map(words).join(" "),
      /This repository is unavailable/,
    );
    assert.match(action("Save").props.className, /disabled/);
    action("Close").props.bindtap();
    await view.settle();

    const conflictedRow: any = version(
      "website",
      "Website",
      "github.com/example/website",
    );
    conflictedRow.version.origin = { deviceName: "First device" };
    conflictedRow.alternatives = [
      {
        ...conflictedRow.version,
        versionId: "website-alternative",
        origin: { deviceName: "Second device" },
        body: {
          case: "repository",
          value: {
            displayName: "Site",
            normalizedUrl: "github.com/example/site",
          },
        },
      },
    ];
    rows = [conflictedRow];
    invalidate();
    await view.settle();
    assert.match(action("Rename or change URL").props.className, /disabled/);
    action("Resolve repository conflict").props.bindtap();
    await view.settle();
    const content = nodes().map(words).join(" ");
    assert.match(content, /Website/);
    assert.match(content, /github.com\/example\/website/);
    assert.match(content, /First device/);
    assert.match(content, /Site/);
    assert.match(content, /github.com\/example\/site/);
    assert.match(content, /Second device/);
  } finally {
    view.unmount();
  }
});
