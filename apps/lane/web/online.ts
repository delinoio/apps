// SPDX-License-Identifier: Apache-2.0
import { createClient } from "@connectrpc/connect";
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
  LaneService,
  SyncService,
  WebhookService,
  PushProvider,
  NotificationContent,
  DataDeletionMode,
} from "@repo/protos/lane/v1/lane_pb";
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
export class OnlineLane {
  private session?: Session;
  private generation = 0;
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
      throw new Error(
        response.status === 401
          ? "Sign in to Lane to continue."
          : `Account request failed (${response.status}). Retry explicitly.`,
      );
    return response.status === 204 ? {} : response.json();
  }
  async start() {
    const session = (await this.auth("/auth/session")) as Session;
    if (
      this.session?.account_id !== session.account_id ||
      this.session?.device_id !== session.device_id
    ) {
      ++this.generation;
      this.deletion = undefined;
      this.cursor = "";
      this.epoch = "";
    }
    this.session = session;
    this.notificationEnabled =
      session.notification_preferences?.enabled ?? false;
    this.notificationContent =
      session.notification_preferences?.content ?? NotificationContent.DETAIL;
    this.options.sessionChanged(this.session);
    this.startWatch();
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
    if (!this.session) throw new Error("Sign in to Lane to continue.");
    const serviceDescriptor = [
      LaneService,
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
    const generation = this.generation;
    const response = await this.transport.unary(
      method as DescMethodUnary,
      undefined,
      30000,
      undefined,
      fromJson(method.input, input),
    );
    if (generation !== this.generation)
      throw new Error("The account changed. Reload before continuing.");
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
          connected: !!this.session && navigator.onLine,
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
        if (enabled && permission !== "granted")
          throw new Error(
            "Notification permission was not granted. In-app indications remain available.",
          );
        const registration =
          await navigator.serviceWorker.register("./push-worker.js");
        let subscription = await registration.pushManager.getSubscription();
        if (enabled && !subscription) {
          const config = (await this.auth("/auth/push-config")) as {
            public_key?: string;
          };
          if (!config.public_key)
            throw new Error(
              "Web Push is not configured. In-app indications remain available.",
            );
          const key = Uint8Array.from(
            atob(config.public_key.replace(/-/g, "+").replace(/_/g, "/")),
            (c) => c.charCodeAt(0),
          );
          subscription = await registration.pushManager.subscribe({
            userVisibleOnly: true,
            applicationServerKey: key,
          });
        }
        if (subscription) {
          const json = subscription.toJSON();
          await createClient(AccountService, this.transport).registerPush({
            meta: { requestId: uuid() },
            deviceId: this.session.device_id,
            provider: PushProvider.WEB_PUSH,
            endpoint: subscription.endpoint,
            publicKey: decodeBase64(json.keys?.p256dh || ""),
            authSecret: decodeBase64(json.keys?.auth || ""),
            enabled,
            content,
          });
          if (!enabled) await subscription.unsubscribe();
        }
        this.notificationEnabled = enabled;
        this.notificationContent = content;
        return this.local("GetNotificationPreferences", {});
      }
      case "ConnectSync":
        return { authorizationUrl: this.loginUrl(), challengeId: "" };
      case "DisconnectSync":
        await this.auth("/auth/logout", {});
        this.stop();
        this.session = undefined;
        ++this.generation;
        this.options.sessionChanged(undefined);
        return { completed: true };
      case "RemoveDeviceData":
        throw new Error("Web does not store Lane task data on this device.");
      default:
        throw new Error("Unsupported online operation.");
    }
  }
  async completeDeletion(challengeId: string) {
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
    this.deletion = undefined;
    this.stop();
    this.session = undefined;
    ++this.generation;
    this.options.sessionChanged(undefined);
  }
  private startWatch() {
    clearTimeout(this.watchRetry);
    this.abort?.abort();
    const controller = new AbortController();
    this.abort = controller;
    const client = createClient(SyncService, this.transport);
    void (async () => {
      try {
        const info = await client.getSyncInfo(
          {},
          { signal: controller.signal },
        );
        if (controller.signal.aborted) return;
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
                formatVersion: 2,
                epoch: this.epoch,
                cursor: this.cursor,
                pageSize: 100,
              },
              { signal: controller.signal },
            );
            this.cursor = page.nextCursor;
            more = page.hasMore;
          }
          void notification;
          this.retryDelay = 1000;
          this.options.invalidate();
        }
      } catch {
        if (!controller.signal.aborted) this.options.invalidate();
      } finally {
        if (!controller.signal.aborted && this.session) {
          this.watchRetry = setTimeout(
            () => this.startWatch(),
            this.retryDelay,
          );
          this.retryDelay = Math.min(this.retryDelay * 2, 30000);
        }
      }
    })();
  }
  resume() {
    if (this.session) {
      this.startWatch();
      this.options.invalidate();
    }
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
