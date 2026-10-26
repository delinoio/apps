// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { OnlineLane, type Session } from "../web/online.ts";
Object.defineProperty(globalThis, "navigator", {
  value: { onLine: true },
  configurable: true,
});
const principals: Record<string, Session> = {
  a: {
    account_id: "account-a",
    device_id: "device-a",
    csrf_token: "csrf-a",
    notification_preferences: { enabled: true, content: 2 },
  },
  b: {
    account_id: "account-b",
    device_id: "device-b",
    csrf_token: "csrf-b",
    notification_preferences: { enabled: false, content: 1 },
  },
  c: {
    account_id: "account-c",
    device_id: "device-c",
    csrf_token: "csrf-c",
  },
  // Same account as A after a new approval: new device, session token and preferences.
  a2: {
    account_id: "account-a",
    device_id: "device-a2",
    csrf_token: "csrf-a2",
    notification_preferences: { enabled: false, content: 3 },
  },
};
interface Call {
  path: string;
  csrf: string | null;
  principal: string;
}
type Mode = "ok" | "unauthorized" | "network";
function connectEnd() {
  // A Connect end-of-stream frame so the watch ends without data.
  return new Response(Buffer.from([2, 0, 0, 0, 2, 123, 125]), {
    headers: { "Content-Type": "application/connect+json" },
  });
}
function harness() {
  const calls: Call[] = [];
  const events: string[] = [];
  const sessions: (Session | undefined)[] = [];
  let principal = "a";
  let sessionMode: Mode = "ok";
  let holdSessions = false;
  const heldSessions: (() => void)[] = [];
  let holdList: Promise<void> | undefined;
  let holdSyncInfoFor: { principal: string; gate: Promise<void> } | undefined;
  let failSaves = 0;
  const mock: typeof fetch = async (input, init) => {
    const path = new URL(
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url,
    ).pathname;
    calls.push({
      path,
      csrf: new Headers(init?.headers).get("X-Lane-CSRF"),
      principal,
    });
    const json = (body: unknown) =>
      new Response(JSON.stringify(body), {
        headers: { "Content-Type": "application/json" },
      });
    if (path === "/auth/session") {
      if (sessionMode === "network") throw new TypeError("network down");
      if (sessionMode === "unauthorized")
        return new Response("{}", { status: 401 });
      const snapshot = principals[principal];
      if (holdSessions)
        await new Promise<void>((resolve) => heldSessions.push(resolve));
      return json(snapshot);
    }
    if (path.endsWith("GetSyncInfo")) {
      const held = holdSyncInfoFor;
      const epoch = `epoch-${principal}`;
      if (held && held.principal === principal) await held.gate;
      return json({ formatVersion: 3, epoch });
    }
    if (path.endsWith("WatchChanges")) return connectEnd();
    if (path.endsWith("ListTasks")) {
      const answeredBy = principal;
      if (holdList) await holdList;
      return json({ items: [{ id: `task-of-${answeredBy}` }] });
    }
    if (path.endsWith("CreateTask")) {
      if (failSaves > 0) {
        --failSaves;
        return new Response(
          JSON.stringify({ code: "unavailable", message: "Save failed" }),
          { status: 503, headers: { "Content-Type": "application/json" } },
        );
      }
      return json({});
    }
    if (path.endsWith("BeginDataDeletion"))
      return json({ challengeId: "challenge" });
    if (path === "/auth/reauth/proof") return json({ reauth_proof: "proof" });
    return json({});
  };
  const lane = new OnlineLane({
    baseUrl: "https://lane.test",
    fetch: mock,
    invalidate: () => events.push("invalidate"),
    sessionChanged: (session) => {
      sessions.push(session);
      events.push(`session:${session?.account_id ?? "none"}`);
    },
    returnTo: "https://app.test/",
  });
  return {
    lane,
    calls,
    events,
    sessions,
    business: () =>
      calls.filter((c) => c.path.startsWith("/lane.v1.")).map((c) => c.path),
    switchTo: (value: string) => {
      principal = value;
    },
    sessionMode: (value: Mode) => {
      sessionMode = value;
    },
    holdSessions: (value: boolean) => {
      holdSessions = value;
    },
    releaseSession: (index: number) => heldSessions[index](),
    heldSessionCount: () => heldSessions.length,
    holdList: () => {
      let release!: () => void;
      holdList = new Promise<void>((r) => (release = r));
      return () => {
        holdList = undefined;
        release();
      };
    },
    holdSyncInfo: (forPrincipal: string) => {
      let release!: () => void;
      holdSyncInfoFor = {
        principal: forPrincipal,
        gate: new Promise<void>((r) => (release = r)),
      };
      return release;
    },
    failSaves: (count: number) => {
      failSaves = count;
    },
  };
}
const tick = () => new Promise((r) => setTimeout(r, 0));
const CREATE = {
  id: "task",
  title: "Keep input",
  repository: { url: "https://example.com/repo" },
  meta: { requestId: "request" },
};
const list = (h: ReturnType<typeof harness>) =>
  h.lane.invoke("lane.v1.LaneService", "ListTasks", {});
