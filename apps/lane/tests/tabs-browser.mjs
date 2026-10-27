// SPDX-License-Identifier: Apache-2.0
// Real shared UI with disposable task/account fixtures, never native acceptance.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium } from "playwright";
const port = Number(process.env.LANE_TABS_PORT || 46428);
const base = `http://127.0.0.1:${port}`;
const evidence =
  process.env.LANE_BROWSER_EVIDENCE || join(tmpdir(), "lane-tabs-evidence");
await mkdir(evidence, { recursive: true });
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: String(port),
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
const errors = [];
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
page.on("pageerror", (error) => errors.push(error.message));
page.setDefaultTimeout(10000);
const button = (name) => page.getByRole("button", { name, exact: true });
const field = (name) => page.getByRole("textbox", { name, exact: true });
const tab = (name) => page.getByRole("tab", { name, exact: true });
const waitTitle = async (title) => {
  for (let i = 0; i < 100; i++) {
    if (
      (await field("Title").count()) &&
      (await field("Title").inputValue()) === title
    )
      return;
    await page.waitForTimeout(50);
  }
  throw new Error(`Expected editor title ${title}`);
};
const selectTab = async (name) => {
  await tab(name).click();
  await page.waitForFunction(
    (label) =>
      [
        ...document
          .querySelector("lynx-view")
          .shadowRoot.querySelectorAll('[role="tab"]'),
      ].some(
        (el) =>
          el.getAttribute("aria-label") === label &&
          el.getAttribute("aria-selected") === "true",
      ),
    name,
  );
};
const control = async (endpoint, query = "") =>
  (await fetch(`${base}/test/${endpoint}?${query}`)).json();
