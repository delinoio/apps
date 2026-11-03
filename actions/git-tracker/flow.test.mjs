// SPDX-License-Identifier: Apache-2.0
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtemp, writeFile, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

test("Action upload gating, cumulative rows, summary and explicit completion", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "git-tracker-action-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, "event.json"), "{}");
  await writeFile(
    join(directory, "dataset.yaml"),
    "id: size\nname: Size\ncolumns:\n  - id: bytes\n    name: Bytes\n    unit: B\n    direction: lower\n    absoluteThreshold: 0\n",
  );
  await writeFile(
    join(directory, "rows.json"),
    '{"rows":[{"id":"file","values":{"bytes":9007199254740993.0000000000000001}}]}',
  );
  let ownRegression = true;
  let failure = false;
  const requests = [];
  const server = createServer(async (request, response) => {
    let raw = "";
    for await (const chunk of request) raw += chunk;
    const body = JSON.parse(raw);
    requests.push({ url: request.url, body });
    response.setHeader("Content-Type", "application/json");
    if (failure) {
      response.writeHead(403);
      response.end('{"code":"permission_denied"}');
      return;
    }
    response.end(
      JSON.stringify({
        execution: { context: body.context },
        comparisons: [
          {
            datasetId: "size",
            rowId: ownRegression ? "file" : "other-job",
            status: "COMPARISON_STATUS_REGRESSION",
          },
        ],
        warnings: ["PR comment could not be updated"],
      }),
    );
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => server.close());
  const run = (extra) =>
    new Promise((resolve, reject) => {
      const child = spawn(
        process.execPath,
        [fileURLToPath(new URL("./main.mjs", import.meta.url))],
        {
          env: {
            PATH: process.env.PATH,
            GITHUB_ACTIONS: "true",
            GITHUB_EVENT_PATH: join(directory, "event.json"),
            GITHUB_SHA: "a".repeat(40),
            GITHUB_REF_NAME: "main",
            GITHUB_REPOSITORY_ID: "42",
            GITHUB_RUN_ID: "123",
            GITHUB_RUN_ATTEMPT: "1",
            GITHUB_OUTPUT: join(directory, "output"),
            GITHUB_STEP_SUMMARY: join(directory, "summary"),
            "INPUT_API-URL": `http://127.0.0.1:${server.address().port}`,
            "INPUT_REPOSITORY-ID": "42",
            INPUT_TOKEN: "fixture-token",
            INPUT_CONFIGURATION: join(directory, "dataset.yaml"),
            INPUT_MEASUREMENTS: join(directory, "rows.json"),
            ...extra,
          },
          stdio: ["ignore", "pipe", "pipe"],
        },
      );
      let log = "";
      child.stdout.on("data", (chunk) => (log += chunk));
      child.stderr.on("data", (chunk) => (log += chunk));
      child.on("error", reject);
      child.on("exit", (code) => resolve({ code, log }));
    });
  let result = await run({});
  assert.equal(result.code, 0);
  assert.match(result.log, /1 regressions/);
  assert.match(result.log, /::warning::PR comment/);
  assert.equal(
    requests[0].body.rows[0].values.bytes,
    "9007199254740993.0000000000000001",
  );
  assert.match(
    await readFile(join(directory, "summary"), "utf8"),
    /Execution details/,
  );
  result = await run({ "INPUT_FAIL-ON-REGRESSION": "true" });
  assert.equal(result.code, 1);
  ownRegression = false;
  result = await run({ "INPUT_FAIL-ON-REGRESSION": "true" });
  assert.equal(result.code, 0);
  result = await run({ INPUT_OPERATION: "complete" });
  assert.equal(result.code, 0);
  assert.equal(
    requests.at(-1).url,
    "/gittracker.v1.GitTrackerService/Complete",
  );
  assert.deepEqual(Object.keys(requests.at(-1).body), ["context"]);
  failure = true;
  result = await run({ "INPUT_WARN-ON-API-ERROR": "true" });
  assert.equal(
    result.code,
    1,
    "permission failures must not be downgraded to warnings",
  );
});