const create = (h: ReturnType<typeof harness>) =>
  h.lane.invoke("lane.v1.LaneService", "CreateTask", CREATE);
const count = (h: ReturnType<typeof harness>, suffix: string) =>
  h.calls.filter((c) => c.path.endsWith(suffix)).length;

test("resume reconciles the new account before any business request and installs its credentials", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.calls.length = 0;
  h.events.length = 0;
  h.switchTo("b");
  await h.lane.resume();
  await create(h);
  // The session read is the first request; business requests follow it.
  assert.equal(h.calls[0].path, "/auth/session");
  // The account-bound view is told about B before queries are invalidated.
  assert.deepEqual(h.events.slice(0, 2), ["session:account-b", "invalidate"]);
  const save = h.calls.find((c) => c.path.endsWith("CreateTask"))!;
  assert.equal(save.csrf, "csrf-b");
  assert.equal(
    h.calls.filter((c) => c.csrf === "csrf-a").length,
    0,
    "the previous token is never sent after reconciliation",
  );
  const status = (await h.lane.invoke("local", "GetLocalStatus", {})) as {
    deviceId: string;
  };
  assert.equal(status.deviceId, "device-b");
  const prefs = (await h.lane.invoke(
    "local",
    "GetNotificationPreferences",
    {},
  )) as { enabled: boolean; content: number };
  assert.deepEqual([prefs.enabled, prefs.content], [false, 1]);
  h.lane.stop();
});

test("late unary and watch results from account A never reach account B", async () => {
  const h = harness();
  const releaseSyncInfo = h.holdSyncInfo("a");
  await h.lane.start();
  await tick();
  const releaseList = h.holdList();
  const settled = list(h).then(
    () => "resolved",
    (e: Error) => e.message,
  );
  await tick();
  h.switchTo("b");
  h.events.length = 0;
  const resumed = h.lane.resume();
  await tick();
  // A's read is released while the new identity is still being confirmed.
  releaseList();
  await resumed;
  releaseSyncInfo();
  await tick();
  await tick();
  assert.match(await settled, /account changed/);
  assert.equal(
    h.calls.some((c) => c.path.endsWith("WatchChanges") && c.principal === "a"),
    false,
    "the retired watch never opens its stream",
  );
  assert.equal(h.events.filter((e) => e === "invalidate").length, 1);
  h.lane.stop();
});

test("a unary read released after B is installed is rejected", async () => {
  const h = harness();
  await h.lane.start();
  const releaseList = h.holdList();
  const settled = list(h).then(
    () => "resolved",
    (e: Error) => e.message,
  );
  await tick();
  h.switchTo("b");
  await h.lane.resume();
  releaseList();
  assert.match(await settled, /account changed/);
  h.lane.stop();
});

test("the newest session response wins when an obsolete one is released last", async () => {
  const h = harness();
  await h.lane.start();
  h.holdSessions(true);
  h.switchTo("b");
  const first = h.lane.reconcile();
  await tick();
  h.switchTo("c");
  const second = h.lane.reconcile();
  await tick();
  assert.equal(h.heldSessionCount(), 2);
  h.releaseSession(1);
  await second;
  assert.equal(h.sessions.at(-1)?.account_id, "account-c");
  h.releaseSession(0);
  await first;
  assert.equal(h.sessions.at(-1)?.account_id, "account-c");
  assert.equal(
    h.sessions.filter((s) => s?.account_id === "account-b").length,
    0,
  );
  const status = (await h.lane.invoke("local", "GetLocalStatus", {})) as {
    deviceId: string;
  };
  assert.equal(status.deviceId, "device-c");
  h.lane.stop();
});

test("a session response released after Disconnect cannot reinstall the signed-out session", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.holdSessions(true);
  const pending = h.lane.reconcile();
  await tick();
  assert.equal(h.heldSessionCount(), 1);
  // Disconnect bypasses the reconciliation guard, so it completes while the
  // focus-triggered /auth/session read is still pending.
  assert.deepEqual(await h.lane.invoke("local", "DisconnectSync", {}), {
    completed: true,
  });
  assert.equal(h.sessions.at(-1), undefined);
  h.events.length = 0;
  h.calls.length = 0;
  h.releaseSession(0);
  await pending;
  await tick();
  assert.deepEqual(h.events, []);
  assert.equal(h.sessions.at(-1), undefined);
  await assert.rejects(list(h), /Sign in to Lane/);
  await assert.rejects(create(h), /Sign in to Lane/);
  assert.deepEqual(h.business(), []);
  // A reconciliation started after the Disconnect still installs normally.
  h.holdSessions(false);
  await h.lane.reconcile();
  assert.equal(h.sessions.at(-1)?.account_id, "account-a");
  h.lane.stop();
});

