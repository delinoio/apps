// SPDX-License-Identifier: Apache-2.0
const methods = Object.freeze({
  LaneStore: Object.freeze({ open: 1, invoke: 2, close: 1 }),
  LaneHost: Object.freeze({
    realqa: 1,
    workspace: 1,
    update: 1,
    copyText: 1,
    openURL: 1,
    cancelReauthentication: 1,
  }),
});
export const unavailable = JSON.stringify({
  ok: false,
  error: {
    reason: "ERROR_REASON_DEPENDENCY_UNAVAILABLE",
    message: "Native service is unavailable. Local data is retained.",
    retryable: true,
  },
});
const unownedHandle = JSON.stringify({
  ok: false,
  error: {
    reason: "ERROR_REASON_INVALID_INPUT",
    message: "This window has no open handle for the data space.",
    retryable: false,
  },
});
const parseReply = (raw) => {
  try {
    return JSON.parse(raw);
  } catch {
    return null;
  }
};
export function invokeNative(native, request) {
  const { module, method, args } = request ?? {};
  if (
    !Object.hasOwn(methods, module ?? "") ||
    !Object.hasOwn(methods[module], method ?? "") ||
    !Array.isArray(args) ||
    args.length !== methods[module][method] ||
    !args.every((value) => typeof value === "string")
  ) {
    return Promise.resolve(
      JSON.stringify({
        ok: false,
        error: {
          reason: "ERROR_REASON_INVALID_INPUT",
          message: "Unknown native operation or invalid arguments.",
          retryable: false,
        },
      }),
    );
  }
  const target = module === "LaneStore" ? native.store : native.host;
  return new Promise((resolve) => {
    try {
      target[method](...args, resolve);
    } catch {
      resolve(unavailable);
    }
  });
}
export function control(native, method, value = "") {
  return new Promise((resolve, reject) =>
    native.control(method, value, (raw) => {
      const reply = JSON.parse(raw);
      if (reply.ok) resolve();
      else
        reject(
          new Error(reply.error?.message || "Native host operation failed."),
        );
    }),
  );
}

export class WindowSession {
  ready = false;
  #active = true;
  #handles = new Map();
  #store;
  #storeTail = Promise.resolve();
  #report;

  constructor(report = () => {}) {
    this.#report = report;
  }

  #enqueue(operation) {
    const pending = this.#storeTail.then(operation);
    this.#storeTail = pending.then(
      () => {},
      () => {},
    );
    return pending;
  }

  async #release(spaceId) {
    let raw;
    try {
      raw = await invokeNative(
        { store: this.#store },
        { module: "LaneStore", method: "close", args: [spaceId] },
      );
    } catch {
      raw = unavailable;
    }
    if (parseReply(raw)?.ok !== true) {
      this.#report("native_close_failed");
      return raw;
    }
    const remaining = this.#handles.get(spaceId) - 1;
    if (remaining) this.#handles.set(spaceId, remaining);
    else this.#handles.delete(spaceId);
    return raw;
  }

  retire() {
    this.#active = false;
    this.ready = false;
    return this.#enqueue(async () => {
      for (const [spaceId] of this.#handles) {
        while (this.#handles.has(spaceId)) {
          const previous = this.#handles.get(spaceId);
          await this.#release(spaceId);
          if (this.#handles.get(spaceId) === previous) break;
        }
      }
    });
  }

  async invoke(native, request) {
    if (!this.#active) return unavailable;
    if (
      request?.module !== "LaneStore" ||
      !["open", "close"].includes(request.method)
    )
      return invokeNative(native, request);
    this.#store = native.store;
    return this.#enqueue(async () => {
      if (request.method === "close") {
        const spaceId = request.args?.[0];
        if (!this.#handles.has(spaceId)) return unownedHandle;
        return this.#release(spaceId);
      }
      let raw;
      try {
        raw = await invokeNative(native, request);
      } catch {
        raw = unavailable;
      }
      const reply = parseReply(raw);
      if (reply?.ok !== true) return raw;
      const spaceId = reply.response?.spaceId;
      if (typeof spaceId !== "string" || !spaceId) {
        this.#report("invalid_open_response");
        return unavailable;
      }
      this.#handles.set(spaceId, (this.#handles.get(spaceId) ?? 0) + 1);
      // Retirement queues its drain after accepted opens and closes. A late
      // completion belongs to this session, never to its replacement.
      if (this.#active) this.ready = true;
      return raw;
    });
  }
}
