// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { inflateSync } from "node:zlib";
import {
  IOS_APP_ICON_NAME,
  ico,
  iosAppIconCatalog,
  markPixels,
  png,
} from "./icons.mjs";

test("renders PNG and ICO marks", () => {
  const image = png(32);
  assert.deepEqual([...image.subarray(1, 4)], [0x50, 0x4e, 0x47]);
  const icon = ico([16, 32]);
  assert.equal(icon.readUInt16LE(4), 2);
  const template = markPixels(16, { template: true });
  assert.ok(template.some((value, index) => index % 4 === 3 && value === 255));
});

test("the iOS app icon is an opaque 1024 PNG in the catalog project.yml uses", () => {
  const catalog = iosAppIconCatalog();
  const set = JSON.parse(catalog["AppIcon.appiconset/Contents.json"]);
  assert.deepEqual(set.images, [
    {
      filename: "AppIcon-1024.png",
      idiom: "universal",
      platform: "ios",
      size: "1024x1024",
    },
  ]);
  const image = catalog["AppIcon.appiconset/AppIcon-1024.png"];
  // IHDR: width, height, bit depth 8 and color type 2 (RGB, no alpha).
  assert.equal(image.readUInt32BE(16), 1024);
  assert.equal(image.readUInt32BE(20), 1024);
  assert.equal(image[24], 8);
  assert.equal(image[25], 2);
  // Corners carry the mark background instead of transparency.
  const length = image.readUInt32BE(33);
  const rows = inflateSync(image.subarray(41, 41 + length));
  assert.deepEqual([...rows.subarray(1, 4)], [32, 33, 36]);
  assert.equal(rows.length, 1024 * (1024 * 3 + 1));
  const project = readFileSync(
    new URL("../native/ios/project.yml", import.meta.url),
    "utf8",
  );
  assert.match(project, /- path: Generated\/Assets\.xcassets/);
  assert.match(
    project,
    new RegExp(
      `ASSETCATALOG_COMPILER_APPICON_NAME: ${IOS_APP_ICON_NAME}$`,
      "m",
    ),
  );
});
