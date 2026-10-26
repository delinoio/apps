// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { chromium } from "playwright";
const evidence =
  process.env.LANE_BROWSER_EVIDENCE ||
  join(tmpdir(), "lane-update-browser-evidence");
await mkdir(evidence, { recursive: true });
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_UPDATES: "1" },
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
const errors = [];
try {
  const page = await browser.newPage({
    viewport: { width: 1280, height: 820 },
  });
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("http://127.0.0.1:46400");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  const update = page.getByRole("button", { name: "Update Lane", exact: true });
  await update.waitFor();
  await page.getByText("Installed release: v1.2.3", { exact: true }).waitFor();
  await page.screenshot({ path: join(evidence, "updates-idle.png") });
  await update.focus();
  await page.keyboard.press("Enter");
  await page.getByText("Checking for updates…", { exact: true }).waitFor();
  assert.equal(
    await page
      .getByRole("button", { name: "Checking…", exact: true })
      .getAttribute("tabindex"),
    "-1",
  );
  assert.equal(await page.evaluate(() => window.laneUpdateFixture.starts), 1);
  for (const [phase, message, progress] of [
    ["downloading", "Downloading update…", 42],
    ["verifying", "Verifying update…", undefined],
    ["installing", "Installing update. Lane will restart…", undefined],
    [
      "error",
      "The update checksum did not match. Retry the update.",
      undefined,
    ],
  ]) {
    await page.evaluate(
      ({ phase, message, progress }) =>
        Object.assign(window.laneUpdateFixture, { phase, message, progress }),
      { phase, message, progress },
    );
    await page.getByText(message, { exact: true }).waitFor();
    if (phase === "downloading") {
      await page.getByText("42%", { exact: true }).waitFor();
      await page.screenshot({ path: join(evidence, "updates-progress.png") });
    }
  }
  await page.getByRole("button", { name: "Retry update", exact: true }).focus();
  await page.keyboard.press(" ");
  await page.getByText("Checking for updates…", { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.laneUpdateFixture.starts), 2);
  await page.evaluate(() =>
    Object.assign(window.laneUpdateFixture, {
      phase: "current",
      message: "Lane is up to date.",
    }),
  );
  await page.getByText("Lane is up to date.", { exact: true }).waitFor();
  for (const width of [900, 899, 390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await page.waitForTimeout(150);
    const bounds = await page.locator(".app-updates").boundingBox();
    assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= width + 1);
  }
  await page.screenshot({ path: join(evidence, "updates-narrow.png") });
  await page.getByRole("button", { name: "Tasks", exact: true }).click();
  await page.waitForFunction(() => window.laneUpdateFixture.cancels > 0);
  assert.deepEqual(errors, []);
  console.log(
    `Updater browser checks passed (mock desktop transport only): ${evidence}`,
  );
} finally {
  await browser.close();
  server.kill();
}