test("malformed configuration and measurements fail before any request", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "git-tracker-action-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, "event.json"), "{}");
  const validConfiguration =
    "id: size\nname: Size\ncolumns:\n  - id: bytes\n    name: Bytes\n    unit: B\n    direction: lower\n";
  const validRows =
    '{"rows":[{"id":"file","values":{"bytes":9007199254740993.0000000000000001}}]}';
  const requests = [];
  const server = createServer(async (request, response) => {
    let raw = "";
    for await (const chunk of request) raw += chunk;
    requests.push(JSON.parse(raw));
    response.setHeader("Content-Type", "application/json");
    response.end("{}");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => server.close());
  const run = async (configuration, rows) => {
    await writeFile(join(directory, "dataset.yaml"), configuration);
    await writeFile(join(directory, "rows.json"), rows);
    return new Promise((resolve, reject) => {
      const child = spawn(
        process.execPath,
        [fileURLToPath(new URL("./main.mjs", import.meta.url))],
        {
          env: {
            PATH: process.env.PATH,
            GITHUB_ACTIONS: "true",
            GITHUB_EVENT_PATH: join(directory, "event.json"),
            GITHUB_SHA: "a".repeat(40),
            GITHUB_REF_NAME: "main",
            GITHUB_REPOSITORY_ID: "42",
            GITHUB_RUN_ID: "123",
            GITHUB_RUN_ATTEMPT: "1",
            "INPUT_API-URL": `http://127.0.0.1:${server.address().port}`,
            "INPUT_REPOSITORY-ID": "42",
            INPUT_TOKEN: "fixture-token",
            INPUT_CONFIGURATION: join(directory, "dataset.yaml"),
            INPUT_MEASUREMENTS: join(directory, "rows.json"),
          },
          stdio: ["ignore", "pipe", "pipe"],
        },
      );
      let log = "";
      child.stdout.on("data", (chunk) => (log += chunk));
      child.stderr.on("data", (chunk) => (log += chunk));
      child.on("error", reject);
      child.on("exit", (code) => resolve({ code, log }));
    });
  };
  const malformed = [
    [
      validConfiguration,
      '{"rows":[{"id":"file","values":123}]}',
      /rows\[0\] \(id "file"\)/u,
    ],
    [
      validConfiguration,
      '{"rows":[{"id":7,"values":{"bytes":1}}]}',
      /rows\[0\].*id/u,
    ],
    [
      validConfiguration.replace(
        "direction: lower",
        "direction:\n      - lower",
      ),
      validRows,
      /columns\[0\] \(id "bytes"\).*direction/u,
    ],
    [
      validConfiguration.replace("unit: B", "unit:\n      - B"),
      validRows,
      /columns\[0\] \(id "bytes"\).*unit/u,
    ],
    [
      `${validConfiguration}    absoluteTreshold: 0\n`,
      validRows,
      /columns\[0\] \(id "bytes"\) has unknown key "absoluteTreshold"/u,
    ],
  ];
  for (const [configuration, rows, message] of malformed) {
    const result = await run(configuration, rows);
    assert.equal(result.code, 1, result.log);
    assert.match(result.log, message);
  }
  assert.equal(requests.length, 0, "no request may be sent for invalid input");
  const result = await run(validConfiguration, validRows);
  assert.equal(result.code, 0, result.log);
  assert.equal(requests.length, 1);
  assert.deepEqual(requests[0].dataset, {
    id: "size",
    name: "Size",
    columns: [
      { id: "bytes", name: "Bytes", unit: "B", direction: "DIRECTION_LOWER" },
    ],
  });
  assert.deepEqual(requests[0].rows, [
    { id: "file", values: { bytes: "9007199254740993.0000000000000001" } },
  ]);
  // The corrected spelling uploads and keeps the threshold.
  const corrected = await run(
    `${validConfiguration}    absoluteThreshold: 0\n`,
    validRows,
  );
  assert.equal(corrected.code, 0, corrected.log);
  assert.equal(requests.length, 2);
  assert.equal(requests[1].dataset.columns[0].absoluteThreshold, "0");
});

