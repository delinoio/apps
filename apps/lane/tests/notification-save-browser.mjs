// SPDX-License-Identifier: Apache-2.0
// Fixture bridge verifies that Settings keeps a rejected notification draft dirty and
// visible without a saved message, then persists it on explicit retry. Registration,
// permission and subscription behavior is covered by tests/push-disabled.test.ts.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
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
    `const calls=[];let saves=0;
window.fixtureCalls=()=>({calls});
view.onNativeModulesCall = async (name, data, moduleName) => {
if(moduleName==='bridge') {
 const {module,method,args}=data;
 if(module==='LaneStore'){
 if(method==='open')return JSON.stringify({ok:true,response:{spaceId:'fixture-space',deviceId:'fixture-device',schemaVersion:3,formatVersion:3,sequence:'0'}});
 if(method==='close')return JSON.stringify({ok:true,response:{}});
 const wrapper=JSON.parse(args[1]);calls.push({method:args[0],request:wrapper.request});
 const lists={ListRepositories:1,ListTasks:1,ListPlanningItems:1,ListDevices:1,ListConnections:1,ListURLConnections:1};
 if(args[0]==='GetLocalStatus')return JSON.stringify({ok:true,response:{syncEnabled:true,pendingItemCount:'0',connected:true,itemErrors:[]}});
 if(args[0]==='GetNotificationPreferences')return JSON.stringify({ok:true,response:{enabled:false,content:1,permission:1}});
 if(args[0]==='SetNotificationPreferences'){saves++;if(saves===1)return JSON.stringify({ok:false,error:{reason:'INVALID_INPUT',message:'Fixture persistence unavailable.'}});return JSON.stringify({ok:true,response:{enabled:false,content:wrapper.request.content,permission:1}});}
 if(lists[args[0]])return JSON.stringify({ok:true,response:{items:[]}});
 return JSON.stringify({ok:false,error:{reason:'INVALID_INPUT',message:'Unexpected fixture call '+args[0]}});
 }
 return JSON.stringify({ok:true,value:{}});
}`,
  );
  await route.fulfill({ response, body });
});
const button = (name) => page.getByRole("button", { name, exact: true });
const saves = async () =>
  (await page.evaluate(() => window.fixtureCalls())).calls.filter(
    (call) => call.method === "SetNotificationPreferences",
  );
const savedText = "Notification preferences saved";
try {
  await page.goto(base);
  await button("+ New task").waitFor();
  await button("Settings").first().click();
  await page
    .getByText("Notifications on this device", { exact: true })
    .first()
    .waitFor();

  // Off stays selected while Generic is chosen; the rejected save keeps that draft.
  await button("Generic notice").click();
  await button("Save notification preferences").click();
  await page
    .getByText("Fixture persistence unavailable.", { exact: false })
    .waitFor();
  assert.equal(await page.getByText(savedText, { exact: false }).count(), 0);
  assert.deepEqual(
    (await saves()).map((call) => call.request),
    [{ enabled: false, content: 2 }],
  );
  // The edited draft stays selected and no automatic retry follows.
  await page.waitForTimeout(500);
  assert.equal((await saves()).length, 1);
  assert.equal(
    await button("Generic notice")
      .getAttribute("class")
      .then((c) => /primary/.test(c ?? "")),
    true,
  );

  // Only an explicit retry persists the same draft and reports success.
  await button("Save notification preferences").click();
  await page.getByText(savedText, { exact: false }).waitFor();
  assert.deepEqual(
    (await saves()).map((call) => call.request),
    [
      { enabled: false, content: 2 },
      { enabled: false, content: 2 },
    ],
  );
} finally {
  await browser.close();
  server.kill();
}
assert.deepEqual(errors, []);
console.log("Notification save fixture checks passed");
