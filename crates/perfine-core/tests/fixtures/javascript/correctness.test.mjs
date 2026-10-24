// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { countPairs } from "./kernel.mjs";
test("independent examples", () => {
  for (const [values, expected] of [
    [[], 0], [[0], 0], [[1, 2, 3], 0], [[255, 0, 255, 0, 255], 4],
    [Array(100).fill(7), 4950], [Array.from({length: 256}, (_, i) => i), 0],
  ]) assert.equal(countPairs(Uint8Array.from(values)), expected);
});
