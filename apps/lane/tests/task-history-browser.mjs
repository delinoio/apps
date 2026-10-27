// SPDX-License-Identifier: Apache-2.0
// Browser-only dialog layout and focus checks against the test fixture server.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = Number(process.env.LANE_PREVIEW_PORT || 46490);
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
  env: { ...process.env, LANE_PREVIEW_PORT: String(port) },
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
  const button = (name) => page.getByRole("button", { name, exact: true });
  for (const width of [1200, 900, 360]) {
    await page.setViewportSize({ width, height: 800 });
    await page.goto(`http://127.0.0.1:${port}`);
    await button("Improve empty states").click();
    for (const [trigger, title] of [
      ["History", "Task history"],
      ["Prompt versions", "Prompt versions"],
    ]) {
      await button(trigger).click();
      const dialog = page.getByRole("dialog", { name: title, exact: true });
      await dialog.waitFor();
      await dialog.locator(".history-card").first().waitFor();
      assert.equal(
        await button("Close").evaluate(
          (element) => element === element.getRootNode().activeElement,
        ),
        true,
      );
      for (let i = 0; i < 4; i++) {
        await page.keyboard.press("Tab");
        assert.ok(
          await dialog.evaluate((element) =>
            element.contains(element.getRootNode().activeElement),
          ),
        );
      }
      const layout = await dialog.evaluate((element) => {
        const card = element.querySelector(".history-card");
        const before = card.querySelector(".history-value:first-child");
        const after = card.querySelector(".history-value:last-child");
        const dialogBox = element.getBoundingClientRect();
        return {
          dialogWidth: dialogBox.width,
          scrollWidth: element.scrollWidth,
          clientWidth: element.clientWidth,
          before: before?.getBoundingClientRect().toJSON(),
          after: after?.getBoundingClientRect().toJSON(),
        };
      });
      assert.ok(layout.dialogWidth <= width - 20);
      assert.ok(layout.scrollWidth <= layout.clientWidth + 1);
      if (trigger === "History") {
        assert.ok(layout.before && layout.after);
        if (width >= 900) assert.ok(layout.after.left > layout.before.left);
        else assert.ok(layout.after.top > layout.before.top);
      }
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      assert.equal(
        await button(trigger).evaluate(
          (element) => element === element.getRootNode().activeElement,
        ),
        true,
      );
    }
  }
  console.log(
    "Task history dialog layout and focus checks passed (mock-only).",
  );
} finally {
  await browser?.close();
  server.kill();
}
