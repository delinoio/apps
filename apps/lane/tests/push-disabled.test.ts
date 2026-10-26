// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { OnlineLane } from "../web/online.ts";

const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

interface Options {
  enabled?: boolean;
  content?: number;
  subscribed?: boolean;
  permission?: NotificationPermission;
  // Hold RegisterPush until release() is called.
  hold?: boolean;
  // Reject RegisterPush with 503 before anything is persisted.
  fail?: boolean;
}

/**
 * Models a durable server-side registration so that a fresh OnlineLane reads
 * only what RegisterPush acknowledged, like a page reload.
 */
function setup(options: Options = {}) {
  const events: string[] = [];
  const registerBodies: Record<string, unknown>[] = [];
  const server = {
    enabled: options.enabled ?? false,
    content: options.content ?? 1,
  };
  let fail = options.fail === true;
  let release: (() => void) | undefined;
  const held = options.hold
    ? new Promise<void>((resolve) => {
        release = resolve;
      })
    : undefined;
  const subscription = {
    endpoint: "https://push.test/sub",
    toJSON: () => ({ keys: { p256dh: "AQ", auth: "AQ" } }),
    unsubscribe: async () => {
      events.push("unsubscribe");
      return true;
    },
  };
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      onLine: true,
      serviceWorker: {
        register: async () => ({
          active: {},
          installing: null,
          waiting: null,
          pushManager: {
            getSubscription: async () =>
              options.subscribed ? subscription : null,
            subscribe: async () => {
              events.push("subscribe");
              return subscription;
            },
          },
        }),
      },
    },
  });
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { PushManager: class {} },
  });
  Object.defineProperty(globalThis, "Notification", {
    configurable: true,
    value: {
      permission: options.permission ?? "granted",
      requestPermission: async () => {
        events.push("request-permission");
        return options.permission ?? "granted";
      },
    },
  });
  const fetcher: typeof fetch = async (input, init) => {
    const path = new URL(
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.href
          : input.url,
    ).pathname;
    events.push(path);
    let body: unknown = {};
    if (path === "/auth/session")
      body = {
        account_id: "account",
        device_id: "device",
        csrf_token: "csrf",
        notification_preferences: { ...server },
      };
    else if (path.endsWith("GetSyncInfo"))
      throw new Error("Test stream not connected");
    else if (path.endsWith("RegisterPush")) {
      const raw = init?.body;
      const request = JSON.parse(
        typeof raw === "string"
          ? raw
          : new TextDecoder().decode(raw as Uint8Array),
      ) as Record<string, unknown>;
      registerBodies.push(request);
      if (held) await held;
      if (fail)
        return new Response(
          JSON.stringify({ code: "unavailable", message: "Save failed" }),
          { status: 503, headers: { "Content-Type": "application/json" } },
        );
      server.enabled = request.enabled === true;
      server.content =
        request.content === "NOTIFICATION_CONTENT_GENERIC" ||
        request.content === 2
          ? 2
          : 1;
      body = { device: { id: "device" } };
    }
    return new Response(JSON.stringify(body), {
      headers: { "Content-Type": "application/json" },
    });
  };
  const make = () =>
    new OnlineLane({
      baseUrl: "https://lane.test",
      fetch: fetcher,
      invalidate: () => {},
      sessionChanged: () => {},
      returnTo: "https://app.test/",
    });
  const lane = make();
  const count = (name: string) => events.filter((e) => e === name).length;
  const registers = () =>
    events.filter((e) => e.endsWith("RegisterPush")).length;
  const set = (enabled: boolean, content: number, target = lane) =>
    target.invoke("local", "SetNotificationPreferences", { enabled, content });
  const read = async (target = lane) =>
    (await target.invoke("local", "GetNotificationPreferences", {})) as {
      enabled: boolean;
      content: number;
    };
  // A fresh client models reload: it reads only the acknowledged server state.
  const reload = async () => {
    const fresh = make();
    await fresh.start();
    const prefs = await read(fresh);
    fresh.stop();
    return { enabled: prefs.enabled, content: prefs.content };
  };
  return {
    lane,
    events,
    registerBodies,
    server,
    count,
    registers,
    set,
    read,
    reload,
    release: () => release?.(),
    stopFailing: () => {
      fail = false;
    },
  };
}

function assertNoPromptOrSubscription(t: ReturnType<typeof setup>) {
  assert.equal(t.count("request-permission"), 0);
  assert.equal(t.count("subscribe"), 0);
  assert.equal(t.count("/auth/push-config"), 0);
  assert.equal(t.count("unsubscribe"), 0);
}

test("disabling without a browser subscription persists enabled=false and content once", async () => {
  const t = setup({ enabled: true, content: 1 });
  await t.lane.start();
  const saved = await t.set(false, 2);
  assert.deepEqual(saved, { enabled: false, content: 2, permission: 2 });
  assert.equal(t.registers(), 1);
  const body = t.registerBodies[0];
  assert.equal(body.deviceId, "device");
  assert.match(String(body.provider), /WEB_PUSH|^1$/);
  assert.equal(body.enabled ?? false, false);
  assert.ok(
    body.content === "NOTIFICATION_CONTENT_GENERIC" || body.content === 2,
  );
  for (const field of ["endpoint", "publicKey", "authSecret"])
    assert.ok(
      body[field] === undefined || body[field] === "",
      `${field} must be empty`,
    );
  assertNoPromptOrSubscription(t);
  assert.deepEqual(await t.reload(), { enabled: false, content: 2 });
  t.lane.stop();
});

