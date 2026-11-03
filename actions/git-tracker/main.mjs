// SPDX-License-Identifier: Apache-2.0
import { readFile, appendFile } from "node:fs/promises";
import { load, FAILSAFE_SCHEMA } from "js-yaml";

const isPlainObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value);
// Name an input item in validation errors without echoing unbounded content.
const describe = (kind, index, item) =>
  typeof item?.id === "string" && item.id
    ? `${kind}[${index}] (id ${JSON.stringify(item.id.slice(0, 64))})`
    : `${kind}[${index}]`;

// Quote numeric tokens before parsing. This preserves their original decimal
// spelling, including integers beyond Number.MAX_SAFE_INTEGER and exponents.
export function parseMeasurements(text) {
  const quoted = text.replace(
    /"(?:[^"\\]|\\.)*"|-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/gu,
    (token) => (token.startsWith('"') ? token : JSON.stringify(token)),
  );
  const data = JSON.parse(quoted);
  // Quoting turns every number into a string, so the JSON types of ids and
  // value containers are checked on a second, unmodified parse of the same
  // text. Both parses have the same shape; only number tokens differ.
  const typed = JSON.parse(text);
  if (!Array.isArray(data?.rows))
    throw new Error("Measurements require a rows array");
  data.rows.forEach((row, index) => {
    const original = typed.rows[index];
    if (!isPlainObject(original))
      throw new Error(
        `Measurement ${describe("rows", index, null)} must be an object`,
      );
    if (typeof original.id !== "string" || !original.id)
      throw new Error(
        `Measurement ${describe("rows", index, null)} requires an id that is a non-empty string`,
      );
    const label = describe("rows", index, original);
    if (!isPlainObject(original.values))
      throw new Error(
        `Measurement ${label} requires values that are an object`,
      );
    for (const value of Object.values(row.values)) {
      if (
        typeof value !== "string" ||
        !/^-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?$/u.test(value)
      )
        throw new Error(
          `Measurement ${label} must contain finite decimal values`,
        );
    }
  });
  return data.rows;
}
export function parseConfiguration(text) {
  const data = load(text, { schema: FAILSAFE_SCHEMA });
  if (!isPlainObject(data)) throw new Error("Configuration must be a mapping");
  // The FAILSAFE schema yields only strings, sequences and mappings, so a
  // type check rejects sequences and mappings where a scalar is required.
  for (const key of ["id", "name"])
    if (typeof data[key] !== "string" || !data[key])
      throw new Error(`Dataset ${key} must be a non-empty string`);
  if (!Array.isArray(data.columns))
    throw new Error("Dataset id, name and columns are required");
  // Any other column key is a typo such as "absoluteTreshold". Dropping it
  // would silently remove a regression threshold, so reject it instead.
  const columnKeys = new Set([
    "id",
    "name",
    "unit",
    "direction",
    "absoluteThreshold",
    "percentageThreshold",
  ]);
  const directions = {
    lower: "DIRECTION_LOWER",
    higher: "DIRECTION_HIGHER",
    neutral: "DIRECTION_NEUTRAL",
  };
  return {
    id: data.id,
    name: data.name,
    columns: data.columns.map((c, index) => {
      if (!isPlainObject(c))
        throw new Error(
          `Column ${describe("columns", index, null)} must be a mapping`,
        );
      const label = describe("columns", index, c);
      if (typeof c.id !== "string" || !c.id)
        throw new Error(
          `Column ${describe("columns", index, null)} requires an id that is a non-empty string`,
        );
      for (const key of ["name", "unit"])
        if (c[key] !== undefined && typeof c[key] !== "string")
          throw new Error(`Column ${label} ${key} must be a string`);
      if (typeof c.direction !== "string")
        throw new Error(`Column ${label} direction must be a string`);
      const unknownKey = Object.keys(c).find((key) => !columnKeys.has(key));
      if (unknownKey !== undefined)
        throw new Error(
          `Column ${label} has unknown key ${JSON.stringify(unknownKey.slice(0, 64))}`,
        );
      // Own keys only: inherited names such as constructor or __proto__ must
      // not pass as directions.
      if (!Object.hasOwn(directions, c.direction))
        throw new Error(
          `Column ${label} direction must be lower, higher or neutral`,
        );
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
const ERROR_BODY_LIMIT = 8192;
const ERROR_MESSAGE_LIMIT = 500;
// Read the Connect error code and message of a failed response for the
// failure text. The body is capped, parsed defensively and reduced to one
// bounded line. Any problem yields an empty string, so the status-only text
// stays the fallback. Only the server's error body is read: request bodies,
// measurement values and credentials never reach this text.
async function errorDetail(response) {
  try {
    const reader = response.body?.getReader();
    if (!reader) return "";
    const decoder = new TextDecoder();
    let text = "";
    let size = 0;
    while (size < ERROR_BODY_LIMIT) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.length;
      text += decoder.decode(value, { stream: true });
    }
    await reader.cancel().catch(() => {});
    // A body beyond the cap is not a normal Connect error; ignore it.
    if (size > ERROR_BODY_LIMIT) return "";
    const body = JSON.parse(text);
    const clean = (value) =>
      String(value)
        .replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ")
        .replace(/\s+/gu, " ")
        .trim();
    const code =
      typeof body?.code === "string" && /^[a-z_]{1,40}$/u.test(body.code)
        ? body.code
        : "";
    let message = typeof body?.message === "string" ? clean(body.message) : "";
    if (message.length > ERROR_MESSAGE_LIMIT)
      message = `${message.slice(0, ERROR_MESSAGE_LIMIT)}...`;
    if (!message) return "";
    return code ? `${code}: ${message}` : message;
  } catch {
    return "";
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
        new URL(`/gittracker.v1.GitTrackerService/${method}`, origin).href,
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
      if (
        (response.status >= 500 ||
          response.status === 408 ||
          response.status === 429) &&
        attempt < 3
      ) {
        await sleep(500 * 2 ** attempt);
        continue;
      }
      const detail = await errorDetail(response);
      throw new ApiFailure(
        `API failed (${response.status}); correlation ID: ${response.headers.get("X-Correlation-Id") ?? "unavailable"}${detail ? `; ${detail}` : ""}`,
        response.status >= 500 ||
          response.status === 408 ||
          response.status === 429,
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
  const defaultExecutionId = `${process.env.GITHUB_REPOSITORY_ID}/${process.env.GITHUB_RUN_ID}/${process.env.GITHUB_RUN_ATTEMPT}`;
  const executionId = input("execution-id") || defaultExecutionId;
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
    const operation = input("operation") || "upload";
    // Validate the input files before acquiring credentials, so invalid
    // input never causes an identity request or an API request.
    let dataset;
    let rows;
    if (operation === "upload") {
      dataset = parseConfiguration(
        await readFile(input("configuration"), "utf8"),
      );
      rows = parseMeasurements(await readFile(input("measurements"), "utf8"));
    }
    const token = input("token");
    if (token) {
      console.log(`::add-mask::${escapeCommand(token)}`);
      authorization = `Bearer ${token}`;
    } else {
      // The API accepts an Actions identity only for the default execution
      // ID, so a different one can never authenticate. Fail before any
      // identity or API request. An explicit value equal to the default is
      // the same identity and stays valid.
      if (executionId !== defaultExecutionId)
        throw new Error(
          "A custom execution-id requires a repository token; provide the token input or omit execution-id to use the default Actions identity",
        );
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
    let result;
    if (operation === "complete")
      result = await rpc(api, "Complete", { context }, authorization);
    else if (operation === "upload") {
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
