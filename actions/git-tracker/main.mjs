// SPDX-License-Identifier: Apache-2.0
import { readFile, appendFile } from "node:fs/promises";
import { load, FAILSAFE_SCHEMA } from "js-yaml";

// Quote numeric tokens before parsing. This preserves their original decimal
// spelling, including integers beyond Number.MAX_SAFE_INTEGER and exponents.
export function parseMeasurements(text) {
  const quoted = text.replace(
    /"(?:[^"\\]|\\.)*"|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/gu,
    (token) => (token.startsWith('"') ? token : JSON.stringify(token)),
  );
  const data = JSON.parse(quoted);
  if (!Array.isArray(data.rows))
    throw new Error("Measurements require a rows array");
  for (const row of data.rows) {
    if (!row.id || !row.values || Array.isArray(row.values))
      throw new Error("Each row requires an id and values");
    for (const value of Object.values(row.values)) {
      if (
        typeof value !== "string" ||
        !/^-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?$/u.test(value)
      )
        throw new Error("Measurements must contain finite decimal values");
    }
  }
  return data.rows;
}
export function parseConfiguration(text) {
  const data = load(text, { schema: FAILSAFE_SCHEMA });
  if (!data?.id || !data.name || !Array.isArray(data.columns))
    throw new Error("Dataset id, name and columns are required");
  const directions = {
    lower: "DIRECTION_LOWER",
    higher: "DIRECTION_HIGHER",
    neutral: "DIRECTION_NEUTRAL",
  };
  return {
    id: data.id,
    name: data.name,
    columns: data.columns.map((c) => {
      if (!directions[c.direction])
        throw new Error("Column direction must be lower, higher or neutral");
      return {
        id: c.id,
        name: c.name,
        unit: c.unit,
        direction: directions[c.direction],
        absoluteThreshold: c.absoluteThreshold,
        percentageThreshold: c.percentageThreshold,
      };
    }),
  };
}
export class ApiFailure extends Error {
  constructor(message, transient = true) {
    super(message);
    this.transient = transient;
  }
}
export async function rpc(
  origin,
  method,
  body,
  authorization,
  {
    request = fetch,
    sleep = (ms) => new Promise((r) => setTimeout(r, ms)),
  } = {},
) {
  for (let attempt = 0; ; attempt++) {
    let response;
    try {
      response = await request(
        `${origin}/gittracker.v1.GitTrackerService/${method}`,
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Connect-Protocol-Version": "1",
            Authorization: authorization,
          },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(30000),
        },
      );
    } catch {
      if (attempt < 3) {
        await sleep(500 * 2 ** attempt);
        continue;
      }
      throw new ApiFailure("API network failure after three retries");
    }
    if (!response.ok) {
      if ((response.status >= 500 || response.status === 429) && attempt < 3) {
        await sleep(500 * 2 ** attempt);
        continue;
      }
      throw new ApiFailure(
        `API failed (${response.status}); correlation ID: ${response.headers.get("X-Correlation-Id") ?? "unavailable"}`,
        response.status >= 500 || response.status === 429,
      );
    }
    return response.json();
  }
}
const input = (name) => process.env[`INPUT_${name.toUpperCase()}`] ?? "";
const escapeCommand = (text) =>
  String(text)
    .replaceAll("%", "%25")
    .replaceAll("\r", "%0D")
    .replaceAll("\n", "%0A");
