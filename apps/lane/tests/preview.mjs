// SPDX-License-Identifier: Apache-2.0
// Explicit test-only server. Never included in production build output.
import { bridgeModule, withUpdateFixture } from "./update-preview.mjs";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
import {
  createPlanningState,
  handle as handlePlanning,
} from "./planning-mock.mjs";
const port = Number(process.env.LANE_PREVIEW_PORT || 46400);
const root = process.env.LANE_PREVIEW_ROOT
  ? resolve(process.env.LANE_PREVIEW_ROOT)
  : resolve(import.meta.dirname, "../dist");
const repo = {
  formatVersion: 1,
  entityId: "019a196d-0000-7000-8000-000000000001",
  versionId: "019a196d-0000-7000-8000-000000000002",
  kind: "ENTITY_KIND_REPOSITORY",
  repository: { normalizedUrl: "github.com/delinoio/apps" },
  origin: { kind: "ORIGIN_KIND_APP", deviceName: "Test device" },
};
const capturePNG =
  "iVBORw0KGgoAAAANSUhEUgAAAeAAAADwCAIAAABXFyDtAAADxElEQVR4nO3doU0FABBEQRw9UjsajQCDAQpA/uPeJZNMAaue3af3jy8Agp7WFwDwJ4EGiBJogCiBBogSaIAogQaIEmiAKIEGiBJogCiBBogSaIAogQaIEmiAKIEGiBJogCiBBogSaIAogQaIEmiAKIEGiBJogCiBBogSaICohwX6+eUbgF8CDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINEJULNACPJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQ8L9OvbJ/DP1gvCKIGGw9YLwiiBhsPWC8IogYbD1gvCKIGGw9YLwiiBhsPWC8IogYbD1gvCKIGGw9YLwiiBhsPWC8IogYbD1gvCKIGGw9YLwiiBhsPWC8IogYbD1gvCKIGGw9YLwiiBhsPWC8IogYbD1gvCKIGGw9YLwiiBhsPWC8IogYbD1gvCKIGGw9YLwiinsQBRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0QJdAAUQINECXQAFECDRAl0ABRAg0Q9QMrcRxzdlKf7wAAAABJRU5ErkJggg==";
let captureState = "available";
let task = {
  formatVersion: 1,
  entityId: "019a196d-0000-7000-8000-000000000003",
  versionId: "019a196d-0000-7000-8000-000000000004",
  kind: "ENTITY_KIND_TASK",
  task: {
    repositoryId: repo.entityId,
    title: "Improve empty states",
    description:
      "A **clear** next step.\n\n<script>alert(1)</script>\n\n![blocked](http://example.test/image.png)",
    prompt:
      "The empty state needs a clear next action. This saved capture is read-only.",
    captures: [
      {
        attachmentId: "019a196d-0000-7000-8000-000000000011",
        width: 480,
        height: 240,
        sha256:
          "b1a3b76227a10218d723bf9a73fba148dd10c4b79e09b25dae364a28e45e6b6f",
        byteSize: "1021",
        pageUrl:
          "https://example.test/app/a-very-long-path-that-must-wrap-on-narrow-mobile-layouts",
        pageTitle: "Empty state QA",
        cssSelector: "main > section[data-test=empty-state] > button",
      },
    ],
    progress: "PROGRESS_TODO",
    proposal: "PROPOSAL_PENDING",
  },
  origin: { kind: "ORIGIN_KIND_MCP", deviceName: "Test connection" },
  createdAt: "2026-10-25T00:00:00Z",
};
const result = (v) => ({
  result: "RESULT_KIND_APPLIED",
  version: v,
  alternatives: [],
});
// LANE_PREVIEW_SCENARIO=planning serves task planning fixtures and records requests.
const planning =
  process.env.LANE_PREVIEW_SCENARIO === "planning"
    ? createPlanningState(task, repo)
    : undefined;
