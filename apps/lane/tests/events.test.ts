// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import {
  loadEvents,
  type EventsBundle,
  type HostNode,
} from "./harness/events.ts";

type Call = { name: string; request: any };
type Deferred = {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

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
const isDisabledButton = (n: HostNode) =>
  /(^| )disabled( |$)/.test(n.props.className) &&
  n.props.focusable === false &&
  n.props["accessibility-value"] === "Disabled";
const isEnabledButton = (n: HostNode) =>
  !/(^| )disabled( |$)/.test(n.props.className) &&
  n.props.focusable === true &&
  n.props["accessibility-value"] !== "Disabled";
const repository = (id: string, displayName: string) => ({
  version: {
    entityId: id,
    body: { case: "repository", value: { displayName } },
  },
});

let bundle: EventsBundle;
test.before(async () => {
  bundle = await loadEvents();
});

// Mounts the real Events screen against a scripted adapter. SaveReceiver is
// held until the test resolves or rejects it, so the pending window can be
// inspected through the rendered controls.
async function open() {
  const calls: Call[] = [];
  const saves: Deferred[] = [];
  let receivers = [{ id: "C", revision: "V0", name: "A0", repositoryId: "R0" }];
  const adapter = {
    mode: "web",
    call: (method: { name: string }, request: any) => {
      calls.push({ name: method.name, request });
      switch (method.name) {
        case "ListReceivers":
          return Promise.resolve({ items: receivers, nextCursor: "" });
        case "ListEvents":
          return Promise.resolve({ items: [], nextCursor: "" });
        case "SaveReceiver":
          return new Promise((resolve, reject) =>
            saves.push({ resolve, reject }),
          );
        default:
          return Promise.reject(new Error(`Unexpected call ${method.name}`));
      }
    },
    local: () => Promise.reject(new Error("unexpected local call")),
    subscribe: () => () => {},
    close: () => {},
    open: async () => {},
  };
  const view = bundle.runtime.mount(bundle.Events, {
    adapter,
    repositories: [
      repository("R0", "Repo Zero"),
      repository("R1", "Repo One"),
      repository("R2", "Repo Two"),
    ],
    openTask: () => {},
  });
  await view.settle();
  const count = (name: string) => calls.filter((c) => c.name === name).length;
  const dialog = () =>
    all(
      view.tree(),
      (n) => n.props["accessibility-label"] === "Webhook receiver",
    )[0];
  const input = () => all([dialog()], (n) => n.type === "input")[0];
  const choice = (option: string) => {
    const [field] = all(
      [dialog()],
      (n) =>
        n.props.className === "field" &&
        n.children.some(
          (c) =>
            typeof c !== "string" &&
            c.props.className === "label" &&
            text(c) === "Repository",
        ),
    );
    return all([field], (n) => isButton(n) && text(n) === option)[0];
  };
  const dialogButton = (label: string) =>
    all([dialog()], (n) => isButton(n) && text(n) === label)[0];
  const edit = async () => {
    const [edit] = all(
      view.tree(),
      (n) => isButton(n) && text(n) === "Edit receiver",
    );
    edit.props.bindtap();
    await view.settle();
  };
  const tap = async (node: HostNode) => {
    node.props.bindtap();
    await view.settle();
  };
  const type = async (value: string) => {
    input().props.bindinput({ detail: { value } });
    await view.settle();
  };
  const notices = () =>
    all([dialog()], (n) => n.props.className === "notice").map(text);
  const selected = (option: string) =>
    /(^| )primary( |$)/.test(choice(option).props.className);
  return {
    calls,
    saves,
    count,
    dialog,
    input,
    choice,
    dialogButton,
    edit,
    tap,
    type,
    notices,
    selected,
    settle: () => view.settle(),
    unmount: () => view.unmount(),
    setReceivers: (next: typeof receivers) => (receivers = next),
  };
}

const request = (name: string, repositoryId: string, id = "C") => ({
  id,
  expectedRevision: "V0",
  name,
  repositoryId,
});
const sent = (call: Call) => {
  const { meta: _meta, ...rest } = call.request;
  return rest;
};

test("pending receiver save locks name, repository and Save", async () => {
  const view = await open();
  try {
    await view.edit();
    await view.type("A1");
    await view.tap(view.choice("Repo One"));
    assert.equal(view.input().props.disabled, false);
    assert.ok(isEnabledButton(view.choice("Repo Two")));
    await view.tap(view.dialogButton("Save"));

    // Exactly one mutation with the submitted C/V0/A1/R1 values.
    assert.equal(view.count("SaveReceiver"), 1);
    assert.deepEqual(
      sent(view.calls.find((c) => c.name === "SaveReceiver")!),
      request("A1", "R1"),
    );
    assert.equal(view.input().props.disabled, true);
    for (const option of ["Repo Zero", "Repo One", "Repo Two"])
      assert.ok(isDisabledButton(view.choice(option)), `${option} disabled`);
    assert.ok(isDisabledButton(view.dialogButton("Save")));

    // Input events and repository activation cannot change the draft, and a
    // second Save cannot start another mutation.
    await view.type("A2");
    await view.tap(view.choice("Repo Two"));
    view.choice("Repo Two").props.bindkeydown({ key: "Enter" });
    view.choice("Repo Two").props.bindkeydown({ key: " " });
    await view.tap(view.dialogButton("Save"));
    view.dialogButton("Save").props.bindkeydown({ key: "Enter" });
    await view.settle();
    assert.equal(view.count("SaveReceiver"), 1);
    assert.equal(view.selected("Repo One"), true);
    assert.equal(view.selected("Repo Two"), false);
    assert.equal(view.input().props.disabled, true);

    // Success closes the submitted editor and reloads.
    const loadsBefore = view.count("ListReceivers");
    view.saves[0].resolve({});
    await view.settle();
    assert.equal(view.dialog(), undefined);
    assert.equal(view.count("ListReceivers"), loadsBefore + 1);
    assert.equal(view.count("SaveReceiver"), 1);
  } finally {
    view.unmount();
  }
});

test("failed receiver save keeps the draft and permits edit and retry", async () => {
  const view = await open();
  try {
    await view.edit();
    await view.type("A1");
    await view.tap(view.choice("Repo One"));
    await view.tap(view.dialogButton("Save"));
    assert.equal(view.input().props.disabled, true);
    const loadsBefore = view.count("ListReceivers");

    view.saves[0].reject(new Error("synthetic conflict"));
    await view.settle();
    // The submitted draft and the existing error remain visible.
    assert.ok(view.dialog());
    assert.equal(view.selected("Repo One"), true);
    assert.ok(view.notices().includes("synthetic conflict"));
    assert.equal(view.count("ListReceivers"), loadsBefore);
    assert.equal(view.input().props.disabled, false);
    assert.ok(isEnabledButton(view.choice("Repo Two")));
    assert.ok(isEnabledButton(view.dialogButton("Save")));

    await view.type("A2");
    await view.tap(view.choice("Repo Two"));
    assert.equal(view.selected("Repo Two"), true);
    await view.tap(view.dialogButton("Save"));
    assert.equal(view.count("SaveReceiver"), 2);
    const retry = view.calls.filter((c) => c.name === "SaveReceiver")[1];
    assert.deepEqual(sent(retry), request("A2", "R2"));
    view.saves[1].resolve({});
    await view.settle();
    assert.equal(view.dialog(), undefined);
  } finally {
    view.unmount();
  }
});

test("receiver editing and ordinary save are unchanged without a pending save", async () => {
  const view = await open();
  try {
    await view.edit();
    assert.equal(view.input().props.disabled, false);
    await view.type("A1");
    await view.tap(view.choice("Repo Two"));
    assert.equal(view.selected("Repo Two"), true);
    await view.tap(view.choice("Repo One"));
    assert.equal(view.selected("Repo One"), true);
    assert.equal(view.selected("Repo Two"), false);
    assert.equal(view.count("SaveReceiver"), 0);

    await view.tap(view.dialogButton("Save"));
    assert.deepEqual(
      sent(view.calls.find((c) => c.name === "SaveReceiver")!),
      request("A1", "R1"),
    );
    const loadsBefore = view.count("ListReceivers");
    view.saves[0].resolve({});
    await view.settle();
    assert.equal(view.dialog(), undefined);
    assert.equal(view.count("ListReceivers"), loadsBefore + 1);
  } finally {
    view.unmount();
  }
});
