// SPDX-License-Identifier: Apache-2.0
// The planning preview provides disposable conflict heads for layout and focus checks.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";
import { ids } from "./planning-mock.mjs";

const port = 46438;
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
let browser;
try {
  browser = await chromium.launch({
    channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
    headless: true,
  });
  const page = await browser.newPage();
  page.setDefaultTimeout(15000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const heads = [1, 2, 3].map((n) => ({
    versionId: `019a0000-0000-7000-8000-${String(n).padStart(12, "0")}`,
    title: `Comparison ${n}`,
    origin: { kind: "ORIGIN_KIND_APP", deviceName: `Device ${n}` },
    task: {
      description: `Description ${n}`,
      prompt:
        n === 1 ? null : n === 2 ? "" : "# Long **literal** source ".repeat(30),
      priority: "TASK_PRIORITY_HIGH",
      storyPoints: 0,
      labelIds: [ids.labelBackend],
      milestoneId: ids.milestone,
      cycleId: ids.cycle,
      parentTaskId: ids.taskC,
      relatedTaskIds: [ids.unknown],
    },
  }));
  await fetch(
    `${base}/test/plan?body=${encodeURIComponent(JSON.stringify({ conflictTask: ids.taskA, heads }))}`,
  );
  for (const viewport of [
    { width: 1180, height: 850 },
    { width: 900, height: 650 },
    { width: 360, height: 640 },
  ]) {
    await page.setViewportSize(viewport);
    await page.goto(base);
    await page
      .getByRole("button", { name: "Comparison 1", exact: true })
      .click();
    const opener = page.getByRole("button", {
      name: "Compare conflict",
      exact: true,
    });
    await opener.click();
    const dialog = page.getByRole("dialog", {
      name: "Resolve whole-task conflict",
    });
    await dialog.waitFor();
    assert.equal(await dialog.getAttribute("aria-modal"), "true");
    assert.equal(await dialog.locator(".conflict-head").count(), 3);
    assert.equal(await dialog.getByText("✓ Selected").count(), 1);
    await dialog.getByText(/Prompt:\s*Not set/).waitFor();
    await dialog.getByText(/Prompt:\s*Empty/).waitFor();
    await dialog.getByText("Parent: Plan the launch").first().waitFor();
    await dialog
      .getByText(`Related: (unavailable task ${ids.unknown})`)
      .first()
      .waitFor();
    const layout = await dialog.evaluate((element) => {
      const cards = [...element.querySelectorAll(".conflict-head")].map(
        (card) => card.getBoundingClientRect(),
      );
      const heading = element.querySelector("[data-dialog-default='true']");
      const active = element.getRootNode().activeElement;
      return {
        cardRows: cards.map((card) => Math.round(card.top)),
        rightmost: Math.max(...cards.map((card) => card.right)),
        viewportWidth: document.documentElement.clientWidth,
        headingFocused: active === heading || heading?.contains(active),
      };
    });
    assert.equal(
      layout.headingFocused,
      true,
      `heading focus at ${viewport.width}`,
    );
    assert.ok(layout.rightmost <= layout.viewportWidth + 1);
    if (viewport.width >= 900) {
      assert.equal(layout.cardRows[0], layout.cardRows[1]);
      assert.ok(layout.cardRows[2] > layout.cardRows[1]);
    } else assert.ok(layout.cardRows[1] > layout.cardRows[0]);
    const backdropBlocksRail = await dialog.evaluate((element) => {
      const root = element.getRootNode();
      const target = root.querySelector(
        ".tab-bar .button, .mobile-tab-toggle .button",
      );
      const rect = target?.getBoundingClientRect();
      if (!rect) return false;
      const hit = root.elementFromPoint(
        rect.x + rect.width / 2,
        rect.y + rect.height / 2,
      );
      return !!hit?.closest(".overlay");
    });
    assert.equal(backdropBlocksRail, true);
    if (viewport.width === 900) {
      const headingSize = await dialog
        .locator(".conflict-version-title")
        .first()
        .evaluate((element) => parseFloat(getComputedStyle(element).fontSize));
      await dialog.evaluate((element) => {
        const style = document.createElement("style");
        style.textContent =
          ".conflict-dialog x-text { font-size: 200% !important; }";
        element.getRootNode().append(style);
      });
      const enlargedSize = await dialog
        .locator(".conflict-version-title")
        .first()
        .evaluate((element) => parseFloat(getComputedStyle(element).fontSize));
      assert.ok(enlargedSize >= headingSize * 1.9);
      const save = dialog.getByRole("button", { name: "Save resolution" });
      await save.scrollIntoViewIfNeeded();
      assert.equal(await save.isVisible(), true);
      assert.equal(
        await dialog.getByRole("button", { name: "Close" }).isVisible(),
        true,
      );
    }
    for (let n = 0; n < 12; n++) {
      await page.keyboard.press("Tab");
      assert.equal(
        await dialog.evaluate((element) =>
          element.contains(element.getRootNode().activeElement),
        ),
        true,
      );
    }
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "detached" });
    assert.equal(
      await opener.evaluate(
        (element) => element === element.getRootNode().activeElement,
      ),
      true,
    );
  }
  assert.deepEqual(errors, []);
} finally {
  await browser?.close();
  server.kill();
}
