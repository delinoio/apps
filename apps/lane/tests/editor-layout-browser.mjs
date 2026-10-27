// SPDX-License-Identifier: Apache-2.0
// Check the measured task-editor breakpoint with disposable planning data.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46417;
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
const page = await browser.newPage({ viewport: { width: 3200, height: 900 } });
page.setDefaultTimeout(15000);
const active = page.locator('[data-active-tab="true"]');
const button = (name) => active.getByRole("button", { name, exact: true });
const editor = () => active.locator(".editor");
const content = () => active.locator(".editor-content");
const settle = () => page.waitForTimeout(250);
const setContentWidth = async (width) => {
  const [outer, inner] = await Promise.all([
    editor().boundingBox(),
    content().boundingBox(),
  ]);
  assert.ok(outer && inner);
  await editor().evaluate(
    (element, pixels) => {
      element.style.width = `${pixels}px`;
    },
    width + outer.width - inner.width,
  );
  await settle();
  assert.ok(Math.abs((await content().boundingBox()).width - width) < 1);
};

try {
  await page.goto(`http://127.0.0.1:${port}`);
  await button("Improve empty states").click();
  await active.getByRole("heading", { name: "Properties" }).waitFor();
  await setContentWidth(1100);
  assert.equal(await active.locator(".editor-content-wide").count(), 1);
  assert.equal(
    await active
      .getByRole("textbox", { name: "Start date (YYYY-MM-DD)" })
      .count(),
    1,
  );
  assert.equal(await button("Add or edit property").count(), 0);

  await setContentWidth(1099);
  assert.equal(await active.locator(".editor-content-wide").count(), 0);
  assert.equal(
    await active
      .getByRole("textbox", { name: "Start date (YYYY-MM-DD)" })
      .count(),
    0,
  );
  await button("Edit Priority: High").waitFor();
  await button("Add or edit property").click();
  await button("Start date").click();
  const start = active.getByRole("textbox", {
    name: "Start date (YYYY-MM-DD)",
  });
  await start.fill("2026-02-30");
  await button("Close properties editor").click();
  await button("Edit Start: 2026-02-30").waitFor();
  await setContentWidth(1100);
  assert.equal(await active.locator(".editor-content-wide").count(), 1);
  assert.equal(
    await active
      .getByRole("textbox", { name: "Start date (YYYY-MM-DD)" })
      .inputValue(),
    "2026-02-30",
  );
  await setContentWidth(1099);
  await button("Edit Start: 2026-02-30").waitFor();

  await editor().evaluate((element) => {
    element.style.width = "";
  });
  await page.setViewportSize({ width: 360, height: 800 });
  await settle();
  assert.equal(await active.locator(".editor-content-wide").count(), 0);
  const bounds = await page.locator("lynx-view").evaluate((view) => {
    const width = document.documentElement.clientWidth;
    const chips = [
      ...view.shadowRoot.querySelectorAll(
        '[data-active-tab="true"] .property-chips .button, [data-active-tab="true"] .relation-chips .button',
      ),
    ];
    return {
      width,
      scroll: document.documentElement.scrollWidth,
      overflow: chips.filter(
        (chip) => chip.getBoundingClientRect().right > width + 1,
      ).length,
    };
  });
  assert.equal(bounds.overflow, 0);
  assert.ok(bounds.scroll <= bounds.width);
  await button("Add or edit property").click();
  await button("Story points").click();
  await active
    .getByRole("textbox", { name: "Story points (whole number)" })
    .waitFor();
  console.log(
    "Task editor content-width and compact interaction checks passed.",
  );
} finally {
  await browser.close();
  server.kill();
}
