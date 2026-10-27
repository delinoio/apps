// SPDX-License-Identifier: Apache-2.0
// Mock-only Pace checks. No screenshots or released-platform acceptance.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = Number(process.env.LANE_PACE_PORT || 46760);
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: String(port),
    LANE_PREVIEW_SCENARIO: "pace",
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
const button = (name) => page.getByRole("button", { name, exact: true });
const title = page.getByRole("textbox", { name: "Title", exact: true });
const requests = async () => (await fetch(`${base}/test/requests`)).json();
const mutations = async () =>
  (await requests()).filter((request) =>
    /^(Create|Save|Delete|SetProposal)/.test(request.method),
  );
const selectedTab = () =>
  page.locator("#lane-root").getAttribute("data-selected-tab");
const choosePace = async () => {
  await button("New tab").click();
  await page.getByRole("heading", { name: "New tab" }).waitFor();
  await button("Pace").click();
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
};

try {
  await page.goto(base);
  await button("pace-fixture").waitFor();
  await button("pace-fixture").click();
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      .shadowRoot.querySelector("#lane-root")
      .getAttribute("data-selected-tab")
      ?.startsWith("repository-"),
  );
  const search = page.getByRole("textbox", {
    name: "Search title or description",
    exact: true,
  });
  await search.fill("preserved filter");
  await button("Filters, 0 active").click();
  await button("Priority: High").click();
  await button("Sort by: Priority").click();
  await button("Sort direction: Descending").click();
  await page
    .locator('[data-active-tab="true"]')
    .getByText("preserved filter row 120", { exact: true })
    .first()
    .waitFor();
  const repositoryTab = await selectedTab();
  const rows = page.locator('[data-active-tab="true"] .rows');
  await rows.hover();
  await page.mouse.wheel(0, 200);
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector('[data-active-tab="true"] .rows').scrollTop >
      0,
  );
  await page.waitForTimeout(500);
  const scrollBefore = await rows.evaluate((element) => element.scrollTop);
  assert.ok(scrollBefore > 0);

  // Keep the Pace tab open before recording the tab count. Returning from Pace
  // must then preserve the existing set of tabs, including the task editor.
  await choosePace();
  await button("Back to Lane").click();
  await page.waitForFunction(
    (expected) =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === expected,
    repositoryTab,
  );

  await page.locator("lynx-view").evaluate((view) => {
    view.sendGlobalEvent("lane.openRoute", [
      { kind: "task", id: "019a196d-0000-7000-8000-000000000003" },
    ]);
  });
  await title.waitFor();
  assert.equal(await title.inputValue(), "Pace preservation fixture");
  await title.fill("Unsaved Pace draft");
  const taskTab = await selectedTab();
  const tabCount = await page.getByRole("tab").count();
  const mutationCount = (await mutations()).length;

  for (const activation of ["click", "Enter", " "]) {
    await choosePace();
    await page.keyboard.press("Escape");
    assert.equal(
      await page.locator("#lane-root").getAttribute("data-app"),
      "Pace",
    );
    if (activation === "click") await button("Back to Lane").click();
    else {
      await button("Back to Lane").focus();
      await page.keyboard.press(activation);
    }
    await title.waitFor();
    assert.equal(await title.inputValue(), "Unsaved Pace draft");
    assert.equal(await selectedTab(), taskTab);
    assert.equal(await page.getByRole("tab").count(), tabCount);
    assert.equal((await mutations()).length, mutationCount);
  }

  await page.getByRole("tab", { name: "pace-fixture", exact: true }).click();
  await page.waitForFunction(
    (expected) =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === expected,
    repositoryTab,
  );
  assert.equal(await search.inputValue(), "preserved filter");
  assert.equal(
    await button("Priority: High").getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await button("Sort by: Priority").getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await button("Sort direction: Descending").getAttribute("aria-pressed"),
    "true",
  );
  assert.equal(
    await page
      .locator('[data-active-tab="true"] .rows')
      .evaluate((rows) => rows.scrollTop),
    scrollBefore,
  );
  assert.equal((await mutations()).length, mutationCount);
  console.log("Pace retained-state browser checks passed (mock-only).");
} finally {
  await browser.close();
  server.kill();
}
