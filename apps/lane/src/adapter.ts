// SPDX-License-Identifier: Apache-2.0
import { toUI, fromUI, type UI } from "./serializable.ts";
import {
  create,
  fromJson,
  toJson,
  type DescMessage,
  type DescMethodUnary,
  type MessageInitShape,
  type MessageShape,
  type JsonValue,
} from "@bufbuild/protobuf";

import { AccountService } from "@repo/protos/lane/v1/lane_pb";

export interface BridgeError {
  reason: string;
  message: string;
  correlationId?: string;
  retryable?: boolean;
  // Named ConstraintViolation (for example PARENT_CYCLE) when reason is CONSTRAINT_VIOLATION.
  violation?: string;
}
export type Reply<T> =
  | { ok: true; response: T; sequence?: string }
  | { ok: false; error: BridgeError };
export interface StoreInfo {
  spaceId: string;
  deviceId: string;
  schemaVersion: number;
  formatVersion: number;
  sequence: string;
}
export interface LaneStoreModule {
  open(
    requestJson: string,
    callback: (value: string | Reply<StoreInfo>) => void,
  ): void;
  invoke(
    method: string,
    requestJson: string,
    callback: (value: string | Reply<JsonValue>) => void,
  ): void;
  close(spaceId: string, callback: (value: unknown) => void): void;
}
export interface NotificationPreferences {
  enabled: boolean;
  content: number | string;
  permission: number | string;
}
export interface LocalStatus {
  spaceId: string;
  deviceId: string;
  syncEnabled: boolean;
  reconnectRequired?: boolean;
  accountId?: string;
  syncErrorReason?: string;
  connected: boolean;
  pendingItemCount: string;
  lastSyncAt?: string;
  itemErrors?: { entityId: string; reason: string }[];
  notificationPreferences?: NotificationPreferences;
}
export interface LocalOperations {
  GetLocalStatus: [{}, LocalStatus];
  GetNotificationPreferences: [{}, NotificationPreferences];
  SetNotificationPreferences: [
    { enabled: boolean; content: number },
    NotificationPreferences,
  ];
  ConnectSync: [
    { accountId: string },
    { authorizationUrl: string; challengeId: string },
  ];
  DisconnectSync: [{ retention: number }, { completed: boolean }];
  RemoveDeviceData: [{}, { completed: boolean }];
  // Explicit pre-release development reset; the host rejects any request whose
  // confirmed flag is not true. Native only: the web host refuses it.
  ResetDevelopmentData: [{ confirmed: boolean }, { completed: boolean }];
  // Returns blocked or rejected pending rows to pending, like `sync run --retry-blocked`.
  RetryBlockedItems: [{}, Record<string, unknown>];
}
// The format written by task planning. Earlier formats are refused, never migrated.
export const STORE_FORMAT_VERSION = 3;
export class LaneError extends Error {
  constructor(readonly detail: BridgeError) {
    super(detail.message);
  }
}
export function decodeReply<T>(raw: string | Reply<T>): T {
  const value: Reply<T> = typeof raw === "string" ? JSON.parse(raw) : raw;
  if (!value.ok) throw new LaneError(value.error);
  return value.response;
}
export interface LaneAdapter {
  readonly mode: "native" | "web";
  readonly spaceId?: string;
  deleteReauthenticated?(challengeId: string): Promise<void>;
  cancelDeletion?(): void;
  open(spaceId?: string): Promise<void>;
  // Native only: reset the active space when it could not be opened (unsupported format).
  resetUnopened?(): Promise<void>;
  call<I extends DescMessage, O extends DescMessage>(
    method: DescMethodUnary<I, O>,
    input: UI<MessageInitShape<I>>,
  ): Promise<UI<MessageShape<O>>>;
  local<K extends keyof LocalOperations>(
    method: K,
    input: LocalOperations[K][0],
  ): Promise<LocalOperations[K][1]>;
  subscribe(listener: () => void): () => void;
  close(): void;
}
export interface EventSource {
  addListener(
    name: string,
    cb: (event: { spaceId?: string; sequence?: string }) => void,
  ): void;
  removeListener(
    name: string,
    cb: (event: { spaceId?: string; sequence?: string }) => void,
  ): void;
}
export class NativeAdapter implements LaneAdapter {
  readonly mode = "native";
  private info?: StoreInfo;
  get spaceId() {
    return this.info?.spaceId;
  }
  private generation = 0;
  private deletion?: {
    generation: number;
    challengeId: string;
    request: Record<string, unknown>;
  };
  constructor(
    private module: LaneStoreModule,
    private events: EventSource,
  ) {}
  async open(spaceId = "") {
    const previous = this.info;
    this.info = undefined;
    this.deletion = undefined;
    const generation = ++this.generation;
    if (previous)
      await new Promise<void>((resolve, reject) =>
        this.module.close(previous.spaceId, (raw) => {
          try {
            const response = typeof raw === "string" ? JSON.parse(raw) : raw;
            if (
              response &&
              typeof response === "object" &&
              "ok" in response &&
              !response.ok
            )
              throw new Error("Could not close the previous data space.");
            resolve();
          } catch (e) {
            reject(e);
          }
        }),
      );
    if (generation !== this.generation)
      throw new Error("The data space changed. Reopen Lane.");
    const info = await new Promise<StoreInfo>((resolve, reject) =>
      this.module.open(
        JSON.stringify({ formatVersion: STORE_FORMAT_VERSION, spaceId }),
        (raw) => {
          try {
            resolve(decodeReply(raw));
          } catch (e) {
            reject(e);
          }
        },
      ),
    );
    if (generation !== this.generation) {
      this.module.close(info.spaceId, () => {});
      throw new Error("The data space changed. Reopen Lane.");
    }
    if (spaceId && info.spaceId !== spaceId) {
      this.module.close(info.spaceId, () => {});
      throw new Error(
        "The host opened a different data space. Navigation was stopped.",
      );
    }
    if (
      info.schemaVersion !== STORE_FORMAT_VERSION ||
      info.formatVersion !== STORE_FORMAT_VERSION
    ) {
      await new Promise<void>((resolve) =>
        this.module.close(info.spaceId, () => resolve()),
      );
      throw new Error(
        "This data space uses an unsupported format. Lane never migrates or resets it automatically; update Lane or explicitly reset disposable development data. Your data has not been changed.",
      );
    }
    this.info = info;
  }
  async resetUnopened() {
    // The host resolves the empty space ID to the registry's active space.
    await this.resetSpace("");
  }
  private resetSpace(spaceId: string) {
    return new Promise<void>((resolve, reject) =>
      this.module.invoke(
        "ResetDevelopmentData",
        JSON.stringify({ spaceId, request: { confirmed: true } }),
        (raw) => {
          try {
            decodeReply(raw);
            resolve();
          } catch (e) {
            reject(e);
          }
        },
      ),
    );
  }
  // The host refuses a development reset with a retryable STORAGE_BUSY while
  // any handle of the space is open, including this window's own. Close the
  // handle first; callers reopen the space after a successful reset, and a
  // refused reset restores the handle so the window keeps working.
  private async resetOpen() {
    const open = this.info;
    if (!open) throw new Error("The local store is not open.");
    this.info = undefined;
    this.deletion = undefined;
    ++this.generation;
    await new Promise<void>((resolve) =>
      this.module.close(open.spaceId, () => resolve()),
    );
    try {
      await this.resetSpace(open.spaceId);
    } catch (reset) {
      await this.open(open.spaceId);
      throw reset;
    }
    return { completed: true };
  }
  private async invoke<T>(method: string, request: unknown): Promise<T> {
    if (!this.info) throw new Error("The local store is not open.");
    const generation = this.generation;
    const spaceId = this.info.spaceId;
    return new Promise((resolve, reject) =>
      this.module.invoke(
        method,
        JSON.stringify({ spaceId, request }),
        (raw) => {
          try {
            if (generation !== this.generation)
              throw new Error(
                "The data space changed. Reload before continuing.",
              );
            resolve(decodeReply(raw) as T);
          } catch (e) {
            reject(e);
          }
        },
      ),
    );
  }
  async call<I extends DescMessage, O extends DescMessage>(
    method: DescMethodUnary<I, O>,
    input: UI<MessageInitShape<I>>,
  ): Promise<UI<MessageShape<O>>> {
    if (
      method.name === AccountService.method.beginDataDeletion.name &&
      method.parent.typeName === AccountService.typeName
    )
      this.deletion = undefined;
    const response = await this.invoke<JsonValue>(
      method.name,
      toJson(method.input, fromUI(method.input, input)),
    );
    if (
      method.name === AccountService.method.beginDataDeletion.name &&
      method.parent.typeName === AccountService.typeName
    ) {
      const data = response as { challengeId?: string };
      const request = toJson(
        method.input,
        fromUI(method.input, input),
      ) as Record<string, unknown>;
      this.deletion = {
        generation: this.generation,
        challengeId: data.challengeId || "",
        request,
      };
    }
    return toUI(fromJson(method.output, response));
  }
  local<K extends keyof LocalOperations>(
    method: K,
    input: LocalOperations[K][0],
  ): Promise<LocalOperations[K][1]> {
    if (method === "ResetDevelopmentData")
      return this.resetOpen() as Promise<LocalOperations[K][1]>;
    return this.invoke(method, input);
  }
  cancelDeletion() {
    this.deletion = undefined;
  }
  async deleteReauthenticated(challengeId: string) {
    const pending = this.deletion;
    if (
      !pending ||
      !challengeId ||
      pending.challengeId !== challengeId ||
      pending.generation !== this.generation
    )
      throw new Error(
        "The deletion approval expired. Start again in this account.",
      );
    await this.invoke("DeleteAccountData", {
      meta: meta(),
      mode: pending.request.mode,
      confirmedAccountId: pending.request.confirmedAccountId,
      challengeId,
    });
    this.deletion = undefined;
  }

