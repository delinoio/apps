// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { test } from "node:test";
import { ico, markPixels, png } from "./icons.mjs";

test("renders PNG and ICO marks", () => {
  const image = png(32);
  assert.deepEqual([...image.subarray(1, 4)], [0x50, 0x4e, 0x47]);
  const icon = ico([16, 32]);
  assert.equal(icon.readUInt16LE(4), 2);
  const template = markPixels(16, { template: true });
  assert.ok(template.some((value, index) => index % 4 === 3 && value === 255));
});
