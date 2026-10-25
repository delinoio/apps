// SPDX-License-Identifier: Apache-2.0
// Explicit test-only server. Never included in production build output.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, resolve } from "node:path";
const root = resolve(import.meta.dirname, "../dist");
const repo = {
  formatVersion: 1,
  entityId: "019a196d-0000-7000-8000-000000000001",
  versionId: "019a196d-0000-7000-8000-000000000002",
  kind: "ENTITY_KIND_REPOSITORY",
  repository: { normalizedUrl: "github.com/delinoio/apps" },
  origin: { kind: "ORIGIN_KIND_APP", deviceName: "Test device" },
};
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
    prompt: "Explain the empty state.",
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
    const url = new URL(req.url, "http://127.0.0.1:46400");
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
        case "GetSyncInfo":
          response = { formatVersion: 1, epoch: "test-epoch" };
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
    if (path.endsWith("index.html"))
      data = Buffer.from(
        data
          .toString()
          .replace("https://lane.api.delino.io", "http://127.0.0.1:46400"),
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
}).listen(46400, "127.0.0.1", () =>
  console.log("Lane test-only preview http://127.0.0.1:46400"),
);
