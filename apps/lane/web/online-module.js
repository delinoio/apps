// SPDX-License-Identifier: Apache-2.0
export default (_modules, call) => ({
  invoke(service, method, request, callback) {
    call("invoke", { service, method, request }).then(callback, () =>
      callback({
        ok: false,
        error: {
          reason: "ONLINE_REQUIRED",
          message: "Online access failed. Retain your input and retry.",
        },
      }),
    );
  },
});
