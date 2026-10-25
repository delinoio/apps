// SPDX-License-Identifier: Apache-2.0
import { toUI, fromUI, type UI } from "./serializable";
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
import {
  decodeReply,
  NativeAdapter,
  type LaneAdapter,
  type LaneStoreModule,
  type LocalOperations,
  type Reply,
  type EventSource,
} from "./adapter";
interface OnlineModule {
  ready(callback: () => void): void;
  invoke(
    service: string,
    method: string,
    request: string,
    callback: (raw: string | Reply<JsonValue>) => void,
  ): void;
}
export function getAdapter(): LaneAdapter {
  "background only";
  const events = lynx.getJSModule(
    "GlobalEventEmitter",
  ) as unknown as EventSource;
  if (NativeModules.LaneOnline) {
    const module = NativeModules.LaneOnline as OnlineModule;
    return {
      mode: "web",
      open: async () => {
        await new Promise<void>((resolve) => module.ready(resolve));
      },
      close: () => {},
      async call<I extends DescMessage, O extends DescMessage>(
        method: DescMethodUnary<I, O>,
        input: UI<MessageInitShape<I>>,
      ): Promise<UI<MessageShape<O>>> {
        const raw = await new Promise<JsonValue>((resolve, reject) =>
          module.invoke(
            method.parent.typeName,
            method.name,
            JSON.stringify(toJson(method.input, fromUI(method.input, input))),
            (raw) => {
              try {
                resolve(decodeReply(raw));
              } catch (e) {
                reject(e);
              }
            },
          ),
        );
        return toUI(fromJson(method.output, raw));
      },
      async local<K extends keyof LocalOperations>(
        method: K,
        input: LocalOperations[K][0],
      ): Promise<LocalOperations[K][1]> {
        return new Promise((resolve, reject) =>
          module.invoke("local", method, JSON.stringify(input), (raw) => {
            try {
              resolve(decodeReply(raw) as LocalOperations[K][1]);
            } catch (e) {
              reject(e);
            }
          }),
        );
      },
      subscribe(listener) {
        events.addListener("lane.onlineChanged", listener);
        return () => events.removeListener("lane.onlineChanged", listener);
      },
    };
  }
  if (!NativeModules.LaneStore)
    throw new Error(
      "Lane storage is unavailable. Install a host with LaneStore support.",
    );
  return new NativeAdapter(NativeModules.LaneStore as LaneStoreModule, events);
}
export function hostAction(
  method: "copyText" | "openURL" | "cancelReauthentication",
  value: string,
): Promise<void> {
  "background only";
  if (method === "openURL" && !/^https:\/\//i.test(value))
    return Promise.reject(new Error("Only HTTPS links can be opened."));
  return new Promise((resolve, reject) => {
    const host = NativeModules.LaneHost;
    if (!host?.[method]) {
      reject(new Error("This host does not support this action."));
      return;
    }
    host[method](
      value,
      (raw: string | { ok: boolean; error?: { message: string } }) => {
        const reply = typeof raw === "string" ? JSON.parse(raw) : raw;
        if (reply.ok) resolve();
        else reject(new Error(reply.error?.message || "The action failed."));
      },
    );
  });
}

export interface OpenRoute {
  kind: "open" | "repository" | "task";
  id?: string;
  spaceId?: string;
}
export function listenHost(
  route: (route: OpenRoute) => void,
  error: (message: string) => void,
  ended: () => void,
) {
  "background only";
  const events = lynx.getJSModule("GlobalEventEmitter");
  const onRoute = (raw: unknown) => {
    if (raw && typeof raw === "object") route(raw as OpenRoute);
  };
  const onError = (raw: unknown) => {
    const value = raw as
      { message?: string; error?: { message?: string } } | undefined;
    error(
      value?.error?.message || value?.message || "The host operation failed.",
    );
  };
  events.addListener("lane.openRoute", onRoute);
  events.addListener("lane.hostError", onError);
  events.addListener("lane.sessionEnded", ended);
  return () => {
    events.removeListener("lane.openRoute", onRoute);
    events.removeListener("lane.hostError", onError);
    events.removeListener("lane.sessionEnded", ended);
  };
}

export function listenReauthentication(ready: (challengeId: string) => void) {
  "background only";
  const events = lynx.getJSModule("GlobalEventEmitter");
  const listener = (raw: unknown) => {
    const value = raw as { challengeId?: unknown };
    if (typeof value?.challengeId === "string") ready(value.challengeId);
  };
  events.addListener("lane.reauthenticated", listener);
  return () => events.removeListener("lane.reauthenticated", listener);
}
