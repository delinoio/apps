// SPDX-License-Identifier: Apache-2.0
// Checks the shared planning panels against the scripted web fixture without
// capturing screenshots or using an account or native host.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46426;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: String(port),
    LANE_PREVIEW_SCENARIO: "planning",
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
try {
  const page = await browser.newPage({
    viewport: { width: 1800, height: 900 },
  });
  page.setDefaultTimeout(15000);
  await page.goto(`http://127.0.0.1:${port}`);
  const surface = page.locator('[data-active-tab="true"]');
  const button = (name) => surface.getByRole("button", { name, exact: true });
  await button("Repositories").click();
  await button("Labels, milestones and cycles").first().click();
  const manager = surface.locator(".planning-manager");
  await manager.waitFor();
  await button("Edit label Backend").click();
  await surface.getByRole("textbox", { name: "Name" }).waitFor();
  await page.waitForFunction(() => {
    const name = document
      .querySelector("lynx-view")
      .shadowRoot.querySelector(
        '[data-active-tab="true"] x-input[accessibility-label="Name"]',
      );
    return name?.shadowRoot?.activeElement?.tagName === "INPUT";
  });
  assert.equal(
    await button("Edit label Backend").getAttribute("aria-pressed"),
    "true",
  );
  assert.ok(await surface.getByText("#075AFF", { exact: true }).isVisible());
  await button("Edit label Frontend").focus();
  await page.keyboard.press("Enter");
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector(
          '[data-active-tab="true"] .planning-row[accessibility-label="Edit label Frontend"]',
        )
        ?.getAttribute("aria-pressed") === "true",
  );
  await button("Edit label Backend").focus();
  await page.keyboard.press("Space");
  await page.waitForFunction(
    () =>
      document
        .querySelector("lynx-view")
        .shadowRoot.querySelector(
          '[data-active-tab="true"] .planning-row[accessibility-label="Edit label Backend"]',
        )
        ?.getAttribute("aria-pressed") === "true",
  );

  for (const width of [900, 899, 360]) {
    await manager.evaluate((element, value) => {
      element.style.width = `${value}px`;
    }, width);
    await page.waitForFunction(
      (stacked) =>
        document
          .querySelector("lynx-view")
          .shadowRoot.querySelector(
            '[data-active-tab="true"] .planning-manager',
          )
          .classList.contains("planning-stacked") === stacked,
      width < 900,
    );
    const geometry = await manager.evaluate((element) => {
      const list = element
        .querySelector(".planning-list")
        .getBoundingClientRect();
      const edit = element
        .querySelector(".planning-edit")
        .getBoundingClientRect();
      return {
        list: {
          x: list.x,
          y: list.y,
          width: list.width,
          right: list.right,
          bottom: list.bottom,
        },
        edit: { x: edit.x, y: edit.y, width: edit.width },
      };
    });
    if (width >= 900) {
      assert.ok(geometry.edit.x - geometry.list.right >= 23);
      assert.ok(geometry.list.width / width > 0.25);
      assert.ok(geometry.list.width / width < 0.4);
    } else {
      assert.ok(geometry.edit.y >= geometry.list.bottom);
      assert.ok(Math.abs(geometry.list.width - geometry.edit.width) < 2);
    }
  }

  await manager.evaluate((element) => {
    element.style.width = "";
  });
  await page.setViewportSize({ width: 360, height: 800 });
  await page.waitForTimeout(400);
  await page.waitForFunction(() => {
    const view = document.querySelector("lynx-view");
    const manager = view.shadowRoot.querySelector(
      '[data-active-tab="true"] .planning-manager',
    );
    return (
      manager?.classList.contains("planning-stacked") &&
      manager.getBoundingClientRect().width <= 360
    );
  });
  const fit = await page.locator("lynx-view").evaluate((view) => {
    const width = document.documentElement.clientWidth;
    const active = view.shadowRoot.querySelector('[data-active-tab="true"]');
    const panels = [
      ...active.querySelectorAll(".planning-list,.planning-edit"),
    ];
    const buttons = [...active.querySelectorAll(".planning-actions .button")];
    return {
      scroll: document.documentElement.scrollWidth,
      width,
      panelBounds: panels.map((element) => ({
        left: element.getBoundingClientRect().left,
        right: element.getBoundingClientRect().right,
        width: element.getBoundingClientRect().width,
      })),
      buttonHeights: buttons.map(
        (element) => element.getBoundingClientRect().height,
      ),
    };
  });
  assert.ok(
    fit.scroll <= fit.width,
    "360px view has no horizontal page scroll",
  );
  assert.ok(
    fit.panelBounds.every(
      (panel) =>
        panel.left >= 0 && panel.right <= fit.width + 1 && panel.width >= 280,
    ),
    `both panels fit the 360px view: ${JSON.stringify(fit)}`,
  );
  assert.ok(fit.buttonHeights.every((height) => height >= 44));
} finally {
  await browser.close();
  server.kill();
}
