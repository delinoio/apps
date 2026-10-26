// SPDX-License-Identifier: Apache-2.0
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include "platform_runtime.h"
#include "windows/notifications.h"
#include "windows/service.h"

namespace lane::desktop {
namespace {
using namespace lane::windows;
LRESULT CALLBACK Dispatch(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
  if (message == WM_NCCREATE) {
    auto* create = reinterpret_cast<CREATESTRUCTW*>(lparam);
    SetWindowLongPtrW(window, GWLP_USERDATA, reinterpret_cast<LONG_PTR>(create->lpCreateParams));
  }
  auto* queue = reinterpret_cast<UIQueue*>(GetWindowLongPtrW(window, GWLP_USERDATA));
  if (message == kDispatch && queue) { queue->Drain(); return 0; }
  return DefWindowProcW(window, message, wparam, lparam);
}
struct WindowsRuntime : PlatformRuntime {
  HWND controller = nullptr;
  std::shared_ptr<UIQueue> ui = std::make_shared<UIQueue>();
  std::shared_ptr<Notifications> notifications = std::make_shared<Notifications>();
  std::shared_ptr<Service> service;
  std::shared_ptr<Events> events;
  std::string scheme;
  WindowsRuntime(const std::string& resources, std::string protocol, std::shared_ptr<Events> sink)
      : events(std::move(sink)), scheme(std::move(protocol)) {
    ApartmentScope apartment;
    WNDCLASSW type{};
    type.lpfnWndProc = Dispatch; type.hInstance = GetModuleHandleW(nullptr);
    type.lpszClassName = L"LaneLynxtronDispatch";
    if (!RegisterClassW(&type) && GetLastError() != ERROR_CLASS_ALREADY_EXISTS)
      throw std::runtime_error("Controller registration failed");
    controller = CreateWindowExW(0, type.lpszClassName, L"", 0, 0, 0, 0, 0,
      HWND_MESSAGE, nullptr, type.hInstance, ui.get());
    if (!controller) throw std::runtime_error("Controller creation failed");
    ui->Attach(controller);
    const auto directory = std::filesystem::u8path(resources);
    notifications->Start(directory, [events = events](std::string payload) {
      try {
        auto value = Parse(payload);
        if (Text(value, L"kind") != "task" || !lane::windows::UUID(Text(value, L"id")) || !lane::windows::UUID(Text(value, L"spaceId"))) return;
        events->Push("lane.activate", "[]");
        events->Push("lane.openRoute", "[" + Dump(value) + "]");
      } catch (...) { Log("lane_notification_activation_failed"); }
    });
    HostActions actions;
    actions.event = [events = events](std::string name, std::string argument) {
      events->Push(std::move(name), "[" + argument + "]");
    };
    actions.permission = [ui = ui, notifications = notifications] {
      return ui->Call([notifications] { return notifications->Permission(); });
    };
    actions.notify = [ui = ui, notifications = notifications](Json item) {
      return ui->Call([notifications, item] { return notifications->Show(item) ? "accepted" : "unavailable"; }) == "accepted";
    };
    actions.browse = [ui = ui, window = controller](std::string url) {
      auto response = ui->Call([window, url] { return OpenURL(window, url); });
      if (!Parse(response).GetNamedBoolean(L"ok", false)) throw std::runtime_error("Browser launch failed");
    };
    service = std::make_shared<Service>(directory / "helpers" / "lane.exe", scheme, std::move(actions));
    store = std::make_shared<Handler>([service = service](const auto& method, const auto& args) {
      return service->Call(method, args);
    });
    host = std::make_shared<Handler>([service = service, ui = ui, window = controller](const auto& method, const auto& args) {
      if (method == "cancelReauthentication") return service->Call(method, args);
      return ui->Call([window, method, value = args.at(0)] {
        return method == "copyText" ? CopyText(window, value) : OpenURL(window, value);
      });
    });
    control = std::make_shared<Handler>([this](const std::string&, const std::vector<std::string>& args) {
      ApartmentScope apartment;
      const auto& method = args.at(0);
      if (method == "resume") service->Resume();
      else if (method == "viewClosed") service->InvalidateReauthentication();
      else if (method == "url") {
        Route route;
        if (!ParseRoute(args.at(1), scheme, route)) return Failure("ERROR_REASON_INVALID_INPUT", "Invalid Lane link.");
        events->Push("lane.activate", "[]");
        if (route.kind == RouteKind::Callback) service->Callback(route.uri);
        else {
          Json value;
          Set(value, L"kind", route.kind == RouteKind::Task ? "task" : route.kind == RouteKind::Repository ? "repository" : "open");
          if (!route.id.empty()) Set(value, L"id", route.id);
          events->Push("lane.openRoute", "[" + Dump(value) + "]");
        }
      } else if (method == "stop") {
        ui->Call([notifications = notifications] { notifications->Stop(); return std::string{}; });
        ui->Stop(); service->Stop();
      } else throw std::runtime_error("Unknown host control operation");
      return std::string("{\"ok\":true}");
    });
  }
  void Pump() override { ui->Drain(); }
  ~WindowsRuntime() {
    ui->Stop();
    if (service) service->Stop();
    notifications->Stop();
    if (controller) DestroyWindow(controller);
  }
};
}
std::string ReadNotificationActivation(const std::string& resources) {
  lane::windows::ApartmentScope apartment;
  auto activation = std::make_shared<Events>();
  lane::windows::Notifications receiver;
  // As in the previous native host, register before querying AppInstance so a
  // cold COM activation can be forwarded to the already-running singleton.
  receiver.Start(std::filesystem::u8path(resources), [activation](std::string value) {
    auto route = lane::windows::Parse(value);
    if (lane::windows::Text(route, L"kind") == "task" && lane::windows::UUID(lane::windows::Text(route, L"id")) &&
        lane::windows::UUID(lane::windows::Text(route, L"spaceId"))) activation->Push("activation", std::move(value));
  });
  receiver.Stop();
  auto pending = activation->Drain();
  return pending.empty() ? std::string{} : pending.front().second;
}
std::shared_ptr<PlatformRuntime> CreatePlatformRuntime(
    const std::string& resources, const std::string& scheme, std::shared_ptr<Events> events) {
  return std::make_shared<WindowsRuntime>(resources, scheme, std::move(events));
}
}
