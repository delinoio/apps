// SPDX-License-Identifier: Apache-2.0
// Explicit test-only server. Never included in production build output.
import { bridgeModule, withUpdateFixture } from "./update-preview.mjs";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
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
    if (url.pathname === "/auth/session") {
      res.setHeader("Content-Type", "application/json");
      res.end(
        JSON.stringify({
          account_id: "test-account",
          device_id: "test-device",
          csrf_token: "test-csrf",
        }),
      );
      return;
    }
    if (url.pathname.startsWith("/lane.v1.")) {
      let body = "";
      for await (const c of req) body += c;
      const input = body ? JSON.parse(body) : {};
      const method = url.pathname.split("/").at(-1);
      let response = {};
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
        case "ListReceivers":
        case "ListEvents":
          response = { items: [] };
          break;
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
