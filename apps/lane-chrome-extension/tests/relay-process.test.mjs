// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createChromeBridge } from "../../lane/native/desktop/chrome-bridge.mjs";
import {
  encodeMessage,
  readMessages,
} from "../../lane/native/desktop/chrome-framing.mjs";
const extensionId = "a".repeat(32),
  profileId = "12345678-1234-1234-1234-123456789012";
const executable = fileURLToPath(
  new URL("../../lane/native/desktop/chrome-native-host.mjs", import.meta.url),
);
async function deadline(promise) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error("relay did not disconnect and exit")),
          2000,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
test("relay child exits with Chrome stdin open when Lane quits, then reconnects after restart", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "lane-relay-process-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const options = {
    dataRoot: root,
    extensionIds: [extensionId],
    requestApproval: async () => true,
    acceptCapture: async () => {},
  };
  let bridge = await createChromeBridge(options);
  t.after(() => bridge.close());
  function start() {
    const child = spawn(
      process.execPath,
      [executable, "--data-root", root, `chrome-extension://${extensionId}/`],
      { stdio: ["pipe", "pipe", "pipe"] },
    );
    t.after(() => {
      if (child.exitCode === null) child.kill();
    });
    child.stdin.on("error", () => {});
    const exited = once(child, "exit"),
      eof = once(child.stdout, "end");
    const response = new Promise((resolve, reject) =>
      readMessages(child.stdout, resolve, reject),
    );
    return { child, exited, eof, response };
  }
  const first = start();
  first.child.stdin.write(
    encodeMessage({
      id: "first",
      type: "connect",
      profileId,
      profileName: "Process test",
    }),
  );
  const approved = await deadline(first.response);
  assert.equal(approved.ok, true);
  // Chrome keeps its write end open until the native host disconnects. Do not end
  // child.stdin here: that would hide the stale-port regression.
  await bridge.close();
  assert.deepEqual(await deadline(first.exited), [0, null]);
  await deadline(first.eof);
  bridge = await createChromeBridge(options);
  const second = start();
  second.child.stdin.write(
    encodeMessage({
      id: "second",
      type: "status",
      profileId,
      token: approved.token,
    }),
  );
  assert.equal((await deadline(second.response)).ok, true);
  await bridge.close();
  assert.deepEqual(await deadline(second.exited), [0, null]);
  await deadline(second.eof);
  const absent = start();
  assert.equal((await deadline(absent.response)).reason, "LANE_NOT_RUNNING");
  assert.deepEqual(await deadline(absent.exited), [0, null]);
  await deadline(absent.eof);
});
