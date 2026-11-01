// SPDX-License-Identifier: Apache-2.0
// Synthetic host trees cover presentation states, not native capture acceptance.
import assert from "node:assert/strict";
import test from "node:test";
import { loadRealQA, type HostNode } from "./harness/realqa.ts";

const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function find(
  nodes: (HostNode | string)[],
  predicate: (node: HostNode) => boolean,
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
const hasClass = (node: HostNode, name: string) =>
  (node.props.className ?? "").split(" ").includes(name);
// Text of each individual text element, for exact status matches.
const texts = (tree: HostNode[]) =>
  find(tree, (node) => node.type === "text").map(text);
const buttons = (tree: HostNode[]) =>
  find(tree, (node) => hasClass(node, "button"));
const button = (tree: HostNode[], label: string, index = 0) => {
  const found = buttons(tree).filter((node) => text(node) === label)[index];
  assert.ok(found, `${label} is present`);
  return found;
};

test("capture cards retain equal mode order and use content-width columns", async () => {
  const bundle = await loadRealQA();
  const captures: string[] = [];
  const controller = {
    busy: false,
    capture: async (mode: string) => {
      captures.push(mode);
    },
  };
  const view = bundle.runtime.mount(bundle.CaptureActions, { controller });
  const grid = () =>
    find(view.tree(), (node) => hasClass(node, "capture-actions"))[0];
  assert.deepEqual(
    find(view.tree(), (node) => hasClass(node, "realqa-capture-card")).map(
      (node) => node.props["accessibility-label"],
    ),
    [
      "Full screen. Choose a monitor",
      "Window. Choose a window",
      "Region. Select an area",
      "Chrome element. Capture a visible element",
    ],
  );
  for (const [width, columns] of [
    [1100, 4],
    [900, 2],
    [899, 2],
    [390, 1],
    [360, 1],
  ]) {
    grid().props.bindlayoutchange({ detail: { width } });
    await view.settle();
    assert.ok(hasClass(grid(), `capture-columns-${columns}`));
  }
  const cards = find(view.tree(), (node) =>
    hasClass(node, "realqa-capture-card"),
  );
  cards[0].props.bindtap();
  cards[1].props.bindkeydown({ key: "Enter" });
  cards[2].props.bindkeydown({ key: " " });
  cards[3].props.bindtap();
  await view.settle();
  assert.deepEqual(captures, ["screen", "window", "region", "dom"]);
  view.unmount();
});

test("RealQA keeps loading neutral, mobile read-only, and identified Chrome actions", async () => {
  const bundle = await loadRealQA();
  const calls: Record<string, unknown>[] = [];
  const urls: string[] = [];
  bundle.captureHost.realqa = async (input) => {
    calls.push(input);
  };
  bundle.urlHost.openURL = async (value) => {
    urls.push(value);
  };
  const common = { repositories: [], back: () => {}, dirty: () => {} };
  const controller = (state?: Record<string, unknown>, available = true) => ({
    available,
    state,
    busy: false,
    error: "",
    capture: async () => {},
    run: async (action: () => Promise<unknown>) => {
      await action();
    },
  });
  let view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: controller(),
  });
  assert.match(
    view.tree().map(text).join(""),
    /Screen capture permission: Loading…/,
  );
  assert.match(view.tree().map(text).join(""), /Loading…/);
  assert.doesNotMatch(
    view.tree().map(text).join(""),
    /Connected|Synchronized|Granted/,
  );
  view.unmount();

  view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: controller(undefined, false),
  });
  assert.match(
    view.tree().map(text).join(""),
    /Capture on desktop\. View saved captures in Lane\./,
  );
  assert.equal(
    find(view.tree(), (node) => hasClass(node, "realqa-capture-card")).length,
    0,
  );
  assert.equal(buttons(view.tree()).length, 1);
  view.unmount();

  view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: controller({
      permission: "not_granted",
      shortcuts: {},
      chrome: {
        state: "approval_pending",
        pending: [
          { id: "pending-a", profileName: "Pending A", extensionId: "ext-a" },
          { id: "pending-b", profileName: "Pending B", extensionId: "ext-b" },
        ],
        connections: [
          {
            profileId: "profile-a",
            profileName: "Profile A",
            extensionId: "ext-a",
            connected: true,
          },
          {
            profileId: "profile-b",
            profileName: "Profile B",
            extensionId: "ext-b",
            connected: false,
          },
        ],
      },
    }),
  });
  const shown = () => view.tree().map(text).join("");
  for (const expected of [
    "Screen capture permission: Not granted",
    "Not set",
    "Pending A",
    "Pending B",
    "Profile A",
    "Profile B",
    "Connected",
    "Disconnected",
    "ext-a",
    "ext-b",
  ])
    assert.ok(shown().includes(expected), expected);
  assert.ok(texts(view.tree()).includes("Approval pending"));
  assert.doesNotMatch(shown(), /approval_pending|not_granted/);
  assert.equal(
    buttons(view.tree()).filter((node) => text(node) === "Assign").length,
    4,
  );
  assert.doesNotMatch(shown(), /Install extension/);
  button(view.tree(), "Approve", 0).props.bindtap();
  button(view.tree(), "Reject", 1).props.bindtap();
  button(view.tree(), "Disconnect", 1).props.bindtap();
  await view.settle();
  button(view.tree(), "Cancel").props.bindtap();
  await view.settle();
  assert.deepEqual(calls, [
    { op: "chrome.approve", id: "pending-a" },
    { op: "chrome.reject", id: "pending-b" },
  ]);
  button(view.tree(), "Disconnect", 1).props.bindtap();
  await view.settle();
  button(view.tree(), "Disconnect Chrome").props.bindtap();
  await view.settle();
  assert.deepEqual(calls.at(-1), {
    op: "chrome.disconnect",
    profileId: "profile-b",
  });
  view.unmount();

  view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: controller({
      permission: "granted",
      shortcuts: {},
      chrome: {
        state: "disconnected",
        listingUrl: "https://chromewebstore.google.com/detail/example",
      },
    }),
  });
  assert.ok(texts(view.tree()).includes("Disconnected"));
  assert.match(shown(), /Screen capture permission: Granted/);
  button(view.tree(), "Install extension").props.bindtap();
  await view.settle();
  assert.deepEqual(urls, ["https://chromewebstore.google.com/detail/example"]);
  view.unmount();
});

