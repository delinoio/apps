// SPDX-License-Identifier: Apache-2.0
// Regression for modal isolation: while any Lane dialog is open, the app rail
// (mini-app selectors and utility controls) must sit below the dialog backdrop.
// The checks use real hit targets (elementFromPoint) and real pointer clicks,
// then verify observable navigation, field values and persistence requests.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = Number(process.env.LANE_PREVIEW_PORT || 46400);
const origin = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
  env: { ...process.env, LANE_PREVIEW_PORT: String(port) },
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
  // Anything other than a read is a persistence request.
  const writes = [];
  page.on("request", (request) => {
    const match = /\/lane\.v1\.[A-Za-z]+\/([A-Za-z]+)$/.exec(request.url());
    if (
      match &&
      request.method() !== "GET" &&
      !/^(List|Get|Watch)/.test(match[1])
    )
      writes.push(match[1]);
  });
  await page.goto(origin);
  const button = (name) => page.getByRole("button", { name, exact: true });
  await button("Improve empty states").waitFor();

  const railTargets = [
    "Lane",
    "Pace",
    "RealQA",
    "Account & sync",
    "App settings",
  ];
  const urlField = page.getByRole("textbox", {
    name: "Repository URL",
    exact: true,
  });
  const nameField = page.getByRole("textbox", {
    name: "Display name (optional)",
    exact: true,
  });
  // Mobile navigation abbreviates the Repositories tab.
  const repositoriesTab = () =>
    button(page.viewportSize().width < 900 ? "Repos" : "Repositories");
  const tasksTab = () =>
    button(page.viewportSize().width < 900 ? "Tasks" : "All tasks");
  // A rail tooltip stays visible while its button is hovered or focused and, in
  // very short narrow viewports, overlaps the tab row. That is unrelated to
  // modal isolation, so release hover and focus before using the content tabs.
  const releaseRailTooltip = async () => {
    await page.mouse.move(page.viewportSize().width - 2, 2);
    await page
      .locator("lynx-view")
      .evaluate((view) => view.shadowRoot.activeElement?.blur());
  };
  const dialogHeading = (title) =>
    page.getByRole("dialog", { name: title, exact: true });
  // Reads the shadow root so the assertion uses the browser's real hit testing.
  const hitTargets = () =>
    page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const scroller = root.querySelector(".app-rail-scroll");
      const overlays = [...root.querySelectorAll(".overlay")];
      const topmost = overlays[overlays.length - 1];
      return [...root.querySelectorAll(".app-switch,.app-utility")].map(
        (element) => {
          const box = element.getBoundingClientRect();
          const x = box.left + box.width / 2;
          const y = box.top + box.height / 2;
          const visible =
            y >= 0 &&
            y <= window.innerHeight &&
            x >= 0 &&
            x <= window.innerWidth;
          const hit = root.elementFromPoint(x, y);
          return {
            name: element.getAttribute("aria-label"),
            x,
            y,
            visible,
            blocked: Boolean(topmost && hit && topmost.contains(hit)),
            scrollTop: scroller?.scrollTop ?? 0,
          };
        },
      );
    });
  // Scrolls the rail's own scroll container, which the dialog cannot hide.
  const scrollRail = (position) =>
    page.locator("lynx-view").evaluate((view, where) => {
      const root = view.shadowRoot;
      const candidates = [
        root.querySelector(".app-rail-scroll"),
        ...root.querySelectorAll(".app-rail-scroll *"),
      ].filter(Boolean);
      for (const element of candidates)
        if (element.scrollHeight > element.clientHeight + 1)
          element.scrollTop = where === "end" ? element.scrollHeight : 0;
    }, position);
  // Every rail control must be covered by the backdrop, and real pointer
  // clicks on it must neither navigate nor persist anything.
  const assertRailBlocked = async (description) => {
    for (const position of ["start", "end"]) {
      await scrollRail(position);
      await page.waitForTimeout(100);
      const targets = await hitTargets();
      assert.deepEqual(
        targets.map((target) => target.name),
        railTargets,
        description,
      );
      const visible = targets.filter((target) => target.visible);
      assert.ok(visible.length > 0, `${description}: rail controls visible`);
      for (const target of visible) {
        assert.equal(
          target.blocked,
          true,
          `${description}: ${target.name} must be covered by the backdrop (rail scrolled to ${position})`,
        );
        await page.mouse.click(target.x, target.y);
      }
    }
    await scrollRail("start");
  };
  const assertAllRailControlsCovered = async (description) => {
    // Controls scrolled outside the viewport are checked after scrolling the
    // rail, so across both positions every control has been hit-tested.
    const seen = new Set();
    for (const position of ["start", "end"]) {
      await scrollRail(position);
      await page.waitForTimeout(100);
      for (const target of await hitTargets())
        if (target.visible) {
          assert.equal(target.blocked, true, `${description}: ${target.name}`);
          seen.add(target.name);
        }
    }
    await scrollRail("start");
    assert.deepEqual([...seen].sort(), [...railTargets].sort(), description);
  };
  const assertLaneSelected = async () => {
    assert.equal(await button("Lane").getAttribute("aria-pressed"), "true");
    // The web runtime drops aria-pressed="false", so unselected means absent.
    assert.notEqual(await button("Pace").getAttribute("aria-pressed"), "true");
    assert.notEqual(
      await button("RealQA").getAttribute("aria-pressed"),
      "true",
    );
  };
  const assertRailWorksAfterClose = async () => {
    await button("Pace").click();
    await page
      .getByText("Pace is not available yet.", { exact: true })
      .waitFor();
    assert.equal(await button("Pace").getAttribute("aria-pressed"), "true");
    await button("Lane").click();
    await repositoriesTab().waitFor();
    await button("Account & sync").click();
    await page.getByText("Online account", { exact: true }).waitFor();
    await button("App settings").click();
    await page
      .getByText("Notifications on this device", { exact: true })
      .waitFor();
    await button("Lane").click();
    await repositoriesTab().waitFor();
  };
  const openAddRepository = async () => {
    await repositoriesTab().click();
    await button("Add repository").click();
    await dialogHeading("Add repository").waitFor();
    await urlField.fill("https://github.com/example/repo");
    await nameField.fill("Unsaved repository");
  };
  const assertAddRepositoryRetained = async () => {
    await dialogHeading("Add repository").waitFor();
    assert.equal(
      await urlField.inputValue(),
      "https://github.com/example/repo",
    );
    assert.equal(await nameField.inputValue(), "Unsaved repository");
    await assertLaneSelected();
    assert.equal(
      (await page.getByText("Repositories", { exact: true }).count()) > 0,
      true,
    );
    assert.equal(
      await page
        .getByText("Pace is not available yet.", { exact: true })
        .count(),
      0,
    );
    assert.equal(
      await page.getByText("Online account", { exact: true }).count(),
      0,
    );
    assert.equal(
      await page
        .getByText("Notifications on this device", { exact: true })
        .count(),
      0,
    );
    assert.deepEqual(writes, []);
  };

  // Scenarios 1 and 2: content-nested Add repository dialog on desktop, a
  // narrow viewport and a rail short enough to need its own scrolling.
  const viewports = [
    { width: 1440, height: 900, label: "desktop" },
    { width: 390, height: 844, label: "narrow" },
    { width: 390, height: 240, label: "narrow with scrolling rail" },
  ];
  for (const viewport of viewports) {
    await page.setViewportSize({
      width: viewport.width,
      height: viewport.height,
    });
    await page.waitForTimeout(200);
    await openAddRepository();
    await assertAllRailControlsCovered(`Add repository, ${viewport.label}`);
    await assertRailBlocked(`Add repository, ${viewport.label}`);
    await assertAddRepositoryRetained();
    // Close restores a working rail; reopening starts from a clean dialog.
    await dialogHeading("Add repository")
      .getByRole("button", { name: "Close", exact: true })
      .click();
    await dialogHeading("Add repository").waitFor({ state: "detached" });
    assert.deepEqual(writes, []);
    await assertRailWorksAfterClose();
  }
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.waitForTimeout(200);

  // Scenario 4: Close, Escape, Tab and Shift+Tab with focus restoration.
  await openAddRepository();
  const insideDialog = () =>
    dialogHeading("Add repository").evaluate((dialog) =>
      dialog.contains(dialog.getRootNode().activeElement),
    );
  for (let index = 0; index < 12; index++) {
    await page.keyboard.press("Tab");
    assert.equal(await insideDialog(), true, "Tab stays inside the dialog");
  }
  for (let index = 0; index < 12; index++) {
    await page.keyboard.press("Shift+Tab");
    assert.equal(
      await insideDialog(),
      true,
      "Shift+Tab stays inside the dialog",
    );
  }
  await assertRailBlocked("Add repository after Tab traversal");
  await assertAddRepositoryRetained();
  await page.keyboard.press("Escape");
  await dialogHeading("Add repository").waitFor({ state: "detached" });
  assert.equal(
    await page.locator("lynx-view").evaluate((view) => {
      const active = view.shadowRoot.activeElement;
      return active?.textContent ?? "";
    }),
    "Add repository",
    "Escape restores focus to the control that opened the dialog",
  );
  await assertRailWorksAfterClose();

  // Scenario 3: the root discard dialog is authoritative over the rail.
  for (const viewport of viewports) {
    await page.setViewportSize({
      width: viewport.width,
      height: viewport.height,
    });
    await page.waitForTimeout(200);
    await releaseRailTooltip();
    await tasksTab().click();
    await button("Improve empty states").click();
    const title = page.getByRole("textbox", { name: "Title", exact: true });
    await title.waitFor();
    await title.fill("Unsaved rail draft");
    await page.waitForTimeout(150);
    // Request app switching through the rail so the discard guard opens.
    if (viewport.height < 400) await scrollRail("start");
    await button("Pace").click();
    const discard = dialogHeading("Discard unsaved changes?");
    await discard.waitFor();
    await assertAllRailControlsCovered(`Discard dialog, ${viewport.label}`);
    await assertRailBlocked(`Discard dialog, ${viewport.label}`);
    await discard.waitFor();
    assert.equal(await title.inputValue(), "Unsaved rail draft");
    assert.equal(
      await page
        .getByText("Pace is not available yet.", { exact: true })
        .count(),
      0,
      "Clicking the background rail must not confirm the discard",
    );
    await assertLaneSelected();
    await button("Keep editing").click();
    await discard.waitFor({ state: "detached" });
    assert.equal(await title.inputValue(), "Unsaved rail draft");
    await assertLaneSelected();
    // Explicit confirmation still performs the existing navigation.
    await button("Account & sync").click();
    await discard.waitFor();
    await button("Discard changes").click();
    await page.getByText("Online account", { exact: true }).waitFor();
    await button("Lane").click();
    await releaseRailTooltip();
    await tasksTab().click();
    await button("Improve empty states").waitFor();
  }
  assert.deepEqual(writes, []);
  assert.deepEqual(errors, []);
  console.log("Modal rail isolation browser checks passed (mock-only).");
} finally {
  await browser?.close();
  server.kill();
}
