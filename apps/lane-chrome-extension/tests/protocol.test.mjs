// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  isSelectablePageUrl,
  safeMetadata,
  visibleRect,
} from "../src/protocol.js";
import { readFile } from "node:fs/promises";
test("clips visible bounds without scrolling or stitching", () => {
  assert.deepEqual(
    visibleRect(
      { x: -10, y: 20, width: 100, height: 100 },
      { width: 50, height: 70 },
    ),
    { x: 0, y: 20, width: 50, height: 50, clipped: true },
  );
  assert.throws(
    () =>
      visibleRect(
        { x: 100, y: 0, width: 20, height: 20 },
        { width: 50, height: 50 },
      ),
    /ELEMENT_NOT_VISIBLE/,
  );
  assert.throws(
    () =>
      visibleRect(
        { x: NaN, y: 0, width: 20, height: 20 },
        { width: 50, height: 50 },
      ),
    /ELEMENT_NOT_VISIBLE/,
  );
});
test("metadata stays literal and rejects executable or credential URLs", () => {
  const value = {
    pageUrl: "https://example.com/?token=review-me#part",
    pageTitle: "<script>alert(1)</script>",
    cssSelector: "body > #target",
  };
  assert.deepEqual(safeMetadata(value), value);
  for (const pageUrl of [
    "javascript:alert(1)",
    "file:///secret",
    "https://user:password@example.com",
  ])
    assert.throws(() => safeMetadata({ ...value, pageUrl }));
});
test("manifest excludes incognito, broad permissions and external messages", async () => {
  const manifest = JSON.parse(
    await readFile(new URL("../src/manifest.json", import.meta.url), "utf8"),
  );
  assert.equal(manifest.incognito, "not_allowed");
  assert.equal(manifest.host_permissions, undefined);
  assert.equal(manifest.externally_connectable, undefined);
  assert.deepEqual(manifest.permissions, [
    "activeTab",
    "scripting",
    "nativeMessaging",
    "storage",
  ]);
  assert.equal(manifest.commands["select-element"].suggested_key, undefined);
});
test("Web Store exclusion matches exact hosts and path boundaries only", () => {
  for (const url of [
    "https://chromewebstore.google.com/",
    "https://chromewebstore.google.com/detail/item/abc?hl=en#reviews",
    "https://CHROMEWEBSTORE.google.com/category/extensions",
    "https://chromewebstore.google.com./",
    "https://chrome.google.com/webstore",
    "https://chrome.google.com/webstore?hl=en",
    "https://chrome.google.com/webstore#top",
    "https://chrome.google.com/webstore/",
    "https://chrome.google.com/webstore/detail/item",
    "https://chrome.google.com/webstore/detail/item?hl=en#reviews",
  ])
    assert.equal(isSelectablePageUrl(url), false, url);
  for (const url of [
    "https://example.com/test",
    "http://example.com/test",
    "https://chrome.google.com/webstorefront",
    "https://chrome.google.com/webstore-extra/detail",
    "https://chrome.google.com/",
    "https://chrome.google.com.example.org/webstore",
    "https://chromewebstore.google.com.example.org/test",
    "https://chrome-google-com.example.org/webstore",
    "https://chromewebstore-google-com.example.org/test",
    "https://example.org/?next=https://chromewebstore.google.com/",
    "https://example.org/chrome.google.com/webstore",
  ])
    assert.equal(isSelectablePageUrl(url), true, url);
  for (const url of [
    "chrome://settings",
    "file:///tmp/example.html",
    "javascript:alert(1)",
    "not a url",
    "",
    undefined,
    null,
  ])
    assert.equal(isSelectablePageUrl(url), false, String(url));
});