test("custom execution IDs require a repository token before any request", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "git-tracker-action-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await writeFile(join(directory, "event.json"), "{}");
  await writeFile(
    join(directory, "dataset.yaml"),
    "id: size\nname: Size\ncolumns:\n  - id: bytes\n    direction: lower\n",
  );
  await writeFile(
    join(directory, "rows.json"),
    '{"rows":[{"id":"file","values":{"bytes":1}}]}',
  );
  const apiRequests = [];
  const identityRequests = [];
  const server = createServer(async (request, response) => {
    let raw = "";
    for await (const chunk of request) raw += chunk;
    response.setHeader("Content-Type", "application/json");
    if (request.url.startsWith("/identity")) {
      identityRequests.push(request.url);
      response.end('{"value":"oidc-fixture"}');
      return;
    }
    apiRequests.push({
      authorization: request.headers.authorization,
      body: JSON.parse(raw),
    });
    response.end("{}");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => server.close());
  const origin = `http://127.0.0.1:${server.address().port}`;
  const run = (extra) =>
    new Promise((resolve, reject) => {
      const child = spawn(
        process.execPath,
        [fileURLToPath(new URL("./main.mjs", import.meta.url))],
        {
          env: {
            PATH: process.env.PATH,
            GITHUB_ACTIONS: "true",
            GITHUB_EVENT_PATH: join(directory, "event.json"),
            GITHUB_SHA: "a".repeat(40),
            GITHUB_REF_NAME: "main",
            GITHUB_REPOSITORY_ID: "42",
            GITHUB_RUN_ID: "123",
            GITHUB_RUN_ATTEMPT: "1",
            ACTIONS_ID_TOKEN_REQUEST_URL: `${origin}/identity?fixture=1`,
            ACTIONS_ID_TOKEN_REQUEST_TOKEN: "request-fixture",
            "INPUT_API-URL": origin,
            "INPUT_REPOSITORY-ID": "42",
            INPUT_CONFIGURATION: join(directory, "dataset.yaml"),
            INPUT_MEASUREMENTS: join(directory, "rows.json"),
            ...extra,
          },
          stdio: ["ignore", "pipe", "pipe"],
        },
      );
      let log = "";
      child.stdout.on("data", (chunk) => (log += chunk));
      child.stderr.on("data", (chunk) => (log += chunk));
      child.on("error", reject);
      child.on("exit", (code) => resolve({ code, log }));
    });

  // A custom ID without a token can never authenticate with OIDC.
  for (const operation of ["upload", "complete"]) {
    const result = await run({
      "INPUT_EXECUTION-ID": "custom/run-1",
      INPUT_OPERATION: operation,
      "INPUT_WARN-ON-API-ERROR": "true",
    });
    assert.equal(result.code, 1, result.log);
    assert.match(
      result.log,
      /custom execution-id requires a repository token/u,
    );
    assert.doesNotMatch(
      result.log,
      /custom\/run-1|request-fixture|oidc-fixture/u,
    );
  }
  assert.equal(identityRequests.length, 0, "no identity request may be made");
  assert.equal(apiRequests.length, 0, "no API request may be made");

  // The default ID spelled explicitly is the same identity and still works.
  let result = await run({ "INPUT_EXECUTION-ID": "42/123/1" });
  assert.equal(result.code, 0, result.log);
  assert.equal(identityRequests.length, 1);
  assert.equal(apiRequests.length, 1);
  assert.equal(apiRequests[0].authorization, "Actions oidc-fixture");
  assert.equal(apiRequests[0].body.context.executionId, "42/123/1");

  // An omitted ID uses the default identity.
  result = await run({});
  assert.equal(result.code, 0, result.log);
  assert.equal(apiRequests.at(-1).body.context.executionId, "42/123/1");
  assert.equal(identityRequests.length, 2);

  // A custom ID with a repository token skips OIDC and is sent unchanged.
  result = await run({
    "INPUT_EXECUTION-ID": "custom/run-1",
    INPUT_TOKEN: "repo-token-fixture",
  });
  assert.equal(result.code, 0, result.log);
  assert.equal(identityRequests.length, 2, "tokens must not request OIDC");
  assert.equal(apiRequests.at(-1).authorization, "Bearer repo-token-fixture");
  assert.equal(apiRequests.at(-1).body.context.executionId, "custom/run-1");
});
