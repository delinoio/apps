// SPDX-License-Identifier: Apache-2.0
// Superseded task-detail reads against the test-only preview with held GetTask replies.
// Fixture evidence verifies the shared ReactLynx screens, not a real account or device.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const port = Number(process.env.LANE_TASK_OPEN_PORT || 46417);
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
    // The preview sends no change notification for the title edit, so the list label
    // updates only when the sync watch reconnects and invalidates the list. That timing
    // is not deterministic: accept the label before or after the refresh.
    await page.getByRole("button", { name: /^Task A( newer)?$/ }).click();
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
  // Task saves whose editor ended before the reply arrived. SaveTask replies are
  // held and released in an order chosen by the test.
  const saveControl = async (query) =>
    (await fetch(`${base}/test/save-hold?${query}`)).json();
  const waitSaves = async (count) => {
    for (let i = 0; i < 100; i++) {
      if ((await saveControl("")).length >= count) return;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error(`Expected ${count} SaveTask requests`);
  };
  // Disabled buttons expose the "Disabled" accessibility value and focusability off.
  const isDisabled = async (locator) =>
    (await locator.evaluate((el) => el.outerHTML)).includes("disabled");
  const idB = "019a196d-0000-7000-8000-000000000021";
  async function saveScenario(run) {
    const page = await browser.newPage({
      viewport: { width: 1440, height: 900 },
    });
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(base);
    await button(page, "Task A").waitFor();
    await saveControl("hold=1");
    const before = (await saveControl("")).length;
    try {
      await run(page, before);
    } finally {
      await saveControl("hold=0&reset=1");
      for (const [i, s] of (await saveControl("")).entries())
        if (!s.done) await saveControl(`release=${i}:ok`);
      await control(`title=${idA}:Task A`);
      await control(`title=${idB}:Task B`);
      await page.close();
    }
  }
  // Opens A, edits it and submits a save that stays pending.
  const saveHeldA = async (page, n, title = "A edited") => {
    await button(page, "Task A").click();
    await field(page, "Title").waitFor();
    await field(page, "Title").fill(title);
    await button(page, "Save").click();
    await waitSaves(n + 1);
  };
  const discardEditor = async (page, leave) => {
    await button(page, leave).click();
    await button(page, "Discard changes").click();
    await page.getByRole("dialog").waitFor({ state: "hidden" });
    await field(page, "Title").waitFor({ state: "detached" });
  };
  // The editor still holds unsaved input: leaving it must ask for confirmation.
  const assertDirtyEditor = async (page) => {
    await button(page, "Back to list").click();
    await page
      .getByRole("dialog", { name: "Discard unsaved changes?" })
      .waitFor();
    await button(page, "Keep editing").click();
    await page.getByRole("dialog").waitFor({ state: "hidden" });
  };
  for (const leave of ["Back to list", "Cancel"]) {
    await saveScenario(async (page, n) => {
      await saveHeldA(page, n);
      await discardEditor(page, leave);
      await button(page, "+ New task").click();
      await field(page, "Title").fill("B unsaved");
      await field(page, "Description Markdown").fill("B unsaved description");
      await button(page, "Prompt").click();
      await button(page, "Include prompt").click();
      await field(page, "Prompt Markdown").fill("B unsaved prompt");
      await saveControl(`release=${n}:ok`);
      await button(page, "A edited").waitFor();
      await settle(page);
      assert.equal(await field(page, "Title").inputValue(), "B unsaved");
      assert.equal(
        await field(page, "Prompt Markdown").inputValue(),
        "B unsaved prompt",
      );
      await button(page, "Description").click();
      assert.equal(
        await field(page, "Description Markdown").inputValue(),
        "B unsaved description",
      );
      await assertDirtyEditor(page);
    });
    await saveScenario(async (page, n) => {
      await saveHeldA(page, n);
      await discardEditor(page, leave);
      await button(page, "Task B").click();
      await field(page, "Title").waitFor();
      assert.equal(await field(page, "Title").inputValue(), "Task B");
      await field(page, "Title").fill("B unsaved");
      await saveControl(`release=${n}:ok`);
      await button(page, "A edited").waitFor();
      await settle(page);
      assert.equal(await field(page, "Title").inputValue(), "B unsaved");
      await assertDirtyEditor(page);
    });
  }
  await saveScenario(async (page, n) => {
    await saveHeldA(page, n, "A first edit");
    await discardEditor(page, "Back to list");
    await button(page, "Task A").click();
    await field(page, "Title").waitFor();
    assert.equal(await field(page, "Title").inputValue(), "Task A");
    await field(page, "Title").fill("A second edit");
    await saveControl(`release=${n}:ok`);
    await settle(page);
    assert.equal(await field(page, "Title").inputValue(), "A second edit");
    await assertDirtyEditor(page);
  });
  await saveScenario(async (page, n) => {
    await saveHeldA(page, n);
    await discardEditor(page, "Back to list");
    await button(page, "Settings").click();
    await saveControl(`release=${n}:ok`);
    await settle(page);
    assert.equal(await field(page, "Title").count(), 0);
    assert.equal(await button(page, "+ New task").count(), 0);
    await button(page, "All tasks").click();
    await button(page, "+ New task").waitFor();
    await settle(page);
    assert.equal(await field(page, "Title").count(), 0);
  });
  await saveScenario(async (page, n) => {
    await saveHeldA(page, n);
    // The duplicate-submit guard stays in place while the save is pending.
    assert.equal(await isDisabled(button(page, "Saving…")), true);
    await saveControl(`release=${n}:ok`);
    await button(page, "Save").waitFor();
    assert.equal(await field(page, "Title").inputValue(), "A edited");
    // The returned version is the new base, so the editor is clean again.
    await button(page, "Back to list").click();
    await page.getByRole("dialog").waitFor({ state: "hidden" });
    await button(page, "+ New task").waitFor();
  });
  await saveScenario(async (page, n) => {
    await saveHeldA(page, n);
    await saveControl(`release=${n}:conflict`);
    await page
      .getByText("Both versions were preserved", { exact: false })
      .waitFor();
    assert.equal(await field(page, "Title").inputValue(), "A edited");
    // The conflicted task stays read-only until it is resolved.
    assert.equal(await isDisabled(button(page, "Retry Save")), true);
  });
  for (const [mode, message] of [
    ["fail", "Save failed"],
    ["empty", "The server returned no task"],
  ]) {
    await saveScenario(async (page, n) => {
      await saveHeldA(page, n);
      await saveControl(`release=${n}:${mode}`);
      await page.getByText(message, { exact: false }).waitFor();
      assert.equal(await field(page, "Title").inputValue(), "A edited");
      // An unchanged retry reuses the request ID.
      await button(page, "Retry Save").click();
      await waitSaves(n + 2);
      await saveControl(`release=${n + 1}:ok`);
      await button(page, "Save").waitFor();
      const saves = await saveControl("");
      assert.equal(saves[n + 1].requestId, saves[n].requestId);
      // Changed input uses a new request ID.
      await field(page, "Title").fill("A changed again");
      await button(page, "Save").click();
      await waitSaves(n + 3);
      await saveControl(`release=${n + 2}:ok`);
      const after = await saveControl("");
      assert.notEqual(after[n + 2].requestId, after[n + 1].requestId);
    });
  }
  assert.deepEqual(errors, []);
  console.log("Task open ownership checks passed");
} finally {
  await browser.close();
  server.kill();
}
