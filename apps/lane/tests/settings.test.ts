// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import {
  loadSettings,
  type HostNode,
  type SettingsBundle,
} from "./harness/settings.ts";

const SAVED = "Notification preferences saved. Check the permission status.";
const SAVE = "Save notification preferences";
const ENABLE = "Enable notifications";
const CONTENT = "Notification content";
type Prefs = { enabled: boolean; content: number; permission: number };
type Call = { name: string; request: any };

const text = (node: HostNode | string): string =>
  typeof node === "string" ? node : node.children.map(text).join("");
function all(
  nodes: (HostNode | string)[],
  match: (n: HostNode) => boolean,
): HostNode[] {
  const found: HostNode[] = [];
  const walk = (n: HostNode | string) => {
    if (typeof n === "string") return;
    if (match(n)) found.push(n);
    n.children.forEach(walk);
  };
  nodes.forEach(walk);
  return found;
}
const isButton = (n: HostNode) =>
  /(^| )button( |$)/.test(n.props.className ?? "");

let bundle: SettingsBundle;
test.before(async () => {
  bundle = await loadSettings();
});

// Mounts the real Settings screen against a scripted local adapter. Tests
// replace `handlers` entries to hold or fail individual host operations.
async function open(mode: "web" | "native" = "web") {
  const calls: Call[] = [];
  const handlers: Record<string, (request: any) => Promise<unknown>> = {
    GetLocalStatus: async () => ({
      spaceId: "s",
      deviceId: "d",
      syncEnabled: false,
      connected: false,
      pendingItemCount: 0,
    }),
    GetNotificationPreferences: async () => ({
      enabled: false,
      content: 1,
      permission: 2,
    }),
  };
  const adapter = {
    mode,
    local: (name: string, request: unknown) => {
      calls.push({ name, request });
      const handler = handlers[name];
      if (!handler) throw new Error(`Unexpected local call ${name}`);
      return handler(request);
    },
    call: async () => ({ items: [], nextCursor: "" }),
    subscribe: () => () => {},
    close: () => {},
    open: async () => {},
  };
  const view = bundle.runtime.mount(bundle.Settings, {
    adapter,
    onReset: () => {},
  });
  await view.settle();
  const buttons = (label?: string) => {
    const all_ = all(view.tree(), isButton);
    return label === undefined ? all_ : all_.filter((b) => text(b) === label);
  };
  const button = (label: string) => {
    const [found, ...rest] = buttons(label);
    assert.ok(found, `button ${label} is rendered`);
    assert.equal(rest.length, 0, `button ${label} is unique`);
    return found;
  };
  // Buttons of one Choices group, found through its visible label.
  const choice = (label: string, option: string) => {
    const [field] = all(
      view.tree(),
      (n) =>
        n.props.className === "field" &&
        n.children.some(
          (c) =>
            typeof c !== "string" &&
            c.props.className === "label" &&
            text(c) === label,
        ),
    );
    assert.ok(field, `choices ${label} are rendered`);
    const [found] = all(
      field.children,
      (n) => isButton(n) && text(n) === option,
    );
    assert.ok(found, `choice ${label}: ${option} is rendered`);
    return found;
  };
  const selected = (label: string, option: string) =>
    /(^| )primary( |$)/.test(choice(label, option).props.className);
  const disabled = (node: HostNode) =>
    /(^| )disabled( |$)/.test(node.props.className) &&
    node.props.focusable === false &&
    node.props["accessibility-value"] === "Disabled";
  const enabled = (node: HostNode) =>
    !/(^| )disabled( |$)/.test(node.props.className) &&
    node.props.focusable === true &&
    node.props["accessibility-value"] !== "Disabled";
  // Operation feedback only; the static connection-access warning is always shown.
  const notices = () =>
    all(view.tree(), (n) => n.props.className === "notice")
      .map(text)
      .filter((t) => !t.startsWith("Each approved connection"));
  const permission = () =>
    text(
      all(
        view.tree(),
        (n) => n.type === "text" && text(n).startsWith("Permission:"),
      )[0],
    );
  const tap = async (node: HostNode) => {
    node.props.bindtap();
    await view.settle();
  };
  const press = async (node: HostNode, key: string) => {
    node.props.bindkeydown({ key });
    await view.settle();
  };
  const dirty = () => bundle.captured.updates!.settingsDirty();
  const busy = () => bundle.captured.updates!.settingsBusy;
  const count = (name: string) => calls.filter((c) => c.name === name).length;
  return {
    view,
    calls,
    handlers,
    button,
    choice,
    selected,
    disabled,
    enabled,
    notices,
    permission,
    tap,
    press,
    dirty,
    busy,
    count,
  };
}

