// SPDX-License-Identifier: Apache-2.0
// Fixture bridge verifies the shared Settings UI for the native development reset,
// not the native store, registry, handle coordination or secure removal.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";

const port = 46415;
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
const page = await browser.newPage({ viewport: { width: 1200, height: 840 } });
const errors = [];
page.on("pageerror", (e) => errors.push(e.message));
await page.route("**/test-bridge.js", (route) =>
  route.fulfill({
    contentType: "application/javascript",
    body: `export default (_modules,call)=>({call(channel,request,callback){call('bridge',request).then(callback);}});`,
  }),
);
await page.route("**/host.js", async (route) => {
  const response = await route.fetch();
  let body = await readFile(new URL("../web/host.js", import.meta.url), "utf8");
  body = body.replace(
    /view\.nativeModulesMap = \{[\s\S]*?\};/,
    `view.nativeModulesMap={bridge:new URL('/test-bridge.js',location.href).href};`,
  );
  body = body.replace(
    "view.onNativeModulesCall = async (name, data, moduleName) => {",
    `const calls=[];let opens=0;let resetDone=false;let retried=false;let resetMode='success';let releaseReset;const flags=new URLSearchParams(location.search);
window.fixtureCalls=()=>({calls,opens});
window.fixtureReset={mode(value){resetMode=value;},release(value){resetMode=value;releaseReset?.();}};
view.onNativeModulesCall = async (name, data, moduleName) => {
if(moduleName==='bridge') {
 const {module,method,args}=data;
 if(module==='LaneStore'){
 if(method==='open'){opens++;if(flags.get('unsupported')&&!resetDone)return JSON.stringify({ok:false,error:{reason:'ERROR_REASON_UNSUPPORTED_FORMAT',message:'Unsupported store format. Run the explicit development reset (lane data reset).'}});return JSON.stringify({ok:true,response:{spaceId:'fixture-space',deviceId:'fixture-device',schemaVersion:3,formatVersion:3,sequence:'0'}});}
 if(method==='close')return JSON.stringify({ok:true,response:{}});
 const wrapper=JSON.parse(args[1]);calls.push({method:args[0],request:wrapper.request});
 const lists={ListRepositories:1,ListTasks:1,ListPlanningItems:1,ListDevices:1,ListConnections:1,ListURLConnections:1};
 if(args[0]==='GetLocalStatus')return JSON.stringify({ok:true,response:{syncEnabled:true,pendingItemCount:'1',connected:true,itemErrors:flags.get('blocked')&&!retried?[{entityId:'task-1',reason:'CONSTRAINT_VIOLATION:PARENT_CYCLE'}]:[]}});
 if(args[0]==='RetryBlockedItems'){retried=true;return JSON.stringify({ok:true,response:{}});}
 if(args[0]==='GetNotificationPreferences')return JSON.stringify({ok:true,response:{enabled:false,content:1,permission:1}});
 if(args[0]==='ResetDevelopmentData'){
  if(resetMode==='pending')await new Promise(resolve=>{releaseReset=resolve;});
  if(resetMode==='busy')return JSON.stringify({ok:false,error:{reason:'STORAGE_BUSY',message:'Close other Lane windows.'}});
  if(resetMode==='permission')return JSON.stringify({ok:false,error:{reason:'STORAGE_PERMISSION',message:'Storage permission denied.'}});
  resetDone=true;return JSON.stringify({ok:true,response:{completed:true}});
 }
 if(lists[args[0]])return JSON.stringify({ok:true,response:{items:[]}});
 return JSON.stringify({ok:false,error:{reason:'INVALID_INPUT',message:'Unexpected fixture call '+args[0]}});
 }
 return JSON.stringify({ok:true,value:{}});
}`,
  );
  await route.fulfill({ response, body });
});
const button = (name) => page.getByRole("button", { name, exact: true });
const dialog = page.locator(".dialog");
const resetCalls = async () =>
  (await page.evaluate(() => window.fixtureCalls())).calls.filter(
    (call) => call.method === "ResetDevelopmentData",
  );
