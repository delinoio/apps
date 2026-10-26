// SPDX-License-Identifier: Apache-2.0
import { ConnectError, createClient } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import {
  create,
  fromJson,
  toJson,
  type DescMethodUnary,
  type JsonValue,
} from "@bufbuild/protobuf";
import {
  CaptureService,
  AccountService,
  ConstraintViolation,
  ErrorDetailSchema,
  ErrorReason,
  LaneService,
  PlanningService,
  SyncService,
  WebhookService,
  PushProvider,
  NotificationContent,
  DataDeletionMode,
} from "@repo/protos/lane/v1/lane_pb";
/**
 * Resolves once the given registration has an active worker. An already active
 * worker (including one with an update installing) resolves immediately.
 * Rejects when the installing or waiting worker becomes redundant, or when the
 * registration has no worker to wait for.
 */
function whenWorkerActive(
  registration: ServiceWorkerRegistration,
): Promise<void> {
  if (registration.active) return Promise.resolve();
  const worker = registration.installing ?? registration.waiting;
  const failure = () =>
    new Error(
      "The notification worker could not be activated. In-app indications remain available.",
    );
  if (!worker) return Promise.reject(failure());
  return new Promise<void>((resolve, reject) => {
    const settle = () => {
      if (worker.state === "activated") {
        worker.removeEventListener("statechange", settle);
        resolve();
      } else if (worker.state === "redundant") {
        worker.removeEventListener("statechange", settle);
        reject(failure());
      }
    };
    worker.addEventListener("statechange", settle);
    // The worker may have changed state before the listener was attached.
    settle();
  });
}
export interface Session {
  account_id: string;
  device_id: string;
  csrf_token: string;
  tenant_id?: string;
  notification_preferences?: { enabled: boolean; content: NotificationContent };
}
export interface OnlineOptions {
  baseUrl: string;
  fetch?: typeof fetch;
  invalidate: () => void;
  sessionChanged: (session: Session | undefined) => void;
  returnTo: string;
}
// Carries the typed Lane error detail across the host-module boundary so the
// frontend can explain constraint violations instead of showing a transport code.
export class LaneRpcError extends Error {
  constructor(
    message: string,
    readonly laneReason: string,
    readonly laneViolation?: string,
  ) {
    super(message);
  }
}
function laneError(error: unknown): unknown {
  if (!(error instanceof ConnectError)) return error;
  const detail = error.findDetails(ErrorDetailSchema)[0];
  if (!detail) return error;
  return new LaneRpcError(
    error.rawMessage || error.message,
    ErrorReason[detail.reason] ?? "UNSPECIFIED",
    detail.violation ? ConstraintViolation[detail.violation] : undefined,
  );
}
// Distinguishes a rejected session (HTTP 401) from a transient failure so the
// reconciler can end the session instead of retaining a possibly stale identity.
export class AuthRequestError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}
const SESSION_UNAVAILABLE_MESSAGE =
  "Lane could not confirm your account session. Your current-screen input is retained. Retry explicitly.";
const SESSION_CHANGED_MESSAGE =
  "The account changed or the session was replaced. Reload before continuing.";
const SESSION_RECONCILING_MESSAGE =
  "Lane is confirming your account session. Your current-screen input is retained. Retry explicitly.";