  subscribe(listener: () => void) {
    // Every subscription tracks the newest sequence it has accepted. Sharing
    // one counter would let the first callback consume an event on behalf of
    // all other subscribers. The baseline is rebased from the open store the
    // first time an event arrives after the store generation changes, which
    // covers registration before open(), reopening and data-space switches.
    let observed: { generation: number; sequence: bigint } | undefined;
    const changed = (event: { spaceId?: string; sequence?: string }) => {
      const info = this.info;
      if (
        !info ||
        event.spaceId !== info.spaceId ||
        !event.sequence ||
        !/^\d+$/.test(event.sequence)
      )
        return;
      if (!observed || observed.generation !== this.generation)
        observed = {
          generation: this.generation,
          sequence: BigInt(
            /^\d+$/.test(info.sequence || "") ? info.sequence : "0",
          ),
        };
      const sequence = BigInt(event.sequence);
      if (sequence <= observed.sequence) return;
      observed.sequence = sequence;
      listener();
    };
    this.events.addListener("lane.storeChanged", changed);
    return () => this.events.removeListener("lane.storeChanged", changed);
  }
  close() {
    ++this.generation;
    if (this.info) this.module.close(this.info.spaceId, () => {});
    this.info = undefined;
  }
}

// IDs are correlation/identity values, never authentication or signing secrets.
export function newId(): string {
  const time = Date.now().toString(16).padStart(12, "0");
  const random = () => Math.floor(Math.random() * 16).toString(16);
  return `${time.slice(0, 8)}-${time.slice(8)}-7${Array.from({ length: 3 }, random).join("")}-${(8 + Math.floor(Math.random() * 4)).toString(16)}${Array.from({ length: 3 }, random).join("")}-${Array.from({ length: 12 }, random).join("")}`;
}
export const meta = () => ({ requestId: newId() });
