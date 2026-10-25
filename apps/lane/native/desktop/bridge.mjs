// SPDX-License-Identifier: Apache-2.0
const methods = Object.freeze({
  LaneStore: Object.freeze({ open: 1, invoke: 2, close: 1 }),
  LaneHost: Object.freeze({
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
