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

export interface BridgeError {
  reason: string;
  message: string;
  correlationId?: string;
  retryable?: boolean;
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
}
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
  open(spaceId?: string): Promise<void>;
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
  private generation = 0;
  constructor(
    private module: LaneStoreModule,
    private events: EventSource,
  ) {}
  async open(spaceId = "") {
    if (this.info) this.module.close(this.info.spaceId, () => {});
    this.info = undefined;
    const generation = ++this.generation;
    const info = await new Promise<StoreInfo>((resolve, reject) =>
      this.module.open(JSON.stringify({ formatVersion: 1, spaceId }), (raw) => {
        try {
          resolve(decodeReply(raw));
        } catch (e) {
          reject(e);
        }
      }),
    );
    if (generation !== this.generation) {
      this.module.close(info.spaceId, () => {});
      throw new Error("The data space changed. Reopen Lane.");
    }
    if (info.schemaVersion !== 1 || info.formatVersion !== 1)
      throw new Error(
        "Update Lane to open this data format. Your data has not been changed.",
      );
    this.info = info;
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
    const response = await this.invoke<JsonValue>(
      method.name,
      toJson(method.input, fromUI(method.input, input)),
    );
    return toUI(fromJson(method.output, response));
  }
  local<K extends keyof LocalOperations>(
    method: K,
    input: LocalOperations[K][0],
  ): Promise<LocalOperations[K][1]> {
    return this.invoke(method, input);
  }
  subscribe(listener: () => void) {
    const changed = (event: { spaceId?: string; sequence?: string }) => {
      if (
        event.spaceId === this.info?.spaceId &&
        event.sequence !== this.info?.sequence
      ) {
        if (this.info && event.sequence) this.info.sequence = event.sequence;
        listener();
      }
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
