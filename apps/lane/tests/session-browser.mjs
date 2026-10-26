// SPDX-License-Identifier: Apache-2.0
// Web session reconciliation through the real browser host focus path, against the
// test-only preview. Fixture evidence only: no real popup, Logto account or cookie.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46417;
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_PORT: String(port) },
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
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
const control = (path) => page.request.get(`${base}/test/${path}`);
const log = async () => (await control("log")).json();
const sessionReads = async () =>
  (await log()).filter((e) => e.path === "/auth/session").length;
// The approval popup returns focus to this tab; dispatch the host's real focus listener.
const returnFocus = () =>
  page.evaluate(() => window.dispatchEvent(new Event("focus")));
const taskButton = (name) => page.getByRole("button", { name, exact: true });
const title = page.getByRole("textbox", { name: "Title", exact: true });
const gate = page.locator("#account-gate");

try {
  await page.goto(base);
  await taskButton("Improve empty states").waitFor();

  // Scenario 1: account A to B. Reconciliation precedes new reads, B's credentials are
  // used and A's list and editor are reset.
  await taskButton("Improve empty states").click();
  await title.fill("Unsaved draft of A");
  await control("principal?name=b");
  await control("log-reset");
  await returnFocus();
  await taskButton("Account B task").waitFor();
  assert.equal(await taskButton("Improve empty states").count(), 0);
  assert.equal(await title.count(), 0, "A's editor is reset");
  const entries = await log();
  assert.equal(entries[0].path, "/auth/session");
  assert.equal(
    entries.filter((e) => e.csrf === "test-csrf").length,
    0,
    "A's CSRF token is never sent after reconciliation",
  );
  assert.ok(entries.some((e) => e.csrf === "other-csrf"));

  // Scenario 3: an unchanged session keeps the current-screen draft.
  await taskButton("Account B task").click();
  await title.fill("Draft of B");
  const before = await sessionReads();
  await returnFocus();
  await page.waitForFunction(
    async ([url, count]) =>
      (await (await fetch(`${url}/test/log`)).json()).filter(
        (e) => e.path === "/auth/session",
      ).length > count,
    [base, before],
  );
  await page.waitForTimeout(300);
  assert.equal(await title.inputValue(), "Draft of B");

  // Same-account reapproval replaces device and CSRF without discarding the draft.
  await control("principal?name=b2");
  await control("log-reset");
  await returnFocus();
  await page.waitForFunction(
    async (url) => (await (await fetch(`${url}/test/log`)).json()).length > 1,
    base,
  );
  await page.waitForTimeout(300);
  assert.equal(await title.inputValue(), "Draft of B");
  await control("fail-save");
  await control("log-reset");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await page.getByText("Simulated storage failure", { exact: false }).waitFor();

  // Scenario 6: the failed save is attempted once with the new token and never replayed.
  await page.waitForTimeout(500);
  const saves = (await log()).filter((e) => e.path.endsWith("/SaveTask"));
  assert.equal(saves.length, 1);
  assert.equal(saves[0].csrf, "other-csrf-2");
  assert.equal(await title.inputValue(), "Draft of B");

  // Scenario 4 (transient): the view is hidden but its input survives, no business
  // request is sent, and only an explicit retry recovers.
  await control("session-mode?mode=503");
  await control("log-reset");
  await returnFocus();
  await gate.waitFor({ state: "visible" });
  await page
    .getByText("Account request failed (503)", { exact: false })
    .waitFor();
  await returnFocus();
  await page.waitForTimeout(300);
  assert.equal(await sessionReads(), 1, "focus does not retry automatically");
  assert.deepEqual(
    (await log()).filter((e) => e.path.startsWith("/lane.v1.")),
    [],
  );
  await control("session-mode?mode=ok");
  await page.getByRole("button", { name: "Retry connection" }).click();
  await gate.waitFor({ state: "hidden" });
  assert.equal(await title.inputValue(), "Draft of B");

  // Scenario 4 (unauthorized): the sign-in gate appears and no business request follows.
  await control("session-mode?mode=401");
  await control("log-reset");
  await returnFocus();
  await gate.waitFor({ state: "visible" });
  await page.getByText("Sign in to Lane to continue.").waitFor();
  await page.waitForTimeout(300);
  assert.deepEqual(
    (await log()).filter((e) => e.path.startsWith("/lane.v1.")),
    [],
  );

  // A full reload recovers once the session is valid again.
  await control("session-mode?mode=ok");
  await page.reload();
  await taskButton("Account B task").waitFor();
  assert.deepEqual(errors, []);
  console.log("Web session reconciliation browser checks passed (mock only).");
} finally {
  await browser.close();
  server.kill();
}
