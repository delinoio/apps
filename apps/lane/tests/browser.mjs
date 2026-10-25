// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
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
  const appButton = (name) => page.getByRole("button", { name, exact: true });
  const rail = await page.locator("lynx-view").evaluate((view) =>
    [...view.shadowRoot.querySelectorAll(".app-switch")].map((element) => {
      const { width, height, top } = element.getBoundingClientRect();
      return { name: element.getAttribute("aria-label"), width, height, top };
    }),
  );
  assert.deepEqual(
    rail.map((button) => button.name),
    ["Lane", "Pace", "RealQA"],
  );
  assert.ok(
    rail.every((button) => button.width === 44 && button.height === 44),
  );
  assert.equal(rail[1].top - rail[0].top, 56);
  assert.equal(rail[2].top - rail[1].top, 56);
  assert.equal(await page.getByText(/Perfine/).count(), 0);
  const search = page.getByRole("textbox", {
    name: "Search title or description",
    exact: true,
  });
  for (const name of ["Lane", "Pace", "Handrail"]) {
    const button = appButton(name);
    const tooltip = button.locator(".app-tooltip");
    await search.focus();
    await page.mouse.move(1400, 880);
    assert.equal(await tooltip.isVisible(), false);
    await button.hover();
    assert.equal(await tooltip.isVisible(), true);
    assert.equal(await tooltip.textContent(), name);
    const triggerBounds = await button.boundingBox();
    const tooltipBounds = await tooltip.boundingBox();
    assert.ok(tooltipBounds.x >= triggerBounds.x + triggerBounds.width);
    const textFits = await tooltip.evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      const text = element.querySelector("x-text").getBoundingClientRect();
      return text.left >= bounds.left && text.right <= bounds.right;
    });
    assert.ok(textFits, `${name} tooltip text must fit its clipping boundary`);
    await page.mouse.move(1400, 880);
    assert.equal(await tooltip.isVisible(), false);
    await button.focus();
    assert.equal(await tooltip.isVisible(), true);
    await search.focus();
    assert.equal(await tooltip.isVisible(), false);
  }
  await search.fill("preserved filter");
  await appButton("Pace").click();
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
  assert.equal(await appButton("Pace").getAttribute("aria-pressed"), "true");
  assert.equal(await appButton("All tasks").count(), 0);
  assert.equal(await appButton("Overview").count(), 1);
  await page.waitForFunction(
    () =>
      document.querySelector("lynx-view").shadowRoot.activeElement
        ?.textContent === "Coming soon",
  );
  await page.screenshot({ path: join(evidence, "desktop-pace.png") });
  await appButton("RealQA").focus();
  await page.keyboard.press(" ");
  await page
    .getByText("RealQA is not available yet.", { exact: true })
    .waitFor();
  assert.equal(await appButton("RealQA").getAttribute("aria-pressed"), "true");
  await page.screenshot({ path: join(evidence, "desktop-realqa.png") });
  for (const width of [899, 900, 390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await page.waitForTimeout(150);
    const layout = await page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const rail = root.querySelector(".app-rail").getBoundingClientRect();
      const nav = root.querySelector(".navigation").getBoundingClientRect();
      const content = root.querySelector(".workspace").getBoundingClientRect();
      const button = root
        .querySelector(".coming-soon .button")
        .getBoundingClientRect();
      return {
        railWidth: rail.width,
        navWidth: nav.width,
        navBottom: nav.bottom,
        contentTop: content.top,
        buttonRight: button.right,
      };
    });
    assert.equal(layout.railWidth, 64);
    if (width < 900) assert.ok(layout.navBottom <= layout.contentTop + 1);
    else assert.equal(layout.navWidth, 240);
    assert.ok(layout.buttonRight <= width);
  }
  await page.screenshot({ path: join(evidence, "mobile-realqa.png") });
  await appButton("Back to Lane").click();
  await search.waitFor();
  assert.equal(await search.inputValue(), "preserved filter");
  await search.fill("");
  await appButton("Improve empty states").waitFor();
  await page
    .getByText("Loading…", { exact: true })
    .waitFor({ state: "hidden" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.request.get("http://127.0.0.1:46400/test/empty-list?enabled=1");
  await page
    .locator("lynx-view")
    .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await page.getByText("No tasks here yet", { exact: true }).waitFor();
  await page
    .getByText("Loading…", { exact: true })
    .waitFor({ state: "hidden" });
  assert.equal(
    await page
      .getByRole("button", { name: "Improve empty states", exact: true })
      .count(),
    0,
  );
  assert.equal(await page.getByText("Loading…", { exact: true }).count(), 0);
  await page.request.get("http://127.0.0.1:46400/test/empty-list?enabled=0");
  await page
    .locator("lynx-view")
    .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await page
    .getByRole("button", { name: "Improve empty states", exact: true })
    .waitFor();
  assert.equal(
    await page.getByText("No tasks here yet", { exact: true }).count(),
    0,
  );
  await page
    .getByRole("button", { name: "Improve empty states", exact: true })
    .click();
  const title = page.getByRole("textbox", { name: "Title", exact: true });
  await title.waitFor();
  assert.equal(await title.inputValue(), "Improve empty states");
  await title.fill("Unsaved local title");
  await page.waitForTimeout(150);
  await appButton("Pace").click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  assert.equal(await title.inputValue(), "Unsaved local title");
  assert.equal(await appButton("Lane").getAttribute("aria-pressed"), "true");
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
  for (const width of [320, 390, 402]) {
    await page.setViewportSize({ width, height: 844 });
    await page.waitForTimeout(150);
    const layout = await page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const fields = [...root.querySelectorAll(".task-list .field")].map(
        (e) => {
          const r = e.getBoundingClientRect();
          return { top: r.top, bottom: r.bottom };
        },
      );
      const tabs = [...root.querySelectorAll(".nav-items .button")].map((e) => {
        const r = e.getBoundingClientRect();
        return { top: r.top, bottom: r.bottom, left: r.left, right: r.right };
      });
      return { fields, tabs };
    });
    for (let i = 1; i < layout.fields.length; i++)
      assert.ok(
        layout.fields[i].top >= layout.fields[i - 1].bottom - 1,
        "Filter groups must not overlap",
      );
    assert.equal(layout.tabs.length, 5);
    assert.ok(
      layout.tabs.every((tab) => Math.abs(tab.top - layout.tabs[0].top) < 1),
      "Mobile tabs remain on one row",
    );
    assert.ok(
      layout.tabs.every((tab) => tab.left >= 0 && tab.right <= width),
      "Mobile tabs fit viewport",
    );
  }

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
        sourceRevision: execFileSync("git", ["rev-parse", "HEAD"], {
          encoding: "utf8",
        }).trim(),
        sourceState: "working tree under validation",
        command: "node tests/browser.mjs (Node.js 24)",
        scope:
          "Mock authenticated server; no live Logto, native, or provider acceptance",
        checks: [
          "equal Lane, Pace and RealQA rail buttons and app-specific sidebars",
          "mini-app keyboard activation, selected state and title focus",
          "Coming soon return preserves Lane filters",
          "mini-app switching respects unsaved-edit confirmation and Escape",
          "app rail and mini-app layout at 320, 390, 899 and 900 pixels",
          "desktop navigation and task details",
          "empty and populated task states replace one another",
          "title visible and accessible textbox",
          "external change retains unsaved input",
          "failed save retains input",
          "deletion dialog Escape",
          "safe Markdown and blocked HTTP image",
          "mobile detail/list navigation",
          "filter groups do not overlap; five tabs fit one row",
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
