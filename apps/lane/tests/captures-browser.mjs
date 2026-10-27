// SPDX-License-Identifier: Apache-2.0
// Exercises the saved renderer through a local fixture, without account access.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const base = "http://127.0.0.1:46413";
const taskId = "019a196d-0000-7000-8000-000000000003";
const ids = ["000011", "000012", "000013"].map(
  (suffix) => `019a0000-0000-7000-8000-000000${suffix}`,
);
const captures = ids.map((attachmentId, index) => ({
  attachmentId,
  byteSize: "70",
  pageUrl: `https://example.com/capture/${index + 1}`,
  pageTitle: `Capture page ${index + 1}`,
  cssSelector: `main.capture-${index + 1}`,
}));
const server = spawn(process.execPath, ["tests/preview.mjs"], {
  env: { ...process.env, LANE_PREVIEW_PORT: "46413" },
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
page.setDefaultTimeout(90000);
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const button = (name) => page.getByRole("button", { name, exact: true });
async function fixture(body = {}) {
  const response = await page.request.get(
    `${base}/test/captures?body=${encodeURIComponent(JSON.stringify(body))}`,
  );
  assert.ok(response.ok());
  return response.json();
}
async function openPrompt() {
  await page.goto(base);
  await button("Improve empty states").click();
  await button("Prompt").click();
  await button("Preview Markdown").click();
}
async function readGeometry() {
  return page.locator(".captures").evaluate((element) => {
    const cards = [...element.querySelectorAll(".capture-result")];
    return cards.map((card) => {
      const rect = card.getBoundingClientRect();
      const stage = card
        .querySelector(".capture-stage")
        .getBoundingClientRect();
      const metadata = [...card.querySelectorAll(".capture-metadata")].map(
        (row) => {
          const label = row.querySelector(".capture-metadata-label");
          const value = row.querySelector(".capture-metadata-value");
          const bounds = value.getBoundingClientRect();
          return {
            label: label.textContent,
            value: value.textContent,
            right: bounds.right,
            labelBottom: label.getBoundingClientRect().bottom,
            valueTop: bounds.top,
          };
        },
      );
      return {
        left: rect.left,
        right: rect.right,
        top: rect.top,
        bottom: rect.bottom,
        stageHeight: stage.height,
        metadata,
      };
    });
  });
}

try {
  await fixture({
    captures,
    replies: { [ids[0]]: "available", [ids[1]]: "hold", [ids[2]]: "available" },
    resetRequests: true,
  });
  await openPrompt();
  await page.getByText("Capture 3 of 3", { exact: true }).waitFor();
  assert.deepEqual(await page.locator(".capture-position").allTextContents(), [
    "Capture 1 of 3",
    "Capture 2 of 3",
    "Capture 3 of 3",
  ]);
  await page.getByText("Loading capture…", { exact: true }).waitFor();
  assert.equal(await page.locator(".capture-image").count(), 2);
  await page.waitForFunction(() => {
    const image = document
      .querySelector("lynx-view")
      .shadowRoot.querySelector(".capture-image");
    const inner = image?.shadowRoot?.querySelector("img");
    return inner?.complete && inner.naturalWidth === 480;
  });
  assert.equal(await button("Retry").count(), 0);
  const before = await fixture();
  assert.deepEqual(
    before.requests,
    ids.map((attachmentId) => ({ taskId, attachmentId })),
  );
  assert.equal(before.held.length, 1);
  await fixture({ release: 0, mode: "waiting" });
  await page
    .getByText(
      "Waiting for synchronization. The capture is saved on its originating device.",
      { exact: true },
    )
    .waitFor();
  assert.equal(await button("Retry").count(), 1);
  await fixture({ replies: { [ids[1]]: "available" } });
  await button("Retry").focus();
  await page.keyboard.press("Enter");
  await page.locator(".capture-image").nth(2).waitFor();
  const after = await fixture();
  assert.deepEqual(
    after.requests.map((request) => request.attachmentId),
    [...ids, ids[1]],
  );
  assert.equal(await button("Retry").count(), 0);

  const long = "x".repeat(256);
  const unusual = {
    ...captures[0],
    pageUrl: `https://example.com/${long}`,
    pageTitle: "<script>alert(1)</script>",
    cssSelector: `#${long}`,
  };
  await fixture({
    captures: [
      unusual,
      { attachmentId: ids[1], pageUrl: "", pageTitle: "", cssSelector: "" },
    ],
  });
  await openPrompt();
  await page.getByText("Capture 2 of 2", { exact: true }).waitFor();
  assert.equal(await page.locator(".capture-result").count(), 2);
  for (const width of [1440, 900, 899, 390, 360]) {
    await page.setViewportSize({ width, height: 840 });
    await page
      .locator(".capture-result")
      .last()
      .evaluate((element) => element.scrollIntoView({ block: "end" }));
    const geometry = await readGeometry();
    assert.equal(geometry.length, 2);
    assert.ok(
      geometry[0].bottom + 15 <= geometry[1].top,
      JSON.stringify(geometry),
    );
    for (const card of geometry) {
      assert.ok(
        card.left >= -0.5 && card.right <= width + 0.5,
        JSON.stringify(card),
      );
      assert.ok(Math.abs(card.stageHeight - (width < 900 ? 220 : 320)) < 1);
      assert.ok(card.metadata.every((row) => row.right <= card.right + 0.5));
      if (width < 900)
        assert.ok(
          card.metadata.every((row) => row.valueTop >= row.labelBottom),
        );
    }
  }
  await page.evaluate(() => {
    document.body.style.zoom = "2";
  });
  await page
    .locator(".capture-result")
    .last()
    .evaluate((element) => element.scrollIntoView({ block: "end" }));
  const enlarged = await readGeometry();
  assert.ok(
    enlarged.every(
      (card) =>
        card.left >= -0.5 &&
        card.right <= 360.5 &&
        card.metadata.every((row) => row.right <= card.right + 0.5),
    ),
    JSON.stringify(enlarged),
  );
  await page.evaluate(() => {
    document.body.style.zoom = "";
  });
  const rows = (await readGeometry()).flatMap((card) => card.metadata);
  assert.deepEqual(
    rows.slice(0, 3).map((row) => row.value),
    [unusual.pageUrl, unusual.pageTitle, unusual.cssSelector],
  );
  assert.deepEqual(
    rows.slice(3).map((row) => row.value),
    ["", "", ""],
  );
  assert.equal(await page.locator(".capture-result script").count(), 0);
  assert.equal(await page.locator(".capture-result input").count(), 0);

  await fixture({ captures: [{ attachmentId: ids[0] }] });
  await openPrompt();
  await page.getByText("Capture 1 of 1", { exact: true }).waitFor();
  assert.equal(await page.locator(".capture-metadata").count(), 0);

  for (const [mode, message, retry] of [
    [
      "error",
      "Could not retrieve this capture. Check your connection and retry.",
      true,
    ],
    ["empty", "Capture bytes are unavailable.", true],
    ["unsafe", "Capture bytes are unavailable.", true],
    ["unavailable", "Capture is unavailable.", true],
    ["broken", "Could not load this capture. Retry to renew access.", true],
    [
      "denied",
      "Permission denied. Use Lane Settings to sign in or reconnect this account.",
      false,
    ],
    ["deleted", "This capture was deleted.", false],
  ]) {
    await fixture({ captures: [captures[0]], replies: { [ids[0]]: mode } });
    await openPrompt();
    await page.getByText(message, { exact: true }).waitFor();
    assert.equal(await button("Retry").count(), retry ? 1 : 0, mode);
    assert.equal(await page.locator(".capture-result").count(), 1);
    assert.equal(
      await page.locator(".capture-position").textContent(),
      "Capture 1 of 1",
    );
    if (mode === "error") {
      const count = (await fixture()).requests.length;
      await button("Retry").focus();
      const read = page.waitForResponse((response) =>
        response.url().endsWith("/GetAttachment"),
      );
      await page.keyboard.press("Space");
      await read;
      assert.equal((await fixture()).requests.length, count + 1);
    }
  }
  await fixture({ captures: [] });
  await openPrompt();
  assert.equal(await page.locator(".capture-result").count(), 0);
  await button("New tab").click();
  await button("RealQA").click();
  await page
    .getByText("Capture on desktop. View saved captures in Lane.", {
      exact: true,
    })
    .waitFor();
  assert.equal(await button("Full screen").count(), 0);
  await button("Open Lane").click();
  assert.deepEqual(errors, []);
  console.log(
    "Saved capture browser checks passed without validation screenshots.",
  );
} finally {
  await browser.close();
  server.kill();
}
