// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { WindowSession } from "../native/desktop/bridge.mjs";

const executable = process.env.LANE_STORE_DRIVER;
if (!executable) throw new Error("Set LANE_STORE_DRIVER to lane_store_driver.");
const root = await mkdtemp(join(tmpdir(), "lane-desktop-handles-"));

function driver() {
  const child = spawn(executable, [root]);
  const exited = once(child, "exit");
  const lines = createInterface({ input: child.stdout });
  const waiting = [];
  lines.on("line", (line) => waiting.shift()?.(JSON.parse(line)));
  let tail = Promise.resolve();
  return {
    call(message) {
      const result = tail.then(
        () =>
          new Promise((resolve, reject) => {
            waiting.push(resolve);
            child.stdin.write(`${JSON.stringify(message)}\n`, (error) => {
              if (error) reject(error);
            });
          }),
      );
      tail = result.then(
        () => {},
        () => {},
      );
      return result;
    },
    async stop() {
      await this.call({ method: "exit" });
      await exited;
    },
  };
}

const mainDriver = driver();
let otherDriver;
const native = {
  store: {
    open: (raw, callback) =>
      void mainDriver
        .call({ method: "open", request: JSON.parse(raw) })
        .then((reply) => callback(JSON.stringify(reply))),
    close: (spaceId, callback) =>
      void mainDriver
        .call({ method: "close", spaceId })
        .then((reply) => callback(JSON.stringify(reply))),
  },
};
const request = (method, argument) => ({
  module: "LaneStore",
  method,
  args: [argument],
});
const open = (session, spaceId = "") =>
  session.invoke(
    native,
    request("open", JSON.stringify({ formatVersion: 3, spaceId })),
  );
const reset = (spaceId) => mainDriver.call({ method: "reset", spaceId });
const busy = async (spaceId) =>
  assert.match((await reset(spaceId)).error.reason, /STORAGE_BUSY/);

try {
  const main = new WindowSession();
  const first = JSON.parse(await open(main));
  assert.equal(first.ok, true);
  const spaceId = first.response.spaceId;
  const composer = new WindowSession();
  assert.equal(JSON.parse(await open(composer, spaceId)).ok, true);
  await composer.retire();
  await busy(spaceId);
  await main.retire();

  // A separate process keeps its lifetime lease after both views close.
  otherDriver = driver();
  const otherOpen = await otherDriver.call({
    method: "open",
    request: { formatVersion: 3, spaceId },
  });
  assert.equal(otherOpen.ok, true, JSON.stringify(otherOpen));
  await busy(spaceId);
  assert.equal((await otherDriver.call({ method: "close", spaceId })).ok, true);
  assert.equal((await reset(spaceId)).ok, true);

  const explicit = new WindowSession();
  assert.equal(JSON.parse(await open(explicit, spaceId)).ok, true);
  assert.equal(
    JSON.parse(await explicit.invoke(native, request("close", spaceId))).ok,
    true,
  );
  await explicit.retire();
  assert.equal((await reset(spaceId)).ok, true);

  let completeLateOpen;
  const delayed = {
    store: {
      ...native.store,
      open: (raw, callback) =>
        void mainDriver
          .call({ method: "open", request: JSON.parse(raw) })
          .then((reply) => {
            completeLateOpen = () => callback(JSON.stringify(reply));
          }),
    },
  };
  const late = new WindowSession();
  const pending = late.invoke(
    delayed,
    request("open", JSON.stringify({ formatVersion: 3, spaceId })),
  );
  while (!completeLateOpen) await new Promise(setImmediate);
  const drained = late.retire();
  completeLateOpen();
  assert.equal(JSON.parse(await pending).ok, true);
  await drained;
  assert.equal((await reset(spaceId)).ok, true);
  console.log("desktop Store session and process ownership passed");
} finally {
  await mainDriver.stop();
  if (otherDriver) await otherDriver.stop();
  await rm(root, { recursive: true, force: true });
}
