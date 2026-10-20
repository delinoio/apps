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