test("source choice and initial approval retain identified host operations", async () => {
  const bundle = await loadRealQA();
  const calls: Record<string, unknown>[] = [];
  const sourceStates: unknown[] = [];
  bundle.captureHost.realqa = async (input) => {
    calls.push(input);
  };
  const controller = {
    state: {
      chrome: {
        pending: [
          { id: "pending", profileName: "Profile", extensionId: "extension" },
        ],
      },
    },
    sources: {
      mode: "screen",
      sources: [
        { id: "display-a", name: "Display A", width: 1920, height: 1080 },
      ],
    },
    busy: false,
    error: "",
    discard: false,
    saved: undefined,
    setSources: (state: unknown) => {
      sourceStates.push(state);
    },
    run: async (action: () => Promise<unknown>) => {
      await action();
    },
  };
  const view = bundle.runtime.mount(bundle.RealQADialogs, { controller });
  assert.match(view.tree().map(text).join(""), /Display A · 1920 × 1080/);
  button(view.tree(), "Cancel selection").props.bindtap();
  await view.settle();
  assert.deepEqual(sourceStates, [undefined]);
  assert.deepEqual(calls, []);
  button(view.tree(), "Display A · 1920 × 1080").props.bindtap();
  await view.settle();
  assert.deepEqual(calls, [
    { op: "capture", mode: "screen", sourceId: "display-a" },
  ]);
  const approval = find(
    view.tree(),
    (node) =>
      node.props["accessibility-label"] === "Approve Chrome connection?",
  )[0];
  assert.ok(approval);
  button(view.tree(), "Reject").props.bindtap();
  await view.settle();
  assert.deepEqual(calls.at(-1), { op: "chrome.reject", id: "pending" });
  view.unmount();

  const retried: string[] = [];
  const empty = bundle.runtime.mount(bundle.RealQADialogs, {
    controller: {
      ...controller,
      state: {},
      sources: { mode: "window", sources: [] },
      capture: async (mode: string) => {
        retried.push(mode);
      },
    },
  });
  button(empty.tree(), "Retry sources").props.bindtap();
  await empty.settle();
  assert.deepEqual(retried, ["window"]);
  empty.unmount();
});

