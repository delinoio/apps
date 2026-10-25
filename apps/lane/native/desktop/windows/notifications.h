// SPDX-License-Identifier: Apache-2.0
#pragma once
#include "platform.h"
#include <WindowsAppSDK-VersionInfo.h>
#include <MddBootstrap.h>
#include <winrt/Microsoft.Windows.AppNotifications.h>
#include <winrt/Microsoft.Windows.AppLifecycle.h>
#include <winrt/Windows.Data.Xml.Dom.h>

namespace lane::windows {
class Notifications {
 public:
  using Manager = winrt::Microsoft::Windows::AppNotifications::AppNotificationManager;
  void Start(const std::filesystem::path& directory, std::function<void(std::string)> activate) {
    // Load only the packaged bootstrap next to this executable. A missing
    // optional runtime must not stop account-free local storage.
    bootstrap_ = LoadLibraryExW((directory / L"Microsoft.WindowsAppRuntime.Bootstrap.dll").c_str(),
        nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!bootstrap_) return;
    auto initialize = reinterpret_cast<decltype(&MddBootstrapInitialize2)>(GetProcAddress(bootstrap_, "MddBootstrapInitialize2"));
    shutdown_ = reinterpret_cast<decltype(&MddBootstrapShutdown)>(GetProcAddress(bootstrap_, "MddBootstrapShutdown"));
    if (!initialize || !shutdown_) return;
    PACKAGE_VERSION version{};
    version.Version = WINDOWSAPPSDK_RUNTIME_VERSION_UINT64;
    if (FAILED(initialize(WINDOWSAPPSDK_RELEASE_MAJORMINOR, WINDOWSAPPSDK_RELEASE_VERSION_TAG_W,
        version, MddBootstrapInitializeOptions_None))) return;
    initialized_ = true;
    try {
      if (!Manager::IsSupported()) return;
      manager_ = Manager::Default();
      token_ = manager_.NotificationInvoked([activate](const auto&, const auto& args) {
        try { activate(winrt::to_string(args.Argument())); } catch (...) {}
      });
      subscribed_ = true;
      manager_.Register();
      registered_ = true;
      // Register before reading activation. On cold COM launch the SDK retains
      // the first notification for AppInstance rather than raising the event.
      auto activation = winrt::Microsoft::Windows::AppLifecycle::AppInstance::GetCurrent().GetActivatedEventArgs();
      if (activation.Kind() == winrt::Microsoft::Windows::AppLifecycle::ExtendedActivationKind::AppNotification) {
        auto args = activation.Data().as<winrt::Microsoft::Windows::AppNotifications::AppNotificationActivatedEventArgs>();
        activate(winrt::to_string(args.Argument()));
      }
    } catch (...) { /* Permission() exposes unsupported; local work remains available. */ }
  }
  ~Notifications() { Stop(); }
  void Stop() {
    try {
      if (subscribed_) manager_.NotificationInvoked(token_);
      if (registered_) manager_.Unregister();
    } catch (...) {}
    subscribed_ = registered_ = false;
    manager_ = nullptr;
    if (initialized_) shutdown_();
    initialized_ = false;
    if (bootstrap_) FreeLibrary(std::exchange(bootstrap_, nullptr));
  }
  std::string Permission() const {
    using Setting = winrt::Microsoft::Windows::AppNotifications::AppNotificationSetting;
    if (!registered_) return "NOTIFICATION_PERMISSION_UNSUPPORTED";
    try {
      switch (manager_.Setting()) {
        case Setting::Enabled: return "NOTIFICATION_PERMISSION_GRANTED";
        case Setting::Unsupported: return "NOTIFICATION_PERMISSION_UNSUPPORTED";
        default: return "NOTIFICATION_PERMISSION_DENIED";
      }
    } catch (...) { return "NOTIFICATION_PERMISSION_UNSUPPORTED"; }
  }
  bool Show(const Json& item) {
    if (Permission() != "NOTIFICATION_PERMISSION_GRANTED") return false;
    const auto proposal = Text(item, L"proposalId"), device = Text(item, L"deviceId"), space = Text(item, L"spaceId");
    if (!UUID(proposal) || !UUID(device) || !UUID(space)) return false;
    using namespace winrt::Windows::Data::Xml::Dom;
    XmlDocument document;
    document.LoadXml(L"<toast><visual><binding template='ToastGeneric'><text/><text/></binding></visual></toast>");
    auto toast = document.DocumentElement();
    Json route; Set(route, L"kind", "task"); Set(route, L"id", proposal); Set(route, L"spaceId", space);
    toast.SetAttribute(L"launch", winrt::to_hstring(Dump(route)));
    auto text = document.GetElementsByTagName(L"text");
    const bool generic = Text(item, L"content") == "NOTIFICATION_CONTENT_GENERIC";
    text.Item(0).AppendChild(document.CreateTextNode(winrt::to_hstring(generic ? "Lane" : Text(item, L"repositoryName"))));
    text.Item(1).AppendChild(document.CreateTextNode(winrt::to_hstring(generic ? "A new task proposal is ready." : Text(item, L"title"))));
    // DOM text nodes escape task content. Stable tag/group replace a retry
    // after a process crash between OS acceptance and durable acknowledgment.
    winrt::Microsoft::Windows::AppNotifications::AppNotification notification(document.GetXml());
    notification.Tag(winrt::to_hstring(proposal));
    notification.Group(winrt::to_hstring(device));
    notification.Expiration(winrt::clock::now() + std::chrono::hours(2));
    manager_.Show(notification);
    return notification.Id() != 0;
  }
 private:
  HMODULE bootstrap_ = nullptr;
  decltype(&MddBootstrapShutdown) shutdown_ = nullptr;
  bool initialized_ = false, registered_ = false, subscribed_ = false;
  Manager manager_{nullptr};
  winrt::event_token token_{};
};
}  // namespace lane::windows
