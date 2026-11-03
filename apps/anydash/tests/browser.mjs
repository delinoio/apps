// SPDX-License-Identifier: Apache-2.0
// Browser checks of the built web host against the in-memory API: every
// design-record state at wide, tablet and phone widths, dialogs, focus and
// WCAG 2.2 AA rules with axe-core. ANYDASH_BROWSERS selects the matrix
// (chromium, firefox, webkit); CI installs and runs Chromium by default.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import playwright from "playwright";

const require = createRequire(import.meta.url);
const axeSource = await readFile(
  require.resolve("axe-core/axe.min.js"),
  "utf8",
);
const evidence =
  process.env.ANYDASH_BROWSER_EVIDENCE ||
  join(tmpdir(), "anydash-browser-evidence");
const port = Number(process.env.ANYDASH_PREVIEW_PORT || 46410);
const base = `http://127.0.0.1:${port}`;
const browsers = (process.env.ANYDASH_BROWSERS || "chromium")
  .split(",")
  .map((name) => name.trim())
  .filter(Boolean);
await mkdir(evidence, { recursive: true });

const ids = {
  personal: "0192f000-0000-7000-8000-0000000000a1",
  team: "0192f000-0000-7000-8000-0000000000b1",
  overview: "0192f000-0000-7000-8000-0000000000c1",
  teamBoard: "0192f000-0000-7000-8000-0000000000c2",
  enrollment: "0192f3aa-0000-7000-8000-000000000e01",
};
const dashboard = `/spaces/${ids.personal}/dashboards/${ids.overview}`;
const teamBoard = `/spaces/${ids.team}/dashboards/${ids.teamBoard}`;
const widths = { wide: 1440, tablet: 900, phone: 390 };

const server = spawn(process.execPath, ["tests/preview.mjs"], {
  stdio: ["ignore", "pipe", "inherit"],
  env: { ...process.env, ANYDASH_PREVIEW_PORT: String(port) },
});
await new Promise((resolve, reject) => {
  server.stdout.once("data", resolve);
  server.once("exit", (code) => reject(new Error(`Preview exited ${code}`)));
});

process.on("exit", () => server.kill());
const failures = [];
async function axe(page, label) {
  await page.addScriptTag({ content: axeSource });
  const result = await page.evaluate(async () =>
    // eslint-disable-next-line no-undef
    window.axe.run(document, {
      runOnly: {
        type: "tag",
        values: ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"],
      },
    }),
  );
  const serious = result.violations.filter((entry) =>
    ["serious", "critical"].includes(entry.impact),
  );
  if (serious.length)
    failures.push(
      `${label}: ${serious
        .map(
          (entry) =>
            `${entry.id} (${entry.nodes.length}): ${entry.nodes
              .slice(0, 2)
              .map((node) => node.target.join(" "))
              .join(" | ")}`,
        )
        .join("; ")}`,
    );
  return serious.length;
}
const activeText = (page) =>
  page.evaluate(() => {
    let active = document.activeElement;
    while (active?.shadowRoot?.activeElement)
      active = active.shadowRoot.activeElement;
    return {
      tag: active?.tagName ?? "",
      text: active?.textContent ?? "",
      label: active?.getAttribute("aria-label") ?? "",
    };
  });

