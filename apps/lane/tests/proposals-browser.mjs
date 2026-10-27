// SPDX-License-Identifier: Apache-2.0
// Proposals list checks use only the planning fixture and never capture screenshots.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46426;
const base = `http://127.0.0.1:${port}`;
const id = (n) => `019a2b00-0000-7000-8000-${n.toString(16).padStart(12, "0")}`;
const repositoryId = id(1);
const makeTask = (n, title, proposal, progress, extra = {}) => ({
  formatVersion: 3,
  entityId: id(n),
  versionId: id(n + 1000),
  kind: "ENTITY_KIND_TASK",
  task: {
    repositoryId,
    title,
    description: "",
    proposal,
    progress,
    ...extra,
  },
  origin: { kind: "ORIGIN_KIND_APP", deviceName: "Fixture" },
  createdAt: "2026-10-25T00:00:00Z",
});
const fixtures = [
  makeTask(
    10,
    "Improve keyboard navigation",
    "PROPOSAL_PENDING",
    "PROGRESS_TODO",
    {
      priority: "TASK_PRIORITY_HIGH",
      type: "TASK_TYPE_IMPROVEMENT",
    },
  ),
  makeTask(
    11,
    "Clarify empty-state guidance",
    "PROPOSAL_PENDING",
    "PROGRESS_IN_PROGRESS",
  ),
  makeTask(
    12,
    "Review offline error messages",
    "PROPOSAL_PENDING",
    "PROGRESS_BLOCKED",
  ),
  makeTask(
    13,
    "Document repository setup",
    "PROPOSAL_PENDING",
    "PROGRESS_DONE",
  ),
  makeTask(
    14,
    "Simplify validation copy",
    "PROPOSAL_PENDING",
    "PROGRESS_CANCELED",
  ),
  makeTask(15, "Regular work", "PROPOSAL_REGULAR", "PROGRESS_TODO"),
  makeTask(16, "Accepted work", "PROPOSAL_ACCEPTED", "PROGRESS_DONE"),
  makeTask(17, "Rejected work", "PROPOSAL_REJECTED", "PROGRESS_BLOCKED"),
];
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: String(port),
    LANE_PREVIEW_SCENARIO: "planning",
  },
  stdio: ["ignore", "pipe", "inherit"],
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) => reject(new Error(`Preview exited ${code}`)));
});
const browser = await chromium.launch({
  channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
  headless: true,
});
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.setDefaultTimeout(15000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const surface = page.locator('[data-active-tab="true"], .tab-bar');
const button = (name) => surface.getByRole("button", { name, exact: true });
const text = (value) => surface.getByText(value, { exact: true });
const requestLog = async () => (await fetch(`${base}/test/requests`)).json();
const listRequests = async () =>
  (await requestLog())
    .filter((request) => request.method === "ListTasks")
    .map((request) => request.input);
const waitRequest = async (accept) => {
  for (let attempt = 0; attempt < 50; attempt++) {
    const matching = (await listRequests()).findLast(accept);
    if (matching) return matching;
    await page.waitForTimeout(100);
  }
  throw new Error("Expected ListTasks request was not sent");
};

try {
  await fetch(
    `${base}/test/plan?body=${encodeURIComponent(JSON.stringify({ proposalTasks: fixtures }))}`,
  );
  await page.goto(base);
  await button("Proposals · 5").click();
  await text("Document repository setup").waitFor();
  await text("Simplify validation copy").waitFor();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        ?.shadowRoot?.activeElement?.getAttribute("accessibility-label") ===
      "Search title or description",
  );
  assert.equal(await text("Regular work").count(), 0);
  const defaultRequest = await waitRequest(
    (request) => request.proposal?.[0] === "PROPOSAL_PENDING",
  );
  assert.deepEqual(defaultRequest.progress, undefined);
  assert.equal(defaultRequest.page.size, 100);
  assert.equal(
    await button("Proposal disposition: Pending proposals").count(),
    1,
  );
  assert.equal(await button("Progress: All").count(), 1);
  assert.equal(
    await button("Sort direction: Ascending").getAttribute("aria-disabled"),
    "true",
  );
  const rows = surface.locator(".proposal-row");
  assert.equal(await rows.count(), 5);
  assert.equal(
    await surface
      .locator('.result-announcement[role="status"][aria-live="polite"]')
      .count(),
    1,
  );
  assert.match(await rows.nth(3).innerText(), /Pending[\s\S]*Done/);
  assert.match(await rows.nth(4).innerText(), /Pending[\s\S]*Canceled/);
  assert.match(
    await rows.first().innerText(),
    /Priority High.*Type Improvement/,
  );

  await button("Proposal disposition: Pending proposals").click();
  await button("Proposal disposition: Rejected").focus();
  await page.keyboard.press("Escape");
  await button("Proposal disposition: Rejected").waitFor({ state: "hidden" });
  assert.equal(
    await button("Proposal disposition: Pending proposals").count(),
    1,
  );
  assert.equal(
    await button("Proposal disposition: Pending proposals").evaluate(
      (element) => element === element.getRootNode().activeElement,
    ),
    true,
  );
  await button("Proposal disposition: Pending proposals").focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      ?.shadowRoot?.activeElement?.classList.contains("menu-option"),
  );
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await button("Proposal disposition: Regular").waitFor();
  assert.equal(
    (
      await waitRequest(
        (request) => request.proposal?.[0] === "PROPOSAL_REGULAR",
      )
    ).proposal[0],
    "PROPOSAL_REGULAR",
  );
  await text("Regular work").waitFor();
  await button("Proposal disposition: Regular").click();
  await button("Proposal disposition: Accepted").click();
  await text("Accepted work").waitFor();
  await waitRequest((request) => request.proposal?.[0] === "PROPOSAL_ACCEPTED");
  await button("Proposal disposition: Accepted").click();
  await button("Proposal disposition: Rejected").click();
  await text("Rejected work").waitFor();
  await waitRequest((request) => request.proposal?.[0] === "PROPOSAL_REJECTED");
  await button("Proposal disposition: Rejected").click();
  await button("Proposal disposition: Pending").click();
  await waitRequest((request) => request.proposal?.[0] === "PROPOSAL_PENDING");
  await button("Proposal disposition: Pending").click();
  await button("Proposal disposition: Pending proposals").click();

  await button("Progress: All").click();
  await button("Progress: Done").click();
  const doneRequest = await waitRequest(
    (request) =>
      request.proposal?.[0] === "PROPOSAL_PENDING" &&
      request.progress?.[0] === "PROGRESS_DONE",
  );
  assert.equal(doneRequest.search, undefined);
  assert.equal(await rows.count(), 1);
  await text("Document repository setup").waitFor();

  await button("Filters, 0 active").click();
  await button("Priority: High").click();
  await button("Sort by: Default order").click();
  await button("Sort by: Due date").click();
  await button("Sort direction: Ascending").click();
  await button("Sort direction: Descending").click();
  await surface
    .getByRole("textbox", { name: "Search title or description" })
    .fill("document");
  await button("Clear filters").first().click();
  const cleared = await waitRequest(
    (request) =>
      request.search === "document" &&
      request.sortKey === "TASK_SORT_KEY_DUE_DATE" &&
      request.sortDirection === "SORT_DIRECTION_DESCENDING" &&
      !request.filter,
  );
  assert.deepEqual(cleared.progress, ["PROGRESS_DONE"]);
  assert.deepEqual(cleared.proposal, ["PROPOSAL_PENDING"]);
  assert.equal(
    await button("Filters, 0 active").getAttribute("aria-expanded"),
    "true",
  );
  await button("Filters, 0 active").click();
  await text(
    "Fields combine with AND; selected values in one field combine with OR. Unassigned adds tasks that have no value for that field.",
  ).waitFor({ state: "hidden" });

  for (const width of [1440, 900, 899, 390, 360]) {
    await page.setViewportSize({ width, height: 800 });
    await page.waitForTimeout(400);
    const result = await page.locator("lynx-view").evaluate((view) => {
      const width = document.documentElement.clientWidth;
      const wide = [...view.shadowRoot.querySelectorAll(".proposal-list *")]
        .filter((element) => {
          const box = element.getBoundingClientRect();
          return box.width > 0 && box.height > 0 && box.right > width + 1;
        })
        .map((element) => element.className);
      return { scroll: document.documentElement.scrollWidth, width, wide };
    });
    assert.deepEqual(result.wide, [], `Proposals content fits ${width}px`);
    assert.ok(
      result.scroll <= result.width,
      `No horizontal scroll at ${width}px`,
    );
  }

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "24px";
  });
  const enlarged = await page.locator("lynx-view").evaluate((view) => {
    const width = document.documentElement.clientWidth;
    return [...view.shadowRoot.querySelectorAll(".proposal-list *")].filter(
      (element) => {
        const box = element.getBoundingClientRect();
        return box.width > 0 && box.right > width + 1;
      },
    ).length;
  });
  assert.equal(enlarged, 0, "Enlarged text does not overflow Proposals");

  const paged = Array.from({ length: 105 }, (_, index) =>
    makeTask(
      100 + index,
      `Paged task ${String(index).padStart(3, "0")}`,
      "PROPOSAL_PENDING",
      "PROGRESS_TODO",
    ),
  );
  assert.equal(
    (
      await fetch(`${base}/test/plan`, {
        method: "POST",
        body: JSON.stringify({ proposalTasks: paged }),
      })
    ).ok,
    true,
  );
  await button("Progress: Done").click();
  await button("Progress: All").click();
  await surface
    .getByRole("textbox", { name: "Search title or description" })
    .fill("Paged");
  await text("Paged task 099").waitFor();
  assert.equal(await rows.count(), 100);
  assert.equal(await button("Load more tasks").count(), 1);
  await button("Load more tasks").click();
  await text("Paged task 104").waitFor();
  assert.equal(await rows.count(), 105);
  assert.equal(await button("Load more tasks").count(), 0);
  assert.equal(
    (await listRequests()).filter((request) => request.page?.cursor === "100")
      .length,
    1,
  );

  await surface
    .getByRole("textbox", { name: "Search title or description" })
    .fill("no match");
  await text("No proposals match").waitFor();
  await fetch(
    `${base}/test/plan?body=${encodeURIComponent(JSON.stringify({ proposalTasks: [] }))}`,
  );
  await surface
    .getByRole("textbox", { name: "Search title or description" })
    .fill("");
  await text("No proposals here yet").waitFor();

  await fetch(
    `${base}/test/plan?body=${encodeURIComponent(JSON.stringify({ failNext: { method: "ListTasks", code: "unavailable", message: "Fixture failure" } }))}`,
  );
  await surface
    .getByRole("textbox", { name: "Search title or description" })
    .fill("error");
  await text("Proposals could not be loaded").waitFor();
  await button("Retry loading tasks").click();
  await text("No proposals match").waitFor();
} finally {
  await browser.close();
  server.kill();
}
assert.deepEqual(errors, []);
console.log("Proposals browser checks passed with fixture data.");
