// SPDX-License-Identifier: Apache-2.0
// Worker-side AnyDashHost module; the page host answers in onNativeModulesCall.
export default (_modules, call) => ({
  call(method, payload, callback) {
    call("call", { method, payload }).then(callback, () =>
      callback(
        JSON.stringify({
          ok: false,
          error: { message: "The browser host is unavailable." },
        }),
      ),
    );
  },
});
