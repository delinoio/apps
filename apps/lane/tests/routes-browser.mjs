// SPDX-License-Identifier: Apache-2.0
// Shared web UI with disposable fixtures; no native or live-account acceptance.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";
const base = "http://127.0.0.1:46442";
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: "46442",
    LANE_PREVIEW_SCENARIO: "task-open",
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
page.setDefaultTimeout(10000);
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
let documents = 0;
page.on("request", (r) => {
  if (r.isNavigationRequest() && r.frame() === page.mainFrame()) documents++;
});
const button = (name) => page.getByRole("button", { name, exact: true });
const tab = (name) => page.getByRole("tab", { name, exact: true });
const field = (name) => page.getByRole("textbox", { name, exact: true });
const path = (value) => page.waitForURL(`${base}${value}`);
try {
  await page.goto(`${base}/settings`);
  await tab("Settings").waitFor();
  await page.evaluate(() => {
    window.retainedView = document.querySelector("lynx-view");
  });
  await button("All tasks").click();
  await path("/tasks");
  // Explicit creation keeps All tasks available while ordinary navigation
  // reuses the clean draft pane for a separate Settings tab.
  await button("+ New task").click();
  await page.waitForURL(/\/tasks\/new\//);
  await field("Title").waitFor();
  await button("Settings").click();
  await tab("All tasks").click();
  await path("/tasks");
  await button("Task A").waitFor();
  await field("Search title or description").fill("Task");
  await button("+ New task").click();
  await page.waitForURL(/\/tasks\/new\//);
  await field("Title").waitFor();
  await button("Task A").click();
  await page.waitForURL(/\/tasks\/[0-9a-f-]+$/);
  await field("Title").waitFor();
  const taskPath = new URL(page.url()).pathname;
  await field("Title").fill("Retained route draft");
  await page.evaluate(() => {
    window.blankFrames = 0;
    window.observeRoutes = true;
    const sample = () => {
      if (!window.observeRoutes) return;
      const view = document.querySelector("lynx-view");
      if (
        view !== window.retainedView ||
        view.hidden ||
        ![
          ...view.shadowRoot.querySelectorAll(
            '.tab-panel[data-active-tab="true"]',
          ),
        ].some((p) => getComputedStyle(p).display !== "none")
      )
        window.blankFrames++;
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });
  await tab("Settings").click();
  await path("/settings");
  await page.goBack();
  await path(taskPath);
  await field("Title").waitFor();
  assert.equal(await field("Title").inputValue(), "Retained route draft");
  await page.goForward();
  await path("/settings");
  await tab("Retained route draft, Unsaved changes").click();
  await path(taskPath);
  assert.equal(await field("Title").inputValue(), "Retained route draft");
  assert.equal(
    documents,
    1,
    "click and history navigation do not reload the document",
  );
  assert.deepEqual(
    await page.evaluate(() => {
      window.observeRoutes = false;
      return {
        retained: document.querySelector("lynx-view") === window.retainedView,
        blankFrames: window.blankFrames,
      };
    }),
    { retained: true, blankFrames: 0 },
  );
  await field("Title").fill("Task A");
  await page.reload();
  await path(taskPath);
  await field("Title").waitFor();
  assert.equal(await field("Title").inputValue(), "Task A");
  for (const route of [
    "/proposals",
    "/repositories",
    "/events",
    "/settings",
    "/pace",
    "/realqa",
    "/new-tab",
  ]) {
    await page.goto(`${base}${route}`);
    await page.waitForFunction(() =>
      document
        .querySelector("lynx-view")
        ?.shadowRoot?.querySelector('.tab-panel[data-active-tab="true"]'),
    );
    await path(route);
  }
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      event: "lane_routes_browser_pass",
      taskPath,
      blankFrames: 0,
    }),
  );
} finally {
  await browser.close();
  server.kill();
}
