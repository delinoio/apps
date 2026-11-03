#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-ALv2
// AnyDash capacity gate: 1,000 concurrent active users and 5,000 collector devices with
// 1-minute heartbeats and usage uploads and 5-minute external polling snapshots.
//
//   node anydash-load.mjs provision --api <url> --users users.json --out state.json
//   node anydash-load.mjs run --api <url> --state state.json [--minutes 30] [--mode gate|smoke]
//
// `run` defaults to the capacity gate mode, which refuses to start unless provisioning completed
// for at least 1,000 users and 5,000 devices and the run lasts at least 30 minutes. `--mode smoke`
// drives a smaller completed population for any duration; its result is never a gate result.
//
// `users.json` is an operator-prepared array of {"accessToken","delcoreAccessToken"} Logto
// test-account tokens for the AnyDash and Delcore resources. Tokens and device credentials stay
// in local files the operator deletes after the run; this script never prints them.
import { readFileSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";

const args = Object.fromEntries(
  process.argv
    .slice(3)
    .reduce(
      (pairs, value, index, all) =>
        index % 2 === 0
          ? [...pairs, [value.replace(/^--/u, ""), all[index + 1]]]
          : pairs,
      [],
    ),
);
const command = process.argv[2];
const api = (args.api ?? "").replace(/\/$/u, "");
const devicesPerUser = Number(args["devices-per-user"] ?? 5);
const minutes = Number(args.minutes ?? 30);
const version = args["client-version"] ?? "1.0.0";
/** Run modes: the capacity gate from the server contract, or an explicit smaller smoke run. */
const RunMode = Object.freeze({ GATE: "gate", SMOKE: "smoke" });
const mode = args.mode ?? RunMode.GATE;
if (
  !api ||
  !["provision", "run"].includes(command) ||
  !Object.values(RunMode).includes(mode)
) {
  console.error(
    "usage: anydash-load.mjs provision|run --api <url> ... [--mode gate|smoke]",
  );
  process.exit(2);
}
// The capacity gate population and duration from the server contract.
const GATE_USERS = 1000;
const GATE_DEVICES = 5000;
const GATE_MINUTES = 30;
// Provisioning creates a Codex and a Vercel connection and registers both on every device.
const CONNECTIONS_PER_DEVICE = 2;

const stats = { requests: 0, errors: 0, byMethod: new Map() };
function uuidv7() {
  const bytes = Buffer.from(randomUUID().replaceAll("-", ""), "hex");
  const now = BigInt(Date.now());
  for (let i = 0; i < 6; i += 1)
    bytes[i] = Number((now >> BigInt(8 * (5 - i))) & 0xffn);
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytes.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

async function call(
  method,
  body,
  { token, kind = "CLIENT_KIND_CLI", allow = [] } = {},
) {
  const headers = {
    "content-type": "application/json",
    "connect-protocol-version": "1",
    "x-anydash-client-kind": kind,
    "x-anydash-client-version": version,
  };
  if (token) headers.authorization = `Bearer ${token}`;
  stats.requests += 1;
  const entry = stats.byMethod.get(method) ?? { requests: 0, errors: 0 };
  entry.requests += 1;
  stats.byMethod.set(method, entry);
  try {
    const response = await fetch(`${api}/anydash.v1.${method}`, {
      method: "POST",
      headers,
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(30_000),
    });
    const value = await response.json().catch(() => ({}));
    if (response.ok) return value;
    if (allow.includes(value.code)) return null;
    throw new Error(
      `${method} ${response.status} ${value.code ?? ""} ${response.headers.get("x-anydash-correlation-id") ?? ""}`,
    );
  } catch (error) {
    stats.errors += 1;
    entry.errors += 1;
    console.error(
      JSON.stringify({
        event: "request_error",
        method,
        error: String(error.message ?? error),
      }),
    );
    return null;
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const bucket = (t) => new Date(Math.floor(t / 900_000) * 900_000).toISOString();

// Provisioning steps that did not complete although their requests did not fail, for example an
// enrollment that was not consumed. Request failures are counted in `stats.errors`.
let incomplete = 0;
function markIncomplete(step) {
  incomplete += 1;
  console.error(JSON.stringify({ event: "provision_incomplete", step }));
}

async function provision() {
  const users = JSON.parse(readFileSync(args.users, "utf8"));
  const state = { users: [], devices: [], complete: false };
  for (const user of users) {
    const kind = "CLIENT_KIND_WEB";
    const account = await call(
      "AccountService/ConnectAccount",
      { delcoreAccessToken: user.delcoreAccessToken },
      { token: user.accessToken, kind },
    );
    if (!account) continue;
    const personal = account.account.personalSpaceId;
    const devices = [];
    for (let i = 0; i < devicesPerUser; i += 1) {
      const start = await call("DeviceService/StartEnrollment", {
        deviceName: `load-${i}`,
        platform: "DEVICE_PLATFORM_LINUX",
        architecture: "CPU_ARCHITECTURE_X64",
        collectorKind: "COLLECTOR_KIND_CLI",
        collectorVersion: version,
        osVersion: "24.04",
      });
      // StartEnrollment allows 10 requests per minute per client address.
      const paced = sleep(6100);
      if (!start) {
        await paced;
        continue;
      }
      await call(
        "DeviceService/ApproveEnrollment",
        { enrollmentId: start.enrollmentId, userCode: start.userCode },
        { token: user.accessToken, kind },
      );
      const polled = await call("DeviceService/PollEnrollment", {
        enrollmentId: start.enrollmentId,
        deviceCode: start.deviceCode,
      });
      if (!polled?.credentials) {
        if (polled) markIncomplete("PollEnrollment");
        await paced;
        continue;
      }
      const device = {
        id: polled.device.id,
        access: polled.credentials.accessToken,
        accessExpiresAt: polled.credentials.accessTokenExpiresAt,
        refresh: polled.credentials.refreshToken,
        connections: [],
      };
      if (i === 0) {
        for (const provider of ["PROVIDER_CODEX", "PROVIDER_VERCEL"]) {
          const created = await call(
            "ConnectionService/CreateConnection",
            { requestId: uuidv7(), provider },
            { token: device.access },
          );
          if (created) device.connections.push(created.connection.id);
        }
      }
      devices.push(device);
      await paced;
    }
    // Every device registers the user's connections so that leases can fail over.
    const shared = devices[0]?.connections ?? [];
    for (const device of devices.slice(1)) {
      const registered = [];
      for (const connectionId of shared) {
        if (
          await call(
            "ConnectionService/RegisterConnectionDevice",
            { connectionId },
            { token: device.access },
          )
        )
          registered.push(connectionId);
      }
      device.connections = registered;
    }
    const created = await call(
      "DashboardService/CreateDashboard",
      { requestId: uuidv7(), spaceId: personal, name: "Load" },
      { token: user.accessToken, kind },
    );
    let dashboardId = created?.dashboard?.id;
    let widgetId;
    if (dashboardId) {
      const added = await call(
        "DashboardService/AddWidget",
        {
          dashboardId,
          expectedVersion: "1",
          requestId: uuidv7(),
          type: "WIDGET_TYPE_LOCAL_USAGE_SERIES",
          layout: {
            x: 0,
            y: 0,
            width: 12,
            height: 4,
            presentation: "WIDGET_PRESENTATION_LINE_CHART",
          },
          query: {
            period: "QUERY_PERIOD_LAST_24_HOURS",
            granularity: "TIME_GRANULARITY_HOUR",
          },
          source: { allDevices: true },
        },
        { token: user.accessToken, kind },
      );
      widgetId = added?.widgetId;
    }
    state.users.push({ accessToken: user.accessToken, dashboardId, widgetId });
    state.devices.push(...devices);
  }
  // The state file is written even when provisioning is incomplete, because it holds the
  // credentials of the devices that were enrolled; `run` refuses an incomplete state.
  const problems = populationProblems(state, {
    users: users.length,
    devices: users.length * devicesPerUser,
  });
  if (stats.errors > 0) problems.push(`${stats.errors} requests failed`);
  if (incomplete > 0) problems.push(`${incomplete} steps did not complete`);
  state.complete = problems.length === 0;
  writeFileSync(args.out, JSON.stringify(state), { mode: 0o600 });
  console.log(
    JSON.stringify({
      event: "provisioned",
      complete: state.complete,
      users: state.users.length,
      devices: state.devices.length,
      errors: stats.errors,
      problems,
    }),
  );
  process.exit(state.complete ? 0 : 1);
}

// Returns why `state` lacks the expected population: at least `expected.users` users with a
// dashboard and a widget and at least `expected.devices` devices registered on every connection.
function populationProblems(state, expected) {
  const users = Array.isArray(state.users) ? state.users : [];
  const devices = Array.isArray(state.devices) ? state.devices : [];
  const problems = [];
  if (users.length < expected.users)
    problems.push(`${users.length} of ${expected.users} users provisioned`);
  if (devices.length < expected.devices)
    problems.push(
      `${devices.length} of ${expected.devices} devices provisioned`,
    );
  const withoutWidget = users.filter((u) => !u.dashboardId || !u.widgetId);
  if (withoutWidget.length > 0)
    problems.push(`${withoutWidget.length} users lack a dashboard widget`);
  const unregistered = devices.filter(
    (d) =>
      !d.id ||
      !d.refresh ||
      (d.connections ?? []).length !== CONNECTIONS_PER_DEVICE,
  );
  if (unregistered.length > 0)
    problems.push(
      `${unregistered.length} devices lack credentials or connection registrations`,
    );
  return problems;
}

// Refuses to start traffic for a population or duration that cannot produce the selected result.
function runProblems(state) {
  const gate = mode === RunMode.GATE;
  const problems = populationProblems(
    state,
    gate
      ? { users: GATE_USERS, devices: GATE_DEVICES }
      : { users: 1, devices: 1 },
  );
  if (state.complete !== true)
    problems.push("provisioning did not complete; provision again");
  if (!Number.isFinite(minutes) || minutes <= 0)
    problems.push("--minutes must be a positive number");
  else if (gate && minutes < GATE_MINUTES)
    problems.push(
      `the capacity gate runs for at least ${GATE_MINUTES} minutes`,
    );
  return problems;
}

async function userLoop(user, until) {
  while (Date.now() < until) {
    if (user.dashboardId) {
      await call(
        "WidgetDataService/QueryDashboardData",
        { dashboardId: user.dashboardId, timeZone: "Asia/Kathmandu" },
        { token: user.accessToken, kind: "CLIENT_KIND_WEB" },
      );
    }
    await call(
      "AccountService/GetAccount",
      {},
      { token: user.accessToken, kind: "CLIENT_KIND_WEB" },
    );
    await sleep(30_000 + Math.random() * 30_000);
  }
}

// Refresh this long before the access token expires, so a slow cycle never uses an expired one.
const refreshMarginMs = 10 * 60_000;

// Rotates the device credentials. Provisioning 5,000 devices takes hours and access tokens are
// valid for one hour, so saved access tokens are usually expired when `run` starts.
async function refreshCredentials(device) {
  const refreshed = await call("DeviceService/RefreshDeviceToken", {
    deviceId: device.id,
    refreshToken: device.refresh,
  });
  if (!refreshed) return;
  device.access = refreshed.credentials.accessToken;
  device.accessExpiresAt = refreshed.credentials.accessTokenExpiresAt;
  device.refresh = refreshed.credentials.refreshToken;
}

function credentialsDue(device) {
  const expires = Date.parse(device.accessExpiresAt ?? "");
  return !Number.isFinite(expires) || Date.now() >= expires - refreshMarginMs;
}

async function deviceLoop(device, until) {
  await sleep(Math.random() * 60_000);
  // Always rotate before the first authenticated request, then follow the returned expiry.
  await refreshCredentials(device);
  const leases = new Map();
  let bucketVersion = Date.now() * 1000;
  let minute = 0;
  while (Date.now() < until) {
    const beat = await call(
      "CollectorService/Heartbeat",
      {
        collectorVersion: "1.0.0",
        osVersion: "24.04",
        readyConnectionIds: device.connections,
      },
      { token: device.access },
    );
    for (const assignment of beat?.assignments ?? []) {
      if (assignment.lease?.holderDeviceId === device.id)
        leases.set(assignment.connectionId, assignment.lease.epoch);
      else leases.delete(assignment.connectionId);
      if (assignment.acquireRecommended) {
        const acquired = await call(
          "CollectorService/AcquireLease",
          { connectionId: assignment.connectionId },
          { token: device.access },
        );
        if (acquired?.granted)
          leases.set(assignment.connectionId, acquired.lease.epoch);
      }
    }
    bucketVersion += 60_000_000;
    const now = Date.now();
    await call(
      "CollectorService/UploadUsage",
      {
        buckets: ["gpt-5", "claude-opus-4"].map((model, i) => ({
          tool: i === 0 ? "LOCAL_TOOL_CODEX" : "LOCAL_TOOL_CLAUDE_CODE",
          model,
          bucketStart: bucket(now),
          version: String(bucketVersion),
          sessionCount: "1",
          tokens: {
            inputTokens: String(1000 + minute),
            outputTokens: "200",
            cacheReadTokens: "500",
            cacheWriteTokens: "0",
          },
        })),
      },
      { token: device.access },
    );
    if (minute % 5 === 0) {
      for (const [connectionId, epoch] of leases) {
        await call(
          "CollectorService/ReportSnapshot",
          {
            connectionId,
            leaseEpoch: epoch,
            snapshot: {
              observedAt: new Date().toISOString(),
              status: "COLLECTION_STATUS_SUCCEEDED",
              source: "OBSERVATION_SOURCE_ACTIVE_POLL",
              metrics: [
                {
                  kind: "METRIC_KIND_SUBSCRIPTION_USED_PERCENT",
                  dimension: "five_hour",
                  value: {
                    state: "METRIC_STATE_VALUE",
                    percent: (minute % 100) + 0.5,
                  },
                  temporality: "METRIC_TEMPORALITY_INSTANTANEOUS",
                },
              ],
            },
          },
          { token: device.access, allow: ["failed_precondition"] },
        );
      }
    }
    if (credentialsDue(device)) await refreshCredentials(device);
    minute += 1;
    await sleep(60_000 - (Date.now() - now));
  }
}

async function run() {
  const state = JSON.parse(readFileSync(args.state, "utf8"));
  const problems = runProblems(state);
  if (problems.length > 0) {
    console.error(JSON.stringify({ event: "invalid_run", mode, problems }));
    process.exit(1);
  }
  const until = Date.now() + minutes * 60_000;
  // Rotated refresh tokens replace the saved ones; presenting a rotated token again revokes the
  // device, so the state file is saved every minute in case the run is interrupted.
  const save = () =>
    writeFileSync(args.state, JSON.stringify(state), { mode: 0o600 });
  const report = setInterval(() => {
    save();
    console.log(
      JSON.stringify({
        event: "progress",
        requests: stats.requests,
        errors: stats.errors,
      }),
    );
  }, 60_000);
  await Promise.all([
    ...state.users.map((u) => userLoop(u, until)),
    ...state.devices.map((d) => deviceLoop(d, until)),
  ]);
  clearInterval(report);
  // Rotated tokens must be persisted for a later run.
  save();
  const methods = Object.fromEntries(stats.byMethod);
  console.log(
    JSON.stringify({
      event: "result",
      mode,
      capacityGate: mode === RunMode.GATE,
      users: state.users.length,
      devices: state.devices.length,
      minutes,
      requests: stats.requests,
      errors: stats.errors,
      methods,
    }),
  );
  process.exit(stats.errors === 0 ? 0 : 1);
}

await (command === "provision" ? provision() : run());