test("a rejected shortcut change keeps its prior binding and editable input", async () => {
  const bundle = await loadRealQA();
  const calls: Record<string, unknown>[] = [];
  bundle.captureHost.realqa = async (input) => {
    calls.push(input);
    throw new Error("Shortcut unavailable");
  };
  const controller = {
    available: true,
    state: { permission: "granted", shortcuts: { screen: "Command+Shift+S" } },
    busy: false,
    error: "",
    capture: async () => {},
    run: async (action: () => Promise<unknown>) => {
      try {
        await action();
      } catch {
        /* the real controller retains the dialog */
      }
    },
  };
  const view = bundle.runtime.mount(bundle.RealQA, {
    controller,
    repositories: [],
    back: () => {},
    dirty: () => {},
  });
  button(view.tree(), "Change").props.bindtap();
  await view.settle();
  const input = find(view.tree(), (node) => node.type === "input")[0];
  assert.ok(input);
  input.props.bindinput({ detail: { value: "Command+Shift+X" } });
  await view.settle();
  button(view.tree(), "Save shortcut").props.bindtap();
  await view.settle();
  assert.deepEqual(calls, [
    { op: "shortcut", mode: "screen", accelerator: "Command+Shift+X" },
  ]);
  assert.ok(
    find(
      view.tree(),
      (node) =>
        node.props["accessibility-label"] === "Assign keyboard shortcut",
    ).length,
  );
  assert.match(view.tree().map(text).join(""), /Command\+Shift\+S/);
  button(view.tree(), "Cancel").props.bindtap();
  await view.settle();
  button(view.tree(), "Discard change").props.bindtap();
  await view.settle();
  assert.equal(
    find(
      view.tree(),
      (node) =>
        node.props["accessibility-label"] === "Assign keyboard shortcut",
    ).length,
    0,
  );
  assert.equal(calls.length, 1);
  view.unmount();
});

// Records selector-query focus and value calls made by the mounted components.
function recordQueries() {
  const focused: string[] = [];
  const values = new Map<string, string>();
  const query: any = {
    select: (selector: string) => ({
      invoke: (call: { method: string; params?: { value?: string } }) => ({
        exec() {
          if (call.method === "setFocus") focused.push(selector);
          if (call.method === "setValue")
            values.set(selector, call.params?.value ?? "");
        },
      }),
    }),
  };
  (globalThis as any).lynx = { createSelectorQuery: () => query };
  return { focused, values };
}
// Dispatches a global key to every registered handler, in either order, as a
// host may deliver it to all overlays.
async function globalKey(
  view: { tree: () => HostNode[]; settle: () => Promise<void> },
  key: string,
  reverse = false,
) {
  const handlers = find(
    view.tree(),
    (node) => typeof node.props["global-bindkeydown"] === "function",
  ).map((node) => node.props["global-bindkeydown"]);
  for (const handler of reverse ? handlers.reverse() : handlers)
    handler({ key });
  await view.settle();
}
const dialogTitled = (tree: HostNode[], title: string) =>
  find(tree, (node) => node.props["accessibility-label"] === title).length > 0;
const staticController = (
  state?: Record<string, unknown>,
  extra: Record<string, unknown> = {},
) => ({
  available: true,
  state,
  busy: false,
  error: "",
  capture: async () => {},
  run: async (action: () => Promise<unknown>) => {
    await action();
  },
  ...extra,
});
const common = { repositories: [], back: () => {}, dirty: () => {} };

test("host status identifiers render as English text", async () => {
  const bundle = await loadRealQA();
  const windowsMessage =
    "Protected windows and Windows privacy policy can prevent capture.";
  const cases: [Record<string, unknown> | undefined, string[], RegExp[]][] = [
    [undefined, ["Screen capture permission: Loading…", "Loading…"], []],
    [
      { permission: "available", message: windowsMessage, shortcuts: {} },
      [
        "Screen capture permission: Available",
        windowsMessage,
        "Disconnected",
        "The Chrome Web Store listing is not available in this build.",
      ],
      [/Install extension/],
    ],
    [
      { permission: "granted", shortcuts: {}, chrome: { state: "connected" } },
      ["Screen capture permission: Granted", "Connected"],
      [/Disconnected/],
    ],
    [
      {
        permission: "not_granted",
        shortcuts: {},
        chrome: {
          state: "error",
          message: "The Chrome connection could not start.",
          listingUrl: "http://chromewebstore.google.com/detail/example",
        },
      },
      [
        "Screen capture permission: Not granted",
        "Error",
        "The Chrome connection could not start.",
      ],
      [/Install extension/, /Disconnected/],
    ],
    [
      { permission: "unknown_value", shortcuts: {}, chrome: { state: "new" } },
      ["Screen capture permission: Unavailable", "Status unavailable"],
      [/unknown_value|\bnew\b/],
    ],
  ];
  for (const [state, expected, absent] of cases) {
    const view = bundle.runtime.mount(bundle.RealQA, {
      ...common,
      controller: staticController(state),
    });
    const shown = view.tree().map(text).join("");
    for (const value of expected)
      assert.ok(texts(view.tree()).includes(value), value);
    for (const pattern of absent) assert.doesNotMatch(shown, pattern);
    view.unmount();
  }
});

test("desktop RealQA renders nothing until host availability is known", async () => {
  const bundle = await loadRealQA();
  const view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: staticController(undefined, { available: undefined }),
  });
  assert.deepEqual(view.tree(), []);
  view.unmount();
});

