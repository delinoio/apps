// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <algorithm>
#include <chrono>
#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

namespace lane::windows {
inline bool UUID(std::string_view value) {
  if (value.size() != 36 || value[14] != '7' ||
      std::string_view("89ab").find(value[19]) == std::string_view::npos) return false;
  for (size_t i = 0; i < value.size(); ++i) {
    if (i == 8 || i == 13 || i == 18 || i == 23) {
      if (value[i] != '-') return false;
    } else if (!((value[i] >= '0' && value[i] <= '9') ||
                 (value[i] >= 'a' && value[i] <= 'f'))) return false;
  }
  return true;
}
struct Reauthentication {
  std::string space, account, mode, challenge;
  uint64_t generation = 0;
  bool ready = false;
  bool Matches(const std::string& requested_space, const std::string& requested_account,
               const std::string& requested_mode, uint64_t current_generation) const {
    return ready && !challenge.empty() && generation == current_generation &&
        space == requested_space && account == requested_account && mode == requested_mode;
  }
};
enum class RouteKind { Open, Repository, Task, Callback };
struct Route { RouteKind kind; std::string id; std::string uri; };
inline bool ParseRoute(const std::string& uri, const std::string& scheme, Route& route) {
  const std::string prefix = scheme + "://";
  if (uri.size() > 16384 || uri.compare(0, prefix.size(), prefix) != 0) return false;
  for (unsigned char c : uri) if (c <= 32 || c >= 127 || c == '\\' || c == '"') return false;
  auto path = uri.substr(prefix.size());
  if (path.empty() || path == "open" || path == "open/") {
    route = {RouteKind::Open, {}, uri}; return true;
  }
  for (auto kind : {RouteKind::Repository, RouteKind::Task}) {
    const std::string start = kind == RouteKind::Task ? "task/" : "repository/";
    if (path.compare(0, start.size(), start) == 0 && UUID(path.substr(start.size()))) {
      route = {kind, path.substr(start.size()), uri}; return true;
    }
  }
  // The helper owns state, PKCE, single-use approval and token storage. Parsing
  // this allowlist is not authorization. Never forward callback data to JS.
  const std::string callback = "auth/callback?";
  if (path.compare(0, callback.size(), callback) != 0 || path.find('#') != std::string::npos) return false;
  const std::string reauth = callback + "reauth_challenge=";
  if (path.compare(0, reauth.size(), reauth) == 0) {
    const auto challenge = path.substr(reauth.size());
    if (!UUID(challenge)) return false;
    route = {RouteKind::Callback, challenge, uri}; return true;
  }
  bool state = false, code = false, error = false;
  auto query = path.substr(callback.size());
  while (!query.empty()) {
    const auto end = query.find('&');
    const auto pair = query.substr(0, end);
    const auto equal = pair.find('=');
    if (equal == std::string::npos || equal + 1 == pair.size()) return false;
    const auto key = pair.substr(0, equal);
    bool* seen = key == "state" ? &state : key == "code" ? &code : key == "error" ? &error : nullptr;
    if (!seen || *seen) return false;
    *seen = true;
    for (size_t i = equal + 1; i < pair.size(); ++i) {
      if (pair[i] == '%') {
        auto hex = [](char c) { return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); };
        if (i + 2 >= pair.size() || !hex(pair[i + 1]) || !hex(pair[i + 2])) return false;
        i += 2;
      }
    }
    if (end == std::string::npos) break;
    query.erase(0, end + 1);
    if (query.empty()) return false;
  }
  if (!state || code == error) return false;
  route = {RouteKind::Callback, {}, uri}; return true;
}

// CommandLineToArgvW / Microsoft CRT quoting. No shell interprets this string.
inline std::wstring QuoteArgument(const std::wstring& value) {
  std::wstring result = L"\"";
  size_t slashes = 0;
  for (wchar_t c : value) {
    if (c == L'\\') { ++slashes; continue; }
    if (c == L'"') result.append(slashes * 2 + 1, L'\\');
    else result.append(slashes, L'\\');
    slashes = 0;
    result += c;
  }
  result.append(slashes * 2, L'\\');
  result += L'"';
  return result;
}
enum class SyncState { Disabled, Starting, Running, MissingHelper, LaunchFailed, Exited, InvalidInvocation, Stopping };
inline const char* StateName(SyncState state) {
  switch (state) {
    case SyncState::Disabled: return "disabled";
    case SyncState::Starting: return "starting";
    case SyncState::Running: return "running";
    case SyncState::MissingHelper: return "missing_helper";
    case SyncState::LaunchFailed: return "launch_failed";
    case SyncState::Exited: return "exited";
    case SyncState::InvalidInvocation: return "invalid_invocation";
    case SyncState::Stopping: return "stopping";
  }
  return "exited";
}
inline std::chrono::seconds RetryDelay(unsigned attempt) {
  static constexpr int seconds[] = {1, 2, 5, 10, 30, 60};
  return std::chrono::seconds(seconds[std::min(attempt, 5u)]);
}
}  // namespace lane::windows