test("pending preference save keeps both choices and Save disabled and ignores pointer and keyboard input", async () => {
  const s = await open();
  assert.equal(s.dirty(), false);
  assert.equal(s.selected(ENABLE, "Off"), true);
  assert.equal(s.selected(CONTENT, "Repository and title"), true);
  assert.ok(s.enabled(s.choice(ENABLE, "On")) && s.enabled(s.button(SAVE)));

  const held = Promise.withResolvers<Prefs>();
  s.handlers.SetNotificationPreferences = () => held.promise;
  await s.tap(s.button(SAVE));
  assert.deepEqual(
    s.calls
      .filter((c) => c.name === "SetNotificationPreferences")
      .map((c) => c.request),
    [{ enabled: false, content: 1 }],
  );
  assert.equal(s.busy(), true);
  for (const [label, option] of [
    [ENABLE, "Off"],
    [ENABLE, "On"],
    [CONTENT, "Repository and title"],
    [CONTENT, "Generic notice"],
  ]) {
    assert.ok(
      s.disabled(s.choice(label, option)),
      `${label}: ${option} is disabled`,
    );
  }
  assert.ok(s.disabled(s.button(SAVE)));

  // Activation of disabled choices must neither change values nor mark the draft dirty.
  for (const [label, option] of [
    [ENABLE, "On"],
    [CONTENT, "Generic notice"],
  ]) {
    await s.tap(s.choice(label, option));
    await s.press(s.choice(label, option), "Enter");
    await s.press(s.choice(label, option), " ");
  }
  await s.tap(s.button(SAVE));
  assert.equal(s.dirty(), false);
  assert.equal(s.selected(ENABLE, "Off"), true);
  assert.equal(s.selected(ENABLE, "On"), false);
  assert.equal(s.selected(CONTENT, "Repository and title"), true);
  assert.equal(s.selected(CONTENT, "Generic notice"), false);
  assert.equal(
    s.count("SetNotificationPreferences"),
    1,
    "disabled Save does not submit a duplicate request",
  );

  held.resolve({ enabled: false, content: 1, permission: 3 });
  await s.view.settle();
  assert.equal(s.busy(), false);
  assert.equal(s.dirty(), false);
  assert.deepEqual(s.notices(), [SAVED]);
  assert.equal(s.permission(), "Permission: 3");
  assert.ok(
    s.enabled(s.choice(ENABLE, "On")) &&
      s.enabled(s.choice(CONTENT, "Generic notice")) &&
      s.enabled(s.button(SAVE)),
  );
});

test("choices made after a completed save stay editable and dirty until the next explicit Save", async () => {
  const s = await open();
  s.handlers.SetNotificationPreferences = async () => ({
    enabled: false,
    content: 1,
    permission: 3,
  });
  await s.tap(s.button(SAVE));
  assert.deepEqual(s.notices(), [SAVED]);

  await s.tap(s.choice(ENABLE, "On"));
  await s.press(s.choice(CONTENT, "Generic notice"), "Enter");
  assert.equal(s.dirty(), true);
  assert.equal(s.selected(ENABLE, "On"), true);
  assert.equal(s.selected(CONTENT, "Generic notice"), true);

  const held = Promise.withResolvers<Prefs>();
  s.handlers.SetNotificationPreferences = () => held.promise;
  await s.tap(s.button(SAVE));
  assert.deepEqual(
    s.calls
      .filter((c) => c.name === "SetNotificationPreferences")
      .map((c) => c.request),
    [
      { enabled: false, content: 1 },
      { enabled: true, content: 2 },
    ],
  );
  assert.equal(s.dirty(), true, "dirty clears only after acknowledgement");
  assert.equal(s.busy(), true);
  held.resolve({ enabled: true, content: 2, permission: 2 });
  await s.view.settle();
  assert.equal(s.dirty(), false);
  assert.equal(s.selected(ENABLE, "On"), true);
  assert.equal(s.selected(CONTENT, "Generic notice"), true);
  assert.deepEqual(s.notices(), [SAVED]);
});

