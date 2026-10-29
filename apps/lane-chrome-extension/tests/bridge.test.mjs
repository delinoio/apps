// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough } from "node:stream";
import { createChromeBridge } from "../../lane/native/desktop/chrome-bridge.mjs";
import {
  encodeMessage,
  readMessages,
} from "../../lane/native/desktop/chrome-framing.mjs";
import { runNativeRelay } from "../../lane/native/desktop/chrome-native-host.mjs";
import {
  installNativeMessaging,
  removeNativeMessaging,
} from "../scripts/native-registration.mjs";
const extensionId = "a".repeat(32),
  profileId = "12345678-1234-1234-1234-123456789012";
async function client(root) {
  const runtime = JSON.parse(
    await readFile(join(root, "realqa-chrome-runtime.json"), "utf8"),
  );
  const socket = net.connect(runtime.endpoint);
  await new Promise((resolve) => socket.on("connect", resolve));
  const responses = [],
    waiters = [];
  readMessages(
    socket,
    (value) => {
      if (waiters.length) waiters.shift()(value);
      else responses.push(value);
    },
    () => {},
  );
  socket.write(
    encodeMessage({ type: "relay", extensionId, secret: runtime.secret }),
  );
  return {
    socket,
    next: () =>
      responses.length
        ? Promise.resolve(responses.shift())
        : new Promise((resolve) => waiters.push(resolve)),
    send(value) {
      socket.write(encodeMessage({ id: "1", profileId, ...value }));
    },
  };
}
const pngBase64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
test("approval, authenticated capture, persistence, revocation and credential isolation", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-chrome-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  let approved = false,
    captures = [];
  let bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => approved,
    acceptCapture: async (value) => captures.push(value),
  });
  t.after(() => bridge.close());
  let c = await client(root);
  c.send({ type: "connect", profileName: "Work" });
  assert.equal((await c.next()).reason, "APPROVAL_REQUIRED");
  approved = true;
  c.send({ type: "connect", profileName: "Work" });
  const { token } = await c.next();
  assert.equal(token.length, 64);
  assert.deepEqual(bridge.listConnections(), [
    { profileId, profileName: "Work", extensionId, connected: true },
  ]);
  const capture = {
    type: "capture",
    pngBase64,
    width: 1,
    height: 1,
    metadata: {
      pageUrl: "https://example.com/path?q=private",
      pageTitle: "Title",
      cssSelector: "#target",
    },
  };
  c.send({ ...capture, token: "wrong" });
  assert.equal((await c.next()).reason, "REVOKED");
  assert.equal(captures.length, 0);
  c.send({ ...capture, token });
  assert.equal((await c.next()).ok, true);
  assert.equal(captures.length, 1);
  c.send({ ...capture, token, width: 2 });
  assert.equal((await c.next()).reason, "INVALID_CAPTURE");
  c.send({
    ...capture,
    token,
    metadata: { ...capture.metadata, pageUrl: "javascript:alert(1)" },
  });
  assert.equal((await c.next()).reason, "INVALID_METADATA");
  const state = await readFile(
    join(root, "realqa-chrome-connections.json"),
    "utf8",
  );
  assert.ok(!state.includes(token));
  assert.ok(!state.includes("private"));
  await bridge.close();
  bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => {
      throw Error("must reuse approval");
    },
    acceptCapture: async () => {},
  });
  c = await client(root);
  c.send({ type: "status", token });
  assert.equal((await c.next()).ok, true);
  const selection = bridge.startSelection();
  const selectMessage = await c.next();
  assert.equal(selectMessage.type, "select");
  c.send({
    type: "selectionResult",
    token,
    selectionId: selectMessage.selectionId,
  });
  assert.equal((await c.next()).ok, true);
  await selection;
  await bridge.revoke(profileId);
  assert.equal((await c.next()).reason, "REVOKED");
  assert.equal(bridge.listConnections().length, 0);
  c = await client(root);
  c.send({ type: "status", token });
  assert.equal((await c.next()).reason, "REVOKED");
  c.socket.destroy();
});
test("relay denies spoofed origin and fails without starting Lane", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-relay-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const output = new PassThrough();
  let result;
  readMessages(
    output,
    (message) => (result = message),
    () => {},
  );
  await runNativeRelay({
    dataRoot: root,
    origin: `chrome-extension://${extensionId}/`,
    input: new PassThrough(),
    output,
  });
  assert.equal(result.reason, "LANE_NOT_RUNNING");
  await assert.rejects(
    runNativeRelay({
      dataRoot: root,
      origin: "https://example.com",
      input: new PassThrough(),
      output,
    }),
    /INVALID_ORIGIN/,
  );
});
test("framing supports fragmentation and rejects invalid length", () => {
  const input = new PassThrough(),
    values = [];
  let failed = false;
  readMessages(
    input,
    (value) => values.push(value),
    () => (failed = true),
    100,
  );
  for (const byte of encodeMessage({ text: "Hello 한글" }))
    input.write(Buffer.from([byte]));
  assert.deepEqual(values, [{ text: "Hello 한글" }]);
  input.write(Buffer.from([255, 255, 255, 255]));
  assert.equal(failed, true);
});
test("registration uses exact origins and removes only its own installation", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-registration-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const options = {
    platform: "darwin",
    home: root,
    extensionIds: [extensionId],
    relayExecutable: join(root, "Lane Relay"),
  };
  const path = await installNativeMessaging(options);
  assert.deepEqual(JSON.parse(await readFile(path, "utf8")).allowed_origins, [
    `chrome-extension://${extensionId}/`,
  ]);
  await assert.rejects(
    removeNativeMessaging({ ...options, relayExecutable: "/other/relay" }),
    /REGISTRATION_OWNER_CHANGED/,
  );
  await removeNativeMessaging(options);
  await assert.rejects(readFile(path), { code: "ENOENT" });
  await assert.rejects(
    installNativeMessaging({ ...options, extensionIds: ["*"] }),
    /REAL_EXTENSION_IDS_REQUIRED/,
  );
});

