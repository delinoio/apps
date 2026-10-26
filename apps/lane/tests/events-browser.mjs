// SPDX-License-Identifier: Apache-2.0
// Events invalidation checks against the test-only preview fixture. Fixture
// evidence verifies the shared ReactLynx screens in a headless browser, not a
// real account, webhook sender or native host.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46418;
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_PORT: String(port) },
  stdio: ["ignore", "pipe", "inherit"],
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) => reject(new Error(`Preview exited ${code}`)));
});
const repoA = "019a196d-0000-7000-8000-000000000001";
const repoB = "019a196d-0000-7000-8000-0000000000b1";
const receiver = (id, name, repositoryId = repoA) => ({
  id,
  name,
  repositoryId,
  revision: "1",
});
const event = (id, state, repositoryId = repoA) => ({
  id,
  receiverId: "r1",
  repositoryId,
  senderEventId: `sender-${id}`,
  eventType: "push",
  state,
  revision: "1",
});
const fixture = (body) =>
  fetch(`${base}/test/events?body=${encodeURIComponent(JSON.stringify(body))}`);
const requests = async () =>
  (await fetch(`${base}/test/events-requests`)).json();
const expectRequests = async (count) => {
  for (let attempt = 0; attempt < 100; attempt++) {
    if ((await requests()).length >= count) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.fail(`Expected at least ${count} Events read requests.`);
};

let browser;
try {
  await fixture({
    receivers: [receiver("r1", "Old receiver")],
    events: [event("e1", "EVENT_STATE_PENDING")],
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
  const invalidate = () =>
    page
      .locator("lynx-view")
      .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await button("Improve empty states").waitFor();
  await page.keyboard.press("Control+k");
  await page
    .getByRole("textbox", { name: "Go to a page or repository", exact: true })
    .fill("Events");
  await page.keyboard.press("Enter");
  await text("Events and webhook receivers").waitFor();
  await text("Old receiver").waitFor();
  await text("Sender event: sender-e1").waitFor();
  await text(/push · PENDING/, false).waitFor();

  // 1 and 2: one invalidation rereads both lists and shows externally changed
  // receivers, processing states, new events and deletions.
  let before = (await requests()).length;
  await fixture({
    receivers: [receiver("r1", "Renamed receiver")],
    events: [
      event("e1", "EVENT_STATE_LEASED"),
      event("e2", "EVENT_STATE_PENDING"),
    ],
  });
  await invalidate();
  await text("Renamed receiver").waitFor();
  await text("Sender event: sender-e2").waitFor();
  await text(/push · LEASED/, false).waitFor();
  assert.equal(await text("Old receiver").count(), 0);
  assert.equal((await requests()).length - before, 2);
  assert.equal(
    (await requests())
      .slice(before)
      .map((r) => r.method)
      .join(),
    "ListReceivers,ListEvents",
  );
  await fixture({
    receivers: [receiver("r1", "Renamed receiver")],
    events: [event("e1", "EVENT_STATE_COMPLETED")],
  });
  await invalidate();
  await text(/push · COMPLETED/, false).waitFor();
  await text("Sender event: sender-e2").waitFor({ state: "detached" });
  await fixture({ receivers: [], events: [] });
  await invalidate();
  await text("No events match these filters.").waitFor();
  await text("Renamed receiver").waitFor({ state: "detached" });

  // 3: filters, continuation and an unsaved receiver draft survive a reload,
  // and the refreshed query supplies the next continuation cursor.
  await fixture({
    receivers: [receiver("r1", "Receiver A")],
    events: [
      event("p1", "EVENT_STATE_PENDING"),
      event("p2", "EVENT_STATE_PENDING"),
      event("p3", "EVENT_STATE_PENDING"),
      event("c1", "EVENT_STATE_COMPLETED"),
    ],
    pageSize: 2,
  });
  await button("Refresh events").click();
  await button("Pending").click();
  await text("Sender event: sender-p1").waitFor();
  await text("Sender event: sender-p2").waitFor();
  assert.equal(await text("Sender event: sender-c1").count(), 0);
  await button("Load more events").click();
  await text("Sender event: sender-p3").waitFor();
  await button("Create receiver").click();
  const name = page.getByRole("textbox", {
    name: "Receiver name",
    exact: true,
  });
  await name.fill("Unsaved receiver draft");
  before = (await requests()).length;
  await fixture({
    receivers: [receiver("r1", "Receiver A")],
    events: [
      event("p0", "EVENT_STATE_PENDING"),
      event("p1", "EVENT_STATE_PENDING"),
      event("p2", "EVENT_STATE_PENDING"),
      event("p3", "EVENT_STATE_PENDING"),
    ],
    pageSize: 2,
  });
  await invalidate();
  await text("Sender event: sender-p0").waitFor();
  await text("Sender event: sender-p3").waitFor({ state: "detached" });
  const reread = (await requests()).slice(before);
  const firstPage = reread.find((r) => r.method === "ListEvents");
  assert.equal(firstPage.input.page.cursor ?? "", "");
  assert.deepEqual(firstPage.input.states, ["EVENT_STATE_PENDING"]);
  assert.equal(await name.inputValue(), "Unsaved receiver draft");
  await page.keyboard.press("Escape");
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  assert.equal(
    (await button("Pending").getAttribute("aria-pressed")) ?? "",
    "",
  );
  await button("Load more events").click();
  await text("Sender event: sender-p3").waitFor();
  assert.equal(await text("Sender event: sender-p1").count(), 1);
  assert.equal(await text("Sender event: sender-p2").count(), 1);
  assert.equal(await text("Sender event: sender-p3").count(), 1);

  // 5: a failed notification load stays visible with the draft intact, and the
  // explicit Refresh events retry recovers it.
  await button("Create receiver").click();
  await name.fill("Draft kept during failure");
  await fixture({
    receivers: [receiver("r1", "Receiver after failure")],
    failReads: 1,
  });
  before = (await requests()).length;
  await invalidate();
  await expectRequests(before + 1);
  assert.equal(await name.inputValue(), "Draft kept during failure");
  await page.keyboard.press("Escape");
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  await text("Simulated read failure", false).waitFor();
  await button("Refresh events").click();
  await text("Receiver after failure").waitFor();
  await text("Simulated read failure", false).waitFor({ state: "detached" });
  assert.deepEqual(errors, []);
  console.log("Events invalidation browser checks passed.");
} finally {
  await browser?.close();
  server.kill();
}