for (const name of browsers) {
  const browser = await playwright[name].launch({
    headless: true,
    // Chromium uses the installed Chrome channel by default, as Lane does;
    // set ANYDASH_BROWSER_CHANNEL=chromium for Playwright's bundled build.
    ...(name === "chromium"
      ? { channel: process.env.ANYDASH_BROWSER_CHANNEL || "chrome" }
      : {}),
  });
  const errors = [];
  const open = async (scenario, path, width = widths.wide, height = 900) => {
    const context = await browser.newContext({ viewport: { width, height } });
    const page = await context.newPage();
    page.on("pageerror", (error) =>
      errors.push(`${scenario}: ${error.message}`),
    );
    await page.goto(
      `${base}${path}${path.includes("?") ? "&" : "?"}scenario=${scenario}`,
    );
    return { page, context };
  };
  const shot = (page, file) =>
    page.screenshot({
      path: join(evidence, `${name}-${file}.png`),
      fullPage: true,
    });
  const see = (page, text) =>
    page
      .locator("lynx-view")
      .getByText(text, { exact: false })
      .first()
      .waitFor({ timeout: 15_000 });
  try {
    for (const [layout, width] of Object.entries(widths)) {
      const { page, context } = await open("states", dashboard, width);
      for (const text of [
        "62% left",
        "Not provided",
        "Couldn’t refresh",
        "Authentication expired",
        "Partial aggregation · 12 records skipped",
        "Can’t be calculated",
        "Sharing ended",
        "Usage from 3 devices is combined.",
        "Loading…",
      ])
        await see(page, text);
      await shot(page, `A-states-${layout}`);
      await axe(page, `${name} states ${layout}`);
      await context.close();
    }
    const cases = [
      [
        "onboarding",
        "/",
        ["Connect a device to start", "Team dashboards work without a device."],
        "B-onboarding",
      ],
      [
        "empty",
        "/",
        ["No dashboards yet", "Up to 50 dashboards per space."],
        "B-empty",
      ],
      ["loading", dashboard, ["Loading dashboard…"], "B-loading"],
      [
        "denied",
        dashboard,
        ["You don’t have access to this dashboard"],
        "B-denied",
      ],
      [
        "error",
        dashboard,
        ["Something went wrong", "Correlation ID"],
        "C-error",
      ],
      [
        "update",
        dashboard,
        ["This version of AnyDash is no longer supported."],
        "C-update-required",
      ],
      [
        "approval",
        `/connect/${ids.enrollment}`,
        ["Approve this device?", "attempts left"],
        "C-approval",
      ],
      [
        "invalid",
        `/connect/${ids.enrollment}`,
        ["This code no longer works"],
        "C-invalid-code",
      ],
      ["team", teamBoard, ["62% left"], "team-dashboard"],
      [
        "states",
        "/settings",
        ["Devices", "Connections", "About and support"],
        "settings",
      ],
      ["signedOut", "/", ["Sign in to AnyDash"], "sign-in"],
    ];
    for (const [scenario, path, texts, file] of cases) {
      const { page, context } = await open(scenario, path);
      for (const text of texts) await see(page, text);
      if (scenario === "approval") {
        const focus = await activeText(page);
        assert.equal(focus.tag, "INPUT", "the code input is focused on load");
      }
      await shot(page, file);
      await axe(page, `${name} ${file}`);
      await context.close();
    }

    // Phone: the design's single column with the compact header.
    {
      const { page, context } = await open(
        "states",
        dashboard,
        widths.phone,
        800,
      );
      await see(page, "62% left");
      assert.equal(
        await page.getByText("Edit", { exact: true }).count(),
        0,
        "phone header has no Edit",
      );
      await context.close();
    }

    // Recovery after a version conflict.
    {
      const { page, context } = await open("conflict", dashboard);
      await see(page, "62% left");
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await see(page, "Select a widget");
      await page.locator(".ad-widget").first().click();
      await page
        .getByRole("button", { name: "Move right", exact: true })
        .click();
      await page.getByRole("button", { name: "Done", exact: true }).click();
      await see(page, "This dashboard changed");
      await shot(page, "C-recovery");
      await axe(page, `${name} recovery`);
      await page
        .getByRole("button", { name: "Continue editing", exact: true })
        .click();
      await see(page, "Select a widget");
      // Keyboard alternative to drag: arrows move, Shift+arrows resize.
      await page.locator(".ad-widget").first().focus();
      await page.keyboard.press("ArrowRight");
      await see(page, "column 2, row 1");
      await page.keyboard.press("Shift+ArrowDown");
      await see(page, "3 rows tall");
      await context.close();
    }

    // Confirmation dialogs: Esc closes and focus starts on Cancel.
    {
      const { page, context } = await open("team", teamBoard);
      await see(page, "62% left");
      await page.getByRole("button", { name: "Edit", exact: true }).click();
      await page
        .getByRole("button", { name: "Add widget", exact: true })
        .first()
        .click();
      await page
        .getByRole("button", { name: "Codex · Codex", exact: true })
        .click();
      await page
        .locator(".lui-dialog")
        .getByRole("button", { name: "Add widget", exact: true })
        .click();
      await see(page, "Share this widget with Team Acme?");
      const focus = await activeText(page);
      assert.match(focus.text, /Cancel/, "initial focus is on Cancel");
      await shot(page, "D-share");
      await axe(page, `${name} share dialog`);
      await page.keyboard.press("Escape");
      await page
        .getByText("Share this widget with Team Acme?")
        .waitFor({ state: "detached" });
      await context.close();
    }
    {
      const { page, context } = await open("states", dashboard);
      await see(page, "62% left");
      await page.getByRole("button", { name: /Choose a dashboard/ }).click();
      await page
        .getByRole("button", { name: "Delete dashboard", exact: true })
        .click();
      await see(page, "Delete “Overview”?");
      await see(
        page,
        "The dashboard and its 9 widgets are removed for everyone in this space.",
      );
      await shot(page, "D-delete-dashboard");
      await axe(page, `${name} delete dashboard`);
      await page.keyboard.press("Escape");
      await context.close();
    }
    {
      const { page, context } = await open("states", "/settings");
      await see(page, "Delete AnyDash data");
      await page
        .getByRole("button", { name: "Delete AnyDash data", exact: true })
        .click();
      await see(page, "Delete all AnyDash data?");
      await shot(page, "D-delete-data");
      await axe(page, `${name} delete data`);
      await page.keyboard.press("Escape");
      await page
        .getByText("Delete all AnyDash data?")
        .waitFor({ state: "detached" });
      await context.close();
    }
    {
      const { page, context } = await open("desktop", "/settings");
      await see(page, "This computer");
      await page
        .getByRole("button", { name: "Add connection", exact: true })
        .click();
      await page.getByRole("button", { name: "Vercel", exact: true }).click();
      await see(page, "Vercel tokens can also make changes");
      await shot(page, "D-connect-vercel");
      await axe(page, `${name} connect vercel`);
      await context.close();
    }
  } catch (error) {
    failures.push(`${name}: ${error.message}`);
  } finally {
    await browser.close();
  }
  failures.push(...errors);
}
server.kill();
console.log(
  JSON.stringify(
    { event: "anydash_browser_checks", browsers, evidence, failures },
    null,
    2,
  ),
);
if (failures.length) process.exitCode = 1;
