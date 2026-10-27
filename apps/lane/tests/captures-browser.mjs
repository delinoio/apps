// SPDX-License-Identifier: Apache-2.0
// Runs against the explicit fixture preview, never an account or production API.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { chromium } from "playwright";
const evidence =
  process.env.LANE_BROWSER_EVIDENCE || "/tmp/realqa-412-ui-evidence";
await mkdir(evidence, { recursive: true });
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_PORT: "46413" },
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
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const button = (name) => page.getByRole("button", { name, exact: true });
const base = "http://127.0.0.1:46413";
async function openPrompt() {
  await page.goto(base);
  await button("Improve empty states").click();
  await button("Prompt").click();
  await button("Preview Markdown").click();
  await page.getByText("Capture 1 of 1", { exact: true }).waitFor();
  await page.locator(".capture-result").scrollIntoViewIfNeeded();
}
try {
  await openPrompt();
  await page.locator(".capture-image").waitFor();
  await page.waitForFunction(() => {
    const image = document
      .querySelector("lynx-view")
      .shadowRoot.querySelector(".capture-image");
    const inner = image?.shadowRoot?.querySelector("img");
    return inner?.complete && inner.naturalWidth === 480;
  });
  await page.screenshot({ path: `${evidence}/desktop-saved-capture.png` });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator(".capture-result").scrollIntoViewIfNeeded();
  await page.waitForTimeout(200);
  const geometry = await page.locator(".capture-result").evaluate((element) => {
    const r = element.getBoundingClientRect();
    return {
      left: r.left,
      right: r.right,
      width: r.width,
      metadata: [...element.querySelectorAll(".capture-metadata")].map((e) => {
        const b = e.getBoundingClientRect();
        return { left: b.left, right: b.right, height: b.height };
      }),
    };
  });
  assert.ok(geometry.right <= 390.5, JSON.stringify(geometry));
  assert.ok(geometry.width > 0);
  assert.ok(geometry.metadata.every((item) => item.right <= 390.5));
  await page.screenshot({ path: `${evidence}/mobile-390-saved-capture.png` });
  for (const [state, message] of [
    ["waiting", "Waiting for synchronization."],
    ["error", "Could not retrieve this capture."],
    ["denied", "Permission denied."],
    ["deleted", "This capture was deleted."],
  ]) {
    await page.request.get(`${base}/test/capture-state?state=${state}`);
    await openPrompt();
    await page.getByText(message, { exact: false }).waitFor();
    assert.equal(await page.locator(".capture-image").count(), 0);
    assert.equal(
      await button("Retry").count(),
      state === "waiting" || state === "error" ? 1 : 0,
    );
    await page.screenshot({ path: `${evidence}/mobile-${state}.png` });
    if (state === "error") {
      await page.request.get(`${base}/test/capture-state?state=available`);
      await button("Retry").click();
      await page.locator(".capture-image").waitFor();
    }
  }
  await button("New tab").click();
  await button("RealQA").click();
  await page
    .getByText("Capture on desktop. View saved captures in Lane.", {
      exact: true,
    })
    .waitFor();
  assert.equal(await button("Full screen").count(), 0);
  await button("Open Lane").click();
  assert.deepEqual(errors, []);
  await writeFile(
    `${evidence}/result.json`,
    JSON.stringify(
      {
        result: "passed",
        fixture: true,
        viewports: [
          { width: 1440, height: 900 },
          { width: 390, height: 844 },
        ],
        geometry,
        errors,
      },
      null,
      2,
    ),
  );
  console.log(`Saved capture browser checks passed: ${evidence}`);
} finally {
  await browser.close();
  server.kill();
}