test("an identical session keeps in-flight reads and the deletion target", async () => {
  const h = harness();
  await h.lane.start();
  await h.lane.invoke("lane.v1.AccountService", "BeginDataDeletion", {
    mode: 1,
    confirmedAccountId: "account-a",
  });
  const releaseList = h.holdList();
  const inFlight = list(h);
  await tick();
  const resumed = h.lane.resume();
  await tick();
  releaseList();
  await resumed;
  await inFlight;
  assert.equal(h.sessions.at(-1)?.csrf_token, "csrf-a");
  await h.lane.completeDeletion("challenge");
  assert.equal(count(h, "/auth/reauth/proof"), 1);
  h.lane.stop();
});

test("same-account reapproval replaces credentials and preferences", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.switchTo("a2");
  h.calls.length = 0;
  await h.lane.resume();
  await create(h);
  assert.equal(h.sessions.at(-1)?.account_id, "account-a");
  assert.equal(h.sessions.at(-1)?.device_id, "device-a2");
  assert.equal(
    h.calls.filter((c) => c.csrf === "csrf-a").length,
    0,
    "the old token is never sent after reconciliation",
  );
  assert.equal(
    h.calls.find((c) => c.path.endsWith("CreateTask"))?.csrf,
    "csrf-a2",
  );
  const prefs = (await h.lane.invoke(
    "local",
    "GetNotificationPreferences",
    {},
  )) as { enabled: boolean; content: number };
  assert.deepEqual([prefs.enabled, prefs.content], [false, 3]);
  const status = (await h.lane.invoke("local", "GetLocalStatus", {})) as {
    deviceId: string;
  };
  assert.equal(status.deviceId, "device-a2");
  h.lane.stop();
});

test("a rejected session stops business work under the retained identity", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.sessionMode("unauthorized");
  h.calls.length = 0;
  h.events.length = 0;
  await assert.rejects(h.lane.resume(), /Sign in to Lane/);
  assert.equal(h.sessions.at(-1), undefined);
  assert.deepEqual(h.events, ["session:none"]);
  await assert.rejects(list(h), /Sign in to Lane/);
  await assert.rejects(create(h), /Sign in to Lane/);
  assert.deepEqual(h.business(), []);
  h.lane.stop();
});

test("a transient session failure keeps input, blocks account-bound work and recovers only through explicit retry", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.sessionMode("network");
  h.calls.length = 0;
  h.events.length = 0;
  await assert.rejects(h.lane.resume(), /network down/);
  // The view is neither reset nor ended.
  assert.deepEqual(h.events, []);
  await assert.rejects(create(h), /Retry explicitly/);
  await assert.rejects(list(h), /Retry explicitly/);
  assert.deepEqual(h.business(), []);
  // Focus and online events do not retry on their own.
  h.sessionMode("ok");
  const before = count(h, "/auth/session");
  await h.lane.resume();
  assert.equal(count(h, "/auth/session"), before);
  await assert.rejects(create(h), /Retry explicitly/);
  // An explicit retry confirms the session; the next explicit save is sent once.
  await h.lane.start();
  await create(h);
  assert.equal(count(h, "CreateTask"), 1);
  assert.equal(h.events.includes("session:none"), false);
  h.lane.stop();
});

test("a deletion challenge started for A cannot be redeemed after switching accounts or devices", async () => {
  for (const next of ["b", "a2"]) {
    const h = harness();
    await h.lane.start();
    await h.lane.invoke("lane.v1.AccountService", "BeginDataDeletion", {
      mode: 1,
      confirmedAccountId: "account-a",
    });
    h.switchTo(next);
    await assert.rejects(
      h.lane.completeDeletion("challenge"),
      /start data deletion again/,
    );
    assert.equal(count(h, "/auth/reauth/proof"), 0);
    assert.equal(count(h, "DeleteAccountData"), 0);
    h.lane.stop();
  }
});

test("a failed save is attempted once and never queued or replayed", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.failSaves(1);
  // A mutation arriving while the session is being confirmed is refused without a request.
  h.holdSessions(true);
  const resumed = h.lane.resume();
  await tick();
  await assert.rejects(create(h), /confirming your account session/);
  assert.equal(count(h, "CreateTask"), 0);
  h.releaseSession(0);
  await resumed;
  h.holdSessions(false);
  await assert.rejects(create(h), /Save failed/);
  await tick();
  await h.lane.resume();
  await tick();
  assert.equal(count(h, "CreateTask"), 1);
  // Only a later explicit user action sends the next attempt.
  await create(h);
  assert.equal(count(h, "CreateTask"), 2);
  h.lane.stop();
});

