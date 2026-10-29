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
const previewBase = `http://127.0.0.1:${process.env.LANE_PREVIEW_PORT || 46400}`;
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
const captureScreenshot = async (path) => {
  if (process.env.LANE_SKIP_VALIDATION_SCREENSHOTS !== "1")
    await page.screenshot({ path });
};
page.on("pageerror", (e) => errors.push(e.message));
try {
  await page.goto(previewBase);
  await page
    .getByRole("button", { name: "Improve empty states", exact: true })
    .waitFor();
  const appButton = (name) => page.getByRole("button", { name, exact: true });
  const search = page.getByRole("textbox", {
    name: "Search title or description",
    exact: true,
  });
  const chooseApp = async (name) => {
    await appButton("New tab").click();
    await appButton(name).click();
  };
  const utility = async (name) => {
    if (page.viewportSize().width < 900)
      await page.getByRole("button", { name: /^Tabs \(/ }).click();
    await appButton(name).click();
  };
  assert.equal(await page.locator(".app-rail").count(), 0);
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
    await page.waitForTimeout(150);
    for (const miniApp of ["Lane", "Pace", "RealQA"]) {
      for (const [name, heading] of [
        ["Account & sync", "Online account"],
        ["App settings", "Notifications on this device"],
      ]) {
        await chooseApp(miniApp);
        await utility(name);
        await settingsFocus(heading);
      }
    }
  }
  assert.deepEqual(navigationWrites, []);
  page.off("request", recordWrites);
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(150);
  await page.getByRole("tab", { name: "All tasks", exact: true }).click();
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
    await page.getByRole("tab", { name: "All tasks", exact: true }).click();
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
  await page.getByRole("tab", { name: "All tasks", exact: true }).click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  await search.fill("preserved filter");
  await chooseApp("Pace");
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
  assert.equal(
    await page
      .getByRole("tab", { name: "Pace", exact: true })
      .getAttribute("aria-selected"),
    "true",
  );
  assert.equal(await appButton("All tasks").count(), 0);
  assert.equal(await appButton("Overview").count(), 1);
  const pace = page.locator(".pace-coming-soon");
  assert.deepEqual(await pace.locator("x-text").allTextContents(), [
    "Pace",
    "Coming soon",
    "Pace is not available yet.",
    "Back to Lane",
  ]);
  assert.equal(
    await pace.getByRole("heading", { name: "Coming soon" }).count(),
    1,
  );
  assert.equal(await pace.getByRole("button").count(), 1);
  assert.equal(
    await pace
      .locator(".app-icon")
      .getAttribute("accessibility-elements-hidden"),
    "true",
  );
  assert.equal(
    await pace.locator(".pace-artwork").getAttribute("src"),
    await page.locator(".tab-bar .pace-artwork").getAttribute("src"),
  );
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      .shadowRoot.activeElement?.classList.contains("coming-soon-title"),
  );
  const paceLayout = async () =>
    page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const select = (selector) => root.querySelector(selector);
      const bounds = (selector) => {
        const { left, top, right, bottom, width, height } =
          select(selector).getBoundingClientRect();
        return { left, top, right, bottom, width, height };
      };
      const scroller = select(".pace-page-scroll");
      return {
        scroller: bounds(".pace-page-scroll"),
        group: bounds(".pace-coming-soon"),
        icon: bounds(".pace-coming-soon .pace-artwork"),
        button: bounds(".pace-return .button"),
        scrollWidth: scroller.scrollWidth,
        clientWidth: scroller.clientWidth,
        headingSize: getComputedStyle(
          select(".pace-coming-soon .coming-soon-title"),
        ).fontSize,
        bodySize: getComputedStyle(select(".pace-coming-soon .muted")).fontSize,
        buttonSize: getComputedStyle(select(".pace-return .button x-text"))
          .fontSize,
        buttonColor: getComputedStyle(select(".pace-return .button x-text"))
          .color,
      };
    });
  for (const width of [1440, 900, 899, 390, 360]) {
    await page.setViewportSize({ width, height: 900 });
    await page.waitForTimeout(150);
    const layout = await paceLayout();
    assert.ok(layout.group.width <= 360);
    assert.ok(Math.abs(layout.icon.width - 72) <= 1);
    assert.ok(Math.abs(layout.icon.height - 72) <= 1);
    assert.ok(layout.button.width <= 200);
    assert.ok(layout.button.height >= 48);
    assert.ok(layout.scrollWidth <= layout.clientWidth + 1);
    assert.ok(layout.group.left >= layout.scroller.left - 1);
    assert.ok(layout.group.right <= layout.scroller.right + 1);
    assert.ok(
      Math.abs(
        (layout.group.left + layout.group.right) / 2 -
          (layout.scroller.left + layout.scroller.right) / 2,
      ) <= 1,
    );
    if (width >= 900)
      assert.ok(
        Math.abs(
          (layout.icon.top + layout.button.bottom) / 2 -
            (layout.scroller.top + layout.scroller.bottom) / 2,
        ) <= 2,
        `Pace content is vertically centered at ${width}px: ${JSON.stringify(layout)}`,
      );
    assert.equal(layout.headingSize, width < 900 ? "28px" : "32px");
    assert.equal(layout.buttonColor, "rgb(7, 90, 255)");
  }
  await page.setViewportSize({ width: 360, height: 400 });
  await page.locator("lynx-view").evaluate((view) => {
    const style = document.createElement("style");
    style.id = "pace-enlarged-text-test";
    style.textContent =
      ".pace-coming-soon .label{font-size:28px!important}" +
      ".pace-coming-soon .coming-soon-title{font-size:56px!important}" +
      ".pace-coming-soon .muted,.pace-return .button x-text{font-size:32px!important}";
    view.shadowRoot.append(style);
  });
  await page.waitForTimeout(150);
  let constrained = await paceLayout();
  assert.equal(constrained.headingSize, "56px");
  assert.equal(constrained.bodySize, "32px");
  assert.equal(constrained.buttonSize, "32px");
  assert.ok(constrained.icon.top >= constrained.scroller.top - 1);
  assert.ok(constrained.scrollWidth <= constrained.clientWidth + 1);
  await appButton("Back to Lane").scrollIntoViewIfNeeded();
  constrained = await paceLayout();
  assert.ok(constrained.button.bottom <= constrained.scroller.bottom + 1);
  await page.locator("lynx-view").evaluate((view) => {
    view.shadowRoot.querySelector("#pace-enlarged-text-test").remove();
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.keyboard.press("Escape");
  assert.equal(
    await page.locator("#lane-root").getAttribute("data-app"),
    "Pace",
  );
  await captureScreenshot(join(evidence, "desktop-pace.png"));
  await appButton("New tab").click();
  await page.getByRole("heading", { name: "New tab" }).waitFor();
  await page.waitForTimeout(150);
  await appButton("RealQA").focus();
  assert.equal(
    await appButton("RealQA").evaluate(
      (button) => button.getRootNode().activeElement === button,
    ),
    true,
  );
  await page.keyboard.press(" ");
  await page
    .getByText("Capture on desktop. View saved captures in Lane.", {
      exact: true,
    })
    .waitFor();
  assert.equal(
    await page
      .getByRole("tab", { name: "RealQA", exact: true })
      .getAttribute("aria-selected"),
    "true",
  );
  await captureScreenshot(join(evidence, "desktop-realqa.png"));
  for (const width of [899, 900, 390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await page.waitForTimeout(150);
    const layout = await page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
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
        navWidth: nav.width,
        navBottom: nav.bottom,
        contentTop: content.top,
        buttonRight: button.right,
      };
    });
    if (width < 900) assert.ok(layout.navBottom <= layout.contentTop + 1);
    else assert.equal(layout.navWidth, 240);
    assert.ok(layout.buttonRight <= width);
  }
  await captureScreenshot(join(evidence, "mobile-realqa.png"));
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
  await page.request.get(`${previewBase}/test/empty-list?enabled=1`);
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
  await page.request.get(`${previewBase}/test/empty-list?enabled=0`);
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
  const retainedTab = await page
    .locator("#lane-root")
    .getAttribute("data-selected-tab");
  const tabCount = await page.getByRole("tab").count();
  const paceMutations = [];
  const recordPaceMutation = (request) => {
    if (
      request.method() !== "GET" &&
      /SaveTask|SetProposal|CreateTask|DeleteTask/.test(request.url())
    )
      paceMutations.push(request.url());
  };
  page.on("request", recordPaceMutation);
  await chooseApp("Pace");
  await page.getByText("Pace is not available yet.", { exact: true }).waitFor();
  assert.equal(await page.getByRole("dialog").count(), 0);
  await appButton("Back to Lane").click();
  await title.waitFor();
  assert.equal(await title.inputValue(), "Unsaved local title");
  assert.equal(
    await page.locator("#lane-root").getAttribute("data-selected-tab"),
    retainedTab,
  );
  assert.equal(await page.getByRole("tab").count(), tabCount);
  for (const key of ["Enter", " "]) {
    await chooseApp("Pace");
    await page
      .getByText("Pace is not available yet.", { exact: true })
      .waitFor();
    await appButton("Back to Lane").focus();
    assert.equal(
      await appButton("Back to Lane").evaluate(
        (button) => getComputedStyle(button).outlineWidth,
      ),
      "3px",
    );
    await page.keyboard.press(key);
    await title.waitFor();
    assert.equal(await title.inputValue(), "Unsaved local title");
    assert.equal(
      await page.locator("#lane-root").getAttribute("data-selected-tab"),
      retainedTab,
    );
    assert.equal(await page.getByRole("tab").count(), tabCount);
  }
  page.off("request", recordPaceMutation);
  assert.deepEqual(paceMutations, []);
  assert.equal(
    await page.locator("#lane-root").getAttribute("data-app"),
    "Lane",
  );
  await page.request.get(`${previewBase}/test/external`);
  await page
    .locator("lynx-view")
    .evaluate((view) => view.sendGlobalEvent("lane.onlineChanged", [{}]));
  await page
    .getByText("External changes are available.", { exact: false })
    .waitFor();
  assert.equal(await title.inputValue(), "Unsaved local title");
  await page.request.get(`${previewBase}/test/fail-save`);
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
  await captureScreenshot(join(evidence, "desktop-editor.png"));
  await page.setViewportSize({ width: 390, height: 844 });
  await page.waitForTimeout(500);
  assert.equal(
    await page.locator("#lane-root").getAttribute("class"),
    "app mobile",
  );
  assert.ok(await title.isVisible());
  await captureScreenshot(join(evidence, "mobile-editor.png"));
  await page.getByRole("button", { name: "Back to list", exact: true }).click();
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector("#lane-root")
        .getAttribute("data-selected-tab") === "page-Tasks",
  );
  assert.equal(await page.getByRole("dialog").count(), 0);
  await page.getByRole("button", { name: /^Tabs \(/ }).click();
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
  await page.getByRole("dialog", { name: "Open tabs", exact: true }).waitFor();
  await page
    .getByRole("button", { name: "Close Unsaved local title", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Discard changes", exact: true })
    .click();
  await page.getByRole("button", { name: "+ New task", exact: true }).waitFor();
  await captureScreenshot(join(evidence, "mobile-list.png"));
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
  await captureScreenshot(join(evidence, "enlarged-text.png"));
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
          "utility names and section focus",
          "utility navigation has no account/notification writes and respects dirty-draft cancel/confirm",
          "shared Lane, Pace and RealQA tabs and app-specific sidebars",
          "mini-app keyboard activation and selected tab state",
          "Pace layout, constrained scrolling, exact content and return preserve Lane state",
          "mini-app switching respects unsaved-edit confirmation and Escape",
          "vertical tabs and mini-app layout at 320, 390, 899 and 900 pixels",
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
          "text enlargement layout",
        ],
        errors,
      },
      null,
      2,
    ),
  );
  console.log(`Browser checks passed. Mock-only evidence: ${evidence}`);
} catch (error) {
  await captureScreenshot(join(evidence, "failure.png"));
  console.log(
    await page.locator("lynx-view").evaluate((view) => ({
      app: view.shadowRoot.querySelector("#lane-root").outerHTML.slice(0, 500),
      pace: view.shadowRoot
        .querySelector(".pace-coming-soon")
        ?.outerHTML.slice(0, 500),
      paceScroll: view.shadowRoot.querySelector(".pace-page-scroll")?.scrollTop,
    })),
  );
  throw error;
} finally {
  await browser.close();
  server.kill();
}