// Switchable API principal and request log for the session reconciliation checks.
// The log records only paths and CSRF headers, never request content.
const principals = {
  a: {
    account_id: "test-account",
    device_id: "test-device",
    csrf_token: "test-csrf",
    title: "Improve empty states",
  },
  b: {
    account_id: "other-account",
    device_id: "other-device",
    csrf_token: "other-csrf",
    title: "Account B task",
  },
  // Same account as B after a new approval: new device and CSRF token.
  b2: {
    account_id: "other-account",
    device_id: "other-device-2",
    csrf_token: "other-csrf-2",
    title: "Account B task",
  },
};
let principal = "a";
let sessionMode = "ok";
let requestLog = [];
// Events fixture served by ListReceivers/ListEvents. /test/events replaces it,
// /test/events-requests returns the recorded read requests. An optional page
// size makes ListEvents paginate with offset cursors, and a fail count makes
// the next reads fail. All fields start empty so other scenarios are unaffected.
// GetEvent replies come from `details` (keyed by event ID). While `holdDetails`
// is set, each GetEvent waits in `heldDetails` until /test/events-release
// settles it by arrival index, which lets a test order replies explicitly.
const eventsFixture = {
  receivers: [],
  events: [],
  pageSize: 0,
  failReads: 0,
  requests: [],
  details: {},
  holdDetails: false,
  heldDetails: [],
};
let emptyList = false;
let failSave = false;
let history = [task];
createServer(async (req, res) => {
  try {
    const url = new URL(req.url, `http://127.0.0.1:${port}`);
    if (
      process.env.LANE_PREVIEW_UPDATES === "1" &&
      url.pathname === "/test-update-module.js"
    ) {
      res.setHeader("Content-Type", "application/javascript");
      res.end(bridgeModule);
      return;
    }
    if (url.pathname === "/test/capture-state") {
      captureState = url.searchParams.get("state") || "available";
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/empty-list") {
      emptyList = url.searchParams.get("enabled") === "1";
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/events") {
      const body = JSON.parse(url.searchParams.get("body") || "{}");
      for (const key of [
        "receivers",
        "events",
        "pageSize",
        "failReads",
        "details",
        "holdDetails",
      ])
        if (key in body) eventsFixture[key] = body[key];
      // Turning holding on or off starts a new release index sequence.
      if ("holdDetails" in body) eventsFixture.heldDetails = [];
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/events-release") {
      const held = eventsFixture.heldDetails[Number(url.searchParams.get("n"))];
      held?.settle(url.searchParams.get("mode") || "ok");
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify({ released: Boolean(held) }));
      return;
    }
    if (url.pathname === "/test/events-requests") {
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(eventsFixture.requests));
      return;
    }
    if (url.pathname === "/test/fail-save") {
      failSave = true;
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/external") {
      task = {
        ...task,
        versionId: "019a196d-0000-7000-8000-000000000099",
        task: { ...task.task, title: "External title" },
      };
      res.end("{}");
      return;
    }
    if (planning && url.pathname === "/test/requests") {
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(planning.requests));
      return;
    }
    if (planning && url.pathname === "/test/reset-requests") {
      planning.requests.length = 0;
      res.end("{}");
      return;
    }
    if (planning && url.pathname === "/test/plan") {
      const body = JSON.parse(url.searchParams.get("body") || "{}");
      if (body.failNext) planning.failNext = body.failNext;
      if (body.delayMs !== undefined) planning.delayMs = body.delayMs;
      if ("conflictTask" in body) planning.conflictTask = body.conflictTask;
      if ("heads" in body) planning.heads = body.heads;
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/principal") {
      principal = url.searchParams.get("name") || "a";
      task = {
        ...task,
        task: { ...task.task, title: principals[principal].title },
      };
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/session-mode") {
      sessionMode = url.searchParams.get("mode") || "ok";
      res.end("{}");
      return;
    }
    if (url.pathname === "/test/log") {
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(requestLog));
      return;
    }
    if (url.pathname === "/test/log-reset") {
      requestLog = [];
      res.end("{}");
      return;
    }
    if (
      url.pathname === "/auth/session" ||
      url.pathname.startsWith("/lane.v1.")
    )
      requestLog.push({
        path: url.pathname,
        csrf: req.headers["x-lane-csrf"] ?? null,
      });
    if (url.pathname === "/auth/session") {
      if (sessionMode !== "ok") {
        res.statusCode = Number(sessionMode);
        res.end("{}");
        return;
      }
      const { title, ...session } = principals[principal];
      void title;
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(session));
      return;
    }
    if (url.pathname.startsWith("/lane.v1.")) {
      let body = "";
      for await (const c of req) body += c;
      const input = body ? JSON.parse(body) : {};
      const method = url.pathname.split("/").at(-1);
      let response = {};
      if (
        planning &&
        /\/lane\.v1\.(LaneService|PlanningService)\//.test(url.pathname)
      ) {
        const handled = await handlePlanning(planning, method, input);
        if (handled) {
          res.statusCode = handled.status ?? 200;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify(handled.body));
          return;
        }
      }
      switch (method) {
        case "GetAttachment":
          if (captureState === "denied") {
            res.statusCode = 403;
            response = {
              code: "permission_denied",
              message: "Permission denied",
            };
          } else if (captureState === "error") {
            res.statusCode = 503;
            response = { code: "unavailable", message: "Retrieval failed" };
          } else
            response =
              captureState === "waiting"
                ? { state: "ATTACHMENT_STATE_WAITING_FOR_SYNC" }
                : captureState === "deleted"
                  ? { state: "ATTACHMENT_STATE_DELETED" }
                  : { state: "ATTACHMENT_STATE_AVAILABLE", png: capturePNG };
          break;
        case "GetSyncInfo":
          response = { formatVersion: 3, epoch: "test-epoch" };
          break;
        case "WatchChanges":
          res.setHeader("Content-Type", "application/connect+json");
          res.end(Buffer.from([2, 0, 0, 0, 2, 123, 125]));
          return;
        case "ListRepositories":
          response = { items: [result(repo)] };
          break;
        case "ListTasks":
          response = { items: emptyList ? [] : [result(task)] };
          break;
        case "GetTask":
          response = { result: result(task) };
          break;
        case "SaveTask":
          if (failSave) {
            res.statusCode = 503;
            response = {
              code: "unavailable",
              message: "Simulated storage failure",
            };
            break;
          }
          task = {
            ...task,
            task: input.task,
            versionId: "019a196d-0000-7000-8000-000000000005",
          };
          history.push(task);
          response = { result: result(task) };
          break;
        case "CreateTask":
          task = {
            ...task,
            entityId: input.id,
            task: { ...input, repositoryId: repo.entityId },
          };
          response = { result: result(task) };
          break;
        case "ListHistory":
          response = { versions: history };
          break;
        case "ListDevices":
          response = { items: [{ id: "test-device", name: "Test browser" }] };
          break;
        case "ListConnections":
          response = { items: [{ id: "test-connection", name: "Test agent" }] };
          break;
        case "GetEvent": {
          eventsFixture.requests.push({ method, input });
          // The reply content is fixed when the request arrives, so a test can
          // change the fixture between two inspections of the same ID.
          const detail = eventsFixture.details[input.id] ?? {};
          let mode = "ok";
          if (eventsFixture.holdDetails)
            mode = await new Promise((settle) =>
              eventsFixture.heldDetails.push({ id: input.id, settle }),
            );
          if (mode === "fail") {
            res.statusCode = 503;
            response = {
              code: "unavailable",
              message: `Simulated detail failure ${input.id}`,
            };
          } else if (mode === "ok") response = detail;
          break;
        }
        case "ReopenEvent": {
          eventsFixture.requests.push({ method, input });
          const event = eventsFixture.details[input.id]?.event;
          if (event)
            response = {
              event: { ...event, state: "EVENT_STATE_PENDING", revision: "2" },
            };
          break;
        }
        case "DeleteEvent":
          eventsFixture.requests.push({ method, input });
          break;
        case "ListReceivers":
        case "ListEvents": {
          eventsFixture.requests.push({ method, input });
          if (eventsFixture.failReads > 0) {
            eventsFixture.failReads--;
            res.statusCode = 503;
            response = {
              code: "unavailable",
              message: "Simulated read failure",
            };
            break;
          }
          const repositoryId = input.repositoryId;
          const matches = (item) =>
            !repositoryId || item.repositoryId === repositoryId;
          if (method === "ListReceivers") {
            response = { items: eventsFixture.receivers.filter(matches) };
            break;
          }
          const states = input.states ?? [];
          const items = eventsFixture.events.filter(
            (item) =>
              matches(item) && (!states.length || states.includes(item.state)),
          );
          const start = Number(input.page?.cursor || 0);
          const size = eventsFixture.pageSize || items.length;
          const end = start + size;
          response = {
            items: items.slice(start, end),
            nextCursor: end < items.length ? String(end) : "",
          };
          break;
        }
      }
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(response));
      return;
    }
    const path = resolve(
      root,
      "." + (url.pathname === "/" ? "/index.html" : url.pathname),
    );
    if (!path.startsWith(root + "/")) {
      res.statusCode = 403;
      res.end();
      return;
    }
    let data = await readFile(path);
    if (process.env.LANE_PREVIEW_UPDATES === "1" && path.endsWith("/host.js"))
      data = Buffer.from(
        withUpdateFixture(
          await readFile(new URL("../web/host.js", import.meta.url), "utf8"),
        ),
      );
    if (path.endsWith("index.html"))
      data = Buffer.from(
        data
          .toString()
          .replace("https://lane.api.delino.io", `http://127.0.0.1:${port}`),
      );
    res.setHeader(
      "Content-Type",
      {
        ".html": "text/html",
        ".js": "application/javascript",
        ".css": "text/css",
        ".wasm": "application/wasm",
      }[extname(path)] || "application/octet-stream",
    );
    res.end(data);
  } catch {
    res.statusCode = 404;
    res.end();
  }
}).listen(port, "127.0.0.1", () =>
  console.log(`Lane test-only preview http://127.0.0.1:${port}`),
);
