// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { chromium } from "playwright";

const previewPort = await new Promise((resolve, reject) => {
  const listener = createServer();
  listener.once("error", reject);
  listener.listen(0, "127.0.0.1", () => {
    const address = listener.address();
    listener.close(() => resolve(address.port));
  });
});
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
  env: { ...process.env, LANE_PREVIEW_PORT: String(previewPort) },
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
  await page.goto(`http://127.0.0.1:${previewPort}`);
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
  const assertLayout = async (width) => {
    const layout = await menu.evaluate((element) => {
      const rect = (selector) =>
        element.querySelector(selector).getBoundingClientRect();
      const panel = element.getBoundingClientRect();
      const header = rect(".command-search-row");
      const close = rect(".command-close");
      const results = rect(".command-results");
      const footer = rect(".command-footer");
      return {
        panel: { left: panel.left, right: panel.right, height: panel.height },
        header: { bottom: header.bottom, height: header.height },
        close: { width: close.width, height: close.height },
        results: { top: results.top, bottom: results.bottom },
        footer: { top: footer.top, height: footer.height },
        footerText: [...element.querySelectorAll(".command-footer .muted")].map(
          (text) => ({
            size: getComputedStyle(text).fontSize,
            width: text.getBoundingClientRect().width,
          }),
        ),
        rows: [...element.querySelectorAll(".command-result")].map(
          (row) => row.getBoundingClientRect().height,
        ),
        textFits: [
          ...element.querySelectorAll(
            ".command-result-label, .command-context, .command-footer .muted",
          ),
        ].every((text) => text.scrollWidth <= text.clientWidth + 1),
        overflow: element.scrollWidth > element.clientWidth + 1,
        radius: getComputedStyle(element).borderTopLeftRadius,
      };
    });
    assert.ok(layout.panel.left >= 15 && layout.panel.right <= width - 15);
    assert.ok(layout.panel.height <= 668);
    assert.ok(layout.header.height >= 76);
    assert.ok(layout.close.width >= 44 && layout.close.height >= 44);
    assert.ok(layout.footer.height >= 48);
    assert.ok(layout.results.top >= layout.header.bottom - 1);
    assert.ok(layout.results.bottom <= layout.footer.top + 1);
    assert.ok(layout.rows.every((height) => height >= 48));
    assert.equal(layout.textFits, true);
    assert.equal(layout.overflow, false);
    assert.equal(layout.radius, "16px");
    return layout;
  };
  await announced("Selected: All tasks, Pages");
  assert.deepEqual(
    await menu.locator(".command-group-label").allTextContents(),
    ["Pages", "Apps", "Repositories"],
  );
  assert.deepEqual(
    await menu
      .locator(".command-result")
      .evaluateAll((rows) => rows.slice(0, 8).map((row) => row.id)),
    [
      "page-Tasks",
      "page-Proposals",
      "page-Repositories",
      "page-Events",
      "page-Settings",
      "app-Lane",
      "app-Pace",
      "app-RealQA",
    ],
  );
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
  await page.getByRole("tab", { name: "Events", exact: true }).waitFor();
  assert.equal(
    await page
      .getByRole("dialog", { name: "Discard unsaved changes?" })
      .count(),
    0,
  );
  await page
    .getByRole("tab", {
      name: "Unsaved command menu draft, Unsaved changes",
      exact: true,
    })
    .click();
  await title.waitFor();
  assert.equal(await title.inputValue(), "Unsaved command menu draft");
  await page.getByRole("tab", { name: "Events", exact: true }).click();
  await page
    .locator(".workspace")
    .getByText("Events and webhook receivers", { exact: true })
    .waitFor();
  await page.keyboard.press("Meta+k");
  await input.fill("DELINOIO APPS");
  await button("github.com/delinoio/apps, Repositories").click();
  await button("Improve empty states").waitFor();
  for (const width of [1440, 900, 899, 390, 360, 320]) {
    await page.setViewportSize({ width, height: 700 });
    await page.keyboard.press("Control+k");
    await menu.waitFor();
    await assertLayout(width);
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
  await page.unroute("**/lane.v1.LaneService/ListRepositories");
  let releaseRepositories;
  const heldRepositories = new Promise((resolve) => {
    releaseRepositories = resolve;
  });
  const fixtureRepositories = Array.from({ length: 40 }, (_, index) => {
    const number = index + 1;
    return {
      result: "RESULT_KIND_APPLIED",
      version: {
        formatVersion: 3,
        entityId: `019a196d-0000-7000-8000-${String(number).padStart(12, "0")}`,
        versionId: `019a196d-0000-7000-8001-${String(number).padStart(12, "0")}`,
        kind: "ENTITY_KIND_REPOSITORY",
        repository: {
          displayName:
            number === 1
              ? "Atlas"
              : number === 40
                ? "Long repository " + "X".repeat(200)
                : `Repository ${String(number).padStart(2, "0")}`,
          normalizedUrl:
            number === 1
              ? "github.com/example/atlas"
              : `github.com/example/repo-${String(number).padStart(2, "0")}`,
        },
      },
    };
  });
  await page.route("**/lane.v1.LaneService/ListRepositories", async (route) => {
    await heldRepositories;
    await route.fulfill({ json: { items: fixtureRepositories } });
  });
  await page.reload({ waitUntil: "domcontentloaded" });
  await button("Improve empty states").waitFor();
  await page.keyboard.press("Control+k");
  await menu.getByText("Loading repositories...", { exact: true }).waitFor();
  assert.equal(await menu.locator(".command-result").count(), 8);
  releaseRepositories();
  await menu.locator(".command-result").nth(47).waitFor();
  assert.equal(await menu.locator(".command-result").count(), 48);
  await input.fill("  EXAMPLE atlas ");
  assert.equal(await menu.locator(".command-result").count(), 1);
  assert.equal(
    await menu.locator(".command-result").first().getAttribute("id"),
    `repo-${fixtureRepositories[0].version.entityId}`,
  );
  await input.fill(" ");
  assert.equal(await menu.locator(".command-result").count(), 48);
  for (const width of [1440, 900, 899, 390, 360]) {
    await page.setViewportSize({ width, height: 700 });
    await assertLayout(width);
    await page.keyboard.press("ArrowUp");
    await page.waitForFunction(() => {
      const root = document.querySelector("lynx-view").shadowRoot;
      const selected = root.querySelector(".command-selected");
      const list = root.querySelector(".command-results");
      return (
        selected?.textContent.includes("Long repository") &&
        selected.getBoundingClientRect().bottom <=
          list.getBoundingClientRect().bottom + 1
      );
    });
    await page.keyboard.press("ArrowDown");
  }
  await page.locator("lynx-view").evaluate((view) => {
    const style = document.createElement("style");
    style.textContent =
      ".command-result-label{font-size:30px!important}.command-context{font-size:24px!important}.command-footer .muted{font-size:28px!important}.command-search{font-size:32px!important}";
    view.shadowRoot.append(style);
  });
  await page.setViewportSize({ width: 360, height: 700 });
  const enlarged = await assertLayout(360);
  assert.deepEqual(
    enlarged.footerText.map((text) => text.size),
    ["28px", "28px", "28px"],
  );
  assert.ok(enlarged.footer.height > 48, JSON.stringify(enlarged));
  assert.ok(enlarged.rows.at(-1) > 48);
  assert.equal(
    await menu
      .locator(".command-selected")
      .evaluate((element) => getComputedStyle(element).borderLeftWidth),
    "3px",
  );
  await menu.locator(".command-result").last().focus();
  assert.equal(
    await menu
      .locator(".command-result")
      .last()
      .evaluate((element) => getComputedStyle(element).borderTopColor),
    "rgb(23, 32, 51)",
  );
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      result: "passed",
      checks:
        "Meta/Ctrl+K, focus restore/trap, filtering/empty state, wrapping, app/page/repository navigation, tab draft preservation, loading/error states, long-list scrolling, responsive and enlarged-text layout",
    }),
  );
} finally {
  await browser?.close();
  server.kill();
}
