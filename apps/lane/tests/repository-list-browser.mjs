// SPDX-License-Identifier: Apache-2.0
// Controlled web fixture for the repository-only list presentation and query path.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";
import { ids } from "./planning-mock.mjs";

const port = 46423;
const base = `http://127.0.0.1:${port}`;
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
const active = page.locator('[data-active-tab="true"]');
const button = (name) => active.getByRole("button", { name, exact: true });
const listRequests = async () =>
  (await (await fetch(`${base}/test/requests`)).json())
    .filter((entry) => entry.method === "ListTasks")
    .map((entry) => entry.input);
const waitList = async (predicate) => {
  for (let attempt = 0; attempt < 50; attempt++) {
    const request = (await listRequests())
      .filter((item) => item.repositoryId === ids.repoApps)
      .at(-1);
    if (request && predicate(request)) return request;
    await page.waitForTimeout(100);
  }
  throw new Error(
    `No matching repository query: ${JSON.stringify((await listRequests()).at(-1))}`,
  );
};
const overflow = () =>
  page.locator("lynx-view").evaluate((view) => {
    const width = document.documentElement.clientWidth;
    const content = view.shadowRoot.querySelector(
      '[data-active-tab="true"] .repository-task-list',
    );
    const wide = [...content.querySelectorAll("*")]
      .filter((element) => {
        const rect = element.getBoundingClientRect();
        return rect.width && rect.height && rect.right > width + 1;
      })
      .map((element) => `${element.tagName}.${element.className}`);
    return { width, scroll: document.documentElement.scrollWidth, wide };
  });