// Get/List methods are read-only and bypass CSRF on the server; everything else mutates.
function isReadMethod(method: string) {
  return /^(Get|List)/.test(method);
}
export class OnlineLane {
  private session?: Session;
  private generation = 0;
  // Newest-started reconciliation wins; older session responses are discarded.
  private reconcileSeq = 0;
  private reconciling?: Promise<void>;
  // True when the retained session could not be confirmed (transient failure).
  // Account-bound work stays blocked until an explicit retry confirms it.
  private unavailable = false;
  private fetcher: typeof fetch;
  private abort?: AbortController;
  private watchRetry?: ReturnType<typeof setTimeout>;
  private retryDelay = 1000;
  private cursor = "";
  private epoch = "";
  private notificationContent = NotificationContent.DETAIL;
  private notificationEnabled = false;
  private deletion?: {
    mode: DataDeletionMode;
    account: string;
    challengeId: string;
    generation: number;
  };
  private transport;
  constructor(private options: OnlineOptions) {
    this.fetcher = options.fetch || globalThis.fetch.bind(globalThis);
    this.transport = createConnectTransport({
      baseUrl: options.baseUrl,
      useBinaryFormat: false,
      fetch: (input, init) =>
        this.fetcher(input, { ...init, credentials: "include" }),
      interceptors: [
        (next) => async (req) => {
          if (this.session)
            req.header.set("X-Lane-CSRF", this.session.csrf_token);
          return next(req);
        },
      ],
    });
  }
  async auth(path: string, body?: unknown) {
    const response = await this.fetcher(`${this.options.baseUrl}${path}`, {
      method: body === undefined ? "GET" : "POST",
      credentials: "include",
      cache: "no-store",
      headers:
        body === undefined
          ? {}
          : {
              "Content-Type": "application/json",
              "X-Lane-CSRF": this.session?.csrf_token || "",
            },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!response.ok)
      throw new AuthRequestError(
        response.status === 401
          ? "Sign in to Lane to continue."
          : `Account request failed (${response.status}). Retry explicitly.`,
        response.status,
      );
    return response.status === 204 ? {} : response.json();
  }
  // Initial connection and the explicit retry control. A reconciliation that
  // failed transiently rejected in-flight reads and their views kept the error,
  // and the same-account host does not reload the view, so a confirmed session
  // also invalidates retained queries, as resume() does. A same-account watch
  // restart cannot be relied on to notify an account with no changes.
  async start() {
    await this.reconcile();
    if (this.session && !this.unavailable) this.options.invalidate();
  }
  // Reads GET /auth/session and installs the newest accepted identity. Concurrent
  // calls are serialized by recency: only the most recently started request may
  // install its result, so an obsolete response released last cannot win.
  reconcile(): Promise<void> {
    const seq = ++this.reconcileSeq;
    const run = this.runReconcile(seq);
    this.reconciling = run;
    const clear = () => {
      if (this.reconciling === run) this.reconciling = undefined;
    };
    run.then(clear, clear);
    return run;
  }
  private async runReconcile(seq: number): Promise<void> {
    let session: Session;
    try {
      session = (await this.auth("/auth/session")) as Session;
    } catch (error) {
      if (seq !== this.reconcileSeq) return this.reconciling;
      this.failReconcile(error);
      throw error;
    }
    if (seq !== this.reconcileSeq) return this.reconciling;
    if (
      typeof session?.account_id !== "string" ||
      typeof session.device_id !== "string" ||
      typeof session.csrf_token !== "string" ||
      !session.account_id
    ) {
      // A malformed body cannot establish an identity; treat it as transient.
      const error = new Error(
        "Account session response was invalid. Retry explicitly.",
      );
      this.failReconcile(error);
      throw error;
    }
    this.installSession(session);
  }
  private installSession(session: Session) {
    const previous = this.session;
    // The CSRF token is bound to the session cookie, so a new token also means
    // new credentials even when the account and device are unchanged.
    const changed =
      !previous ||
      previous.account_id !== session.account_id ||
      previous.device_id !== session.device_id ||
      previous.csrf_token !== session.csrf_token;
    if (changed) {
      ++this.generation;
      this.stop();
      this.deletion = undefined;
      this.cursor = "";
      this.epoch = "";
    }
    this.unavailable = false;
    this.session = session;
    this.notificationEnabled =
      session.notification_preferences?.enabled ?? false;
    this.notificationContent =
      session.notification_preferences?.content ?? NotificationContent.DETAIL;
    this.options.sessionChanged(this.session);
    this.startWatch();
  }
  private failReconcile(error: unknown) {
    if (error instanceof AuthRequestError && error.status === 401) {
      // The session is gone: stop account-bound work and show the sign-in gate.
      this.endSession();
      return;
    }
    // Transient failure: keep the view and its input, but stop account-bound work
    // and block saves until an explicit retry confirms the session.
    if (this.session) {
      this.unavailable = true;
      ++this.generation;
      this.stop();
    }
  }
  private endSession() {
    // Ending the session (sign-in gate, Disconnect, data deletion) also ends any
    // reconciliation still waiting on /auth/session. Its response predates this
    // transition, so it must not reinstall the identity that was just removed. The
    // pending run is detached from `reconciling` because a superseded run returns
    // that promise and would otherwise wait on itself.
    ++this.reconcileSeq;
    this.reconciling = undefined;
    this.stop();
    this.session = undefined;
    this.unavailable = false;
    this.deletion = undefined;
    this.cursor = "";
    this.epoch = "";
    ++this.generation;
    this.options.sessionChanged(undefined);
  }
  async refreshSession() {
    await this.auth("/auth/refresh", {});
    await this.start();
  }
  loginUrl() {
    return `${this.options.baseUrl}/auth/login?return_to=${encodeURIComponent(this.options.returnTo)}`;
  }
  async invoke(
    service: string,
    methodName: string,
    input: JsonValue,
  ): Promise<JsonValue> {
    if (!navigator.onLine)
      throw new Error(
        "You are offline. Your current-screen input is retained. Reconnect and retry explicitly.",
      );
    if (service === "local")
      return this.local(methodName, input as Record<string, unknown>);
    // Captured before any wait so a response for an earlier identity is rejected.
    const generation = this.generation;
    const read = isReadMethod(methodName);
    if (this.reconciling) {
      // Reads wait for the in-flight reconciliation; a mutation is never held or
      // queued and must be retried explicitly.
      if (!read) throw new Error(SESSION_RECONCILING_MESSAGE);
      await this.settled();
      if (generation !== this.generation)
        throw new Error(SESSION_CHANGED_MESSAGE);
    }
    if (this.unavailable) throw new Error(SESSION_UNAVAILABLE_MESSAGE);
    if (!this.session) throw new Error("Sign in to Lane to continue.");
    const serviceDescriptor = [
      LaneService,
      PlanningService,
      CaptureService,
      AccountService,
      WebhookService,
      SyncService,
    ].find((s) => s.typeName === service);
    const method = serviceDescriptor?.methods.find(
      (m) => m.name === methodName,
    );
    if (!method || method.methodKind !== "unary")
      throw new Error("Unsupported Lane operation.");
    if (service === CaptureService.typeName && methodName !== "GetAttachment")
      throw new Error("Capture and RealQA settings are available on desktop.");
    const response = await this.transport
      .unary(
        method as DescMethodUnary,
        undefined,
        30000,
        undefined,
        fromJson(method.input, input),
      )
      .catch((error: unknown) => {
        throw laneError(error);
      });
    // A read that finishes while the session is being reconciled cannot be
    // attributed to an identity yet; hold it until the outcome is known.
    if (read) await this.settled();
    if (generation !== this.generation)
      throw new Error(SESSION_CHANGED_MESSAGE);
    if (read && this.unavailable) throw new Error(SESSION_UNAVAILABLE_MESSAGE);
    if (
      service === AccountService.typeName &&
      methodName === "BeginDataDeletion"
    ) {
      const request = input as {
        mode?: string | number;
        confirmedAccountId?: string;
      };
      this.deletion = {
        challengeId:
          (toJson(method.output, response.message) as { challengeId?: string })
            .challengeId || "",
        generation,
        mode:
          request.mode === "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL" ||
          request.mode === 2
            ? DataDeletionMode.CLEAR_SERVER_KEEP_LOCAL
            : DataDeletionMode.DELETE_EVERYWHERE,
        account: request.confirmedAccountId || "",
      };
    }
    return toJson(method.output, response.message);
  }
  private async local(
    method: string,
    input: Record<string, unknown>,
  ): Promise<JsonValue> {
    switch (method) {
      case "GetLocalStatus":
        return {
          spaceId: "",
          deviceId: this.session?.device_id || "",
          syncEnabled: true,
          connected: !!this.session && !this.unavailable && navigator.onLine,
          pendingItemCount: "0",
          itemErrors: [],
        };
      case "GetNotificationPreferences":
        return {
          enabled: this.notificationEnabled,
          content: this.notificationContent,
          permission:
            typeof Notification === "undefined"
              ? 4
              : Notification.permission === "granted"
                ? 2
                : Notification.permission === "denied"
                  ? 3
                  : 1,
        };
      case "SetNotificationPreferences": {
        if (!this.session)
          throw new Error("Sign in before changing notifications.");
        // A save binds to the account device registered for the current identity,
        // so it follows the same reconciliation rules as other mutations: it is
        // refused while the session is being confirmed or could not be confirmed,
        // and it is never queued. The identity is captured here because the
        // permission prompt and service worker work below can outlive a focus
        // reconciliation that installs another account.
        if (this.reconciling) throw new Error(SESSION_RECONCILING_MESSAGE);
        if (this.unavailable) throw new Error(SESSION_UNAVAILABLE_MESSAGE);
        const generation = this.generation;
        const session = this.session;
        const ensureCurrent = () => {
          if (generation !== this.generation)
            throw new Error(SESSION_CHANGED_MESSAGE);
        };
        if (
          typeof Notification === "undefined" ||
          !("serviceWorker" in navigator) ||
          !("PushManager" in window)
        )
          throw new Error(
            "Web Push is unsupported here. In-app proposal counts remain available.",
          );
        const enabled = input.enabled === true;
        const content = Number(input.content) as NotificationContent;
        const permission = enabled
          ? await Notification.requestPermission()
          : Notification.permission;
        ensureCurrent();
        if (enabled && permission !== "granted")
          throw new Error(
            "Notification permission was not granted. In-app indications remain available.",
          );
        const registration =
          await navigator.serviceWorker.register("./push-worker.js");
        ensureCurrent();
        let subscription = await registration.pushManager.getSubscription();
        ensureCurrent();
        if (enabled && !subscription) {
          const config = (await this.auth("/auth/push-config")) as {
            public_key?: string;
          };
          ensureCurrent();
          if (!config.public_key)
            throw new Error(
              "Web Push is not configured. In-app indications remain available.",
            );
          const key = Uint8Array.from(
            atob(config.public_key.replace(/-/g, "+").replace(/_/g, "/")),
            (c) => c.charCodeAt(0),
          );
          // register() resolves while the worker may still be installing, and
          // PushManager.subscribe() rejects with InvalidStateError until this
          // registration has an active worker. Wait on the registration returned
          // above, never on navigator.serviceWorker.ready, which can resolve
          // for a different registration.
          await whenWorkerActive(registration);
          ensureCurrent();
          subscription = await registration.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey: key,
          });
        }
        // Nothing is sent for an identity that was replaced while this save waited.
        ensureCurrent();
        // The server owns per-device preferences, so every save is acknowledged
        // by RegisterPush even when this browser has no push subscription. A
        // disabled registration needs no endpoint or keys; the server validates
        // those fields only when enabled. The adapter state below is installed
        // only after this call resolves, and a failure is surfaced without retry.
        const json = subscription?.toJSON();
        await createClient(AccountService, this.transport).registerPush({
          meta: { requestId: uuid() },
          deviceId: session.device_id,
          provider: PushProvider.WEB_PUSH,
          endpoint: subscription?.endpoint ?? "",
          publicKey: decodeBase64(json?.keys?.p256dh || ""),
          authSecret: decodeBase64(json?.keys?.auth || ""),
          enabled,
          content,
        });
        // The response belongs to the earlier identity; it must not change the
        // browser subscription or the preferences installed for the new one.
        ensureCurrent();
        if (!enabled && subscription) await subscription.unsubscribe();
        ensureCurrent();
        this.notificationEnabled = enabled;
        this.notificationContent = content;
        return this.local("GetNotificationPreferences", {});
      }
      case "ConnectSync":
        return { authorizationUrl: this.loginUrl(), challengeId: "" };
      case "DisconnectSync":
        await this.auth("/auth/logout", {});
        this.endSession();
        return { completed: true };
      case "RemoveDeviceData":
        throw new Error("Web does not store Lane task data on this device.");
      case "ResetDevelopmentData":
        throw new Error(
          "Development data reset is available in the desktop and mobile apps only.",
        );
      default:
        throw new Error("Unsupported online operation.");
    }
  }
  async completeDeletion(challengeId: string) {
    // The reauthentication popup shares the browser cookie; confirm which
    // identity it belongs to before redeeming any proof.
    await this.reconcile();
    if (
      !this.deletion ||
      this.deletion.challengeId !== challengeId ||
      this.deletion.generation !== this.generation ||
      this.deletion.account !== this.session?.account_id
    )
      throw new Error(
        "Return to Settings and start data deletion again to confirm its target.",
      );
    const target = this.deletion;
    const { reauth_proof } = (await this.auth("/auth/reauth/proof", {
      challenge_id: challengeId,
    })) as { reauth_proof: string };
    if (this.deletion !== target || target.generation !== this.generation)
      throw new Error("The account changed. Start data deletion again.");
    await createClient(AccountService, this.transport).deleteAccountData({
      meta: { requestId: uuid() },
      mode: this.deletion.mode,
      confirmedAccountId: this.deletion.account,
      reauthProof: reauth_proof,
    });
    this.endSession();
  }
  private async settled() {
    while (this.reconciling) await this.reconciling.catch(() => {});
  }
  private startWatch() {
    clearTimeout(this.watchRetry);
    this.abort?.abort();
    const controller = new AbortController();
    this.abort = controller;
    // A stream belongs to the identity that opened it; its results are dropped
    // once the generation moves on.
    const generation = this.generation;
    const current = () =>
      !controller.signal.aborted && generation === this.generation;
    const client = createClient(SyncService, this.transport);
    void (async () => {
      try {
        const info = await client.getSyncInfo(
          {},
          { signal: controller.signal },
        );
        if (!current()) return;
        this.epoch = info.epoch;
        for await (const notification of client.watchChanges(
          { epoch: this.epoch, cursor: this.cursor },
          { signal: controller.signal },
        )) {
          // Pull every page before advancing the memory-only cursor; a notification is not data.
          let more = true;
          while (more) {
            const page = await client.pullChanges(
              {
                formatVersion: 3,
                epoch: this.epoch,
                cursor: this.cursor,
                pageSize: 100,
              },
              { signal: controller.signal },
            );
            if (!current()) return;
            this.cursor = page.nextCursor;
            more = page.hasMore;
          }
          void notification;
          if (!current()) return;
          this.retryDelay = 1000;
          this.options.invalidate();
        }
      } catch {
        if (current()) this.options.invalidate();
      } finally {
        if (current() && this.session) {
          this.watchRetry = setTimeout(
            () => this.startWatch(),
            this.retryDelay,
          );
          this.retryDelay = Math.min(this.retryDelay * 2, 30000);
        }
      }
    })();
  }
  // Focus and online resume. Credentials may have been replaced in an approval
  // popup, so the session is reconciled before the stream restarts or any
  // business query is invalidated. After a transient failure the user must retry
  // explicitly, so resume does nothing until start() succeeds.
  async resume() {
    if (!this.session || this.unavailable || !navigator.onLine) return;
    await this.reconcile();
    this.options.invalidate();
  }
  stop() {
    clearTimeout(this.watchRetry);
    this.abort?.abort();
  }
}
function decodeBase64(value: string) {
  return Uint8Array.from(
    atob(value.replace(/-/g, "+").replace(/_/g, "/")),
    (c) => c.charCodeAt(0),
  );
}
function uuid() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let time = BigInt(Date.now());
  for (let i = 5; i >= 0; i--) {
    bytes[i] = Number(time & 255n);
    time >>= 8n;
  }
  bytes[6] = (bytes[6] & 15) | 112;
  bytes[8] = (bytes[8] & 63) | 128;
  const h = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
