// SPDX-License-Identifier: Apache-2.0
// Events detail-read ownership checks against the test-only preview fixture.
// GetEvent replies are held and released in a chosen order. Fixture evidence
// verifies the shared ReactLynx screens in a headless browser, not a real
// account, webhook sender or native host.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46419;
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_PORT: String(port) },
  stdio: ["ignore", "pipe", "inherit"],
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) => reject(new Error(`Preview exited ${code}`)));
});
const repo = "019a196d-0000-7000-8000-000000000001";
const event = (id, state = "EVENT_STATE_COMPLETED", payload = id) => ({
  id,
  receiverId: "r1",
  repositoryId: repo,
  senderEventId: `sender-${id}`,
  eventType: "push",
  state,
  revision: "1",
  payload: { note: `payload-${payload}` },
});
const detail = (id, payload = id, taskIds = []) => ({
  event: event(id, "EVENT_STATE_COMPLETED", payload),
  processing: [
    {
      id: `${id}-${payload}-record`,
      origin: { deviceName: `Device-${payload}` },
      taskIds,
    },
  ],
});
const fixture = (body) =>
  fetch(`${base}/test/events?body=${encodeURIComponent(JSON.stringify(body))}`);
const release = async (n, mode = "ok") => {
  const r = await (
    await fetch(`${base}/test/events-release?n=${n}&mode=${mode}`)
  ).json();
  assert.equal(r.released, true, `GetEvent #${n} was not held`);
};
const requests = async () =>
  (await fetch(`${base}/test/events-requests`)).json();
const methodCount = async (method) =>
  (await requests()).filter((r) => r.method === method).length;
