// SPDX-License-Identifier: Apache-2.0
#pragma once
#include "platform.h"
#include "process.h"
#include "logging.h"
#include "serial_worker.h"
#include "../../store/lane_store.h"
#include <set>
#include <atomic>

namespace lane::windows {
inline constexpr wchar_t kSettings[] = L"Software\\Delino\\Lane";
inline void PendingAccountSpace(const std::string& space) {
  HKEY key = nullptr;
  if (RegCreateKeyExW(HKEY_CURRENT_USER, kSettings, 0, nullptr, 0, KEY_SET_VALUE,
      nullptr, &key, nullptr) != ERROR_SUCCESS) throw std::runtime_error("Cannot retain connection routing");
  auto wide = winrt::to_hstring(space);
  const auto result = space.empty() ? RegDeleteValueW(key, L"PendingAccountSpace") :
      RegSetValueExW(key, L"PendingAccountSpace", 0, REG_SZ, reinterpret_cast<const BYTE*>(wide.c_str()), (wide.size() + 1) * sizeof(wchar_t));
  RegCloseKey(key);
  if (result != ERROR_SUCCESS && result != ERROR_FILE_NOT_FOUND) throw std::runtime_error("Cannot retain connection routing");
}
inline std::string PendingAccountSpace() {
  wchar_t value[37]{}; DWORD size = sizeof(value);
  if (RegGetValueW(HKEY_CURRENT_USER, kSettings, L"PendingAccountSpace", RRF_RT_REG_SZ,
      nullptr, value, &size) != ERROR_SUCCESS) return {};
  auto result = winrt::to_string(value);
  return UUID(result) ? result : std::string{};
}
struct HostActions {
  std::function<void(std::string, std::string)> event;
  std::function<std::string()> permission;
  std::function<bool(Json)> notify;
  std::function<void(std::string)> browse;
};
class Service {
 public:
  Service(std::filesystem::path helper, std::string scheme, HostActions actions)
      : helper_(std::move(helper)), scheme_(std::move(scheme)), actions_(std::move(actions)) {
    worker_ = std::make_unique<SerialWorker>([this] { Start(); }, [this] { Tick(); }, [this] {
      supervisor_.reset(); store_.reset();
      if (apartment_) winrt::uninit_apartment();
    });
  }
  ~Service() { worker_->Stop(); }
  std::string Call(const std::string& method, const std::vector<std::string>& args) {
    ApartmentScope apartment;
    const bool invalidate = method == "open" || method == "close" || method == "cancelReauthentication" ||
        (method == "invoke" && !args.empty() && args[0] == "BeginDataDeletion");
    const auto generation = invalidate ? ++reauth_generation_ : reauth_generation_.load();
    try {
      return worker_->Submit([this, method, args, invalidate, generation] {
        if (invalidate) {
          const auto previous = reauth_.challenge;
          auto canceled = CancelPending();
          if (method == "cancelReauthentication") {
            if (!previous.empty() && previous != args.at(0))
              return Failure("ERROR_REASON_INVALID_INPUT", "The reauthentication challenge does not match.");
            return canceled;
          }
        }
        active_generation_ = generation;
        if (method == "open") {
          auto response = store_->open(args.at(0));
          auto envelope = Parse(response);
          if (envelope.GetNamedBoolean(L"ok", false)) {
            auto space = Text(envelope.GetNamedObject(L"response"), L"spaceId");
            if (UUID(space)) watched_.insert(space);
            Tick();
          }
          return response;
        }
        if (method == "close") {
          auto response = store_->close(args.at(0));
          // This application-owned subscription observes local MCP proposals
          // after the rendering handle closes. It does not keep the view alive.
          if (watched_.count(args.at(0))) OpenBackground(args.at(0));
          return response;
        }
        if (method != "invoke") return Failure("ERROR_REASON_INVALID_INPUT", "Unknown native operation.");
        const auto& operation = args.at(0);
        Json input;
        try { input = Parse(args.at(1)); }
        catch (...) { return Failure("ERROR_REASON_INVALID_INPUT", "The native request must be a JSON object."); }
        routed_space_ = Text(input, L"spaceId");
        if (operation == "ResetDevelopmentData") {
          // The explicit development reset needs the exclusive space lease: stop
          // the supervised `lane sync run` helper first. The store still fails
          // with STORAGE_BUSY while any other process holds a handle.
          CancelPending();
          if (!routed_space_.empty()) supervisor_->Stop(routed_space_);
        }
        if (operation == "DeleteAccountData") {
          auto request = input.GetNamedObject(L"request");
          if (request.HasKey(L"reauthProof") || request.HasKey(L"reauth_proof"))
            return Failure("ERROR_REASON_INVALID_INPUT", "Reauthentication proof must remain in the native helper.");
          auto status = Parse(store_->invoke("GetLocalStatus", Request(routed_space_)));
          if (!status.GetNamedBoolean(L"ok", false)) return Dump(status);
          if (!reauth_.ready || reauth_.generation != reauth_generation_.load() ||
              reauth_.space != routed_space_ || reauth_.challenge != Text(request, L"challengeId") ||
              reauth_.account != Text(request, L"confirmedAccountId") ||
              reauth_.account != Text(status.GetNamedObject(L"response"), L"accountId"))
            return Failure("ERROR_REASON_UNAUTHENTICATED", "Complete reauthentication for this account and deletion mode.");
          request.Remove(L"challengeId");
          input.Insert(L"request", request);
        }
        auto response = store_->invoke(operation, Dump(input));
        if (operation == "RemoveDeviceData" || operation == "DisconnectSync") {
          if (Parse(response).GetNamedBoolean(L"ok", false)) {
            supervisor_->Stop(routed_space_);
            if (operation == "RemoveDeviceData" ||
                Text(input.GetNamedObject(L"request"), L"retention") == "LOCAL_RETENTION_REMOVE_LOCAL") watched_.erase(routed_space_);
          }
        }
        Tick();
        return response;
      }).get();
    } catch (...) { return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Windows could not complete the native operation.", true); }
  }
  void Resume() {
    try { worker_->Submit([this] { Tick(); return std::string{}; }); } catch (...) {}
  }
  void InvalidateReauthentication() {
    ++reauth_generation_;
    try { worker_->Submit([this] { return CancelPending(); }); } catch (...) {}
  }
  void Callback(std::string uri) {
    const auto generation = reauth_generation_.load();
    try {
      worker_->Submit([this, uri = std::move(uri), generation] {
        Route route;
        if (!ParseRoute(uri, scheme_, route) || route.kind != RouteKind::Callback) return std::string{};
        if (!route.id.empty()) {
          if (reauth_.challenge != route.id || reauth_.generation != generation ||
              generation != reauth_generation_.load()) return std::string{};
          auto status = Parse(store_->invoke("GetLocalStatus", Request(reauth_.space)));
          if (!status.GetNamedBoolean(L"ok", false) ||
              Text(status.GetNamedObject(L"response"), L"accountId") != reauth_.account) {
            CancelPending(); return std::string{};
          }
          auto response = Parse(Helper({L"--json", L"--space", Wide(reauth_.space), L"sync", L"callback", Wide(uri)}));
          if (response.GetNamedBoolean(L"ok", false) && generation == reauth_generation_.load() &&
              Text(response.GetNamedObject(L"result"), L"challengeId") == reauth_.challenge) {
            reauth_.ready = true;
            Json event; Set(event, L"challengeId", reauth_.challenge);
            actions_.event("lane.reauthenticated", Dump(event));
          } else if (!response.GetNamedBoolean(L"ok", false)) {
            actions_.event("lane.hostStatus", Dump(response));
          }
          return std::string{};
        }
        const auto original = PendingAccountSpace();
        if (original.empty()) {
          actions_.event("lane.hostStatus", Failure("ERROR_REASON_UNAUTHENTICATED", "No pending browser connection matches this callback."));
          return std::string{};
        }
        auto result = Helper({L"--json", L"--space", Wide(original), L"sync", L"callback", Wide(uri)});
        auto envelope = Parse(result);
        if (envelope.GetNamedBoolean(L"ok", false)) {
          // The helper validates state/PKCE and updates registry routing using
          // its credential store. Never copy its tokens into NAPI or events.
          PendingAccountSpace({});
          auto opened = Parse(store_->open(R"({"formatVersion":3,"spaceId":""})"));
          if (opened.GetNamedBoolean(L"ok", false)) {
            const auto space = Text(opened.GetNamedObject(L"response"), L"spaceId");
            watched_.insert(space);
            auto selected = opened.GetNamedObject(L"response");
            Json event; Set(event, L"spaceId", space); Set(event, L"sequence", Text(selected, L"sequence"));
            actions_.event("lane.storeChanged", Dump(event));
            Json route; Set(route, L"kind", "open"); Set(route, L"spaceId", space);
            actions_.event("lane.openRoute", Dump(route));
          }
          Tick();
        } else {
          actions_.event("lane.hostStatus", result);
        }
        return std::string{};
      });
    } catch (...) {}
  }
  void Stop() { worker_->Stop(); }
 private:
  std::string CancelPending() {
    const auto space = reauth_.space, challenge = reauth_.challenge;
    reauth_.space.clear(); reauth_.account.clear(); reauth_.mode.clear(); reauth_.challenge.clear();
    reauth_.ready = false;
    if (challenge.empty()) return R"({"ok":true})";
    auto result = Parse(Helper({L"--json", L"--space", Wide(space), L"sync", L"cancel-reauth", L"--challenge-id", Wide(challenge)}));
    if (!result.GetNamedBoolean(L"ok", false)) return Dump(result);
    if (!result.GetNamedObject(L"result").GetNamedBoolean(L"completed", false))
      return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Could not cancel reauthentication.", true);
    return R"({"ok":true})";
  }
  static std::wstring Wide(const std::string& value) { return winrt::to_hstring(value).c_str(); }
  std::string Helper(const std::vector<std::wstring>& args, const std::string& input = {}) {
    if (!std::filesystem::is_regular_file(helper_)) return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "The packaged Lane helper is missing. Reinstall Lane.");
    try { return RunHelper(helper_, args, input); }
    catch (...) { return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Windows could not start the packaged Lane helper.", true); }
  }
  void Start() {
    winrt::init_apartment(winrt::apartment_type::multi_threaded); apartment_ = true;
    owner_ = NewID();
    store_ = std::make_unique<lane::Store>(DataRoot().u8string());
    supervisor_ = std::make_unique<SyncSupervisor>(helper_, [this](const auto& space, auto state, DWORD code) {
      Log("lane_sync_helper_state", StateName(state), code);
      if (state == SyncState::MissingHelper || state == SyncState::LaunchFailed ||
          state == SyncState::Exited || state == SyncState::InvalidInvocation) {
        std::string reason = "ERROR_REASON_DEPENDENCY_UNAVAILABLE";
        if (state == SyncState::Exited) {
          // The helper can persist a more specific authentication, epoch or
          // format error before exit. Do not overwrite that evidence.
          auto status = Parse(store_->invoke("GetLocalStatus", Request(space)));
          if (status.GetNamedBoolean(L"ok", false)) {
            auto recorded = Text(status.GetNamedObject(L"response"), L"syncErrorReason");
            if (!recorded.empty()) reason = recorded;
          }
        }
        store_->recordSyncFailure(space, reason);
      }
      Json event; Set(event, L"spaceId", space); Set(event, L"syncState", StateName(state));
      event.Insert(L"exitCode", Value::CreateNumberValue(code));
      actions_.event("lane.hostStatus", Dump(event));
    });
    lane::HostHooks hooks;
    hooks.remote = [this](std::string space, std::string method, std::string input) {
      std::vector<std::wstring> arguments{L"--remote", L"--json", L"--expected-space", Wide(space)};
      if (method == "DeleteAccountData") {
        // The store has normalized ProtoJSON enums before entering this hook.
        const auto request = Parse(input);
        if (!reauth_.Matches(space, Text(request, L"confirmedAccountId"), Text(request, L"mode"), reauth_generation_.load()))
          return Failure("ERROR_REASON_UNAUTHENTICATED", "Reauthentication is no longer valid.");
        arguments.insert(arguments.end(), {L"--reauth-challenge", Wide(reauth_.challenge)});
        reauth_.ready = false;
      }
      arguments.insert(arguments.end(), {L"rpc", Wide(method), L"--request-json", L"-"});
      auto envelope = Parse(Helper(arguments, input));
      if (!envelope.GetNamedBoolean(L"ok", false)) return Dump(envelope);
      auto result = envelope.GetNamedObject(L"result");
      if (method == "BeginDataDeletion") {
        auto request = Parse(input);
        const auto challenge = Text(result, L"challengeId"), url = Text(result, L"authorizationUrl");
        if (!UUID(challenge) || !HTTPS(url))
          return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Invalid reauthentication response.");
        reauth_.space = space; reauth_.account = Text(request, L"confirmedAccountId");
        reauth_.mode = Text(request, L"mode"); reauth_.challenge = challenge;
        reauth_.generation = active_generation_; reauth_.ready = false;
        if (reauth_.generation != reauth_generation_.load()) {
          CancelPending();
          return Failure("ERROR_REASON_UNAUTHENTICATED", "Reauthentication was canceled.");
        }
        // The UI opens this authorization URL after showing the deletion
        // confirmation. Only ConnectSync opens its browser from native code.
      }
      return Dump(result);
    };
    hooks.authorize = [this](std::string account) {
      if (!UUID(routed_space_)) throw std::runtime_error("Connection space is missing");
      auto status = Parse(store_->invoke("GetLocalStatus", Request(routed_space_)));
      if (!status.GetNamedBoolean(L"ok", false)) return Dump(status);
      auto local = status.GetNamedObject(L"response");
      if (!account.empty() && account != Text(local, L"accountId"))
        return Failure("ERROR_REASON_INVALID_INPUT", "Select the matching account space before reconnecting.");
      std::vector<std::wstring> arguments{L"--json", L"--space", Wide(routed_space_), L"sync", L"connect",
          L"--redirect-uri", Wide(scheme_ + "://auth/callback")};
      // Only explicit ConnectSync after the UI confirmation can authorize an
      // epoch change. The helper persists and revalidates this old-space intent.
      if (local.GetNamedBoolean(L"reconnectRequired", false)) arguments.push_back(L"--confirm-epoch-change");
      PendingAccountSpace(routed_space_);
      auto envelope = Parse(Helper(arguments));
      if (!envelope.GetNamedBoolean(L"ok", false)) return Dump(envelope);
      auto result = envelope.GetNamedObject(L"result");
      const auto url = Text(result, L"authorizationUrl"), challenge = Text(result, L"challengeId");
      if (!HTTPS(url) || challenge.empty()) throw std::runtime_error("Invalid browser approval response");
      // Native code opens the browser. The returned URL contains no tokens.
      actions_.browse(url);
      Json response; Set(response, L"authorizationUrl", url); Set(response, L"challengeId", challenge);
      return Dump(response);
    };
    hooks.requestNotificationPermission = [this] { return actions_.permission(); };
    hooks.disconnect = [this](std::string space) {
      supervisor_->Stop(space);
      auto status = Parse(store_->invoke("GetLocalStatus", Request(space)));
      if (status.GetNamedBoolean(L"ok", false) &&
          !status.GetNamedObject(L"response").GetNamedBoolean(L"syncEnabled", false)) return;
      auto result = Parse(Helper({L"--json", L"--space", Wide(space), L"sync", L"disconnect", L"--retention", L"keep-local"}));
      if (!result.GetNamedBoolean(L"ok", false)) throw std::runtime_error("Cannot disconnect the sync helper");
    };
    store_->setHostHooks(std::move(hooks));
    auto initial = Parse(store_->open(R"({"formatVersion":3,"spaceId":""})"));
    if (initial.GetNamedBoolean(L"ok", false)) watched_.insert(Text(initial.GetNamedObject(L"response"), L"spaceId"));
  }
  void OpenBackground(const std::string& space) {
    Json request; Set(request, L"spaceId", space);
    request.Insert(L"formatVersion", Value::CreateNumberValue(3));
    store_->open(Dump(request));
  }
  void Tick() {
    try {
      auto changes = Array::Parse(winrt::to_hstring(store_->poll()));
      for (const auto& value : changes) {
        auto change = value.GetObject();
        const auto space = Text(change, L"spaceId");
        if (!UUID(space)) continue;
        watched_.insert(space);
        Json event; Set(event, L"spaceId", space); Set(event, L"sequence", Text(change, L"sequence"));
        actions_.event("lane.storeChanged", Dump(event));
      }
      for (auto it = watched_.begin(); it != watched_.end();) {
        const auto space = *it;
        auto status = Parse(store_->invoke("GetLocalStatus", Request(space)));
        if (!status.GetNamedBoolean(L"ok", false)) {
          const auto reason = Text(status.GetNamedObject(L"error"), L"reason");
          if (reason == "ERROR_REASON_NOT_FOUND" || reason == "ERROR_REASON_FORBIDDEN") {
            supervisor_->Stop(space); it = watched_.erase(it);
          } else { ++it; }
          continue;
        }
        ++it;
        auto local = status.GetNamedObject(L"response");
        supervisor_->Reconcile(space, local.GetNamedBoolean(L"syncEnabled", false));
        auto preferences = local.GetNamedObject(L"notificationPreferences");
        if (!preferences.GetNamedBoolean(L"enabled", false)) continue;
        const auto permission = actions_.permission();
        if (permission != Text(preferences, L"permission")) store_->recordNotificationPermission(space, permission);
        if (permission != "NOTIFICATION_PERMISSION_GRANTED") continue;
        auto claimed = Parse(store_->claimNotifications(space, owner_));
        if (!claimed.GetNamedBoolean(L"ok", false)) continue;
        for (const auto& item : claimed.GetNamedObject(L"response").GetNamedArray(L"items")) {
          auto notification = item.GetObject();
          Set(notification, L"spaceId", space);
          if (actions_.notify(notification)) store_->acknowledgeNotification(space, Text(notification, L"proposalId"));
        }
      }
    } catch (...) {
      // Content and OS exception strings can include private paths. Emit only
      // the category, and retry on the next bounded observer tick.
      if (!observer_failed_) actions_.event("lane.hostStatus", Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "The Windows observer could not refresh native state.", true));
      if (!observer_failed_) Log("lane_observer_failed");
      observer_failed_ = true;
      return;
    }
    observer_failed_ = false;
  }
  std::filesystem::path helper_;
  std::string scheme_, routed_space_, owner_;
  std::atomic<uint64_t> reauth_generation_{0};
  uint64_t active_generation_ = 0;
  Reauthentication reauth_;
  HostActions actions_;
  bool apartment_ = false, observer_failed_ = false;
  std::set<std::string> watched_;
  std::unique_ptr<lane::Store> store_;
  std::unique_ptr<SyncSupervisor> supervisor_;
  std::unique_ptr<SerialWorker> worker_;
};
}  // namespace lane::windows