test("revoking a pending approval prevents a late approval from restoring access", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-chrome-race-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  let decide, notify;
  const shown = new Promise((resolve) => (notify = resolve));
  const bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: () => {
      notify();
      return new Promise((resolve) => (decide = resolve));
    },
    acceptCapture: async () => {},
  });
  t.after(() => bridge.close());
  const c = await client(root);
  c.send({ type: "connect", profileName: "Pending" });
  await shown;
  await bridge.revoke(profileId);
  decide(true);
  assert.equal((await c.next()).reason, "APPROVAL_REQUIRED");
  assert.deepEqual(bridge.listConnections(), []);
  c.socket.destroy();
});

test("only the configured extension and runtime secret reach approval", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-chrome-origin-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  let asked = false;
  const bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => {
      asked = true;
      return true;
    },
    acceptCapture: async () => {},
  });
  t.after(() => bridge.close());
  const runtime = JSON.parse(
    await readFile(join(root, "realqa-chrome-runtime.json"), "utf8"),
  );
  for (const hello of [
    { extensionId: "b".repeat(32), secret: runtime.secret },
    { extensionId, secret: "incorrect" },
  ]) {
    const socket = net.connect(runtime.endpoint);
    await new Promise((resolve) => socket.on("connect", resolve));
    const closed = new Promise((resolve) => socket.on("close", resolve));
    socket.write(encodeMessage({ type: "relay", ...hello }));
    socket.write(
      encodeMessage({
        id: "1",
        type: "connect",
        profileId,
        profileName: "Spoofed",
      }),
    );
    await closed;
  }
  assert.equal(asked, false);
});

test("stdio relay forwards framed messages to the already running bridge and exits on quit", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-relay-live-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => true,
    acceptCapture: async () => {},
  });
  t.after(() => bridge.close());
  const input = new PassThrough(),
    output = new PassThrough();
  let reply;
  const received = new Promise((resolve) => (reply = resolve));
  readMessages(
    output,
    (message) => reply(message),
    () => {},
  );
  const relay = runNativeRelay({
    dataRoot: root,
    origin: `chrome-extension://${extensionId}/`,
    input,
    output,
  });
  input.write(
    encodeMessage({
      id: "relay-request",
      type: "connect",
      profileId,
      profileName: "Relay test",
    }),
  );
  const response = await received;
  assert.equal(response.id, "relay-request");
  assert.equal(response.ok, true);
  assert.equal(response.token.length, 64);
  await bridge.close();
  await relay;
});

test(
  "in-app selection activates only the last connected approved profile",
  { timeout: 2000 },
  async (t) => {
    const root = await mkdtemp(join(tmpdir(), "lane-chrome-"));
    t.after(() => rm(root, { recursive: true, force: true }));
    const bridge = await createChromeBridge({
      dataRoot: root,
      extensionIds: [extensionId],
      requestApproval: async () => true,
      acceptCapture: async () => {},
    });
    t.after(() => bridge.close());
    const first = await client(root),
      second = await client(root);
    first.send({ type: "connect", profileName: "First" });
    const firstToken = (await first.next()).token;
    const secondId = "22345678-1234-1234-1234-123456789012";
    second.send({
      type: "connect",
      profileId: secondId,
      profileName: "Second",
    });
    const secondToken = (await second.next()).token;
    const selection = bridge.startSelection({ activate: true });
    const message = await second.next();
    assert.equal(message.type, "select");
    assert.equal(message.activate, true);
    first.send({ type: "status", token: firstToken });
    assert.equal(
      (await first.next()).type,
      undefined,
      "other profile receives no focus request",
    );
    second.send({
      type: "selectionResult",
      profileId: secondId,
      token: secondToken,
      selectionId: message.selectionId,
    });
    assert.equal((await second.next()).ok, true);
    await selection;
  },
);

