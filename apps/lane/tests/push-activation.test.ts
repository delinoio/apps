// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { createECDH } from "node:crypto";
import { OnlineLane } from "../web/online.ts";

const ec = createECDH("prime256v1");
ec.generateKeys();
const publicKey = ec.getPublicKey().toString("base64url");
const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

type WorkerState =
  "installing" | "installed" | "activating" | "activated" | "redundant";
class FakeWorker extends EventTarget {
  state: WorkerState = "installing";
  set(next: WorkerState) {
    this.state = next;
    this.dispatchEvent(new Event("statechange"));
  }
}
interface FakeSubscription {
  endpoint: string;
  toJSON(): { keys: { p256dh: string; auth: string } };
  unsubscribe(): Promise<boolean>;
}
interface Options {
  permission?: NotificationPermission;
  active?: boolean;
  installing?: FakeWorker | null;
  existing?: boolean;
  config?: { public_key?: string };
  registerFails?: boolean;
}

function setup(options: Options = {}) {
  const events: string[] = [];
  const registerBodies: Record<string, unknown>[] = [];
  const worker =
    options.installing === undefined ? new FakeWorker() : options.installing;
  let active = options.active === true;
  const subscription: FakeSubscription = {
    endpoint: "https://push.test/sub",
    toJSON: () => ({ keys: { p256dh: "AQ", auth: "AQ" } }),
    unsubscribe: async () => {
      events.push("unsubscribe");
      return true;
    },
  };
  let current: FakeSubscription | null = options.existing ? subscription : null;
  const registration = {
    get active() {
      return active ? {} : null;
    },
    get installing() {
      return worker;
    },
    waiting: null,
    pushManager: {
      getSubscription: async () => {
        events.push("getSubscription");
        return current;
      },
      subscribe: async () => {
        events.push(active ? "subscribe" : "subscribe-without-active-worker");
        if (!active)
          throw new DOMException(
            "No active Service Worker",
            "InvalidStateError",
          );
        current = subscription;
        return subscription;
      },
    },
  };
  // A different registration that becomes ready while the intended one is inactive.
  const otherRegistration = {
    active: {},
    pushManager: {
      getSubscription: async () => null,
      subscribe: async () => {
        events.push("subscribe-through-wrong-registration");
        return subscription;
      },
    },
  };
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      onLine: true,
      serviceWorker: {
        register: async () => {
          events.push("register");
          if (options.registerFails) throw new Error("Worker script failed");
          return registration;
        },
        ready: Promise.resolve(otherRegistration),
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
        notification_preferences: { enabled: false, content: 2 },
      };
    else if (path === "/auth/push-config")
      body = options.config ?? { public_key: publicKey };
    else if (path.endsWith("GetSyncInfo"))
      throw new Error("Test stream not connected");
    else if (path.endsWith("RegisterPush")) {
      const raw = init?.body;
      registerBodies.push(
        JSON.parse(
          typeof raw === "string"
            ? raw
            : new TextDecoder().decode(raw as Uint8Array),
        ),
      );
    }
    return new Response(JSON.stringify(body), {
      headers: { "Content-Type": "application/json" },
    });
  };
  const lane = new OnlineLane({
    baseUrl: "https://lane.test",
    fetch: fetcher,
    invalidate: () => {},
    sessionChanged: () => {},
    returnTo: "https://app.test/",
  });
  const count = (name: string) => events.filter((e) => e === name).length;
  const registers = () =>
    events.filter((e) => e.endsWith("RegisterPush")).length;
  const set = (enabled: boolean) =>
    lane.invoke("local", "SetNotificationPreferences", { enabled, content: 2 });
  const enabled = async () =>
    (
      (await lane.invoke("local", "GetNotificationPreferences", {})) as {
        enabled: boolean;
      }
    ).enabled;
  const activate = () => {
    active = true;
    worker?.set("activated");
  };
  return {
    lane,
    events,
    registerBodies,
    worker,
    registration,
    count,
    registers,
    set,
    enabled,
    activate,
    subscription,
  };
}

