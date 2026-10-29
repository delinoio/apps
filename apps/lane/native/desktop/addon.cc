// SPDX-License-Identifier: Apache-2.0
#include "async_module.h"
#include "platform_runtime.h"
#include "realqa.h"
#include "../store/vendor/json/json.hpp"

namespace {
using namespace lane::desktop;
struct State {
  std::shared_ptr<PlatformRuntime> runtime;
  std::shared_ptr<Handler> capture;
  std::shared_ptr<Events> events = std::make_shared<Events>();
};
napi_value Configure(napi_env env, napi_callback_info info) {
  size_t count = 2;
  napi_value args[2], undefined;
  void* opaque = nullptr;
  napi_get_undefined(env, &undefined);
  napi_get_cb_info(env, info, &count, args, nullptr, &opaque);
  auto& state = *static_cast<State*>(opaque);
  std::string resources, scheme;
  if (count != 2 || !String(env, args[0], resources) || !String(env, args[1], scheme) ||
      (scheme != "lane-dev" && scheme != "lane") || state.runtime) {
    napi_throw_type_error(env, nullptr, "Expected a resource directory and Lane scheme, once per process");
    return undefined;
  }
  try { state.runtime = CreatePlatformRuntime(resources, scheme, state.events); state.capture = CreateCaptureHost(state.events); }
  catch (...) { napi_throw_error(env, nullptr, "Lane native service could not start"); }
  return undefined;
}
napi_value Activation(napi_env env, napi_callback_info info) {
  size_t count = 1;
  napi_value argument, result;
  napi_get_cb_info(env, info, &count, &argument, nullptr, nullptr);
  std::string resources, payload;
  if (count == 1 && String(env, argument, resources)) {
    try { payload = ReadNotificationActivation(resources); } catch (...) {}
  }
  napi_create_string_utf8(env, payload.data(), payload.size(), &result);
  return result;
}
napi_value Poll(napi_env env, napi_callback_info info) {
  size_t count = 0;
  void* opaque = nullptr;
  napi_get_cb_info(env, info, &count, nullptr, nullptr, &opaque);
  auto& state = *static_cast<State*>(opaque);
  if (state.runtime) state.runtime->Pump();
  auto events = nlohmann::json::array();
  for (auto& [name, arguments] : state.events->Drain())
    events.push_back({{"name", name}, {"arguments", nlohmann::json::parse(arguments)}});
  auto text = events.dump();
  napi_value result;
  napi_create_string_utf8(env, text.data(), text.size(), &result);
  return result;
}
}
NAPI_MODULE_INIT() {
  // The addon is owned by the main process, never by an individual Lynx view.
  auto state = std::make_shared<State>();
  for (const auto& item : std::vector<std::pair<const char*, napi_callback>>{{"configure", Configure}, {"poll", Poll}, {"activation", Activation}}) {
    napi_value function;
    napi_create_function(env, item.first, NAPI_AUTO_LENGTH, item.second, state.get(), &function);
    auto owner = new std::shared_ptr<State>(state);
    napi_add_finalizer(env, function, owner, [](napi_env, void* data, void*) {
      delete static_cast<std::shared_ptr<State>*>(data);
    }, nullptr, nullptr);
    napi_set_named_property(env, exports, item.first, function);
  }
  auto bind = [state](int kind) {
    return std::make_shared<Handler>([state, kind](const auto& method, const auto& arguments) {
      if (!state->runtime) return std::string(kUnavailable);
      auto handler = kind == 0 ? state->runtime->store : kind == 1 ? state->runtime->host : state->runtime->control;
      return (*handler)(method, arguments);
    });
  };
  napi_value store, host;
  napi_create_object(env, &store); napi_create_object(env, &host);
  auto storeHandler = bind(0), hostHandler = bind(1);
  StoreModule(env, store, "LaneStore", &storeHandler);
  HostModule(env, host, "LaneHost", &hostHandler);
  napi_set_named_property(env, exports, "store", store);
  napi_set_named_property(env, exports, "host", host);
  Export(env, exports, bind(2), "control", 2);
  Export(env, exports, std::make_shared<Handler>([state](const auto& method, const auto& args) {
    return state->capture ? (*state->capture)(method, args) : std::string(kUnavailable);
  }), "capture", 1);
  napi_value testing;
#ifdef LANE_NATIVE_TESTING
  napi_get_boolean(env, true, &testing);
#else
  napi_get_boolean(env, false, &testing);
#endif
  napi_set_named_property(env, exports, "testing", testing);
  return exports;
}
