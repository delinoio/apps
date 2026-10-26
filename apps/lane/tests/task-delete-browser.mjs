// SPDX-License-Identifier: Apache-2.0
// Rejected task deletion against the test-only preview with a controlled DeleteTask reply.
// Fixture evidence verifies the shared ReactLynx screens, not a real account or device.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = Number(process.env.LANE_TASK_DELETE_PORT || 46418);
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
const errors = [];
const control = async (query) =>
  (await fetch(`${base}/test/delete?${query}`)).json();
const button = (page, name) => page.getByRole("button", { name, exact: true });
const field = (page, name) => page.getByRole("textbox", { name, exact: true });
const confirmation = (page) =>
  page.getByRole("dialog", { name: "Permanently delete task?" });
const retained = {
  title: "Retained title",
  description: "Retained description",
  prompt: "Retained prompt",
};
// Gives an unexpected automatic request time to arrive before asserting its absence.
const settle = (page) => page.waitForTimeout(300);

// Opens the fixture task with an unsaved title, description and prompt draft.
async function openDirtyTask(viewport, dirty = true) {
  await control("mode=ok&reset=1");
  const context = await browser.newContext({ viewport });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(base);
  await button(page, "Improve empty states").click();
  if (!dirty) {
    await field(page, "Title").waitFor();
    return { page, context };
  }
  await field(page, "Title").fill(retained.title);
  await field(page, "Description Markdown").fill(retained.description);
  await button(page, "Prompt").click();
  await button(page, "Include prompt").click();
  await field(page, "Prompt Markdown").fill(retained.prompt);
  return { page, context };
}
// The draft is intact and still dirty: leaving the editor asks to discard it.
async function assertDraftRetained(page) {
  await button(page, "Description").click();
  assert.equal(await field(page, "Title").inputValue(), retained.title);
  assert.equal(
    await field(page, "Description Markdown").inputValue(),
    retained.description,
  );
  await button(page, "Prompt").click();
  assert.equal(
    await field(page, "Prompt Markdown").inputValue(),
    retained.prompt,
  );
  await button(page, "Cancel").click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?" })
    .waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("dialog").waitFor({ state: "hidden" });
  assert.equal(await field(page, "Title").inputValue(), retained.title);
}
const requestCount = async () => (await control("")).length;
async function openConfirmation(page) {
  await button(page, "Delete task").click();
  const dialog = confirmation(page);
  await dialog.waitFor();
  return dialog;
}
// A Notice inside the named dialog; plain text nodes have no layout box of their own.
const noticeIn = (dialog, message) =>
  dialog.locator(".notice").filter({ hasText: message });
const deleteButton = (dialog) =>
  dialog.getByRole("button", { name: "Delete permanently" });
// Clicks Delete permanently and expects the failure inside the still-open named dialog.
async function rejectDeletion(page, dialog, message) {
  await deleteButton(dialog).click();
  // The failure is inside the named confirmation, not just somewhere on the page.
  await noticeIn(dialog, message).waitFor();
  assert.equal(await dialog.count(), 1);
  assert.equal(await deleteButton(dialog).isDisabled(), false);
  // The permanent-deletion warning is unchanged.
  assert.ok(
    await dialog
      .getByText("There is no recovery.", { exact: false })
      .isVisible(),
  );
  // The notice is laid out within the dialog box at this viewport.
  const box = await dialog.boundingBox();
  const notice = await noticeIn(dialog, message).boundingBox();
  assert.ok(box && notice);
  assert.ok(notice.x >= box.x && notice.x + notice.width <= box.x + box.width);
  assert.ok(
    notice.y >= box.y && notice.y + notice.height <= box.y + box.height,
  );
  // No automatic retry is sent after the rejection settles.
  const settled = await requestCount();
  await settle(page);
  assert.equal(await requestCount(), settled);
}
const viewports = [
  { width: 1440, height: 900 },
  { width: 390, height: 844 },
];
try {
  for (const viewport of viewports) {
    for (const [mode, message] of [
      ["fail", "Simulated deletion failure"],
      ["denied", "Permission denied"],
    ]) {
      const { page, context } = await openDirtyTask(viewport);
      await control(`mode=${mode}`);
      let dialog = await openConfirmation(page);
      await rejectDeletion(page, dialog, message);
      assert.equal(await requestCount(), 1);
      // Close keeps the draft and sends nothing.
      await dialog.getByRole("button", { name: "Close" }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await assertDraftRetained(page);
      assert.equal(await requestCount(), 1);
      // Escape after a second rejection behaves the same way.
      dialog = await openConfirmation(page);
      // A failure from the previous attempt is not shown before a new attempt.
      assert.equal(await noticeIn(dialog, message).count(), 0);
      await rejectDeletion(page, dialog, message);
      assert.equal(await requestCount(), 2);
      await page.keyboard.press("Escape");
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await assertDraftRetained(page);
      assert.equal(await requestCount(), 2);
      if (mode === "fail") {
        // An explicit retry after the cause is gone completes the existing success path.
        dialog = await openConfirmation(page);
        await control("mode=ok");
        await deleteButton(dialog).click();
        // Only the confirmation is asserted here: closing an editor with a draft goes
        // through the existing discard confirmation, which is outside this change.
        await confirmation(page).waitFor({ state: "hidden" });
        assert.equal(await requestCount(), 3);
      }
      await context.close();
    }
    {
      // A clean task: failure, then an explicit retry follows the existing success path.
      const { page, context } = await openDirtyTask(viewport, false);
      await control("mode=fail");
      const dialog = await openConfirmation(page);
      await rejectDeletion(page, dialog, "Simulated deletion failure");
      assert.equal(await requestCount(), 1);
      await control("mode=ok");
      await deleteButton(dialog).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await button(page, "+ New task").waitFor();
      assert.equal(await field(page, "Title").count(), 0);
      assert.equal(await requestCount(), 2);
      await context.close();
    }
    {
      // The web adapter's offline error is shown in the confirmation; nothing is sent.
      const { page, context } = await openDirtyTask(viewport);
      const dialog = await openConfirmation(page);
      await context.setOffline(true);
      await rejectDeletion(page, dialog, "You are offline");
      assert.equal(await requestCount(), 0);
      await context.setOffline(false);
      await dialog.getByRole("button", { name: "Close" }).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await assertDraftRetained(page);
      assert.equal(await requestCount(), 0);
      await context.close();
    }
    {
      // First-attempt success closes the confirmation and the editor.
      const { page, context } = await openDirtyTask(viewport, false);
      const dialog = await openConfirmation(page);
      await deleteButton(dialog).click();
      await page.getByRole("dialog").waitFor({ state: "hidden" });
      await button(page, "+ New task").waitFor();
      assert.equal(await field(page, "Title").count(), 0);
      assert.equal(await requestCount(), 1);
      await context.close();
    }
  }
  assert.deepEqual(errors, []);
  console.log("Task deletion failure checks passed");
} finally {
  await browser.close();
  server.kill();
}