// Installs the browser APIs SetNotificationPreferences needs and restores them
// afterwards. `prompt` controls when the permission request resolves.
async function withPush(
  prompt: () => Promise<string>,
  body: (state: { prompts: number; unsubscribed: number }) => Promise<void>,
) {
  const state = { prompts: 0, unsubscribed: 0 };
  const globals = globalThis as Record<string, unknown>;
  const nav = navigator as unknown as Record<string, unknown>;
  const saved = {
    Notification: globals.Notification,
    window: globals.window,
    serviceWorker: nav.serviceWorker,
  };
  globals.Notification = {
    permission: "default",
    requestPermission: () => {
      ++state.prompts;
      return prompt();
    },
  };
  globals.window = { PushManager: function PushManager() {} };
  nav.serviceWorker = {
    register: async () => ({
      pushManager: {
        getSubscription: async () => ({
          endpoint: "https://push.test/endpoint",
          toJSON: () => ({ keys: { p256dh: "AAAA", auth: "AAAA" } }),
          unsubscribe: async () => {
            ++state.unsubscribed;
          },
        }),
      },
    }),
  };
  try {
    await body(state);
  } finally {
    globals.Notification = saved.Notification;
    globals.window = saved.window;
    nav.serviceWorker = saved.serviceWorker;
  }
}
const savePreferences = (h: ReturnType<typeof harness>, enabled = true) =>
  h.lane.invoke("local", "SetNotificationPreferences", { enabled, content: 2 });

test("a notification preference save is refused while the session is reconciled or unconfirmed", async () => {
  await withPush(
    async () => "granted",
    async (state) => {
      const h = harness();
      await h.lane.start();
      await tick();
      h.calls.length = 0;
      // Never held or queued while the session is being confirmed.
      h.holdSessions(true);
      const resumed = h.lane.resume();
      await tick();
      await assert.rejects(
        savePreferences(h),
        /confirming your account session/,
      );
      h.releaseSession(0);
      await resumed;
      h.holdSessions(false);
      // After a transient failure it stays blocked until an explicit retry.
      h.sessionMode("network");
      await assert.rejects(h.lane.resume(), /network down/);
      await assert.rejects(savePreferences(h), /Retry explicitly/);
      assert.equal(state.prompts, 0, "no permission prompt is opened");
      assert.equal(count(h, "RegisterPush"), 0);
      h.sessionMode("ok");
      await h.lane.start();
      await savePreferences(h);
      assert.equal(count(h, "RegisterPush"), 1);
      h.lane.stop();
    },
  );
});

test("a notification preference save started for A is retired when B is installed", async () => {
  let grant!: (value: string) => void;
  await withPush(
    () => new Promise<string>((resolve) => (grant = resolve)),
    async (state) => {
      const h = harness();
      await h.lane.start();
      await tick();
      h.calls.length = 0;
      const saved = savePreferences(h).then(
        () => "resolved",
        (e: Error) => e.message,
      );
      await tick();
      assert.equal(state.prompts, 1);
      // Focus returns after the approval popup replaced the account.
      h.switchTo("b");
      await h.lane.resume();
      grant("granted");
      assert.match(await saved, /account changed/);
      assert.equal(
        count(h, "RegisterPush"),
        0,
        "the old save never registers a device for the new account",
      );
      assert.equal(state.unsubscribed, 0);
      // B's preferences from the session stay installed.
      const prefs = (await h.lane.invoke(
        "local",
        "GetNotificationPreferences",
        {},
      )) as { enabled: boolean; content: number };
      assert.deepEqual([prefs.enabled, prefs.content], [false, 1]);
      h.lane.stop();
    },
  );
});

test("an explicit connection retry invalidates retained queries after the session is confirmed", async () => {
  const h = harness();
  await h.lane.start();
  await tick();
  h.sessionMode("network");
  await assert.rejects(h.lane.resume(), /network down/);
  h.events.length = 0;
  // A failed retry confirms nothing, so nothing is invalidated.
  await assert.rejects(h.lane.start(), /network down/);
  assert.deepEqual(h.events, []);
  // The successful retry reloads views that kept a failed-loading state.
  h.sessionMode("ok");
  await h.lane.start();
  assert.equal(h.events.at(-1), "invalidate");
  assert.equal(h.events.filter((e) => e === "invalidate").length, 1);
  await list(h);
  h.lane.stop();
});