const until = async (condition) => {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    if (condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  throw new Error("TIMEOUT");
};
// Records what an observer reading the bridge would see at each notification.
function observed() {
  const state = { bridge: undefined, snapshots: [], approvals: 0 };
  state.onStatus = () =>
    state.snapshots.push(
      state.bridge.listConnections().map((item) => item.connected),
    );
  state.connected = (index = state.snapshots.length - 1) =>
    state.snapshots[index]?.some(Boolean) === true;
  return state;
}

test("authenticated reconnect notifies observers once without a new approval", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-chrome-reconnect-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const seen = observed();
  seen.bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => {
      seen.approvals += 1;
      return true;
    },
    acceptCapture: async () => {},
    onStatus: seen.onStatus,
  });
  t.after(() => seen.bridge.close());
  const first = await client(root);
  first.send({ type: "connect", profileName: "Work" });
  const granted = await first.next();
  assert.equal(granted.ok, true);
  assert.equal(seen.approvals, 1);
  assert.equal(seen.snapshots.length, 1, "fresh approval notifies once");
  assert.equal(seen.connected(), true);
  first.socket.end();
  await until(() => seen.snapshots.length === 2 && !seen.connected());
  const before = seen.snapshots.length;

  const second = await client(root);
  second.send({ type: "connect", profileName: "Work", token: granted.token });
  const accepted = await second.next();
  assert.equal(accepted.ok, true);
  assert.equal(accepted.token, granted.token, "token is unchanged");
  assert.equal(seen.approvals, 1, "no new approval prompt");
  assert.equal(seen.snapshots.length, before + 1, "exactly one notification");
  assert.equal(seen.connected(), true, "observer reads connected state");
  assert.equal(seen.bridge.listConnections()[0].connected, true);
  // A later authenticated status request does not add a notification.
  second.send({ type: "status", token: granted.token });
  assert.equal((await second.next()).ok, true);
  assert.equal(seen.snapshots.length, before + 1);
  second.socket.destroy();
});

test("authenticated reconnect after a bridge restart notifies connected without approval", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-chrome-restart-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  let bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => true,
    acceptCapture: async () => {},
  });
  const first = await client(root);
  first.send({ type: "connect", profileName: "Work" });
  const { token } = await first.next();
  first.socket.destroy();
  await bridge.close();

  const seen = observed();
  seen.bridge = bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => {
      seen.approvals += 1;
      throw Error("must reuse approval");
    },
    acceptCapture: async () => {},
    onStatus: seen.onStatus,
  });
  t.after(() => bridge.close());
  assert.deepEqual(seen.snapshots, []);
  assert.equal(bridge.listConnections()[0].connected, false);
  const second = await client(root);
  second.send({ type: "connect", profileName: "Work", token });
  const accepted = await second.next();
  assert.equal(accepted.ok, true);
  assert.equal(accepted.token, token);
  assert.equal(seen.approvals, 0);
  assert.equal(seen.snapshots.length, 1);
  assert.equal(seen.connected(), true);
  second.socket.destroy();
});

test("rejected, invalid and revoked credentials never connect or bypass approval", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-chrome-unauthorized-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const seen = observed();
  let approve = false;
  seen.bridge = await createChromeBridge({
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => {
      seen.approvals += 1;
      return approve;
    },
    acceptCapture: async () => {},
    onStatus: seen.onStatus,
  });
  t.after(() => seen.bridge.close());
  const anyConnected = () =>
    seen.snapshots.some((value) => value.some(Boolean));

  const rejected = await client(root);
  rejected.send({ type: "connect", profileName: "Work" });
  assert.equal((await rejected.next()).reason, "APPROVAL_REQUIRED");
  assert.equal(seen.approvals, 1);
  assert.equal(anyConnected(), false, "rejected approval never connects");
  assert.deepEqual(seen.bridge.listConnections(), []);

  approve = true;
  const granted = await client(root);
  granted.send({ type: "connect", profileName: "Work" });
  const { token } = await granted.next();
  granted.socket.destroy();
  await until(() => !seen.connected());
  approve = false;
  const approvals = seen.approvals;

  const invalid = await client(root);
  invalid.send({ type: "connect", profileName: "Work", token: "wrong" });
  assert.equal((await invalid.next()).reason, "APPROVAL_REQUIRED");
  assert.equal(
    seen.approvals,
    approvals + 1,
    "invalid token asks for approval",
  );
  assert.equal(seen.bridge.listConnections()[0].connected, false);
  assert.equal(seen.connected(), false);

  await seen.bridge.revoke(profileId);
  const afterRevoke = seen.snapshots.length;
  const revoked = await client(root);
  revoked.send({ type: "connect", profileName: "Work", token });
  assert.equal((await revoked.next()).reason, "APPROVAL_REQUIRED");
  assert.equal(
    seen.approvals,
    approvals + 2,
    "revoked token asks for approval",
  );
  assert.deepEqual(seen.bridge.listConnections(), []);
  assert.equal(
    seen.snapshots.slice(afterRevoke).some((value) => value.some(Boolean)),
    false,
  );
  for (const socket of [rejected, invalid, revoked]) socket.socket.destroy();
});