try {
  await page.goto(base);
  await active
    .locator(".repo-nav")
    .getByRole("button", { name: "apps" })
    .click();
  await active
    .locator(".repository-context", { hasText: "Repository" })
    .waitFor();
  await active.getByRole("heading", { name: "apps" }).waitFor();
  assert.equal(
    (await waitList((request) => request.repositoryId === ids.repoApps))
      .repositoryId,
    ids.repoApps,
  );
  assert.equal(
    await active.locator(".repository-task-columns").getByText("Task").count(),
    1,
  );
  assert.equal(await active.locator(".repository-task-row").count(), 3);
  assert.equal(
    await active
      .locator(".repository-task-row")
      .first()
      .getByRole("button", { name: "Task: Improve empty states" })
      .count(),
    1,
  );
  assert.equal(
    await active.getByRole("button", { name: "Fix docs typo" }).count(),
    0,
  );
  assert.equal(
    await active
      .locator(".repository-task-row")
      .first()
      .getByText("Proposal", { exact: true })
      .count(),
    1,
  );

  await button("Progress filter: All").click();
  await button("Progress filter: Blocked").waitFor();
  assert.equal(await button("Progress filter: Blocked").count(), 1);
  await button("Progress filter: Blocked").click();
  let request = await waitList(
    (item) =>
      item.repositoryId === ids.repoApps &&
      item.progress?.includes("PROGRESS_BLOCKED"),
  );
  assert.equal(request.page.size, 100);
  assert.equal(
    await button("Progress filter: Blocked").getAttribute("aria-expanded"),
    null,
  );
  await button("Progress filter: Blocked").click();
  await page.keyboard.press("Escape");
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.activeElement?.getAttribute("accessibility-label") ===
      "Progress filter: Blocked",
  );
  assert.equal(await button("Progress filter: All").count(), 0);
  assert.equal(await button("Progress filter: To do").count(), 0);
  assert.equal(
    await page
      .locator("lynx-view")
      .evaluate((view) =>
        view.shadowRoot.activeElement?.getAttribute("accessibility-label"),
      ),
    "Progress filter: Blocked",
  );
  await button("Proposal filter: All").click();
  await button("Proposal filter: Pending").click();
  request = await waitList(
    (item) =>
      item.repositoryId === ids.repoApps &&
      item.proposal?.includes("PROPOSAL_PENDING"),
  );
  assert.deepEqual(request.progress, ["PROGRESS_BLOCKED"]);

  await button("Filters, 0 active").click();
  assert.equal(await button("Labels: apps / Backend").count(), 0);
  assert.equal(await button("Labels: docs / Backend").count(), 0);
  await button("Priority: High").click();
  await waitList(
    (item) =>
      item.repositoryId === ids.repoApps &&
      item.filter?.priorities?.includes("TASK_PRIORITY_HIGH"),
  );
  await button("Priority: High").focus();
  await page.keyboard.press("Escape");
  await button("Priority: High").waitFor({ state: "detached" });
  await button("Filters, 1 active").waitFor();
  assert.equal(await button("Priority: High").count(), 0);
  assert.equal(
    await page
      .locator("lynx-view")
      .evaluate((view) =>
        view.shadowRoot.activeElement?.getAttribute("accessibility-label"),
      ),
    "Filters, 1 active",
  );
  await button("Clear filters").click();
  await button("Filters, 0 active").waitFor();
  assert.equal(await button("Filters, 0 active").count(), 1);
  await button("Sort by: Default order").click();
  await button("Sort by: Due date").click();
  await button("Sort direction: Ascending").click();
  await button("Sort direction: Descending").click();
  request = await waitList(
    (item) =>
      item.repositoryId === ids.repoApps &&
      item.sortDirection === "SORT_DIRECTION_DESCENDING",
  );
  assert.equal(request.sortKey, "TASK_SORT_KEY_DUE_DATE");

  const longRepository = `github.com/acme/${"repository".repeat(12)}`;
  const longTask = "Task title ".repeat(20);
  const longLabel = "accessibility".repeat(8);
  await page.route("**/lane.v1.LaneService/ListRepositories", async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.items[0].version.repository.displayName = "";
    body.items[0].version.repository.normalizedUrl = longRepository;
    await route.fulfill({ response, json: body });
  });
  await page.route("**/lane.v1.LaneService/ListTasks", async (route) => {
    const response = await route.fetch();
    const body = await response.json();
    body.items[0].version.task.title = longTask;
    await route.fulfill({ response, json: body });
  });
  await page.route(
    "**/lane.v1.PlanningService/ListPlanningItems",
    async (route) => {
      const response = await route.fetch();
      const body = await response.json();
      const label = body.items.find(
        (item) => item.version.label?.repositoryId === ids.repoApps,
      );
      if (label) label.version.label.name = longLabel;
      await route.fulfill({ response, json: body });
    },
  );
  await page.reload();
  await active
    .locator(".repo-nav")
    .getByRole("button", { name: longRepository })
    .click();
  await active.getByRole("heading", { name: longRepository }).waitFor();
  await button(`Task: ${longTask}`).waitFor();
  await active.getByText(longLabel, { exact: true }).waitFor();

  for (const width of [1440, 900, 899, 390, 360]) {
    await page.setViewportSize({ width, height: 900 });
    await page.locator(width < 900 ? ".app.mobile" : ".app.desktop").waitFor();
    await page.waitForTimeout(100);
    const bounds = await overflow();
    if (width < 900)
      assert.equal(
        await active.locator(".repository-task-columns").isVisible(),
        false,
      );
    assert.equal(
      bounds.scroll <= bounds.width + 1,
      true,
      `${width}: document scroll ${JSON.stringify(bounds)}`,
    );
    assert.deepEqual(bounds.wide, [], `${width}: repository content overflow`);
  }
  await page.locator("lynx-view").evaluate((view) => {
    const style = document.createElement("style");
    style.textContent =
      ".repository-task-list x-text,.repository-task-list .button{font-size:1.5rem!important}";
    view.shadowRoot.append(style);
  });
  await page.setViewportSize({ width: 360, height: 420 });
  await page.waitForTimeout(100);
  assert.deepEqual(
    (await overflow()).wide,
    [],
    "enlarged repository text overflows",
  );
  await page.setViewportSize({ width: 360, height: 900 });
  await button("docs").click();
  await active.getByRole("heading", { name: "docs" }).waitFor();
  assert.equal(
    await page.locator("#lane-root").getAttribute("data-selected-tab"),
    `repository-${ids.repoDocs}`,
  );
  assert.equal(
    await page
      .locator(`.tab-panel [data-tab-id="repository-${ids.repoApps}"]`)
      .count(),
    1,
  );
  await button("All repositories").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        ?.getAttribute("data-selected-tab") === "page-Tasks",
  );
  assert.equal(
    await page.locator("#lane-root").getAttribute("data-selected-tab"),
    "page-Tasks",
  );
  console.log(
    JSON.stringify({ event: "repository_list_browser", status: "passed" }),
  );
} finally {
  await browser.close();
  server.kill();
}