export async function main() {
  const warn = (text) => console.log(`::warning::${escapeCommand(text)}`);
  const api = input("api-url") || "https://git-tracker.api.delino.io";
  const url = new URL(api);
  if (
    url.protocol !== "https:" &&
    !(
      url.protocol === "http:" &&
      ["127.0.0.1", "localhost"].includes(url.hostname)
    )
  )
    throw new Error("API URL must use HTTPS");
  const event = JSON.parse(
    await readFile(process.env.GITHUB_EVENT_PATH, "utf8"),
  );
  const pr = event.pull_request;
  const executionId =
    input("execution-id") ||
    `${process.env.GITHUB_REPOSITORY_ID}/${process.env.GITHUB_RUN_ID}/${process.env.GITHUB_RUN_ATTEMPT}`;
  if (!/^[A-Za-z0-9_./-]{1,200}$/u.test(executionId))
    throw new Error("Invalid execution ID");
  const context = {
    repositoryId: input("repository-id"),
    executionId,
    commit: pr?.head.sha ?? process.env.GITHUB_SHA,
    branch: pr?.base.ref ?? process.env.GITHUB_REF_NAME,
    pullRequest: pr ? String(event.number) : "0",
    baselineCommit: input("baseline"),
  };
  const details = `https://git-tracker.delino.io/?repository=${encodeURIComponent(context.repositoryId)}&execution=${encodeURIComponent(executionId)}`;
  if (process.env.GITHUB_OUTPUT)
    await appendFile(
      process.env.GITHUB_OUTPUT,
      `execution-id=${executionId}\ndetails-url=${details}\n`,
    );
  let authorization;
  try {
    const token = input("token");
    if (token) {
      console.log(`::add-mask::${escapeCommand(token)}`);
      authorization = `Bearer ${token}`;
    } else {
      if (
        !process.env.ACTIONS_ID_TOKEN_REQUEST_URL ||
        !process.env.ACTIONS_ID_TOKEN_REQUEST_TOKEN
      )
        throw new Error("Grant id-token: write or provide a repository token");
      const endpoint = new URL(process.env.ACTIONS_ID_TOKEN_REQUEST_URL);
      endpoint.searchParams.set(
        "audience",
        "https://git-tracker.api.delino.io",
      );
      const response = await fetch(endpoint, {
        headers: {
          Authorization: `Bearer ${process.env.ACTIONS_ID_TOKEN_REQUEST_TOKEN}`,
        },
        signal: AbortSignal.timeout(30000),
      });
      if (!response.ok) throw new Error("Actions identity request failed");
      const result = await response.json();
      if (!result.value) throw new Error("Actions identity is unavailable");
      console.log(`::add-mask::${escapeCommand(result.value)}`);
      authorization = `Actions ${result.value}`;
    }
    const operation = input("operation") || "upload";
    let result;
    if (operation === "complete")
      result = await rpc(api, "Complete", { context }, authorization);
    else if (operation === "upload") {
      const dataset = parseConfiguration(
        await readFile(input("configuration"), "utf8"),
      );
      const rows = parseMeasurements(
        await readFile(input("measurements"), "utf8"),
      );
      result = await rpc(
        api,
        "Upload",
        { context, dataset, rows },
        authorization,
      );
      const ids = new Set(rows.map((row) => row.id));
      // The server returns cumulative comparisons. This job gates only its rows.
      const regressions = (result.comparisons ?? []).filter(
        (c) =>
          c.datasetId === dataset.id &&
          ids.has(c.rowId) &&
          c.status === "COMPARISON_STATUS_REGRESSION",
      );
      console.log(
        `Git Tracker: ${regressions.length} regressions in this job. ${details}`,
      );
      if (process.env.GITHUB_STEP_SUMMARY)
        await appendFile(
          process.env.GITHUB_STEP_SUMMARY,
          `## Git Tracker\n\n${regressions.length} regressions in this job. [Execution details](${details})\n`,
        );
      if (regressions.length && input("fail-on-regression") === "true")
        process.exitCode = 1;
    } else throw new Error("operation must be upload or complete");
    for (const warning of result.warnings ?? []) warn(warning);
    if (operation === "complete") {
      console.log(`Measurement execution complete. ${details}`);
      if (process.env.GITHUB_STEP_SUMMARY)
        await appendFile(
          process.env.GITHUB_STEP_SUMMARY,
          `## Git Tracker\n\nMeasurement execution complete. [Details](${details})\n`,
        );
    }
  } catch (error) {
    if (
      error instanceof ApiFailure &&
      error.transient &&
      input("warn-on-api-error") === "true"
    )
      warn(error.message);
    else throw error;
  }
}
if (
  process.env.GITHUB_ACTIONS === "true" &&
  !process.env.GIT_TRACKER_ACTION_TEST
)
  main().catch((error) => {
    console.error(`::error::${escapeCommand(error.message)}`);
    process.exitCode = 1;
  });