const held = async (endpoint, count) => {
  for (let i = 0; i < 100; i++) {
    const rows = await control(endpoint);
    if (rows.length >= count) return rows;
    await page.waitForTimeout(50);
  }
  throw new Error(`Expected ${count} held requests at ${endpoint}`);
};
try {
  await page.goto(base);
  await button("Task A").waitFor();
  await field("Search title or description").fill("Task");
  await button("Task A").click();
  await waitTitle("Task A");
  await field("Title").fill("A unsaved");
  await field("Description Markdown").fill("Keep A draft verbatim");
  await button("Task B").click();
  await waitTitle("Task B");
  await field("Title").fill("B unsaved");
  await selectTab("A unsaved, Unsaved changes");
  assert.equal(
    await field("Description Markdown").inputValue(),
    "Keep A draft verbatim",
  );
  assert.equal(await page.getByRole("dialog").count(), 0);
  await button("Task B").click();
  await waitTitle("B unsaved");
  assert.equal(
    await page.getByRole("tab").count(),
    3,
    "duplicate task reuses its tab",
  );
  assert.equal(await field("Title").inputValue(), "B unsaved");
  await selectTab("All tasks");
  assert.equal(await field("Search title or description").inputValue(), "Task");
  await button("New tab").click();
  await button("Pace").click();
  await button("Back to Lane").click();
  await button("Close Pace").click();
  assert.equal(await field("Search title or description").inputValue(), "Task");
  await button("Settings").click();
  await selectTab("Settings");
  await button("All tasks").click();
  await selectTab("All tasks");
  await button("Events").click();
  await selectTab("Events");
  assert.equal(await page.getByRole("tab").count(), 5);
  await selectTab("A unsaved, Unsaved changes");

  // Saving in a hidden tab must not replace another editor, even on response loss.
  await control("save-hold", "hold=1");
  await button("Save").click();
  await held("save-hold", 1);
  await page.waitForFunction(() =>
    [
      ...document
        .querySelector("lynx-view")
        .shadowRoot.querySelectorAll(".button"),
    ].some(
      (el) =>
        el.getAttribute("aria-label") === "Close A unsaved" &&
        el.getAttribute("aria-disabled") === "true",
    ),
  );
  await selectTab("B unsaved, Unsaved changes");
  await field("Description Markdown").fill("Keep B while A saves");
  await control("save-hold", "release=0:ok");
  await control("save-hold", "hold=0");
  await page.waitForTimeout(300);
  assert.equal(await field("Title").inputValue(), "B unsaved");
  assert.equal(
    await field("Description Markdown").inputValue(),
    "Keep B while A saves",
  );
  await button("Close B unsaved").click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?", exact: true })
    .waitFor();
  await button("Keep editing").click();
  assert.equal(await field("Title").inputValue(), "B unsaved");

  // Vertical tabs replace the icon rail. Narrow screens open the same list.
  for (const width of [1440, 900, 899, 390, 360]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForTimeout(200);
    assert.equal(await page.locator(".app-rail-scroll").count(), 0);
    if (width >= 900) {
      const tabs = await page.locator(".tab-bar").boundingBox();
      const panel = await page
        .locator('[data-active-tab="true"]')
        .boundingBox();
      assert.equal(tabs.width, 240);
      assert.ok(panel.x >= tabs.x + tabs.width - 1);
      assert.equal(
        await page.getByRole("tablist").getAttribute("aria-orientation"),
        "vertical",
      );
    } else {
      await button("Tabs (5)").click();
      await tab("B unsaved, Unsaved changes").waitFor();
      await page.keyboard.press("Escape");
      await page
        .getByRole("dialog", { name: "Open tabs", exact: true })
        .waitFor({ state: "detached" });
    }
    const editor = await page
      .locator('[data-active-tab="true"] .editor')
      .boundingBox();
    assert.ok(editor.x + editor.width <= width + 1);
    await page.screenshot({ path: join(evidence, `tabs-${width}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(200);
  // Browser zoom enlarges real text and controls, rather than changing only
  // a parent font that rem-sized controls can ignore.
  await page.evaluate(() => {
    document.body.style.zoom = "2";
  });
  await page.waitForTimeout(300);
  const enlargedAction = await button("New tab").boundingBox();
  assert.ok(
    enlargedAction.x >= 0 && enlargedAction.x + enlargedAction.width <= 1441,
  );
  await page.screenshot({ path: join(evidence, "tabs-zoom-200.png") });
  await page.evaluate(() => {
    document.body.style.zoom = "";
  });
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      .shadowRoot.querySelector("#lane-root")
      .classList.contains("desktop"),
  );
  // Roving focus changes focus without activation until Enter.
  await tab("B unsaved, Unsaved changes").focus();
  await page.keyboard.press("Home");
  assert.equal(
    await tab("All tasks").evaluate(
      (el) => el.getRootNode().activeElement === el,
    ),
    true,
  );
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector('[data-workspace-tab="page-Tasks"]')
        ?.getAttribute("aria-selected") === "true",
  );
  await button("New tab").click();
  for (const app of ["Lane", "Pace", "RealQA"]) await button(app).waitFor();
  await page.screenshot({ path: join(evidence, "new-tab-desktop.png") });
  await button("Lane").click();
  await selectTab("B unsaved, Unsaved changes");
  await button("Close B unsaved").click();
  await button("Discard changes").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelectorAll('[role="tab"]').length === 4,
  );
  assert.equal(await page.getByRole("tab").count(), 4);
  assert.equal(
    await tab("Settings").getAttribute("aria-selected"),
    "true",
    "close selects right neighbor",
  );
  await selectTab("All tasks");
  await button("+ New task").click();
  await waitTitle("");
  await field("Title").fill("Independent new draft");
  await selectTab("A unsaved");
  await selectTab("Independent new draft, Unsaved changes");
  assert.equal(await field("Title").inputValue(), "Independent new draft");
  const longTitle =
    "A long task title keeps its complete text and unsaved state readable in the open tabs menu on a narrow screen";
  await field("Title").fill(longTitle);
  await tab(`${longTitle}, Unsaved changes`).waitFor();
  await page.setViewportSize({ width: 360, height: 900 });
  await button("Tabs (5)").click();
  const longChoice = page
    .getByRole("dialog", { name: "Open tabs", exact: true })
    .getByRole("tab", {
      name: `${longTitle}, Unsaved changes`,
      exact: true,
    });
  await longChoice.waitFor();
  const menuBounds = await page
    .getByRole("dialog", { name: "Open tabs", exact: true })
    .boundingBox();
  const choiceBounds = await longChoice.boundingBox();
  assert.ok(
    choiceBounds.x >= menuBounds.x &&
      choiceBounds.x + choiceBounds.width <=
        menuBounds.x + menuBounds.width + 1,
  );
  assert.equal(await longChoice.getAttribute("aria-selected"), "true");
  assert.ok((await longChoice.textContent()).includes("Unsaved changes"));
  await page.screenshot({
    path: join(evidence, "tabs-long-title-menu-360.png"),
  });
  await page.keyboard.press("Escape");
  await page.setViewportSize({ width: 1440, height: 900 });
  await button("Cancel").click();
  await button("Discard changes").click();
  // Close all remaining tabs through their own controls; the final close keeps
  // a clean All tasks tab instead of leaving an unusable empty workspace.
  for (const name of ["Events", "Settings", "A unsaved", "All tasks"])
    await button(`Close ${name}`).click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelectorAll('[role="tab"]').length === 1,
  );
  assert.equal(await page.getByRole("tab").count(), 1);
  await button("Task B").waitFor();
  // An old read can finish in its own inactive tab, never pull selection back.
  await control("task-open", "hold=1");
  await button("Task B").click();
  await held("task-open", 1);
  await selectTab("All tasks");
  await control("task-open", "release=0:fail");
  await page.waitForTimeout(150);
  assert.equal(await tab("All tasks").getAttribute("aria-selected"), "true");
  await selectTab("Task B");
  await page
    .locator('[data-active-tab="true"] .notice')
    .filter({ hasText: "Read failed" })
    .waitFor();
  await control("task-open", "hold=0");
  await button("Retry").click();
  await field("Title").waitFor();
  assert.equal(await field("Title").inputValue(), "Task B");
  await button("Close Task B").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelectorAll('[role="tab"]').length === 1,
  );
  await control("task-open", "hold=1");
  const readsBeforeClose = (await control("task-open")).length;
  await button("Task B").click();
  await held("task-open", readsBeforeClose + 1);
  await button("Close Task B").click();
  await control("task-open", `release=${readsBeforeClose}:ok`);
  await control("task-open", "hold=0");
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelectorAll('[role="tab"]').length === 1,
  );
  await page.waitForTimeout(200);
  assert.equal(
    await field("Title").count(),
    0,
    "a closed pane's late read cannot reopen an editor",
  );
  assert.deepEqual(errors, []);
  console.log(
    JSON.stringify({
      event: "lane_tabs_browser_passed",
      evidence,
      widths: [1440, 900, 899, 390, 360],
    }),
  );
} catch (error) {
  await page.screenshot({ path: join(evidence, "failure.png") });
  console.log(
    await page.locator("lynx-view").evaluate((view) =>
      [...view.shadowRoot.querySelectorAll("[data-workspace-tab]")].map(
        (el) => ({
          name: el.getAttribute("aria-label"),
          value: el.getAttribute("aria-selected"),
          data: el.getAttribute("data-tab-selected"),
        }),
      ),
    ),
  );
  throw error;
} finally {
  await browser.close();
  server.kill();
}
