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
    events: [
      event("e1", "EVENT_STATE_PENDING"),
      event("e2", "EVENT_STATE_LEASED"),
      event("e3", "EVENT_STATE_COMPLETED"),
    ],
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
  await page
    .locator(".events-event-row .events-state")
    .getByText("Pending")
    .waitFor();
  assert.deepEqual(
    await page.locator(".events-event-row .events-state").allTextContents(),
    ["Pending", "Processing", "Completed"],
  );
  const overview = await page.locator(".events-page").textContent();
  const ordered = [
    "Events and webhook receivers",
    "Lane retains events.",
    "Repository",
    "Event state",
    "Receiving settings",
    "Retained events",
  ];
  let previous = -1;
  for (const label of ordered) {
    const position = overview.indexOf(label, previous + 1);
    assert.ok(position > previous, `${label} follows the prior section`);
    previous = position;
  }
  assert.equal(await page.locator(".events-page [role='tab']").count(), 0);
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
  await page
    .locator(".events-event-row .events-state")
    .getByText("Processing")
    .waitFor();
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
  await page
    .locator(".events-event-row .events-state")
    .getByText("Completed")
    .waitFor();
  await text("Sender event: sender-e2").waitFor({ state: "detached" });
  await fixture({ receivers: [], events: [] });
  await invalidate();
  await text("No events match these filters.").waitFor();
  await text("No receivers for this repository.").waitFor();
  await text("Renamed receiver").waitFor({ state: "detached" });

  await fixture({ receivers: [], events: [], failReads: 1 });
  await button("Refresh events").click();
  await text("Simulated read failure", false).waitFor();
  await page
    .getByRole("status")
    .filter({ hasText: "Simulated read failure" })
    .waitFor();
  assert.equal(await text("No events match these filters.").count(), 0);
  assert.equal(await text("No receivers for this repository.").count(), 0);
  await button("Refresh events").click();
  await text("No events match these filters.").waitFor();

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
  await button("Event state: Pending").click();
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
    (await button("Event state: Pending").getAttribute("aria-pressed")) ?? "",
    "true",
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
  await page.setViewportSize({ width: 900, height: 900 });
  const fits = async (target, width) => {
    const dimensions = await target
      .locator(".events-page")
      .evaluate((element) => ({
        clientWidth: element.clientWidth,
        scrollWidth: element.scrollWidth,
        offenders: [...element.querySelectorAll("*")]
          .filter(
            (child) =>
              child.getBoundingClientRect().right >
              element.getBoundingClientRect().right + 1,
          )
          .slice(0, 5)
          .map((child) => ({
            className: child.className,
            text: child.textContent?.slice(0, 40),
          })),
      }));
    assert.equal(
      dimensions.scrollWidth <= dimensions.clientWidth + 1,
      true,
      `Events content fits at ${width}px: ${JSON.stringify(dimensions)}`,
    );
  };
  await fits(page, 900);
  const mobileContext = await browser.newContext({
    viewport: { width: 360, height: 900 },
    storageState: await page.context().storageState(),
  });
  const mobilePage = await mobileContext.newPage();
  await mobilePage.goto(base);
  await mobilePage
    .getByRole("button", { name: "Improve empty states" })
    .waitFor();
  await mobilePage.keyboard.press("Control+k");
  await mobilePage
    .getByRole("textbox", { name: "Go to a page or repository", exact: true })
    .fill("Events");
  await mobilePage.keyboard.press("Enter");
  await mobilePage.getByText("Events and webhook receivers").waitFor();
  await fits(mobilePage, 360);
  await fixture({
    receivers: [receiver("r1", "R".repeat(90))],
    events: [
      {
        ...event("e1", "EVENT_STATE_PENDING"),
        eventType: "event-" + "x".repeat(90),
        senderEventId: "sender-" + "y".repeat(90),
      },
    ],
  });
  await mobilePage
    .getByRole("button", { name: "Refresh events", exact: true })
    .click();
  await mobilePage.getByText("R".repeat(90)).waitFor();
  await mobilePage.evaluate(() => {
    document.documentElement.style.fontSize = "32px";
  });
  await fits(mobilePage, "360px with long identifiers and 200% text");
  await mobileContext.close();
  await page.setViewportSize({ width: 1440, height: 900 });
  await fixture({
    receivers: [receiver("r1", "Old receiver")],
    events: [event("e1", "EVENT_STATE_PENDING")],
  });

  assert.deepEqual(errors, []);
  console.log("Events invalidation browser checks passed.");
} finally {
  await browser?.close();
  server.kill();
}
