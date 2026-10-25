// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { create, fromJson, toJson } from "@bufbuild/protobuf";
import {
  SaveReceiverRequestSchema,
  SyncMutationSchema,
  TaskSchema,
  SetProgressRequestSchema,
  EntityVersionSchema,
} from "@delino/protos/lane/v1/lane_pb";
import { toUI, fromUI } from "../src/serializable.ts";
import { progressOptions } from "../src/model.ts";
test("uint64 larger than MAX_SAFE_INTEGER remains exact through UI and schema restoration", () => {
  const value = 18446744073709551615n;
  const original = create(SaveReceiverRequestSchema, {
    expectedRevision: value,
  });
  const ui = toUI(original);
  assert.equal(ui.expectedRevision, "18446744073709551615");
  assert.doesNotThrow(() => JSON.stringify(ui));
  const restored = fromUI(SaveReceiverRequestSchema, ui);
  assert.equal(restored.expectedRevision, value);
  assert.equal(
    toJson(SaveReceiverRequestSchema, restored).expectedRevision,
    "18446744073709551615",
  );
});
test("every shared sync fixture preserves immutable snapshots and prompt restoration markers through UI boundary", async () => {
  const fixtures = JSON.parse(
    await readFile(
      new URL("../../../protos/lane/v1/fixtures/sync.json", import.meta.url),
      "utf8",
    ),
  );
  let count = 0;
  for (const c of fixtures.cases) {
    for (const mutation of c.input.mutations || []) {
      const original = fromJson(SyncMutationSchema, mutation);
      assert.deepEqual(
        toJson(SyncMutationSchema, fromUI(SyncMutationSchema, toUI(original))),
        toJson(SyncMutationSchema, original),
        c.id,
      );
      count++;
    }
    for (const version of c.input.initial || []) {
      const original = fromJson(EntityVersionSchema, version);
      assert.deepEqual(
        toJson(
          EntityVersionSchema,
          fromUI(EntityVersionSchema, toUI(original)),
        ),
        toJson(EntityVersionSchema, original),
        c.id,
      );
    }
  }
  assert.ok(count > 10);
});
test("shared 25 transition vectors all have selectable GUI progress targets", async () => {
  const fixtures = JSON.parse(
    await readFile(
      new URL("../../../protos/lane/v1/fixtures/tasks.json", import.meta.url),
      "utf8",
    ),
  );
  const cases = fixtures.cases.filter((c: any) => c.id.startsWith("progress-"));
  assert.equal(cases.length, 25);
  for (const c of cases) {
    const request = fromJson(SetProgressRequestSchema, c.input.request);
    const expected = fromJson(TaskSchema, c.expected.task);
    assert.ok(progressOptions.some(([value]) => value === request.progress));
    assert.equal(request.progress, expected.progress);
    assert.deepEqual(
      toJson(
        SetProgressRequestSchema,
        fromUI(SetProgressRequestSchema, toUI(request)),
      ),
      toJson(SetProgressRequestSchema, request),
    );
  }
});
