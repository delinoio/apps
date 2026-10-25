// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { safeMetadata, visibleRect } from "../src/protocol.js";
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
