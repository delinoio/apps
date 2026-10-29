// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { CompletedSelection } from "@repo/protos/lane/v1/lane_pb";
import { completedFilter, completedOptions } from "../src/completedDisplay.ts";

test("completed choices use the approved order and labels", () => {
  assert.deepEqual(
    completedOptions.map(([, label]) => label),
    ["None", "Last 24 hours", "Last 7 days", "Last 30 days", "All"],
  );
});

test("duration cutoffs retain the exact UTC microsecond instant", () => {
  const now = 1_800_000_000_123_456n;
  for (const [selection, days] of [
    [CompletedSelection.LAST_24_HOURS, 1n],
    [CompletedSelection.LAST_7_DAYS, 7n],
    [CompletedSelection.LAST_30_DAYS, 30n],
  ] as const) {
    const result = completedFilter(selection, now);
    assert.ok(result.cutoff);
    const expected = now - days * 24n * 60n * 60n * 1_000_000n;
    assert.equal(
      BigInt(result.cutoff.seconds) * 1_000_000n +
        BigInt(result.cutoff.nanos) / 1_000n,
      expected,
    );
  }
  assert.deepEqual(completedFilter(CompletedSelection.NONE, now), {
    selection: CompletedSelection.NONE,
  });
  assert.deepEqual(completedFilter(CompletedSelection.ALL, now), {
    selection: CompletedSelection.ALL,
  });
});
