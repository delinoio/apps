// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { OnlineLane } from "../web/online.ts";
Object.defineProperty(globalThis, "navigator", {
  value: { onLine: true },
  configurable: true,
});
function harness() {
  const calls: { path: string; init?: RequestInit }[] = [];
  let account = "account-a";
  let failure = false;
  let delay: Promise<Response> | undefined;
  const mock: typeof fetch = async (input, init) => {
    const path = new URL(
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url,
    ).pathname;
    calls.push({ path, init });
    let body: unknown = {};
    if (path === "/auth/session")
      body = {
        account_id: account,
        device_id: "device",
        csrf_token: "csrf-test",
        notification_preferences: { enabled: true, content: 2 },
      };
    else if (path.endsWith("GetSyncInfo"))
      throw new Error("Test stream not connected");
    else if (path.endsWith("ListTasks")) {
      if (delay) return delay;
      body = { items: [] };
    } else if (path.endsWith("CreateTask")) {
      if (failure)
        return new Response(
          JSON.stringify({ code: "unavailable", message: "Save failed" }),
          { status: 503, headers: { "Content-Type": "application/json" } },
        );
      body = {};
    } else if (path.endsWith("BeginDataDeletion"))
      body = {
        challengeId: "challenge",
        authorizationUrl: "https://login.test/approve",
      };
    else if (path === "/auth/reauth/proof")
      body = { reauth_proof: "one-use-proof" };
    else if (path.endsWith("DeleteAccountData"))
      body = { newEpoch: "epoch", disconnected: true };
    return new Response(JSON.stringify(body), {
      headers: { "Content-Type": "application/json" },
    });
  };
  const lane = new OnlineLane({
    baseUrl: "https://lane.test",
    fetch: mock,
    invalidate: () => {},
    sessionChanged: () => {},
    returnTo: "https://app.test/",
  });
  return {
    lane,
    calls,
    setAccount: (v: string) => {
      account = v;
    },
    fail: () => {
      failure = true;
    },
    delay: (v: Promise<Response>) => {
      delay = v;
    },
  };
}
test("credentialed Connect mutation sends CSRF and does not retry a failed save", async () => {
  const h = harness();
  await h.lane.start();
  h.fail();
  await assert.rejects(
    h.lane.invoke("lane.v1.LaneService", "CreateTask", {
      id: "task",
      title: "Keep input",
      repository: { url: "https://example.com/repo" },
      meta: { requestId: "request" },
    }),
    /Save failed/,
  );
  const calls = h.calls.filter((c) => c.path.endsWith("CreateTask"));
  assert.equal(calls.length, 1);
  assert.equal(calls[0].init?.credentials, "include");
  assert.equal(
    new Headers(calls[0].init?.headers).get("X-Lane-CSRF"),
    "csrf-test",
  );
  h.lane.stop();
});
test("offline web blocks save without sending or queueing it", async () => {
  const h = harness();
  await h.lane.start();
  Object.defineProperty(navigator, "onLine", {
    value: false,
    configurable: true,
  });
  await assert.rejects(
    h.lane.invoke("lane.v1.LaneService", "CreateTask", {}),
    /offline/,
  );
  Object.defineProperty(navigator, "onLine", {
    value: true,
    configurable: true,
  });
  assert.equal(h.calls.filter((c) => c.path.endsWith("CreateTask")).length, 0);
  h.lane.stop();
});
test("late server response cannot update another account", async () => {
  const h = harness();
  await h.lane.start();
  let resolve!: (v: Response) => void;
  h.delay(
    new Promise((r) => {
      resolve = r;
    }),
  );
  const pending = h.lane.invoke("lane.v1.LaneService", "ListTasks", {});
  await new Promise((r) => setTimeout(r, 0));
  h.setAccount("account-b");
  await h.lane.start();
  resolve(
    new Response('{"items":[]}', {
      headers: { "Content-Type": "application/json" },
    }),
  );
  await assert.rejects(pending, /account changed/);
  h.lane.stop();
});
test("deletion requires prior target confirmation and consumes bound proof over POST, never URL", async () => {
  const h = harness();
  await h.lane.start();
  await assert.rejects(
    h.lane.completeDeletion("challenge"),
    /confirm its target/,
  );
  await h.lane.invoke("lane.v1.AccountService", "BeginDataDeletion", {
    mode: "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL",
    confirmedAccountId: "account-a",
    meta: { requestId: "request" },
  });
  await h.lane.completeDeletion("challenge");
  const proof = h.calls.find((c) => c.path === "/auth/reauth/proof")!;
  assert.equal(proof.init?.method, "POST");
  assert.deepEqual(JSON.parse(proof.init?.body as string), {
    challenge_id: "challenge",
  });
  const deletion = h.calls.find((c) => c.path.endsWith("DeleteAccountData"))!;
  const body = JSON.parse(
    typeof deletion.init?.body === "string"
      ? deletion.init.body
      : new TextDecoder().decode(deletion.init?.body as Uint8Array),
  );
  assert.equal(body.confirmedAccountId, "account-a");
  assert.equal(body.mode, "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL");
  assert.equal(body.reauthProof, "one-use-proof");
  h.lane.stop();
});

test("unmatched and old-account reauthentication cannot delete data", async () => {
  const h = harness();
  await h.lane.start();
  await h.lane.invoke("lane.v1.AccountService", "BeginDataDeletion", {
    mode: 1,
    confirmedAccountId: "account-a",
  });
  await assert.rejects(h.lane.completeDeletion("unmatched"));
  h.setAccount("account-b");
  await h.lane.start();
  await assert.rejects(h.lane.completeDeletion("challenge"));
  assert.equal(
    h.calls.filter(
      (c) =>
        c.path.endsWith("DeleteAccountData") || c.path === "/auth/reauth/proof",
    ).length,
    0,
  );
  h.lane.stop();
});