const expectHeld = async (method, count) => {
  for (let attempt = 0; attempt < 100; attempt++) {
    if ((await methodCount(method)) >= count) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.fail(`Expected ${count} ${method} requests.`);
};
const settle = (ms = 300) => new Promise((resolve) => setTimeout(resolve, ms));

let browser;
try {
  await fixture({
    receivers: [
      { id: "r1", name: "Receiver", repositoryId: repo, revision: "1" },
    ],
    events: [event("A"), event("B")],
  });
  browser = await chromium.launch({
    channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
    headless: true,
  });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 900 },
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(base);
  const button = (name) => page.getByRole("button", { name, exact: true });
  const text = (value, exact = true) => page.getByText(value, { exact });
  const dialog = page.getByRole("dialog");
  const goToPage = async (name) => {
    await page.keyboard.press("Control+k");
    await page
      .getByRole("textbox", { name: "Go to a page or repository", exact: true })
      .fill(name);
    await page.keyboard.press("Enter");
  };
  const inspect = (index) => button("Inspect event").nth(index);
  // Each scenario starts from a fresh held-reply sequence and a closed dialog.
  const start = async (details) => {
    await fixture({ details, holdDetails: true });
  };
  let seen = 0;
  const expectNewReads = async (count) => {
    seen += count;
    await expectHeld("GetEvent", seen);
  };
  const expectOnlyDetail = async (payload, device) => {
    await dialog.waitFor();
    await text(`payload-${payload}`, false).waitFor();
    await text(`Device-${device ?? payload}`, false).waitFor();
  };
  await button("Improve empty states").waitFor();
  await goToPage("Events");
  await text("Sender event: sender-A").waitFor();

  // 1: a late A success or failure cannot replace B, with click, Enter and
  // Space activation.
  for (const [activation, lateA] of [
    ["click", "ok"],
    ["click", "fail"],
    ["keyboard", "ok"],
  ]) {
    await start({ A: detail("A"), B: detail("B") });
    if (activation === "click") {
      await inspect(0).click();
      await inspect(1).click();
    } else {
      await inspect(0).focus();
      await page.keyboard.press("Enter");
      await inspect(1).focus();
      await page.keyboard.press("Space");
    }
    await expectNewReads(2);
    await release(1);
    await expectOnlyDetail("B");
    await release(0, lateA);
    await settle();
    await expectOnlyDetail("B");
    assert.equal(await text("payload-A", false).count(), 0);
    assert.equal(await text("Device-A", false).count(), 0);
    assert.equal(await text("Simulated detail failure", false).count(), 0);
    await button("Close").click();
    await dialog.waitFor({ state: "hidden" });
  }

  // 2: Close and Escape end the ownership of an older pending read.
  for (const [dismiss, lateA] of [
    ["Close", "ok"],
    ["Close", "fail"],
    ["Escape", "ok"],
    ["Escape", "fail"],
  ]) {
    await start({ A: detail("A"), B: detail("B") });
    await inspect(0).click();
    await inspect(1).click();
    await expectNewReads(2);
    await release(1);
    await expectOnlyDetail("B");
    if (dismiss === "Close") await button("Close").click();
    else await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    await release(0, lateA);
    await settle();
    assert.equal(await dialog.count(), 0);
    assert.equal(await text("Simulated detail failure", false).count(), 0);
  }

  // 3: A, B, A keeps the newest A response.
  await start({ A: detail("A", "old-A"), B: detail("B") });
  await inspect(0).click();
  await inspect(1).click();
  await fixture({ details: { A: detail("A", "new-A"), B: detail("B") } });
  await inspect(0).click();
  await expectNewReads(3);
  await release(2);
  await expectOnlyDetail("new-A");
  await release(0);
  await release(1);
  await settle();
  await expectOnlyDetail("new-A");
  assert.equal(await text("payload-old-A", false).count(), 0);
  assert.equal(await text("payload-B", false).count(), 0);
  await button("Close").click();
  await dialog.waitFor({ state: "hidden" });

  // 3: Open linked task closes the detail and invalidates an older A.
  await start({ A: detail("A", "old-A"), B: detail("B") });
  await inspect(0).click();
  await fixture({ details: { A: detail("A", "new-A", ["linked-1"]) } });
  await inspect(0).click();
  await expectNewReads(2);
  await release(1);
  await expectOnlyDetail("new-A");
  await button("Open linked task linked-1").click();
  await dialog.waitFor({ state: "hidden" });
  await release(0);
  await settle();
  await goToPage("Events");
  await text("Sender event: sender-A").waitFor();
  assert.equal(await dialog.count(), 0);

  // 3: leaving the Events page ends the lifetime of a pending read.
  await start({ A: detail("A") });
  await inspect(0).click();
  await expectNewReads(1);
  await goToPage("All tasks");
  await button("Improve empty states").waitFor();
  await release(0);
  await settle();
  await goToPage("Events");
  await text("Sender event: sender-A").waitFor();
  assert.equal(await dialog.count(), 0);

  // 4: a single current read, a closed detail, requests in order, a missing
  // event and an explicit retry after a failure keep their behavior.
  await start({ A: detail("A"), B: detail("B") });
  await inspect(0).click();
  await expectNewReads(1);
  await release(0);
  await expectOnlyDetail("A");
  await button("Close").click();
  await dialog.waitFor({ state: "hidden" });
  await settle();
  assert.equal(await dialog.count(), 0);
  await inspect(0).click();
  await inspect(1).click();
  await expectNewReads(2);
  // A is superseded by B before it settles, so only B ever opens.
  await release(1);
  await settle();
  assert.equal(await dialog.count(), 0);
  await release(2);
  await expectOnlyDetail("B");
  await button("Close").click();
  await dialog.waitFor({ state: "hidden" });

  await start({});
  await inspect(0).click();
  await expectNewReads(1);
  await release(0);
  await settle();
  assert.equal(await dialog.count(), 0);
  assert.equal(await text("Simulated detail failure", false).count(), 0);

  await start({ A: detail("A") });
  await inspect(0).click();
  await expectNewReads(1);
  await release(0, "fail");
  await text("Simulated detail failure A", false).waitFor();
  assert.equal(await dialog.count(), 0);
  await inspect(0).click();
  await expectNewReads(1);
  await text("Simulated detail failure A", false).waitFor({
    state: "detached",
  });
  await release(1);
  await expectOnlyDetail("A");
  await text("Simulated detail failure A", false).waitFor({
    state: "detached",
  });
  await button("Close").click();
  await dialog.waitFor({ state: "hidden" });

  // 5: Reopen and Delete use the accepted event, and a read alone changes
  // nothing. An opened deletion confirmation keeps its captured event.
  await start({ A: detail("A"), B: detail("B") });
  await inspect(0).click();
  await inspect(1).click();
  await expectNewReads(2);
  await release(1);
  await expectOnlyDetail("B");
  await release(0);
  await settle();
  assert.equal(await methodCount("ReopenEvent"), 0);
  assert.equal(await methodCount("DeleteEvent"), 0);
  await button("Reopen for processing").click();
  await text(/push · PENDING/, false)
    .first()
    .waitFor();
  const reopen = (await requests()).find((r) => r.method === "ReopenEvent");
  assert.equal(reopen.input.id, "B");
  assert.equal(reopen.input.expectedRevision, "1");
  await text("Device-B", false).waitFor();
  await button("Close").click();
  await dialog.waitFor({ state: "hidden" });

  await start({ A: detail("A"), B: detail("B") });
  await inspect(0).click();
  await inspect(1).click();
  await expectNewReads(2);
  await release(1);
  await expectOnlyDetail("B");
  await button("Delete event").click();
  await text("Delete event?").waitFor();
  await release(0);
  await settle();
  await text("Delete event?").waitFor();
  assert.equal(await methodCount("DeleteEvent"), 0);
  await button("Delete permanently").click();
  await text("Delete event?").waitFor({ state: "detached" });
  const removed = (await requests()).filter((r) => r.method === "DeleteEvent");
  assert.equal(removed.length, 1);
  assert.equal(removed[0].input.id, "B");
  assert.deepEqual(errors, []);
  console.log("Events detail ownership browser checks passed.");
} finally {
  await browser?.close();
  server.kill();
}