test("a never-subscribed disabled user saves and reloads Generic and Detail", async () => {
  const generic = setup({ enabled: false, content: 1 });
  await generic.lane.start();
  await generic.set(false, 2);
  assert.equal(generic.registers(), 1);
  assertNoPromptOrSubscription(generic);
  assert.deepEqual(await generic.reload(), { enabled: false, content: 2 });
  generic.lane.stop();

  const detail = setup({ enabled: false, content: 2 });
  await detail.lane.start();
  await detail.set(false, 1);
  assert.equal(detail.registers(), 1);
  assert.deepEqual(await detail.reload(), { enabled: false, content: 1 });
  detail.lane.stop();
});

test("disabled saves never prompt or subscribe under denied, default or granted permission", async () => {
  for (const permission of ["denied", "default", "granted"] as const) {
    const t = setup({ enabled: true, content: 1, permission });
    await t.lane.start();
    const saved = (await t.set(false, 2)) as { enabled: boolean };
    assert.equal(saved.enabled, false, permission);
    assert.equal(t.registers(), 1, permission);
    assert.equal(
      (globalThis as unknown as { Notification: { permission: string } })
        .Notification.permission,
      permission,
    );
    assertNoPromptOrSubscription(t);
    assert.deepEqual(await t.reload(), { enabled: false, content: 2 });
    t.lane.stop();
  }
});

test("a live subscription is unsubscribed only after the server acknowledges the disabled save", async () => {
  const t = setup({
    enabled: true,
    content: 1,
    subscribed: true,
    hold: true,
  });
  await t.lane.start();
  let returned = false;
  const pending = t.set(false, 2).then((value) => {
    returned = true;
    return value;
  });
  await settle();
  await settle();
  assert.equal(t.registers(), 1);
  assert.equal(returned, false);
  assert.equal(t.count("unsubscribe"), 0);
  assert.deepEqual(await t.read(), {
    enabled: true,
    content: 1,
    permission: 2,
  });
  t.release();
  assert.deepEqual(await pending, {
    enabled: false,
    content: 2,
    permission: 2,
  });
  assert.equal(t.count("unsubscribe"), 1);
  assert.equal(t.registerBodies[0].endpoint, "https://push.test/sub");
  assert.equal(
    t.events.indexOf("unsubscribe") >
      t.events.findIndex((e) => e.endsWith("RegisterPush")),
    true,
  );
  assert.equal(t.count("request-permission"), 0);
  assert.deepEqual(await t.reload(), { enabled: false, content: 2 });
  t.lane.stop();
});

test("enablement still requires permission before registering and subscribes when absent", async () => {
  const denied = setup({ permission: "denied" });
  await denied.lane.start();
  await assert.rejects(denied.set(true, 2), /permission was not granted/);
  assert.equal(denied.registers(), 0);
  assert.equal(denied.count("subscribe"), 0);
  denied.lane.stop();

  const absent = setup({ permission: "granted" });
  await absent.lane.start();
  // push-config is not served by this fixture, so enablement stops at config.
  await assert.rejects(absent.set(true, 2));
  assert.equal(absent.count("request-permission"), 1);
  assert.equal(absent.count("/auth/push-config"), 1);
  assert.equal(absent.registers(), 0);
  absent.lane.stop();

  const existing = setup({ subscribed: true });
  await existing.lane.start();
  await existing.set(true, 1);
  assert.equal(existing.count("request-permission"), 1);
  assert.equal(existing.registers(), 1);
  assert.equal(existing.registerBodies[0].endpoint, "https://push.test/sub");
  assert.equal(existing.registerBodies[0].enabled, true);
  assert.deepEqual(await existing.reload(), { enabled: true, content: 1 });
  existing.lane.stop();
});

test("a rejected disabled save does not succeed, install preferences, unsubscribe or retry", async () => {
  for (const subscribed of [false, true]) {
    const t = setup({ enabled: true, content: 1, subscribed, fail: true });
    await t.lane.start();
    await assert.rejects(t.set(false, 2), /Save failed/);
    await settle();
    assert.equal(t.registers(), 1, `subscribed=${subscribed}`);
    assert.equal(t.count("unsubscribe"), 0);
    assert.deepEqual(await t.read(), {
      enabled: true,
      content: 1,
      permission: 2,
    });
    assert.deepEqual(await t.reload(), { enabled: true, content: 1 });
    // Only an explicit retry persists the draft.
    t.stopFailing();
    await t.set(false, 2);
    assert.equal(t.registers(), 2);
    assert.equal(t.count("unsubscribe"), subscribed ? 1 : 0);
    assert.deepEqual(await t.reload(), { enabled: false, content: 2 });
    t.lane.stop();
  }
});
