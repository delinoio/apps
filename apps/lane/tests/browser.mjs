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
  for (const name of ["Lane", "Pace", "RealQA"]) {
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
  const settingsFocus = async (heading) => {
    await page.waitForFunction((expected) => {
      const root = document.querySelector("lynx-view").shadowRoot;
      return root.activeElement?.textContent === expected;
    }, heading);
  };
  const navigationWrites = [];
  const recordWrites = (request) => {
    if (
      request.method() !== "GET" &&
      /ConnectSync|Notification|Deletion|Disconnect|auth/.test(request.url())
    )
      navigationWrites.push(request.url());
  };
  page.on("request", recordWrites);
  for (const width of [1440, 900, 899, 390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    for (const miniApp of ["Lane", "Pace", "RealQA"]) {
      for (const [utility, heading] of [
        ["Account & sync", "Online account"],
        ["App settings", "Notifications on this device"],
      ]) {
        await appButton(miniApp).click();
        await page.waitForTimeout(100);
        assert.equal(await page.locator(".app-switch").count(), 3);
        assert.equal(await page.locator(".app-utility").count(), 2);
        const button = appButton(utility);
        assert.equal(await button.getAttribute("aria-pressed"), null);
        await button.focus();
        const tooltip = button.locator(".app-tooltip");
        assert.equal(await tooltip.isVisible(), true);
        assert.equal(await tooltip.textContent(), utility);
        const bounds = await button.boundingBox();
        assert.equal(bounds.width, 44);
        assert.equal(bounds.height, 44);
        const tooltipBounds = await tooltip.boundingBox();
        assert.ok(
          Math.abs(tooltipBounds.y - bounds.y - 4) <= 1,
          "Utility tooltip stays beside its trigger after resize/scroll",
        );
        await page.keyboard.press(utility === "Account & sync" ? "Enter" : " ");
        await settingsFocus(heading);
        assert.equal(
          await appButton("Lane").getAttribute("aria-pressed"),
          "true",
        );
      }
    }
  }
  await page.screenshot({
    path: join(evidence, "mobile-settings-shortcuts.png"),
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await appButton("Account & sync").click();
  await settingsFocus("Online account");
  await appButton("Account & sync").hover();
  await page.screenshot({
    path: join(evidence, "desktop-settings-shortcuts.png"),
  });
  assert.deepEqual(navigationWrites, []);
  page.off("request", recordWrites);
  await page.setViewportSize({ width: 320, height: 240 });
  await page.locator("lynx-view").evaluate((view) => {
    view.shadowRoot.querySelector("#lane-root").style.fontSize = "200%";
  });
  for (const name of [
    "Lane",
    "Pace",
    "RealQA",
    "Account & sync",
    "App settings",
  ]) {
    const button = appButton(name);
    await button.scrollIntoViewIfNeeded();
    const box = await button.boundingBox();
    assert.ok(
      box.y >= 0 && box.y + box.height <= 241,
      `${name} remains reachable at a short height`,
    );
  }
  await page.locator("lynx-view").evaluate((view) => {
    view.shadowRoot.querySelector("#lane-root").style.fontSize = "";
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await appButton("All tasks").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  await appButton("Improve empty states").click();
  const utilityDraft = page.getByRole("textbox", {
    name: "Title",
    exact: true,
  });
  for (const utility of ["Account & sync", "App settings"]) {
    await utilityDraft.fill("Keep this utility draft");
    await appButton(utility).click();
    assert.equal(
      await page
        .getByRole("dialog", { name: "Discard unsaved changes?" })
        .count(),
      0,
    );
    await settingsFocus(
      utility === "Account & sync"
        ? "Online account"
        : "Notifications on this device",
    );
    await appButton("All tasks").click();
    await page.waitForFunction(
      () =>
        document
          .querySelector("lynx-view")
          .shadowRoot.querySelector("#lane-root")
          .getAttribute("data-selected-tab") === "page-Tasks",
    );
    await appButton("Improve empty states").click();
    await page
      .getByRole("tab", {
        name: "Keep this utility draft, Unsaved changes",
        exact: true,
      })
      .waitFor();
    assert.equal(await utilityDraft.inputValue(), "Keep this utility draft");
  }
  await page
    .getByRole("button", { name: "Close Keep this utility draft", exact: true })
    .click();
  await appButton("Discard changes").click();
  await appButton("All tasks").click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
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
    .getByText("Capture on desktop. View saved captures in Lane.", {
      exact: true,
    })
    .waitFor();
  assert.equal(await appButton("RealQA").getAttribute("aria-pressed"), "true");
  await page.screenshot({ path: join(evidence, "desktop-realqa.png") });
  for (const width of [899, 900, 390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await page.waitForTimeout(150);
    const layout = await page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const rail = root.querySelector(".app-rail").getBoundingClientRect();
      const nav = root
        .querySelector(".app-content:not(.lane-tab-content) .navigation")
        .getBoundingClientRect();
      const content = root
        .querySelector(".app-content:not(.lane-tab-content) .workspace")
        .getBoundingClientRect();
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
  await appButton("Open Lane").click();
  await search.waitFor();
  assert.equal(await search.inputValue(), "preserved filter");
  await search.fill("");
  await appButton("Improve empty states").waitFor();
  await page
    .locator('[data-active-tab="true"]')
    .getByText("Loading…", { exact: true })
    .waitFor({ state: "hidden" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.request.get("http://127.0.0.1:46400/test/empty-list?enabled=1");
  await page
    .locator("lynx-view")
    .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await page
    .locator('[data-active-tab="true"]')
    .getByText("No tasks here yet", { exact: true })
    .waitFor();
  await page
    .locator('[data-active-tab="true"]')
    .getByText("Loading…", { exact: true })
    .waitFor({ state: "hidden" });
  assert.equal(
    await page
      .getByRole("button", { name: "Improve empty states", exact: true })
      .count(),
    0,
  );
  assert.equal(
    await page
      .locator('[data-active-tab="true"]')
      .getByText("Loading…", { exact: true })
      .count(),
    0,
  );
  await page.request.get("http://127.0.0.1:46400/test/empty-list?enabled=0");
  await page
    .locator("lynx-view")
    .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await page
    .getByRole("button", { name: "Improve empty states", exact: true })
    .waitFor();
  assert.equal(
    await page
      .locator('[data-active-tab="true"]')
      .getByText("No tasks here yet", { exact: true })
      .count(),
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
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
  assert.equal(await page.getByRole("dialog").count(), 0);
  await appButton("Lane").click();
  await title.waitFor();
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
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  assert.equal(await page.getByRole("dialog").count(), 0);
  await page
    .getByRole("button", { name: "Close Unsaved local title", exact: true })
    .click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor();
  await page.getByRole("button", { name: "Keep editing", exact: true }).click();
  assert.equal(
    await page
      .getByRole("textbox", { name: "Title", exact: true, includeHidden: true })
      .inputValue(),
    "Unsaved local title",
  );
  await page
    .getByRole("button", { name: "Close Unsaved local title", exact: true })
    .click();
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
      const fields = [
        ...root.querySelectorAll('[data-active-tab="true"] .task-list .field'),
      ].map((e) => {
        const r = e.getBoundingClientRect();
        return { top: r.top, bottom: r.bottom };
      });
      const tabs = [
        ...root.querySelectorAll('[data-active-tab="true"] .nav-items .button'),
      ].map((e) => {
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
          "settings shortcuts from every mini-app at 1440, 900, 899, 390 and 320 pixels",
          "utility names, tooltips, dimensions, Enter/Space and section focus",
          "all five rail controls reachable at 320 by 240 with enlarged text",
          "utility navigation has no account/notification writes and respects dirty-draft cancel/confirm",
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
