// SPDX-License-Identifier: Apache-2.0
// Fixture bridge verifies the shared ReactLynx UI, not OS capture or rasterization.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { chromium } from "playwright";
const evidence = "/tmp/realqa-412-ui-evidence";
await mkdir(evidence, { recursive: true });
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_PORT: "46414" },
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
const base = "http://127.0.0.1:46414";
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
    `let fixtureDraft;
let windowKind='main';let savedCount=0;let attempts=[];let failOnce=true;const operations=[];
const fixtureId='019a196d-0000-7000-8000-000000000001';
const state=()=>({window:windowKind,draft:fixtureDraft,shortcuts:{},permission:'granted',chrome:{state:'disconnected',pending:[],connections:[]}});
const emit=()=>view.sendGlobalEvent('lane.realqa.changed',[{}]);
window.fixtureState=()=>({draft:fixtureDraft,savedCount,attempts,operations});
view.onNativeModulesCall = async (name, data, moduleName) => {
if(moduleName==='bridge') {
 const {module,method,args}=data;
 if(module==='LaneStore'){
 if(method==='open')return JSON.stringify({ok:true,response:{spaceId:'fixture-space',deviceId:'fixture-device',schemaVersion:2,formatVersion:2,sequence:'0'}});
 if(method==='close')return JSON.stringify({ok:true,response:{}});
 const input=JSON.parse(args[1]).request;let response={};
 if(args[0]==='GetLocalStatus')response={syncEnabled:false,pendingItemCount:'0',connected:false};
 else if(args[0]==='ListURLConnections')response={items:[]};
 else if(args[0]==='CreateCaptureTask') {attempts.push(input);if(failOnce){failOnce=false;return JSON.stringify({ok:false,error:{reason:'STORAGE_FULL',message:'Synthetic storage failure'}});}savedCount++;response={result:{result:'RESULT_KIND_APPLIED',version:{entityId:input.id,versionId:fixtureId,kind:'ENTITY_KIND_TASK',task:{repositoryId:input.repositoryId,title:input.title}}}};}
 else response=await online.invoke('lane.v1.LaneService',args[0],input);
 return JSON.stringify({ok:true,response});
 }
 const request=JSON.parse(args[0]);operations.push(request);let value={};
 if(request.op==='state')value=state();
 else if(request.op==='sources')value={sources:[{id:'monitor',name:'Test monitor',width:480,height:240}]};
 else if(request.op==='capture') {const response=await online.invoke('lane.v1.CaptureService','GetAttachment',{});fixtureDraft={id:fixtureId,requestId:'019a196d-0000-7000-8000-000000000009',repositoryId:'',title:'',description:'',images:[{id:'019a196d-0000-7000-8000-000000000011',png:response.png,width:480,height:240,layers:[],metadata:{pageUrl:'https://example.test/app',pageTitle:'Sample page',cssSelector:'main > button'},canUndo:false,canRedo:false,sha256:'a'.repeat(64)}]};windowKind='composer';emit();}
 else if(request.op==='updateDraft'){if(request.imageId)fixtureDraft.images[0].metadata=request.metadata;else for(const key of ['title','description','repositoryId'])if(key in request)fixtureDraft[key]=request[key];emit();}
 else if(request.op==='edit'){fixtureDraft.images[0].layers=request.layers;fixtureDraft.images[0].canUndo=true;emit();}
 else if(request.op==='export')value=fixtureDraft;
 else if(request.op==='complete'){fixtureDraft=null;windowKind='main';emit();view.sendGlobalEvent('lane.realqa.saved',[{taskId:request.taskId,spaceId:request.spaceId}]);}
 return JSON.stringify({ok:true,value});
}`,
  );
  await route.fulfill({ response, body });
});
const button = (name) => page.getByRole("button", { name, exact: true });
try {
  await page.goto(base);
  await button("RealQA").click();
  await button("Full screen").waitFor();
  await page.screenshot({
    path: `evidence/home.png`.replace("evidence", evidence),
  });
  await button("Full screen").click();
  await button("Test monitor · 480 × 240").click();
  await page.getByRole("textbox", { name: "Title", exact: true }).waitFor();
  await page
    .getByRole("textbox", { name: "Title", exact: true })
    .fill("Synthetic capture task");
  await page
    .getByRole("textbox", { name: "Description", exact: true })
    .fill("Synthetic problem description");
  await button("github.com/delinoio/apps").click();
  await button("Remove CSS selector").click();
  await button("Opaque cover").click();
  await button("Apply tool").click();
  await page.waitForFunction(
    () => window.fixtureState().draft.images[0].layers.length === 1,
  );
  await page.locator(".editor-tools").scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${evidence}/desktop-composer.png` });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator(".composer-form").scrollIntoViewIfNeeded();
  await page.waitForTimeout(150);
  const form = await page.locator(".composer-form").boundingBox();
  assert.ok(form.x + form.width <= 390.5, JSON.stringify(form));
  await page.screenshot({ path: `${evidence}/mobile-390-composer.png` });
  await page.setViewportSize({ width: 1200, height: 840 });
  await button("Save to Lane").click();
  await page.getByText("Synthetic storage failure", { exact: false }).waitFor();
  assert.equal(
    await page
      .getByRole("textbox", { name: "Title", exact: true })
      .inputValue(),
    "Synthetic capture task",
  );
  await button("Retry").click();
  await page.getByRole("dialog", { name: "Saved to Lane" }).waitFor();
  const result = await page.evaluate(() => window.fixtureState());
  assert.equal(result.savedCount, 1);
  assert.equal(result.attempts.length, 2);
  assert.deepEqual(result.attempts[0], result.attempts[1]);
  assert.equal(result.attempts[1].images[0].capture.cssSelector, undefined);
  assert.equal(result.attempts[1].description, "Synthetic problem description");
  assert.ok(result.operations.some((op) => op.op === "saveFailed"));
  assert.deepEqual(errors, []);
  await writeFile(
    `${evidence}/composer-result.json`,
    JSON.stringify(
      {
        result: "passed",
        fixture: true,
        savedCount: result.savedCount,
        retries: result.attempts.length,
        errors,
      },
      null,
      2,
    ),
  );
  console.log("Composer fixture checks passed");
} finally {
  await browser.close();
  server.kill();
}
