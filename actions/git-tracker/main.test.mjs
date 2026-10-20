// SPDX-License-Identifier: Apache-2.0
import { test } from "node:test";
import assert from "node:assert/strict";
// Import helpers without starting the Action when tests run on GitHub Actions.
process.env.GIT_TRACKER_ACTION_TEST = "1";
const { parseMeasurements, parseConfiguration, rpc } =
  await import("./main.mjs");
test("JSON and YAML decimals retain their exact value", () => {
  assert.equal(
    parseMeasurements(
      '{"rows":[{"id":"a","values":{"bytes":9007199254740993.0000000000000001}}]}',
    )[0].values.bytes,
    "9007199254740993.0000000000000001",
  );
  assert.equal(
    parseConfiguration(
      "id: size\nname: Size\ncolumns:\n  - id: bytes\n    name: Bytes\n    unit: B\n    direction: lower\n    absoluteThreshold: 0.00000000000000001",
    ).columns[0].absoluteThreshold,
    "0.00000000000000001",
  );
  assert.throws(() =>
    parseMeasurements('{"rows":[{"id":"a","values":{"x":NaN}}]}'),
  );
});
test("transient failures retry three times, permanent failures do not", async () => {
  let calls = 0;
  const request = async () => {
    calls++;
    return new Response("{}", { status: 503 });
  };
  await assert.rejects(
    rpc("https://example.test", "Upload", {}, "secret", {
      request,
      sleep: async () => {},
    }),
  );
  assert.equal(calls, 4);
  calls = 0;
  await assert.rejects(
    rpc("https://example.test", "Upload", {}, "secret", {
      request: async () => {
        calls++;
        return new Response("{}", { status: 403 });
      },
      sleep: async () => {},
    }),
  );
  assert.equal(calls, 1);
});

test("RPC origin trailing slashes preserve POST endpoint and body", async () => {
  for (const origin of ["https://api.example", "https://api.example/"]) {
    await rpc(
      origin,
      "Complete",
      { context: { executionId: "run" } },
      "Bearer fixture",
      {
        request: async (url, options) => {
          assert.equal(
            url,
            "https://api.example/gittracker.v1.GitTrackerService/Complete",
          );
          assert.equal(options.method, "POST");
          assert.equal(JSON.parse(options.body).context.executionId, "run");
          return new Response("{}", { status: 200 });
        },
      },
    );
  }
});
