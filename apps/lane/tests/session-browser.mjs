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
  // A rejected first session keeps normal browser focus and the typed Logto
  // destination while exposing only the standalone sign-in gate.
  const signedOut = await browser.newPage({
    viewport: { width: 1200, height: 900 },
  });
  await signedOut.route("**/auth/session", (route) =>
    route.fulfill({ status: 401, body: "{}" }),
  );
  const taskId = "019a0000-0000-7000-8000-000000000001";
  const accountId = "019a0000-0000-7000-8000-000000000002";
  await signedOut.goto(
    `${base}/?task=${taskId}&account=${accountId}&utm=ignored`,
  );
  await signedOut.getByRole("heading", { name: "Sign in to Lane" }).waitFor();
  assert.equal(
    await signedOut.locator("#account-gate").getAttribute("data-state"),
    "signed-out",
  );
  assert.equal(
    await signedOut.locator("#account-status").textContent(),
    "Sign in to use Lane on the web.",
  );
  assert.equal(
    await signedOut.locator("#account-gate").getAttribute("aria-busy"),
    "false",
  );
  assert.equal(
    await signedOut.getByRole("status").getAttribute("aria-atomic"),
    "true",
  );
  assert.equal(await signedOut.locator("#account-guidance").isHidden(), true);
  assert.equal(await signedOut.locator("lynx-view").isHidden(), true);
  assert.equal(
    await signedOut.evaluate(() => document.activeElement?.tagName),
    "BODY",
  );
  const controls = signedOut.locator("#account-gate a, #account-gate button");
  assert.deepEqual(await controls.allTextContents(), [
    "Sign in with Logto",
    "Retry connection",
  ]);
  assert.equal(
    await signedOut
      .locator("#sign-in")
      .evaluate((node) => getComputedStyle(node).backgroundColor),
    "rgb(7, 90, 255)",
  );
  const login = new URL(
    await signedOut.locator("#sign-in").getAttribute("href"),
  );
  assert.equal(login.pathname, "/auth/login");
  assert.equal(login.searchParams.get("return_to"), `${base}/`);
  assert.equal(login.searchParams.get("task"), taskId);
  assert.equal(login.searchParams.get("account"), accountId);
  assert.equal(login.searchParams.has("utm"), false);
  await signedOut.keyboard.press("Escape");
  assert.equal(await signedOut.locator("#account-gate").isVisible(), true);
  for (const width of [1200, 900, 899, 360]) {
    await signedOut.setViewportSize({
      width,
      height: width === 360 ? 360 : 900,
    });
    assert.equal(
      await signedOut.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
      true,
      `gate overflows at ${width}px`,
    );
  }
  await signedOut.evaluate(() => {
    document.body.style.zoom = "2";
  });
  assert.equal(
    await signedOut.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
    "gate overflows at 200% text scale",
  );
  assert.equal(
    await signedOut.evaluate(
      () => document.documentElement.scrollHeight > innerHeight,
    ),
    true,
    "short gate page must scroll",
  );
  await signedOut.context().setOffline(true);
  await signedOut.evaluate(() => window.dispatchEvent(new Event("offline")));
  assert.match(
    await signedOut.locator("#account-guidance").textContent(),
    /Reconnect, then select Retry connection/,
  );
  await signedOut.context().setOffline(false);
  await signedOut.close();

  const firstFailure = await browser.newPage({
    viewport: { width: 360, height: 360 },
  });
  await firstFailure.route("**/auth/session", (route) =>
    route.fulfill({ status: 503, body: "{}" }),
  );
  await firstFailure.goto(base);
  await firstFailure
    .getByRole("heading", { name: "Unable to connect" })
    .waitFor();
  assert.equal(
    await firstFailure.locator("#account-status").textContent(),
    "Account request failed (503). Retry explicitly.",
  );
  assert.equal(
    await firstFailure.locator("#account-guidance").isHidden(),
    true,
  );
  assert.equal(
    await firstFailure
      .locator("#retry")
      .evaluate((node) => getComputedStyle(node).backgroundColor),
    "rgb(7, 90, 255)",
  );
  // Use long safe text to verify the status box can grow without clipping.
  await firstFailure.locator("#account-status").evaluate((node) => {
    node.textContent = "Connection failed. " + "unbroken-status-".repeat(30);
    document.body.style.zoom = "2";
  });
  assert.equal(
    await firstFailure.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    true,
    "long error overflows at 200% text scale",
  );
  await firstFailure.close();

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
  await page.locator("body").focus();
  await control("session-mode?mode=503");
  await control("log-reset");
  await returnFocus();
  await gate.waitFor({ state: "visible" });
  await page
    .getByText("Account request failed (503)", { exact: false })
    .waitFor();
  await page.locator("#gate-heading").focus();
  assert.equal(await gate.getAttribute("data-state"), "error");
  assert.equal(
    await page.getByRole("heading", { name: "Unable to connect" }).count(),
    1,
  );
  assert.match(
    await page.locator("#account-guidance").textContent(),
    /current-screen input is retained/,
  );
  assert.equal(
    await page.evaluate(() => document.activeElement?.id),
    "gate-heading",
  );
  await returnFocus();
  await page.waitForTimeout(300);
  assert.equal(await sessionReads(), 1, "focus does not retry automatically");
  assert.deepEqual(
    (await log()).filter((e) => e.path.startsWith("/lane.v1.")),
    [],
  );
  await control("session-mode?mode=ok");
  // Hold the explicit retry and try mouse, Enter and Space again. The host
  // starts one attempt, announces busy, then releases it after a failure.
  let releaseRetry;
  let sawRetry;
  const retryStarted = new Promise((resolve) => {
    sawRetry = resolve;
  });
  const heldRetry = new Promise((resolve) => {
    releaseRetry = resolve;
  });
  let retryRequests = 0;
  const holdSession = async (route) => {
    retryRequests++;
    sawRetry();
    await heldRetry;
    await route.fulfill({ status: 503, body: "{}" });
  };
  await page.route("**/auth/session", holdSession);
  const retryButton = page.getByRole("button", { name: "Retry connection" });
  await retryButton.click();
  await retryStarted;
  assert.equal(await gate.getAttribute("aria-busy"), "true");
  assert.equal(await retryButton.getAttribute("aria-disabled"), "true");
  assert.equal(
    await page.getByRole("heading", { name: "Connecting to Lane" }).count(),
    1,
  );
  assert.equal(
    await page.locator("#account-status").textContent(),
    "Connecting…",
  );
  await retryButton.press("Enter");
  await retryButton.press("Space");
  await retryButton.dispatchEvent("click");
  assert.equal(retryRequests, 1);
  releaseRetry();
  await page.getByRole("heading", { name: "Unable to connect" }).waitFor();
  await page
    .getByText("Account request failed (503). Retry explicitly.")
    .waitFor();
  assert.equal(await gate.getAttribute("aria-busy"), "false");
  assert.equal(await retryButton.getAttribute("aria-disabled"), null);
  assert.equal(await page.evaluate(() => document.activeElement?.id), "retry");
  await page.context().setOffline(true);
  await page.evaluate(() => window.dispatchEvent(new Event("offline")));
  assert.match(
    await page.locator("#account-guidance").textContent(),
    /Reconnect, then select Retry connection/,
  );
  await page.context().setOffline(false);
  await page.evaluate(() => window.dispatchEvent(new Event("online")));
  await page.unroute("**/auth/session", holdSession);
  await retryButton.click();
  await gate.waitFor({ state: "hidden" });
  assert.equal(await gate.getAttribute("aria-busy"), "false");
  assert.equal(
    await page.evaluate(() => {
      let active =
        document.querySelector("lynx-view")?.shadowRoot?.activeElement;
      while (active?.shadowRoot?.activeElement)
        active = active.shadowRoot.activeElement;
      return active?.id;
    }),
    "inner-box",
    "fallback recovery focus uses the focusable heading target",
  );
  // Reconnection invalidates every retained pane. Finish those authorized reads
  // before resetting the log for the next rejected-session scenario.
  await page.waitForLoadState("networkidle");
  assert.equal(await title.inputValue(), "Draft of B");
  assert.equal(
    await page.evaluate(() => document.activeElement?.tagName),
    "LYNX-VIEW",
  );

  // Scenario 4 (unauthorized): the sign-in gate appears and no business request follows.
  await control("session-mode?mode=401");
  await control("log-reset");
  await returnFocus();
  await gate.waitFor({ state: "visible" });
  await page.getByText("Sign in to use Lane on the web.").waitFor();
  assert.equal(await gate.getAttribute("data-state"), "signed-out");
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
