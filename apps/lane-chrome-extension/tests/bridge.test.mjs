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