try {
  await page.goto(base);
  await button("+ New task").waitFor();
  await button("+ New task").click();
  const draftTitle = page.getByRole("textbox", { name: "Title", exact: true });
  await draftTitle.fill("Retain this hidden draft");
  await button("Settings").first().click();
  await page.getByText("Development data", { exact: true }).waitFor();

  // Cancel paths change nothing: Escape, Cancel and a mismatched confirmation.
  await button("Reset development data").click();
  await page.getByText("Reset development data?", { exact: true }).waitFor();
  const confirm = dialog.getByRole("button", {
    name: "Reset development data",
    exact: true,
  });
  assert.equal(await confirm.getAttribute("aria-disabled"), "true");
  const input = dialog.getByRole("textbox");
  assert.equal(await dialog.getAttribute("role"), "dialog");
  assert.equal(await dialog.getAttribute("aria-modal"), "true");
  assert.equal(
    await dialog.getAttribute("aria-label"),
    "Reset development data?",
  );
  await page.waitForFunction(() =>
    document
      .querySelector("lynx-view")
      ?.shadowRoot?.activeElement?.matches(
        'x-input[data-dialog-default="true"]',
      ),
  );
  assert.equal(
    await input.evaluate(
      (element) => element === element.getRootNode().activeElement,
    ),
    true,
  );
  for (const sentence of [
    "Only reset disposable development data.",
    "Permanent local deletion",
    "This removes every repository, task, prompt, label, milestone, cycle, history, attachment and pending change in this local data space.",
    "There is no recovery.",
    "Server data is not deleted. This does not contact the server or sign you out.",
    "An account-linked space will need explicit reconnection before sync resumes.",
  ])
    assert.equal(
      (await dialog.getByText(sentence, { exact: false }).count()) > 0,
      true,
    );
  assert.equal(
    await dialog
      .locator(".reset-actions .button")
      .first()
      .getAttribute("aria-label"),
    "Cancel",
  );
  await input.fill("reset");
  assert.equal(await confirm.getAttribute("aria-disabled"), "true");
  await page.keyboard.press("Escape");
  await page
    .getByText("Reset development data?", { exact: true })
    .waitFor({ state: "detached" });
  // Dialogs return focus to the control that opened them.
  assert.equal(
    await page
      .locator("lynx-view")
      .evaluate((view) =>
        view.shadowRoot.activeElement?.getAttribute("aria-label"),
      ),
    "Reset development data",
  );
  assert.deepEqual(await resetCalls(), []);

  for (const action of ["Cancel", "Close"]) {
    await button("Reset development data").click();
    await dialog.getByRole("button", { name: action, exact: true }).click();
    await dialog.waitFor({ state: "detached" });
    assert.deepEqual(await resetCalls(), []);
    assert.equal(
      await page
        .locator("lynx-view")
        .evaluate((view) =>
          view.shadowRoot.activeElement?.getAttribute("aria-label"),
        ),
      "Reset development data",
    );
  }

  await button("Reset development data").click();
  for (const phrase of [
    "",
    "RESET DEVELOPMENT DATA",
    "reset development",
    "reset development datum",
  ]) {
    await dialog.getByRole("textbox").fill(phrase);
    assert.equal(await confirm.getAttribute("aria-disabled"), "true");
    assert.deepEqual(await resetCalls(), []);
  }
  await dialog.getByRole("textbox").fill("  reset development data  ");
  await dialog.getByRole("textbox").press("Enter");
  assert.deepEqual(await resetCalls(), []);
  await dialog.getByRole("button", { name: "Cancel" }).click();

  // The exact displayed text enables the destructive action; it sends the confirmation flag once.
  await button("Reset development data").click();
  const opensBefore = (await page.evaluate(() => window.fixtureCalls())).opens;
  await dialog.getByRole("textbox").fill("reset development data");
  await page.waitForTimeout(300);
  assert.notEqual(await confirm.getAttribute("aria-disabled"), "true");
  await confirm.click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?", exact: true })
    .waitFor();
  await button("Keep editing").click();
  await page
    .getByText("Data reset cancelled. Your input is retained.", {
      exact: false,
    })
    .waitFor();
  assert.deepEqual(
    await resetCalls(),
    [],
    "cancel preserves every tab and performs no reset",
  );
  assert.equal(
    await page
      .getByRole("textbox", { name: "Title", exact: true, includeHidden: true })
      .inputValue(),
    "Retain this hidden draft",
  );
  await confirm.click();
  await page
    .getByRole("dialog", { name: "Discard unsaved changes?", exact: true })
    .waitFor();
  await button("Discard changes").click();
  await page
    .getByText("Reset development data?", { exact: true })
    .waitFor({ state: "detached" });
  assert.deepEqual(await resetCalls(), [
    { method: "ResetDevelopmentData", request: { confirmed: true } },
  ]);
  // The host reopens the space; Lane never reuses cached reads.
  await page.waitForFunction(
    (before) => window.fixtureCalls().opens > before,
    opensBefore,
  );

  // Hold the bridge request to verify one submission and all pending controls.
  await button("Settings").first().click();
  await page.getByText("Development data", { exact: true }).waitFor();
  await page.evaluate(() => window.fixtureReset.mode("pending"));
  await button("Reset development data").click();
  const heldInput = dialog.getByRole("textbox");
  await heldInput.fill("  reset development data  ");
  const callsBeforeHold = (await resetCalls()).length;
  await dialog
    .getByRole("button", { name: "Reset development data" })
    .evaluate((button) => {
      button.click();
      button.click();
    });
  await page.waitForFunction(
    (count) =>
      window
        .fixtureCalls()
        .calls.filter((call) => call.method === "ResetDevelopmentData").length >
      count,
    callsBeforeHold,
  );
  for (const name of ["Resetting…", "Cancel", "Close"]) {
    assert.equal(
      await dialog
        .getByRole("button", { name, exact: true })
        .getAttribute("aria-disabled"),
      "true",
    );
  }
  assert.equal(await heldInput.getAttribute("disabled"), "");
  assert.equal(
    await dialog
      .locator("[accessibility-live-region]")
      .getAttribute("aria-live"),
    "polite",
  );
  await page.keyboard.press("Escape");
  assert.equal(await dialog.count(), 1);
  await page.evaluate(() => window.fixtureReset.release("busy"));
  await dialog
    .getByText(
      "Close other Lane windows, CLI or MCP handles, or stop the sync worker",
      { exact: false },
    )
    .waitFor();
  assert.equal(
    await dialog.locator(".validation").getAttribute("role"),
    "alert",
  );
  assert.equal(await heldInput.inputValue(), "  reset development data  ");
  assert.equal((await resetCalls()).length, callsBeforeHold + 1);
  await page.evaluate(() => window.fixtureReset.mode("permission"));
  await dialog.getByRole("button", { name: "Reset development data" }).click();
  await dialog
    .getByText("Storage permission denied.", { exact: false })
    .waitFor();
  assert.equal(await heldInput.inputValue(), "  reset development data  ");
  await dialog.getByRole("button", { name: "Cancel" }).click();

  // Rejected synchronization items are explained, kept visible and retried explicitly.
  await page.goto(`${base}/?blocked=1`);
  await button("+ New task").waitFor();
  await button("Settings").first().click();
  await page
    .getByText("This parent would create a loop", { exact: false })
    .waitFor();
  await button("Retry blocked items").click();
  await page
    .getByText("Blocked items were returned to pending", { exact: false })
    .waitFor();
  assert.equal(
    (await page.evaluate(() => window.fixtureCalls())).calls.filter(
      (c) => c.method === "RetryBlockedItems",
    ).length,
    1,
  );
  assert.equal(await button("Retry blocked items").count(), 0);

  // An unsupported format cannot be opened: only the typed explicit reset is offered.
  await page.goto(`${base}/?unsupported=1`);
  await page
    .getByText("This data space cannot be opened", { exact: true })
    .waitFor();
  await page
    .getByText("Your data has not been changed.", { exact: true })
    .waitFor();
  await page
    .getByText("Lane cannot open this data format.", { exact: false })
    .waitFor();
  await page
    .getByText(
      "For a specific data space, use the existing CLI reset procedure.",
      { exact: true },
    )
    .waitFor();
  await page
    .getByText("Unsupported store format.", { exact: false })
    .first()
    .waitFor();
  assert.equal(await button("+ New task").count(), 0);
  assert.equal((await resetCalls()).length, 0);
  assert.equal(
    await page.locator(".store-recovery .button.primary").count(),
    0,
  );
  await page.setViewportSize({ width: 360, height: 560 });
  await page.locator(".app.mobile").waitFor();
  assert.equal(
    await page
      .locator(".store-recovery")
      .evaluate(
        (recovery) => recovery.getBoundingClientRect().right <= innerWidth,
      ),
    true,
  );
  await button("Reset development data").click();
  const recoveryConfirm = dialog.getByRole("button", {
    name: "Reset development data",
    exact: true,
  });
  assert.equal(await recoveryConfirm.getAttribute("aria-disabled"), "true");
  assert.equal(
    await page
      .getByText("Your data has not been changed.", { exact: true })
      .getAttribute("aria-hidden"),
    "true",
  );
  assert.equal(
    await dialog
      .locator(".reset-actions .button")
      .first()
      .getAttribute("aria-label"),
    "Cancel",
  );
  assert.equal(
    await dialog
      .locator(".reset-actions .button")
      .first()
      .evaluate((node) => node.getBoundingClientRect().height >= 44),
    true,
  );
  await dialog.getByRole("textbox").fill("reset development data");
  await recoveryConfirm.focus();
  await page.keyboard.press("Enter");
  await button("+ New task").waitFor();
  const unopened = (
    await page.evaluate(() => window.fixtureCalls())
  ).calls.filter((c) => c.method === "ResetDevelopmentData");
  assert.deepEqual(unopened, [
    { method: "ResetDevelopmentData", request: { confirmed: true } },
  ]);
} catch (error) {
  console.error(errors);
  throw error;
} finally {
  await browser.close();
  server.kill();
}
assert.deepEqual(errors, []);
console.log("Native reset fixture checks passed");
