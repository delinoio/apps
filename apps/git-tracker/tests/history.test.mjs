import { test } from "node:test";
import assert from "node:assert/strict";
import {
  points,
  representatives,
  readable,
  graphValue,
} from "../src/history.mjs";
const execution = (commit, id, order, state = 2) => ({
  context: { commit, executionId: id },
  registrationOrder: order,
  state,
  measurements: [
    {
      dataset: { id: "size", columns: [{ id: "bytes", unit: "B" }] },
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

test("graph coordinates normalize historical units and reject incompatible kinds", () => {
  const old = execution("a", "old", "1");
  const recent = execution("b", "new", "2");
  recent.measurements[0].dataset.columns[0].unit = "KiB";
  recent.measurements[0].rows[0].values.bytes = "1";
  const history = points(["b", "a"], [recent, old], "size", "bytes", "file");
  assert.deepEqual(
    history.map((p) => [p.value, p.unit]),
    [
      ["1024", "B"],
      ["1", "KiB"],
    ],
  );
  assert.deepEqual(
    history.map((p) => graphValue(p.value, p.unit, "KiB")),
    [1, 1],
  );
  assert.equal(graphValue("1000", "ms", "s"), 1);
  assert.equal(graphValue("1", "MiB/s", "KiB/s"), 1024);
  assert.ok(Number.isNaN(graphValue("1", "ms", "B")));
});
