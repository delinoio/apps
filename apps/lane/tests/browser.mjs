// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";
const evidence =
  process.env.LANE_BROWSER_EVIDENCE ||
  join(tmpdir(), "lane-ui-browser-evidence");
await mkdir(evidence, { recursive: true });
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) =>
    reject(new Error(`Mock preview exited ${code}`)),
  );
});
const browser = await chromium.launch({
  channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
  headless: true,
});
const errors = [];
const context = await browser.newContext({
  viewport: { width: 1440, height: 900 },
});
const page = await context.newPage();
page.on("pageerror", (e) => errors.push(e.message));
try {
  await page.goto("http://127.0.0.1:46400");
  await page
    .getByRole("button", { name: "Improve empty states", exact: true })
    .waitFor();
  await page
    .getByRole("button", { name: "Improve empty states", exact: true })
    .click();
  const title = page.getByRole("textbox", { name: "Title", exact: true });
  await title.waitFor();
  assert.equal(await title.inputValue(), "Improve empty states");
  await title.fill("Unsaved local title");
  await page.waitForTimeout(150);
  await page.request.get("http://127.0.0.1:46400/test/external");
  await page
    .locator("lynx-view")
    .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await page
    .getByText("External changes are available.", { exact: false })
    .waitFor();
  assert.equal(await title.inputValue(), "Unsaved local title");
  await page.request.get("http://127.0.0.1:46400/test/fail-save");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page.getByText("Simulated storage failure", { exact: false }).waitFor();
  assert.equal(await title.inputValue(), "Unsaved local title");
  await page.getByRole("button", { name: "Delete task", exact: true }).click();
  await page
    .getByRole("dialog", { name: "Permanently delete task?" })
    .waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  assert.equal(await title.inputValue(), "Unsaved local title");
  await page
    .getByRole("button", { name: "Preview Markdown", exact: true })
    .click();
  assert.equal(await page.locator("lynx-view script").count(), 0);
  await page.getByText("[Image blocked: blocked]", { exact: true }).waitFor();
  await page.screenshot({ path: join(evidence, "desktop-editor.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(500);
  assert.equal(
    await page.locator("#lane-root").getAttribute("class"),
    "app mobile",
  );
  assert.ok(await title.isVisible());
  await page.screenshot({ path: join(evidence, "mobile-editor.png") });
  await page.getByRole("button", { name: "Back to list", exact: true }).click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor();
  await page.getByRole("button", { name: "Keep editing", exact: true }).click();
  assert.equal(await title.inputValue(), "Unsaved local title");
  await page.getByRole("button", { name: "Back to list", exact: true }).click();
  await page
    .getByRole("button", { name: "Discard changes", exact: true })
    .click();
  await page.getByRole("button", { name: "+ New task", exact: true }).waitFor();
  await page.screenshot({ path: join(evidence, "mobile-list.png") });
  await page.getByRole("button", { name: "+ New task", exact: true }).focus();
  await page.keyboard.press("Enter");
  await page.getByRole("textbox", { name: "Title", exact: true }).waitFor();
  await page.waitForTimeout(150);
  assert.equal(
    await page
      .getByRole("textbox", { name: "Title", exact: true })
      .evaluate((e) => e.getRootNode().activeElement === e),
    true,
  );
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.locator("lynx-view").evaluate((view) => {
    view.shadowRoot.querySelector("style").textContent +=
      ":host{font-size:32px}";
  });
  await page.waitForTimeout(300);
  await page.screenshot({ path: join(evidence, "enlarged-text.png") });
  assert.deepEqual(errors, []);
  await writeFile(
    join(evidence, "result.json"),
    JSON.stringify(
      {
        scope:
          "Mock authenticated server; no live Logto, native, or provider acceptance",
        checks: [
          "desktop navigation and task details",
          "title visible and accessible textbox",
          "external change retains unsaved input",
          "failed save retains input",
          "deletion dialog Escape",
          "safe Markdown and blocked HTTP image",
          "mobile detail/list navigation",
          "discard confirmation retains input on cancel",
          "keyboard activation",
          "new task title autofocus",
          "text enlargement screenshot",
        ],
        errors,
      },
      null,
      2,
    ),
  );
  console.log(`Browser checks passed. Mock-only evidence: ${evidence}`);
} finally {
  await browser.close();
  server.kill();
}
