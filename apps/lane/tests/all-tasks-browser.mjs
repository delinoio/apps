// SPDX-License-Identifier: Apache-2.0
// Automated web-renderer checks use illustrative task fixtures, not a live account.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46428;
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
const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
page.setDefaultTimeout(15000);
const surface = page.locator('[data-active-tab="true"]');
const button = (name) => surface.getByRole("button", { name, exact: true });
const bounds = (selector) =>
  surface
    .locator(selector)
    .first()
    .evaluate((element) => {
      const rect = element.getBoundingClientRect();
      return {
        x: rect.x,
        y: rect.y,
        right: rect.right,
        bottom: rect.bottom,
        width: rect.width,
      };
    });
const assertWithin = (inner, outer, label) => {
  assert.ok(inner.x >= outer.x - 1, `${label} starts outside its pane`);
  assert.ok(
    inner.right <= outer.right + 1,
    `${label} ends outside its pane: ${JSON.stringify({ inner, outer })}`,
  );
};
try {
  await page.goto(base);
  await button("Display Options").click();
  await button("Layout: Board").click();
  const heading = await bounds(".task-list-content .heading");
  const headerActions = await bounds(".all-task-header-actions");
  assert.ok(
    headerActions.x >= heading.right - 1 &&
      headerActions.y < heading.bottom &&
      heading.y < headerActions.bottom,
    `Header actions should stay unwrapped at the wide viewport: ${JSON.stringify({ heading, headerActions })}`,
  );
  assertWithin(
    await bounds(".display-options-menu"),
    await bounds(".task-list-content"),
    "Display Options at the wide viewport",
  );
  await surface.locator(".task-list").evaluate((element) => {
    element.style.flex = "0 0 300px";
    element.style.width = "300px";
  });
  assertWithin(
    await bounds(".display-options-menu"),
    await bounds(".task-list-content"),
    "Display Options in a constrained pane",
  );
  await surface.locator(".task-list").evaluate((element) => {
    element.style.flex = "";
    element.style.width = "";
  });
  for (const width of [900, 899, 360]) {
    await page.setViewportSize({ width, height: 800 });
    await surface
      .locator(
        width >= 900 ? ".task-board:not(.stacked)" : ".task-board.stacked",
      )
      .waitFor();
    await surface.locator(".task-board-column").first().waitFor();
    const first = await bounds(".task-board-column:nth-child(1)");
    const second = await bounds(".task-board-column:nth-child(2)");
    if (width >= 900)
      assert.ok(second.x > first.x, `Board is horizontal at ${width}px`);
    else assert.ok(second.y > first.y, `Board stacks at ${width}px`);
    const overflow = await surface.evaluate((element) => {
      const content = element.querySelector(".task-list-content");
      const box = content.getBoundingClientRect();
      return box.right > document.documentElement.clientWidth + 1;
    });
    assert.equal(overflow, false, `All tasks content fits at ${width}px`);
  }
  await button("Completed items: None").click();
  for (const label of [
    "None",
    "Last 24 hours",
    "Last 7 days",
    "Last 30 days",
    "All",
  ])
    await button(`Select completed items: ${label}`).waitFor();
  const menu = await bounds(".display-options-menu");
  assertWithin(
    menu,
    await bounds(".task-list-content"),
    "Display Options at 360px",
  );
  await page.keyboard.press("Escape");
  await button("Select completed items: All").waitFor({ state: "hidden" });
  await button("Completed items: None").waitFor();
  await page.keyboard.press("Escape");
  await surface.locator(".display-options-menu").waitFor({ state: "hidden" });
} finally {
  await browser.close();
  server.kill();
}
