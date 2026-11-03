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
const rowsJson = (row) => JSON.stringify({ rows: [row] });
const columnYaml = (column) =>
  `id: size\nname: Size\ncolumns:\n  - ${column.replaceAll("\n", "\n    ")}\n`;
const validColumn = "id: bytes\nname: Bytes\nunit: B\ndirection: lower";

test("measurement rows reject malformed id and values types", () => {
  const cases = {
    "numeric id": [
      '{"rows":[{"id":123,"values":{"a":"1"}}]}',
      /rows\[0\].*id/u,
    ],
    "array id": [
      '{"rows":[{"id":["a"],"values":{"a":"1"}}]}',
      /rows\[0\].*id/u,
    ],
    "object id": [
      '{"rows":[{"id":{"a":1},"values":{"a":"1"}}]}',
      /rows\[0\].*id/u,
    ],
    "empty id": ['{"rows":[{"id":"","values":{"a":"1"}}]}', /rows\[0\].*id/u],
    "number values": [
      '{"rows":[{"id":"a","values":123}]}',
      /rows\[0\] \(id "a"\).*values/u,
    ],
    "string values": [
      '{"rows":[{"id":"a","values":"123"}]}',
      /rows\[0\] \(id "a"\).*values/u,
    ],
    "array values": [
      '{"rows":[{"id":"a","values":["1"]}]}',
      /rows\[0\] \(id "a"\).*values/u,
    ],
    "null values": [
      '{"rows":[{"id":"a","values":null}]}',
      /rows\[0\] \(id "a"\).*values/u,
    ],
    "missing values": [
      '{"rows":[{"id":"a"}]}',
      /rows\[0\] \(id "a"\).*values/u,
    ],
    "null row": ['{"rows":[null]}', /rows\[0\].*object/u],
    "string row": ['{"rows":["a"]}', /rows\[0\].*object/u],
    "second row is named": [
      '{"rows":[{"id":"ok","values":{"a":1}},{"id":"bad","values":7}]}',
      /rows\[1\] \(id "bad"\)/u,
    ],
    "non-decimal value": [
      '{"rows":[{"id":"a","values":{"x":"abc"}}]}',
      /rows\[0\] \(id "a"\).*decimal/u,
    ],
    "null document": ["null", /rows array/u],
  };
  for (const [name, [text, message]] of Object.entries(cases))
    assert.throws(() => parseMeasurements(text), message, name);
});

test("valid measurements keep exact decimals and string decimal values", () => {
  assert.deepEqual(
    parseMeasurements(
      '{"rows":[{"id":"a","name":"A","values":{"n":1e3,"s":"2.50","big":9007199254740993,"neg":-0.5}}]}',
    ),
    [
      {
        id: "a",
        name: "A",
        values: { n: "1e3", s: "2.50", big: "9007199254740993", neg: "-0.5" },
      },
    ],
  );
});

