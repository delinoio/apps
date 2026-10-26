// SPDX-License-Identifier: Apache-2.0
// Superseded task-detail reads against the test-only preview with held GetTask replies.
// Fixture evidence verifies the shared ReactLynx screens, not a real account or device.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = 46417;
const base = `http://127.0.0.1:${port}`;
const idA = "019a196d-0000-7000-8000-000000000003";
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: {
    ...process.env,
    LANE_PREVIEW_PORT: String(port),
    LANE_PREVIEW_SCENARIO: "task-open",
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
const errors = [];
const control = async (query) =>
  (await fetch(`${base}/test/task-open?${query}`)).json();
const release = (index, mode = "ok") => control(`release=${index}:${mode}`);
const button = (page, name) => page.getByRole("button", { name, exact: true });
const field = (page, name) => page.getByRole("textbox", { name, exact: true });
// Polls the preview until the expected number of GetTask replies is being held.
const waitHeld = async (count) => {
  for (let i = 0; i < 100; i++) {
    if ((await control("")).length >= count) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Expected ${count} held GetTask replies`);
};
// Gives an obsolete reply time to (incorrectly) publish before asserting.
const settle = (page) => page.waitForTimeout(300);
// Every scenario starts from a fresh page so held replies never leak between them.
async function scenario(run) {
  const page = await browser.newPage({
    viewport: { width: 1440, height: 900 },
  });
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(base);
  await button(page, "Task A").waitFor();
  await control("hold=1");
  const before = (await control("")).length;
  try {
    await run(page, before);
  } finally {
    await control("hold=0");
    await control(`title=${idA}:Task A`);
    await page.close();
  }
}
try {
  for (const outcome of ["ok", "fail"]) {
    await scenario(async (page, n) => {
      await button(page, "Task A").click();
      await button(page, "Task B").click();
      await waitHeld(n + 2);
      await release(n + 1);
      await field(page, "Title").waitFor();
      assert.equal(await field(page, "Title").inputValue(), "Task B");
      await field(page, "Title").fill("B unsaved");
      await field(page, "Description Markdown").fill("B unsaved description");
      await release(n, outcome);
      await settle(page);
      assert.equal(await field(page, "Title").inputValue(), "B unsaved");
      assert.equal(
        await field(page, "Description Markdown").inputValue(),
        "B unsaved description",
      );
      assert.equal(await page.getByText("Read failed").count(), 0);
      assert.equal(await page.getByRole("dialog").count(), 0);
    });
  }
  for (const outcome of ["ok", "fail"]) {
    await scenario(async (page, n) => {
      await button(page, "Task A").click();
      await button(page, "+ New task").click();
      await field(page, "Title").fill("New unsaved");
      await waitHeld(n + 1);
      await release(n, outcome);
      await settle(page);
      assert.equal(await field(page, "Title").inputValue(), "New unsaved");
      assert.equal(await page.getByText("Read failed").count(), 0);
    });
  }
  await scenario(async (page, n) => {
    await button(page, "Task A").click();
    await waitHeld(n + 1);
    await button(page, "Settings").click();
    await release(n);
    await settle(page);
    assert.equal(await field(page, "Title").count(), 0);
    assert.equal(await button(page, "+ New task").count(), 0);
  });
  await scenario(async (page, n) => {
    await button(page, "Task A").click();
    await waitHeld(n + 1);
    await button(page, "Task B").click();
    await waitHeld(n + 2);
    await release(n + 1);
    await field(page, "Title").waitFor();
    await button(page, "Back to list").click();
    await release(n);
    await settle(page);
    assert.equal(await field(page, "Title").count(), 0);
  });
  await scenario(async (page, n) => {
    await button(page, "Task A").click();
    await waitHeld(n + 1);
    await control(`title=${idA}:Task A newer`);
    await button(page, "Task B").click();
    await waitHeld(n + 2);
    await button(page, "Task A newer").click();
    await waitHeld(n + 3);
    await release(n + 2);
    await field(page, "Title").waitFor();
    assert.equal(await field(page, "Title").inputValue(), "Task A newer");
    await field(page, "Title").fill("Latest A unsaved");
    await release(n);
    await release(n + 1);
    await settle(page);
    assert.equal(await field(page, "Title").inputValue(), "Latest A unsaved");
  });
  await scenario(async (page, n) => {
    await button(page, "Task A").click();
    await waitHeld(n + 1);
    await release(n);
    await field(page, "Title").waitFor();
    assert.equal(await field(page, "Title").inputValue(), "Task A");
    await field(page, "Title").fill("A edited");
    await button(page, "Task B").click();
    await page
      .getByRole("dialog", { name: "Discard unsaved changes?" })
      .waitFor();
    await page.keyboard.press("Escape");
    await page.getByRole("dialog").waitFor({ state: "hidden" });
    assert.equal(await field(page, "Title").inputValue(), "A edited");
    assert.equal((await control("")).length, n + 1);
  });
  assert.deepEqual(errors, []);
  console.log("Task open ownership checks passed");
} finally {
  await browser.close();
  server.kill();
}
