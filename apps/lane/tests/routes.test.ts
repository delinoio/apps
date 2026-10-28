// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import test from "node:test";
import { routeTarget, targetPath } from "../src/routes.ts";
import { RouteHistory } from "../web/navigation.ts";

const ID = "019a0000-0000-7000-8000-000000000001";
test("every workspace destination round-trips through a path", () => {
  for (const path of [
    "/tasks",
    "/proposals",
    "/repositories",
    "/events",
    "/settings",
    "/pace",
    "/realqa",
    "/new-tab",
    `/tasks/${ID}`,
    `/repositories/${ID}`,
    `/tasks/new/${ID}`,
  ]) {
    assert.equal(targetPath(routeTarget(path)!), path);
    assert.equal(targetPath(routeTarget(`${path}/`)!), path);
  }
  assert.equal(targetPath(routeTarget("/")!), "/tasks");
  for (const path of [
    "/missing",
    "/toString",
    "/tasks/nope",
    `/tasks/${ID}/extra`,
    "/tasks/%2e%2e",
    "//settings",
  ])
    assert.equal(routeTarget(path), undefined, path);
});

function fixture(path = "/settings") {
  const location = { pathname: path };
  const entries = [{ path, state: null as any }];
  let cursor = 0;
  const requests: { path: string; requestId: number }[] = [];
  const history = {
    get state() {
      return entries[cursor].state;
    },
    replaceState(state: any, _title: string, path: any) {
      entries[cursor] = { state, path };
      location.pathname = path;
    },
    pushState(state: any, _title: string, path: any) {
      entries.splice(++cursor);
      entries.push({ state, path });
      location.pathname = path;
    },
    go(delta: number) {
      cursor += delta;
      location.pathname = entries[cursor].path;
    },
  };
  const routes = new RouteHistory({ location, history }, (target, requestId) =>
    requests.push({ path: targetPath(target), requestId }),
  );
  const acknowledge = () => {
    const request = requests.at(-1)!;
    routes.report(request.path, request.requestId);
  };
  return { routes, history, location, entries, requests, acknowledge };
}

test("startup waits for the app and clicks push only changed destinations", () => {
  const f = fixture();
  f.routes.start();
  f.routes.report("/tasks");
  assert.equal(f.location.pathname, "/settings");
  f.acknowledge();
  f.routes.report("/events");
  f.routes.report("/events");
  assert.equal(f.entries.length, 2);
  assert.equal(f.location.pathname, "/events");
  f.history.go(-1);
  f.routes.pop();
  f.acknowledge();
  assert.equal(
    f.entries.length,
    2,
    "Back does not create another history entry",
  );
  assert.equal(f.location.pathname, "/settings");
  f.history.go(1);
  f.routes.pop();
  f.acknowledge();
  assert.equal(f.location.pathname, "/events");
});

test("cancelled and superseded history navigation preserve the selected screen", () => {
  const f = fixture();
  f.routes.start();
  f.acknowledge();
  f.routes.report("/events");
  f.routes.report("/realqa");
  f.history.go(-1);
  f.routes.pop();
  const stale = f.requests.at(-1)!;
  f.history.go(-1);
  f.routes.pop();
  f.routes.report(stale.path, stale.requestId);
  assert.equal(f.location.pathname, "/settings");
  f.routes.report("/realqa", f.requests.at(-1)!.requestId);
  assert.equal(f.location.pathname, "/realqa");
  f.routes.pop();
  f.history.go(-1);
  f.routes.pop();
  f.acknowledge();
  assert.equal(
    f.location.pathname,
    "/events",
    "cancel does not overwrite the destination entry",
  );
});

test("saving a draft replaces its URL without leaving a blank-draft history entry", () => {
  const f = fixture(`/tasks/new/${ID}`);
  f.routes.start();
  f.acknowledge();
  f.routes.report(`/tasks/${ID}`);
  assert.equal(f.entries.length, 1);
  assert.equal(f.location.pathname, `/tasks/${ID}`);
});

test("Back accepts a saved task selected through an earlier draft history entry", () => {
  const draft = `/tasks/new/${ID}`;
  const saved = `/tasks/${ID}`;
  const f = fixture(draft);
  f.routes.start();
  f.acknowledge();
  f.routes.report("/settings");
  f.routes.report(draft);
  f.routes.report(saved);
  f.history.go(-1);
  f.routes.pop();
  f.acknowledge();
  f.history.go(-1);
  f.routes.pop();
  assert.equal(f.requests.at(-1)!.path, draft);
  // The retained pane has already been saved, so its acknowledgement uses
  // the saved path even though this history entry still names the draft.
  f.routes.report(saved, f.requests.at(-1)!.requestId);
  assert.equal(f.location.pathname, saved);
  assert.deepEqual(
    f.entries.map((entry) => entry.path),
    [saved, "/settings", saved],
  );
  f.history.go(1);
  f.routes.pop();
  f.acknowledge();
  assert.equal(f.location.pathname, "/settings");
});

test("refresh keeps history positions and ignores acknowledgements from an older request", () => {
  const f = fixture();
  f.routes.start();
  f.acknowledge();
  f.routes.report("/events");
  f.routes.report("/realqa");
  f.routes.start();
  f.acknowledge();
  f.history.go(-1);
  f.routes.pop();
  const stale = f.requests.at(-1)!;
  f.routes.report("/realqa", stale.requestId);
  assert.equal(
    f.location.pathname,
    "/realqa",
    "cancelled Back after refresh returns to the right entry",
  );
  f.routes.pop();
  f.history.go(-1);
  f.routes.pop();
  f.acknowledge();
  f.routes.report("/realqa", stale.requestId);
  assert.equal(
    f.location.pathname,
    "/events",
    "a late acknowledgement cannot navigate again",
  );
});
