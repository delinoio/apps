// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <functional>
#include <memory>
#include <string>
#include <vector>
#include "lynx_native_module.h"
#include "third_party/weak-node-api/headers/weak_napi_defines.h"

namespace lane::desktop {
// Execute runs on the SDK worker pool. Complete runs on the originating BTS
// runtime. Never retain a napi_value or invoke a callback from a host DB thread.
using Handler = std::function<std::string(const std::string&, const std::vector<std::string>&)>;
struct Binding { std::shared_ptr<Handler> handler; std::string method; size_t arguments; };
struct Work {
  std::shared_ptr<Handler> handler;
  std::string method, response;
  std::vector<std::string> arguments;
  napi_ref callback = nullptr;
  napi_async_work work = nullptr;
};
inline constexpr const char* kUnavailable = R"({"ok":false,"error":{"reason":"ERROR_REASON_DEPENDENCY_UNAVAILABLE","message":"Native operation failed. Retry the operation.","retryable":true}})";
inline bool String(napi_env env, napi_value value, std::string& output) {
  size_t length = 0;
  if (napi_get_value_string_utf8(env, value, nullptr, 0, &length) != napi_ok) return false;
  std::vector<char> bytes(length + 1);
  if (napi_get_value_string_utf8(env, value, bytes.data(), bytes.size(), &length) != napi_ok) return false;
  output.assign(bytes.data(), length);
  return true;
}
inline void Complete(napi_env env, napi_status status, void* data) {
  std::unique_ptr<Work> work(static_cast<Work*>(data));
  napi_value callback, receiver, response, ignored;
  if (status == napi_ok && napi_get_reference_value(env, work->callback, &callback) == napi_ok &&
      napi_get_undefined(env, &receiver) == napi_ok &&
      napi_create_string_utf8(env, work->response.data(), work->response.size(), &response) == napi_ok) {
    napi_call_function(env, receiver, callback, 1, &response, &ignored);
  }
  napi_delete_reference(env, work->callback);
  napi_delete_async_work(env, work->work);
}
inline napi_value Call(napi_env env, napi_callback_info info) {
  size_t count = 4;
  napi_value args[4], undefined;
  void* data = nullptr;
  napi_get_undefined(env, &undefined);
  if (napi_get_cb_info(env, info, &count, args, nullptr, &data) != napi_ok || !data) return undefined;
  const auto& binding = *static_cast<Binding*>(data);
  if (count != binding.arguments + 1) {
    napi_throw_type_error(env, nullptr, "Expected string arguments and a callback");
    return undefined;
  }
  auto work = std::make_unique<Work>();
  work->handler = binding.handler;
  work->method = binding.method;
  for (size_t i = 0; i < binding.arguments; ++i) {
    std::string value;
    if (!String(env, args[i], value)) {
      napi_throw_type_error(env, nullptr, "Expected a string argument");
      return undefined;
    }
    work->arguments.push_back(std::move(value));
  }
  napi_valuetype type;
  if (napi_typeof(env, args[binding.arguments], &type) != napi_ok || type != napi_function) {
    napi_throw_type_error(env, nullptr, "Expected a callback");
    return undefined;
  }
  napi_value name;
  napi_create_string_utf8(env, "Lane native operation", NAPI_AUTO_LENGTH, &name);
  if (napi_create_reference(env, args[binding.arguments], 1, &work->callback) != napi_ok) return undefined;
  auto status = napi_create_async_work(env, nullptr, name,
    [](napi_env, void* data) {
      auto& work = *static_cast<Work*>(data);
      try { work.response = (*work.handler)(work.method, work.arguments); }
      catch (...) { work.response = kUnavailable; }
    }, Complete, work.get(), &work->work);
  if (status == napi_ok) status = napi_queue_async_work(env, work->work);
  if (status != napi_ok) {
    if (work->work) napi_delete_async_work(env, work->work);
    napi_delete_reference(env, work->callback);
    napi_throw_error(env, nullptr, "Native worker is unavailable");
    return undefined;
  }
  work.release();
  return undefined;
}
inline void Export(napi_env env, napi_value exports, std::shared_ptr<Handler> handler,
                   const char* method, size_t arguments) {
  auto binding = std::make_unique<Binding>(Binding{std::move(handler), method, arguments});
  napi_value function;
  if (napi_create_function(env, method, NAPI_AUTO_LENGTH, Call, binding.get(), &function) != napi_ok) return;
  if (napi_add_finalizer(env, function, binding.get(),
      [](napi_env, void* data, void*) { delete static_cast<Binding*>(data); }, nullptr, nullptr) != napi_ok) return;
  binding.release();
  napi_set_named_property(env, exports, method, function);
}
inline napi_value StoreModule(napi_env env, napi_value exports, const char*, void* opaque) {
  auto handler = *static_cast<std::shared_ptr<Handler>*>(opaque);
  Export(env, exports, handler, "open", 1);
  Export(env, exports, handler, "invoke", 2);
  Export(env, exports, handler, "close", 1);
  return exports;
}
inline napi_value HostModule(napi_env env, napi_value exports, const char*, void* opaque) {
  auto handler = *static_cast<std::shared_ptr<Handler>*>(opaque);
  Export(env, exports, handler, "copyText", 1);
  Export(env, exports, handler, "openURL", 1);
  Export(env, exports, handler, "cancelReauthentication", 1);
  return exports;
}
}  // namespace lane::desktop
