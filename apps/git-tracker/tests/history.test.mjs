import { test } from "node:test";
import assert from "node:assert/strict";
import { points, representatives, readable } from "../src/history.mjs";
const execution = (commit, id, order, state = 2) => ({
  context: { commit, executionId: id },
  registrationOrder: order,
  state,
  measurements: [
    {
      dataset: { id: "size" },
      rows: [{ id: "file", values: { bytes: "1024" } }],
    },
  ],
});
test("graphs keep first-parent positions and never fabricate missing points", () => {
  const result = points(
    ["c", "b", "a"],
    [execution("a", "old", "1"), execution("c", "new", "2")],
    "size",
    "bytes",
    "file",
  );
  assert.deepEqual(
    result.map((p) => p.index),
    [0, 2],
  );
  assert.deepEqual(
    result.map((p) => p.commit),
    ["a", "c"],
  );
});
test("latest registered completed execution wins, measuring runs stay separate", () => {
  assert.equal(
    representatives([
      execution("a", "new", "2"),
      execution("a", "old", "1"),
      execution("a", "pending", "3", 1),
    ]).get("a").context.executionId,
    "new",
  );
  assert.equal(readable("1048576", "B"), "1 MiB");
});
