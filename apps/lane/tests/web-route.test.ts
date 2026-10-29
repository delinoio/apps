// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import {
  ACCOUNT_MISMATCH_MESSAGE,
  OnlineLane,
  createRouteDelivery,
  loginRoute,
} from "../web/online.ts";

Object.defineProperty(globalThis, "navigator", {
  value: { onLine: true },
  configurable: true,
});

const TASK = "019a0000-0000-7000-8000-000000000001";
const ACCOUNT = "019a0000-0000-7000-8000-000000000002";
const OTHER = "019a0000-0000-7000-8000-000000000003";
const BASE = "https://lane.example.test/";
const API = "https://lane.api.example.test";

function lane() {
  return new OnlineLane({
    baseUrl: API,
    returnTo: BASE,
    invalidate: () => {},
    sessionChanged: () => {},
    fetch: async () => new Response("{}"),
  });
}

test("plain and ConnectSync sign-in links stay query-free", () => {
  const url = new URL(lane().loginUrl());
  assert.equal(url.origin + url.pathname, `${API}/auth/login`);
  assert.deepEqual([...url.searchParams.keys()], ["return_to"]);
  assert.equal(url.searchParams.get("return_to"), BASE);
});

test("a notification link keeps its task and account through sign-in", () => {
  // A 401 session response shows the gate; its sign-in link carries the destination.
  const search = `?task=${TASK}&account=${ACCOUNT}&utm=ignored`;
  const url = new URL(lane().loginUrl(loginRoute(search)));
  assert.equal(url.searchParams.get("return_to"), BASE);
  assert.equal(url.searchParams.get("task"), TASK);
  assert.equal(url.searchParams.get("account"), ACCOUNT);
  assert.equal(url.searchParams.has("repository"), false);
  assert.equal(url.searchParams.has("utm"), false);
  // The API redirects to the registered base callback plus the typed fields.
  const returned = `${BASE}?task=${TASK}&account=${ACCOUNT}`;
  const deliver = createRouteDelivery();
  const search2 = new URL(returned).search;
  // Ready can arrive before the session is initialized, and more than once.
  assert.equal(deliver(search2, undefined), undefined);
  assert.deepEqual(deliver(search2, ACCOUNT), {
    name: "lane.openRoute",
    payload: { kind: "task", id: TASK },
  });
  assert.equal(deliver(search2, ACCOUNT), undefined);
});

test("a repository link is restored with or without an account hint", () => {
  for (const hint of ["", `&account=${ACCOUNT}`]) {
    const search = `?repository=${TASK}${hint}`;
    const url = new URL(lane().loginUrl(loginRoute(search)));
    assert.equal(url.searchParams.get("repository"), TASK);
    assert.equal(url.searchParams.get("account"), hint ? ACCOUNT : null);
    const deliver = createRouteDelivery();
    assert.deepEqual(deliver(search, ACCOUNT), {
      name: "lane.openRoute",
      payload: { kind: "repository", id: TASK },
    });
    assert.equal(deliver(search, ACCOUNT), undefined);
  }
});

test("a different authenticated account gets the mismatch error and no route", () => {
  for (const target of [`task=${TASK}`, `repository=${TASK}`]) {
    const search = `?${target}&account=${ACCOUNT}`;
    const deliver = createRouteDelivery();
    assert.deepEqual(deliver(search, OTHER), {
      name: "lane.hostError",
      payload: { message: ACCOUNT_MISMATCH_MESSAGE },
    });
    // The hint is not rewritten or switched, and nothing is delivered later.
    assert.equal(
      new URL(lane().loginUrl(loginRoute(search))).searchParams.get("account"),
      ACCOUNT,
    );
    assert.equal(deliver(search, OTHER), undefined);
    assert.equal(deliver(search, ACCOUNT), undefined);
  }
});

test("an already authenticated link delivers once and prefers the task", () => {
  const search = `?repository=${OTHER}&task=${TASK}&account=${ACCOUNT}`;
  assert.deepEqual(loginRoute(search), {
    kind: "task",
    id: TASK,
    account: ACCOUNT,
  });
  const deliver = createRouteDelivery();
  assert.deepEqual(deliver(search, ACCOUNT), {
    name: "lane.openRoute",
    payload: { kind: "task", id: TASK },
  });
  assert.equal(deliver(search, ACCOUNT), undefined);
  // A page without a destination consumes the delivery without an event.
  const plain = createRouteDelivery();
  assert.equal(plain("", ACCOUNT), undefined);
  assert.equal(plain(`?task=${TASK}`, ACCOUNT), undefined);
});

test("malformed or incomplete links sign in plainly instead of failing", () => {
  const plain = lane().loginUrl();
  for (const search of [
    "",
    `?account=${ACCOUNT}`,
    "?task=not-a-uuid",
    "?task=",
    `?repository=${TASK.toUpperCase()}`,
    `?task=${TASK}&account=nope`,
    "?task=00000000-0000-4000-8000-000000000001",
    "?next=https://evil.example&redirect=/x",
  ]) {
    assert.equal(loginRoute(search), undefined, search);
    assert.equal(lane().loginUrl(loginRoute(search)), plain, search);
  }
});