test("first enablement waits for the intended registration to activate and then succeeds once", async () => {
  const t = setup();
  await t.lane.start();
  const pending = t.set(true);
  await settle();
  t.worker!.set("installed");
  await settle();
  assert.equal(t.count("subscribe-without-active-worker"), 0);
  assert.equal(t.count("subscribe"), 0);
  assert.equal(t.registers(), 0);
  t.worker!.set("activating");
  await settle();
  assert.equal(t.count("subscribe"), 0);
  assert.equal(t.registers(), 0);
  t.activate();
  assert.deepEqual(await pending, { enabled: true, content: 2, permission: 2 });
  assert.equal(t.count("subscribe"), 1);
  assert.equal(t.count("subscribe-without-active-worker"), 0);
  assert.equal(t.registers(), 1);
  assert.equal(t.registerBodies[0].endpoint, "https://push.test/sub");
  assert.equal(t.registerBodies[0].enabled, true);
  t.lane.stop();
});

test("an already activated worker that finished before listening does not hang", async () => {
  const worker = new FakeWorker();
  worker.state = "activated";
  const t = setup({ installing: worker, active: true });
  await t.lane.start();
  await t.set(true);
  assert.equal(t.count("subscribe"), 1);
  t.lane.stop();
});

test("an active worker with an installing update proceeds without waiting", async () => {
  const t = setup({ active: true });
  await t.lane.start();
  await t.set(true);
  assert.equal(t.count("subscribe"), 1);
  assert.equal(t.registers(), 1);
  t.lane.stop();
});

test("an existing subscription is reused without waiting for activation", async () => {
  const t = setup({ existing: true });
  await t.lane.start();
  await t.set(true);
  assert.equal(t.count("subscribe"), 0);
  assert.equal(t.count("subscribe-without-active-worker"), 0);
  assert.equal(t.registers(), 1);
  assert.equal(t.registerBodies[0].endpoint, "https://push.test/sub");
  assert.equal(t.registerBodies[0].enabled, true);
  t.lane.stop();
});

test("disable registers the server change before unsubscribing and never prompts", async () => {
  const t = setup({ existing: true });
  await t.lane.start();
  assert.equal(await t.enabled(), false);
  await t.set(false);
  assert.equal(t.count("request-permission"), 0);
  assert.equal(t.count("/auth/push-config"), 0);
  assert.equal(t.registers(), 1);
  assert.equal(t.registerBodies[0].enabled ?? false, false);
  const register = t.events.findIndex((e) => e.endsWith("RegisterPush"));
  const unsubscribe = t.events.indexOf("unsubscribe");
  assert.ok(register >= 0 && unsubscribe > register);
  t.lane.stop();
});

test("failure boundaries stay errors without saving preferences or registering push", async () => {
  const denied = setup({ permission: "denied" });
  await denied.lane.start();
  await assert.rejects(denied.set(true), /permission was not granted/);
  assert.equal(denied.count("register"), 0);

  const noKey = setup({ config: {}, active: true });
  await noKey.lane.start();
  await assert.rejects(noKey.set(true), /Web Push is not configured/);
  assert.equal(noKey.count("subscribe"), 0);

  const rejected = setup({ registerFails: true });
  await rejected.lane.start();
  await assert.rejects(rejected.set(true), /Worker script failed/);

  const redundant = setup();
  await redundant.lane.start();
  const pending = assert.rejects(redundant.set(true), /could not be activated/);
  await settle();
  redundant.worker!.set("redundant");
  await pending;

  const missingWorker = setup({ installing: null });
  await missingWorker.lane.start();
  await assert.rejects(missingWorker.set(true), /could not be activated/);

  for (const t of [denied, noKey, rejected, redundant, missingWorker]) {
    assert.equal(t.registers(), 0);
    assert.equal(t.count("subscribe"), 0);
    assert.equal(t.count("subscribe-without-active-worker"), 0);
    assert.equal(await t.enabled(), false);
    t.lane.stop();
  }
});

test("readiness of another registration is not accepted for the intended worker", async () => {
  const t = setup();
  await t.lane.start();
  const pending = t.set(true);
  await settle();
  await settle();
  assert.equal(t.count("subscribe-through-wrong-registration"), 0);
  assert.equal(t.count("subscribe"), 0);
  assert.equal(t.registers(), 0);
  t.activate();
  await pending;
  assert.equal(t.count("subscribe-through-wrong-registration"), 0);
  assert.equal(t.count("subscribe"), 1);
  assert.equal(t.registers(), 1);
  t.lane.stop();
});