test("dataset and column scalars must be strings", () => {
  const dataset = {
    "dataset id sequence": [
      "id:\n  - size\nname: Size\ncolumns: []\n",
      /Dataset id/u,
    ],
    "dataset id mapping": [
      "id:\n  a: b\nname: Size\ncolumns: []\n",
      /Dataset id/u,
    ],
    "dataset name sequence": [
      "id: size\nname:\n  - Size\ncolumns: []\n",
      /Dataset name/u,
    ],
    "missing dataset name": ["id: size\ncolumns: []\n", /Dataset name/u],
    "document is a sequence": ["- a\n- b\n", /mapping/u],
    "empty document": ["", /mapping/u],
    "columns mapping": ["id: size\nname: Size\ncolumns:\n  a: b\n", /columns/u],
  };
  for (const [name, [text, message]] of Object.entries(dataset))
    assert.throws(() => parseConfiguration(text), message, name);
  const column = {
    "column is a scalar": [
      "id: size\nname: Size\ncolumns:\n  - bytes\n",
      /columns\[0\].*mapping/u,
    ],
    "column id sequence": [
      columnYaml("id:\n  - bytes\nname: Bytes\ndirection: lower"),
      /columns\[0\].*id/u,
    ],
    "column id missing": [
      columnYaml("name: Bytes\ndirection: lower"),
      /columns\[0\].*id/u,
    ],
    "column name mapping": [
      columnYaml("id: bytes\nname:\n  a: b\ndirection: lower"),
      /columns\[0\] \(id "bytes"\).*name/u,
    ],
    "column unit sequence": [
      columnYaml("id: bytes\nunit:\n  - B\ndirection: lower"),
      /columns\[0\] \(id "bytes"\).*unit/u,
    ],
    "direction sequence": [
      columnYaml("id: bytes\ndirection:\n  - lower"),
      /columns\[0\] \(id "bytes"\).*direction must be a string/u,
    ],
    "direction mapping": [
      columnYaml("id: bytes\ndirection:\n  lower: x"),
      /columns\[0\] \(id "bytes"\).*direction must be a string/u,
    ],
    "direction missing": [
      columnYaml("id: bytes"),
      /columns\[0\] \(id "bytes"\).*direction/u,
    ],
    "direction unknown": [
      columnYaml("id: bytes\ndirection: sideways"),
      /columns\[0\] \(id "bytes"\).*lower, higher or neutral/u,
    ],
  };
  for (const [name, [text, message]] of Object.entries(column))
    assert.throws(() => parseConfiguration(text), message, name);
  assert.throws(
    () =>
      parseConfiguration(
        `id: size\nname: Size\ncolumns:\n  - ${validColumn.replaceAll("\n", "\n    ")}\n  - id: second\n    direction:\n      - higher\n`,
      ),
    /columns\[1\] \(id "second"\)/u,
  );
});

test("unknown column keys are rejected and named", () => {
  assert.throws(
    () => parseConfiguration(columnYaml(`${validColumn}\nabsoluteTreshold: 0`)),
    /columns\[0\] \(id "bytes"\) has unknown key "absoluteTreshold"/u,
  );
  // The first unknown key in document order is reported.
  assert.throws(
    () =>
      parseConfiguration(
        columnYaml(`${validColumn}\nzeta: 1\npercentageTreshold: 5\nalpha: 2`),
      ),
    /unknown key "zeta"/u,
  );
  // An unknown key in the second column names that column.
  assert.throws(
    () =>
      parseConfiguration(
        `id: size\nname: Size\ncolumns:\n  - id: a\n    direction: lower\n  - id: b\n    direction: lower\n    Unit: B\n`,
      ),
    /columns\[1\] \(id "b"\) has unknown key "Unit"/u,
  );
  // The correctly spelled thresholds are accepted, and omitting them is fine.
  const [column] = parseConfiguration(
    columnYaml(`${validColumn}\nabsoluteThreshold: 0\npercentageThreshold: 5`),
  ).columns;
  assert.deepEqual(
    [column.absoluteThreshold, column.percentageThreshold],
    ["0", "5"],
  );
  assert.equal(
    parseConfiguration(columnYaml(validColumn)).columns[0].absoluteThreshold,
    undefined,
  );
});

test("unknown keys outside columns are not rejected", () => {
  const dataset = parseConfiguration(
    `${columnYaml(validColumn)}description: extra\n`,
  );
  assert.equal(dataset.id, "size");
  assert.deepEqual(Object.keys(dataset), ["id", "name", "columns"]);
});

test("valid configuration produces the same dataset as before", () => {
  assert.deepEqual(
    parseConfiguration(
      `${columnYaml(`${validColumn}\nabsoluteThreshold: 0.10\npercentageThreshold: 5`)}`,
    ),
    {
      id: "size",
      name: "Size",
      columns: [
        {
          id: "bytes",
          name: "Bytes",
          unit: "B",
          direction: "DIRECTION_LOWER",
          absoluteThreshold: "0.10",
          percentageThreshold: "5",
        },
      ],
    },
  );
  // Name and unit stay optional, as before.
  assert.equal(
    parseConfiguration(columnYaml("id: bytes\ndirection: neutral")).columns[0]
      .direction,
    "DIRECTION_NEUTRAL",
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

test("HTTP 408 retries and remains a transient failure", async () => {
  let calls = 0;
  await assert.rejects(
    rpc("https://example.test", "Upload", {}, "secret", {
      request: async () => {
        calls++;
        return new Response("{}", { status: 408 });
      },
      sleep: async () => {},
    }),
    (error) => error.transient === true,
  );
  assert.equal(calls, 4);
});
