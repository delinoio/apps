// SPDX-License-Identifier: Apache-2.0
// Real hit testing verifies that dialogs isolate the shared tab sidebar and
// narrow-screen Tabs controls. This uses disposable fixtures, not native data.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";
const port = Number(process.env.LANE_PREVIEW_PORT || 46400);
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
  env: { ...process.env, LANE_PREVIEW_PORT: String(port) },
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) => reject(new Error(`Preview exited ${code}`)));
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
  const errors = [],
    writes = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("request", (request) => {
    const method = /\/lane\.v1\.[A-Za-z]+\/([A-Za-z]+)$/.exec(
      request.url(),
    )?.[1];
    if (
      method &&
      request.method() !== "GET" &&
      !/^(List|Get|Watch)/.test(method)
    )
      writes.push(method);
  });
  const button = (name) => page.getByRole("button", { name, exact: true });
  const dialog = (name) => page.getByRole("dialog", { name, exact: true });
  const assertBlocked = async () => {
    const targets = await page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const overlays = [...root.querySelectorAll(".overlay")];
      const topmost = overlays.at(-1);
      return [
        ...root.querySelectorAll(".tab-bar .button,.mobile-tab-toggle .button"),
      ]
        .map((element) => {
          const box = element.getBoundingClientRect();
          const x = box.x + box.width / 2,
            y = box.y + box.height / 2;
          const hit = root.elementFromPoint(x, y);
          return {
            x,
            y,
            visible: x >= 0 && x <= innerWidth && y >= 0 && y <= innerHeight,
            blocked: !!(hit && topmost.contains(hit)),
            backdrop: !!(
              hit &&
              topmost.contains(hit) &&
              !hit.closest(".dialog")
            ),
            name: element.getAttribute("aria-label"),
          };
        })
        .filter((target) => target.visible);
    });
    assert.ok(targets.length > 0);
    for (const target of targets) {
      assert.ok(target.blocked, `${target.name} must be covered`);
      // A sidebar coordinate can overlap a real dialog action. Only click
      // coordinates that hit the backdrop, while checking coverage for all.
      if (target.backdrop) await page.mouse.click(target.x, target.y);
    }
  };
  const utilities = async (name) => {
    if (page.viewportSize().width < 900)
      await page.getByRole("button", { name: /^Tabs \(/ }).click();
    await button(name).click();
  };
  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 900, height: 900 },
    { width: 899, height: 900 },
    { width: 390, height: 844 },
    { width: 360, height: 780 },
    { width: 390, height: 240 },
  ]) {
    await page.setViewportSize(viewport);
    await page.goto(`http://127.0.0.1:${port}`);
    await button("Improve empty states").waitFor();
    await button(viewport.width < 900 ? "Repos" : "Repositories").click();
    await button("Add repository").click();
    const url = page.getByRole("textbox", { name: /^Repository URL/ });
    assert.equal(
      await url.evaluate((el) => el === el.getRootNode().activeElement),
      true,
      "the repository URL receives initial focus",
    );
    assert.ok(
      (
        await dialog("Add repository")
          .getByRole("button", { name: "Close" })
          .boundingBox()
      ).height >= 44,
    );
    await url.fill("https://github.com/example/repo");
    await assertBlocked();
    assert.equal(await url.inputValue(), "https://github.com/example/repo");
    for (let i = 0; i < 8; i++) {
      await page.keyboard.press("Tab");
      assert.ok(
        await dialog("Add repository").evaluate((el) =>
          el.contains(el.getRootNode().activeElement),
        ),
      );
    }
    await page.keyboard.press("Escape");
    await dialog("Add repository").waitFor({ state: "detached" });
    assert.equal(
      await button("Add repository").evaluate(
        (el) => el === el.getRootNode().activeElement,
      ),
      true,
    );
    await button("Delete repository").click();
    assert.equal(
      await dialog("Permanently delete repository?")
        .getByRole("button", { name: "Close" })
        .evaluate((el) => el === el.getRootNode().activeElement),
      true,
      "deletion starts on the safe Close action",
    );
    await dialog("Permanently delete repository?")
      .getByRole("button", { name: "Close" })
      .click();
    await page.locator("lynx-view").evaluate((view) => {
      const root = view.shadowRoot;
      const url = root.querySelector(".repository-url");
      url.textContent = `example.com/${"very-long-repository-segment".repeat(12)}`;
    });
    const overflow = await page.locator("lynx-view").evaluate((view) => {
      const width = document.documentElement.clientWidth;
      return [
        ...view.shadowRoot.querySelectorAll(
          ".repository-directory, .repository-row, .repository-url, .repository-actions, .repository-actions .button",
        ),
      ]
        .filter((element) => element.getBoundingClientRect().right > width + 1)
        .map((element) => element.className);
    });
    assert.deepEqual(
      overflow,
      [],
      `repository content overflows at ${viewport.width}px`,
    );
    if (viewport.width === 360) {
      await page.evaluate(() => {
        document.documentElement.style.fontSize = "24px";
      });
      const enlargedOverflow = await page
        .locator("lynx-view")
        .evaluate((view) => {
          const width = document.documentElement.clientWidth;
          return [
            ...view.shadowRoot.querySelectorAll(
              ".repository-directory, .repository-row, .repository-url, .repository-actions, .repository-actions .button",
            ),
          ]
            .filter(
              (element) => element.getBoundingClientRect().right > width + 1,
            )
            .map((element) => element.className);
        });
      assert.deepEqual(
        enlargedOverflow,
        [],
        "repository content overflows with enlarged text",
      );
      await page.evaluate(() => {
        document.documentElement.style.fontSize = "";
      });
    }
    await button(viewport.width < 900 ? "Tasks" : "All tasks").click();
    await button("Improve empty states").click();
    const title = page.getByRole("textbox", { name: "Title", exact: true });
    await title.fill("Unsaved tab draft");
    if (viewport.width < 900)
      await page.getByRole("button", { name: /^Tabs \(/ }).click();
    await button("Close Unsaved tab draft").click();
    await dialog("Discard unsaved changes?").waitFor();
    await assertBlocked();
    assert.equal(await title.inputValue(), "Unsaved tab draft");
    await button("Keep editing").click();
    await dialog("Discard unsaved changes?").waitFor({ state: "detached" });
    assert.equal(await title.inputValue(), "Unsaved tab draft");
    if (viewport.width < 900 && !(await dialog("Open tabs").count()))
      await page.getByRole("button", { name: /^Tabs \(/ }).click();
    await button("Close Unsaved tab draft").click();
    await button("Discard changes").click();
    await utilities("Account & sync");
    await page.getByText("Online account", { exact: true }).waitFor();
  }
  assert.deepEqual(writes, []);
  assert.deepEqual(errors, []);
  console.log("Modal vertical-tab isolation checks passed (mock-only).");
} finally {
  await browser?.close();
  server.kill();
}