test("a failed capture offers an explicit retry of the same mode", async () => {
  const bundle = await loadRealQA();
  const captures: string[] = [];
  const capture = async (mode: string) => {
    captures.push(mode);
  };
  let view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: staticController(
      { permission: "granted", shortcuts: {} },
      {
        error: "Capture failed. Retry explicitly.",
        retryMode: "region",
        capture,
      },
    ),
  });
  const retry = button(view.tree(), "Retry capture");
  assert.equal(retry.props["accessibility-label"], "Retry Region capture");
  retry.props.bindtap();
  await view.settle();
  assert.deepEqual(captures, ["region"]);
  view.unmount();

  view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: staticController(
      { permission: "granted", shortcuts: {} },
      { error: "Shortcut unavailable", capture },
    ),
  });
  assert.equal(
    buttons(view.tree()).filter((node) => text(node) === "Retry capture")
      .length,
    0,
  );
  view.unmount();
});

test("shortcut input asks before discarding and returns focus on close", async () => {
  const bundle = await loadRealQA();
  const { focused, values } = recordQueries();
  const dirty: boolean[] = [];
  const view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    dirty: (value: boolean) => dirty.push(value),
    controller: staticController({ permission: "granted", shortcuts: {} }),
  });
  const title = "Assign keyboard shortcut";
  const confirmation = "Discard shortcut change?";
  const assign = button(view.tree(), "Assign");
  assert.equal(assign.props.id, "realqa-shortcut-screen");
  assign.props.bindtap();
  await view.settle();
  assert.ok(dialogTitled(view.tree(), title));
  assert.equal(dirty.at(-1), false, "an unchanged dialog is not an edit");
  await globalKey(view, "Escape");
  assert.ok(!dialogTitled(view.tree(), title));
  assert.equal(focused.at(-1), "#realqa-shortcut-screen");

  button(view.tree(), "Assign").props.bindtap();
  await view.settle();
  const input = find(view.tree(), (node) => node.type === "input")[0];
  input.props.bindinput({ detail: { value: "Command+Shift+X" } });
  await view.settle();
  assert.equal(dirty.at(-1), true);
  for (const close of [
    () => globalKey(view, "Escape"),
    async () => {
      button(view.tree(), "Close").props.bindtap();
      await view.settle();
    },
    async () => {
      button(view.tree(), "Cancel").props.bindtap();
      await view.settle();
    },
  ]) {
    await close();
    assert.ok(dialogTitled(view.tree(), confirmation));
    assert.ok(dialogTitled(view.tree(), title));
    for (const reverse of [false, true]) {
      // Escape closes only the topmost confirmation, in any delivery order.
      await globalKey(view, "Escape", reverse);
      assert.ok(!dialogTitled(view.tree(), confirmation));
      assert.ok(dialogTitled(view.tree(), title));
      if (!reverse) {
        await close();
        assert.ok(dialogTitled(view.tree(), confirmation));
      }
    }
    assert.equal(
      values.get(
        `#${find(view.tree(), (node) => node.type === "input")[0].props.id}`,
      ),
      "Command+Shift+X",
    );
  }
  await globalKey(view, "Escape");
  button(view.tree(), "Keep editing").props.bindtap();
  await view.settle();
  assert.ok(!dialogTitled(view.tree(), confirmation));
  assert.ok(dialogTitled(view.tree(), title));
  assert.equal(dirty.at(-1), true);
  await globalKey(view, "Escape");
  button(view.tree(), "Discard change").props.bindtap();
  await view.settle();
  assert.ok(!dialogTitled(view.tree(), title));
  assert.ok(!dialogTitled(view.tree(), confirmation));
  assert.equal(dirty.at(-1), false);
  // Both dialogs close together; focus returns to the original trigger.
  assert.equal(focused.at(-1), "#realqa-shortcut-screen");
  view.unmount();
});

test("Chrome disconnect confirmation returns focus to its trigger", async () => {
  const bundle = await loadRealQA();
  const { focused } = recordQueries();
  const view = bundle.runtime.mount(bundle.RealQA, {
    ...common,
    controller: staticController({
      permission: "granted",
      shortcuts: {},
      chrome: {
        state: "connected",
        connections: [
          {
            profileId: "profile-a",
            profileName: "Profile A",
            extensionId: "ext-a",
            connected: true,
          },
        ],
      },
    }),
  });
  const trigger = button(view.tree(), "Disconnect");
  assert.equal(trigger.props.id, "realqa-disconnect-profile-a");
  trigger.props.bindtap();
  await view.settle();
  await globalKey(view, "Escape");
  assert.ok(!dialogTitled(view.tree(), "Disconnect Chrome?"));
  assert.equal(focused.at(-1), "#realqa-disconnect-profile-a");
  view.unmount();
});