test("rejected save keeps the submitted draft, shows the error without success feedback and retries only on request", async () => {
  const s = await open();
  await s.tap(s.choice(ENABLE, "On"));
  await s.tap(s.choice(CONTENT, "Generic notice"));
  assert.equal(s.dirty(), true);

  const held = Promise.withResolvers<Prefs>();
  s.handlers.SetNotificationPreferences = () => held.promise;
  await s.tap(s.button(SAVE));
  assert.ok(
    s.disabled(s.choice(ENABLE, "Off")) &&
      s.disabled(s.choice(CONTENT, "Repository and title")) &&
      s.disabled(s.button(SAVE)),
  );
  held.reject(new Error("synthetic save failure"));
  await s.view.settle();

  assert.equal(s.busy(), false);
  assert.equal(s.dirty(), true);
  assert.equal(s.selected(ENABLE, "On"), true);
  assert.equal(s.selected(CONTENT, "Generic notice"), true);
  assert.deepEqual(s.notices(), ["synthetic save failure"]);
  assert.ok(
    s.enabled(s.choice(ENABLE, "Off")) &&
      s.enabled(s.choice(CONTENT, "Repository and title")) &&
      s.enabled(s.button(SAVE)),
  );
  await s.view.settle();
  assert.equal(s.count("SetNotificationPreferences"), 1, "no automatic retry");

  s.handlers.SetNotificationPreferences = async () => ({
    enabled: true,
    content: 2,
    permission: 2,
  });
  await s.tap(s.button(SAVE));
  assert.deepEqual(
    s.calls
      .filter((c) => c.name === "SetNotificationPreferences")
      .map((c) => c.request),
    [
      { enabled: true, content: 2 },
      { enabled: true, content: 2 },
    ],
  );
  assert.equal(s.dirty(), false);
  assert.deepEqual(s.notices(), [SAVED]);
});

test("explicit load disables choices while pending, adopts clean preferences and keeps a dirty draft", async () => {
  const s = await open();
  const refresh = s.button("Refresh settings");

  // Clean load: delayed read adopts the returned preferences.
  const clean = Promise.withResolvers<Prefs>();
  s.handlers.GetNotificationPreferences = () => clean.promise;
  await s.tap(refresh);
  assert.equal(s.busy(), true);
  assert.ok(
    s.disabled(s.choice(ENABLE, "On")) &&
      s.disabled(s.choice(CONTENT, "Generic notice")) &&
      s.disabled(s.button(SAVE)),
  );
  await s.tap(s.choice(ENABLE, "On"));
  assert.equal(s.dirty(), false);
  clean.resolve({ enabled: true, content: 2, permission: 3 });
  await s.view.settle();
  assert.equal(s.busy(), false);
  assert.equal(s.dirty(), false);
  assert.equal(s.selected(ENABLE, "On"), true);
  assert.equal(s.selected(CONTENT, "Generic notice"), true);
  assert.equal(s.permission(), "Permission: 3");
  assert.ok(s.enabled(s.choice(ENABLE, "Off")));

  // Dirty draft: the same read must not overwrite independent edits.
  await s.tap(s.choice(ENABLE, "Off"));
  await s.tap(s.choice(CONTENT, "Repository and title"));
  assert.equal(s.dirty(), true);
  const dirtyRead = Promise.withResolvers<Prefs>();
  s.handlers.GetNotificationPreferences = () => dirtyRead.promise;
  await s.tap(s.button("Refresh settings"));
  assert.ok(s.disabled(s.choice(ENABLE, "On")));
  dirtyRead.resolve({ enabled: true, content: 2, permission: 2 });
  await s.view.settle();
  assert.equal(s.busy(), false);
  assert.equal(s.dirty(), true);
  assert.equal(s.selected(ENABLE, "Off"), true);
  assert.equal(s.selected(CONTENT, "Repository and title"), true);
});

test("delayed initial load keeps the screen busy and then adopts clean preferences", async () => {
  const held = Promise.withResolvers<Prefs>();
  const calls: string[] = [];
  const adapter = {
    mode: "web",
    local: (name: string) => {
      calls.push(name);
      if (name === "GetNotificationPreferences") return held.promise;
      return Promise.resolve({
        spaceId: "s",
        deviceId: "d",
        syncEnabled: false,
        connected: false,
        pendingItemCount: 0,
      });
    },
    call: async () => ({ items: [], nextCursor: "" }),
  };
  const view = bundle.runtime.mount(bundle.Settings, {
    adapter,
    onReset: () => {},
  });
  await view.settle();
  assert.equal(bundle.captured.updates!.settingsBusy, true);
  assert.equal(
    all(view.tree(), (n) => n.type === "text" && text(n) === ENABLE).length,
    0,
    "no stale choices before the first read",
  );
  held.resolve({ enabled: true, content: 2, permission: 2 });
  await view.settle();
  assert.equal(bundle.captured.updates!.settingsBusy, false);
  const field = all(
    view.tree(),
    (n) => n.props.className === "field" && text(n).startsWith(ENABLE),
  )[0];
  assert.ok(field);
  const on = all(field.children, (n) => isButton(n) && text(n) === "On")[0];
  assert.match(on.props.className, /primary/);
});
