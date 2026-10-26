// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";

const evidence =
  process.env.LANE_BROWSER_EVIDENCE ||
  join(tmpdir(), "lane-command-palette-evidence");
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
let browser;
try {
  browser = await chromium.launch({
    channel: process.env.LANE_BROWSER_CHANNEL || "chrome",
    headless: true,
  });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 900 },
  });
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("http://127.0.0.1:46400");
  const button = (name) => page.getByRole("button", { name, exact: true });
  await button("Improve empty states").waitFor();
  const menu = page.getByRole("dialog", {
    name: "Go to a page or repository",
    exact: true,
  });
  const input = page.getByRole("textbox", {
    name: "Go to a page or repository",
    exact: true,
  });
  const search = page.getByRole("textbox", {
    name: "Search title or description",
    exact: true,
  });
  await search.focus();
  await page.keyboard.press("Meta+k");
  await menu.waitFor();
  assert.equal(
    await input.evaluate(
      (element) => element === element.getRootNode().activeElement,
    ),
    true,
  );
  assert.equal(await menu.locator(".command-result").count(), 9);
  const announcement = menu.getByRole("status");
  const announced = async (text) => {
    await page.waitForFunction(
      (expected) =>
        document
          .querySelector("lynx-view")
          .shadowRoot.querySelector(".command-announcement")?.textContent ===
        expected,
      text,
    );
    assert.equal(await announcement.getAttribute("aria-live"), "polite");
    assert.equal(await announcement.getAttribute("aria-atomic"), "true");
    assert.equal(
      await input.evaluate(
        (element) => element === element.getRootNode().activeElement,
      ),
      true,
    );
  };
  await announced("Selected: All tasks, Pages");
  await page.keyboard.press("ArrowUp");
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      .shadowRoot.querySelector(".command-selected")
      ?.textContent.includes("github.com/delinoio/apps"),
  );
  await announced("Selected: github.com/delinoio/apps, Repositories");
  await page.keyboard.press("ArrowDown");
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      .shadowRoot.querySelector(".command-selected")
      ?.textContent.includes("All tasks"),
  );
  await announced("Selected: All tasks, Pages");
  await page.screenshot({
    path: join(evidence, "desktop-command-palette.png"),
  });
  await input.fill("does not exist");
  await menu
    .locator(".command-empty")
    .filter({ hasText: "No matching destinations" })
    .waitFor();
  await announced("No matching destinations");
  await page.keyboard.press("Enter");
  assert.equal(await menu.isVisible(), true);
  await page.keyboard.press("Escape");
  await menu.waitFor({ state: "detached" });
  assert.equal(
    await search.evaluate(
      (element) => element === element.getRootNode().activeElement,
    ),
    true,
  );
  await page.keyboard.press("Control+k");
  await input.fill("  pAcE ");
  await announced("Selected: Pace, Apps");
  await page.keyboard.press("Enter");
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
  await page.keyboard.press("Control+k");
  await input.fill("All tasks");
  await page.keyboard.press("Enter");
  await button("Improve empty states").click();
  const title = page.getByRole("textbox", { name: "Title", exact: true });
  await title.fill("Unsaved command menu draft");
  await page.keyboard.press("Meta+k");
  await input.fill("Events");
  await page.keyboard.press("Enter");
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor();
  await page.keyboard.press("Meta+k");
  assert.equal(await menu.count(), 0);
  await button("Keep editing").click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor({ state: "detached" });
  assert.equal(await title.inputValue(), "Unsaved command menu draft");
  await page.keyboard.press("Meta+k");
  await input.fill("Events");
  await page.keyboard.press("Enter");
  await button("Discard changes").click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor({ state: "detached" });
  await page
    .locator(".workspace")
    .getByText("Events and webhook receivers", { exact: true })
    .waitFor();
  await page.keyboard.press("Meta+k");
  await input.fill("DELINOIO APPS");
  await button("github.com/delinoio/apps, Repositories").click();
  await button("Improve empty states").waitFor();
  for (const width of [900, 899, 390, 320]) {
    await page.setViewportSize({ width, height: 700 });
    await page.keyboard.press("Control+k");
    await menu.waitFor();
    const bounds = await menu.boundingBox();
    assert.ok(bounds.x >= 15 && bounds.x + bounds.width <= width - 15);
    assert.equal(
      await menu.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        return element.contains(
          element.getRootNode().elementFromPoint(bounds.x + 8, bounds.y + 30),
        );
      }),
      true,
      "The dialog must render above the app rail",
    );
    await page.keyboard.press("ArrowUp");
    await page.waitForFunction(() => {
      const root = document.querySelector("lynx-view").shadowRoot;
      const selected = root.querySelector(".command-selected");
      const list = root.querySelector(".command-results");
      if (!selected?.textContent.includes("github.com/delinoio/apps"))
        return false;
      const row = selected.getBoundingClientRect();
      const container = list.getBoundingClientRect();
      return row.top >= container.top - 1 && row.bottom <= container.bottom + 1;
    });
    await page.keyboard.press("ArrowDown");
    await input.focus();
    await page.keyboard.press("Shift+Tab");
    assert.equal(
      await menu.evaluate((element) =>
        element.contains(element.getRootNode().activeElement),
      ),
      true,
    );
    if (width === 320)
      await page.screenshot({
        path: join(evidence, "mobile-command-palette.png"),
      });
    await page.keyboard.press("Escape");
    await menu.waitFor({ state: "detached" });
  }
  await page.keyboard.press("Meta+k");
  await menu.waitFor();
  await page.keyboard.press("Meta+k");
  await menu.waitFor({ state: "detached" });
  await page.route("**/lane.v1.LaneService/ListRepositories", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({
        code: "unavailable",
        message: "Fixture repository refresh failure",
      }),
    }),
  );
  await page.reload();
  await button("Improve empty states").waitFor();
  await page.keyboard.press("Control+k");
  await menu
    .getByText("Repositories could not be refreshed.", { exact: true })
    .waitFor();
  assert.equal(await menu.locator(".command-result").count(), 8);
  await input.fill("Pace");
  await page.keyboard.press("Enter");
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      result: "passed",
      evidence,
      checks:
        "Meta/Ctrl+K, focus restore/trap, filtering/empty state, wrapping, app/page/repository navigation, dirty cancel/discard, responsive widths",
    }),
  );
} finally {
  await browser?.close();
  server.kill();
}
