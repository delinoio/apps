// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
function worker() {
  const handlers = new Map<string, (e: any) => void>();
  const shown: any[] = [];
  const opened: string[] = [];
  runInNewContext(
    readFileSync(new URL("../web/push-worker.js", import.meta.url), "utf8"),
    {
      self: {
        addEventListener: (name: string, cb: any) => handlers.set(name, cb),
        registration: {
          showNotification: (title: string, options: any) => {
            shown.push({ title, ...options });
            return Promise.resolve();
          },
        },
      },
      clients: {
        openWindow: (url: string) => {
          opened.push(url);
          return Promise.resolve();
        },
      },
      encodeURIComponent,
    },
  );
  return { handlers, shown, opened };
}
test("WebPush renders supplied generic/detail text and pins notification account", () => {
  const h = worker();
  h.handlers.get("push")!({
    data: {
      json: () => ({
        title: "Repository",
        body: "Task title",
        proposalId: "task",
        accountId: "account",
        tag: "delivery",
      }),
    },
    waitUntil: () => {},
  });
  assert.equal(h.shown[0].title, "Repository");
  assert.equal(h.shown[0].body, "Task title");
  h.handlers.get("notificationclick")!({
    notification: { close: () => {}, data: h.shown[0].data },
    waitUntil: () => {},
  });
  assert.equal(h.opened[0], "/?task=task&account=account");
});
test("WebPush without authenticated account routing is not displayed", () => {
  const h = worker();
  h.handlers.get("push")!({
    data: {
      json: () => ({ title: "Lane", body: "Notice", proposalId: "task" }),
    },
    waitUntil: () => {},
  });
  assert.equal(h.shown.length, 0);
});
