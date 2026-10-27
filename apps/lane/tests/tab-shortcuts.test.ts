// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import {
  dispatchReadyTabCommand,
  tabCommandAvailable,
  tabShortcutItems,
  TabCommand,
} from "../native/desktop/tab-shortcuts.mjs";

const mac = tabShortcutItems("darwin", () => {});
const windows = tabShortcutItems("win32", () => {});
const accelerators = (items: typeof mac, command: string) =>
  items
    .filter((item) => item.command === command)
    .map((item) => item.accelerator);

test("desktop menu registers the complete platform-specific tab matrix", () => {
  assert.deepEqual(accelerators(mac, TabCommand.New), ["Command+T"]);
  assert.deepEqual(accelerators(windows, TabCommand.New), ["Control+T"]);
  assert.deepEqual(accelerators(mac, TabCommand.Close), ["Command+W"]);
  assert.deepEqual(accelerators(windows, TabCommand.Close), [
    "Control+W",
    "Control+F4",
  ]);
  assert.deepEqual(accelerators(mac, TabCommand.Reopen), ["Command+Shift+T"]);
  assert.deepEqual(accelerators(windows, TabCommand.Reopen), [
    "Control+Shift+T",
  ]);
  assert.deepEqual(accelerators(mac, TabCommand.Next), [
    "Control+Tab",
    "Control+PageDown",
    "Command+Alt+Right",
    "Command+Shift+]",
    "Command+PageDown",
  ]);
  assert.deepEqual(accelerators(mac, TabCommand.Previous), [
    "Control+Shift+Tab",
    "Control+PageUp",
    "Command+Alt+Left",
    "Command+Shift+[",
    "Command+PageUp",
  ]);
  assert.deepEqual(accelerators(windows, TabCommand.Next), [
    "Control+Tab",
    "Control+PageDown",
  ]);
  assert.deepEqual(accelerators(windows, TabCommand.Previous), [
    "Control+Shift+Tab",
    "Control+PageUp",
  ]);
  for (const [items, prefix] of [
    [mac, "Command"],
    [windows, "Control"],
  ] as const) {
    assert.deepEqual(
      accelerators(items, TabCommand.SelectIndex),
      Array.from({ length: 8 }, (_, index) => `${prefix}+${index + 1}`),
    );
    assert.deepEqual(accelerators(items, TabCommand.SelectLast), [
      `${prefix}+9`,
    ]);
    assert.deepEqual(accelerators(items, TabCommand.MoveEarlier), [
      "Control+Shift+PageUp",
    ]);
    assert.deepEqual(accelerators(items, TabCommand.MoveLater), [
      "Control+Shift+PageDown",
    ]);
    assert.deepEqual(accelerators(items, TabCommand.MoveFirst), [
      `${prefix}+Shift+Home`,
    ]);
    assert.deepEqual(accelerators(items, TabCommand.MoveLast), [
      `${prefix}+Shift+End`,
    ]);
    assert.equal(
      new Set(items.map((item) => item.accelerator)).size,
      items.length,
    );
  }
});

test("tab menu enables only available destinations", () => {
  const state = { count: 3, history: 0, selectedIndex: 0, focusedIndex: -1 };
  assert.equal(
    tabCommandAvailable({ command: TabCommand.Reopen }, state),
    false,
  );
  assert.equal(
    tabCommandAvailable({ command: TabCommand.SelectIndex, index: 3 }, state),
    false,
  );
  assert.equal(
    tabCommandAvailable({ command: TabCommand.MoveEarlier }, state),
    false,
  );
  assert.equal(
    tabCommandAvailable({ command: TabCommand.MoveFirst }, state),
    false,
  );
  assert.equal(
    tabCommandAvailable({ command: TabCommand.MoveLater }, state),
    true,
  );
  assert.equal(
    tabCommandAvailable({ command: TabCommand.SelectLast }, state),
    true,
  );
  assert.equal(
    tabCommandAvailable(
      { command: TabCommand.Reopen },
      { ...state, history: 1 },
    ),
    true,
  );
  assert.equal(
    tabCommandAvailable(
      { command: TabCommand.MoveFirst },
      { ...state, focusedIndex: 1 },
    ),
    true,
  );
});

test("each menu alias submits one typed command", () => {
  const received: unknown[] = [];
  const owner = {};
  for (const item of tabShortcutItems("darwin", (...args) =>
    received.push(args),
  )) {
    const before = received.length;
    item.click(item, owner, {});
    assert.equal(received.length, before + 1);
    assert.deepEqual(received.at(-1), [
      {
        command: item.command,
        ...(item.index === undefined ? {} : { index: item.index }),
      },
      owner,
      {},
    ]);
  }
});

test("host sends one command only to its focused, ready main session", () => {
  const sent: unknown[] = [];
  let focused = true;
  let destroyed = false;
  const owner = {
    isDestroyed: () => destroyed,
    isFocused: () => focused,
    sendGlobalEvent: (...args: unknown[]) => sent.push(args),
  };
  const session = { ready: true };
  const state = { count: 2, history: 1, selectedIndex: 0, focusedIndex: 0 };
  const request = { command: TabCommand.Next };
  const deliver = (other?: object, event?: object) =>
    dispatchReadyTabCommand(
      request,
      other,
      event,
      owner,
      session,
      state,
      false,
    );
  assert.equal(deliver(owner), true);
  assert.deepEqual(sent, [["lane.workspace.tabCommand", request]]);
  for (const event of [{ repeat: true }, { isComposing: true }])
    assert.equal(deliver(owner, event), false);
  assert.equal(deliver({}), false);
  focused = false;
  assert.equal(deliver(owner), false);
  focused = true;
  session.ready = false;
  assert.equal(deliver(owner), false);
  session.ready = true;
  destroyed = true;
  assert.equal(deliver(owner), false);
  assert.equal(sent.length, 1);
  destroyed = false;
  owner.sendGlobalEvent = () => {
    throw new Error("private renderer details");
  };
  assert.throws(() => deliver(owner), /private renderer details/);
});
