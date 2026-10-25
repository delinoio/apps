// SPDX-License-Identifier: Apache-2.0
#pragma once
#include <windows.h>
#include <shlobj.h>
#include <shellapi.h>
#include <bcrypt.h>
#include <roapi.h>
#include <winrt/Windows.Data.Json.h>
#include <winrt/Windows.Foundation.h>
#include <chrono>
#include <cstring>
#include <filesystem>
#include <functional>
#include <future>
#include <mutex>
#include <deque>
#include <stdexcept>
#include <string>
#include <utility>
#include "policy.h"

namespace lane::windows {
class ApartmentScope {
 public:
  ApartmentScope() : result_(RoInitialize(RO_INIT_MULTITHREADED)) {
    if (result_ != RPC_E_CHANGED_MODE) winrt::check_hresult(result_);
  }
  ~ApartmentScope() { if (SUCCEEDED(result_)) RoUninitialize(); }
 private:
  HRESULT result_;
};
using Json = winrt::Windows::Data::Json::JsonObject;
using Array = winrt::Windows::Data::Json::JsonArray;
using Value = winrt::Windows::Data::Json::JsonValue;
inline Json Parse(const std::string& value) { return Json::Parse(winrt::to_hstring(value)); }
inline std::string Dump(const Json& value) { return winrt::to_string(value.Stringify()); }
inline std::string Text(const Json& value, const wchar_t* key, const wchar_t* fallback = L"") {
  return winrt::to_string(value.GetNamedString(key, fallback));
}
inline void Set(Json& value, const wchar_t* key, const std::string& text) {
  value.Insert(key, Value::CreateStringValue(winrt::to_hstring(text)));
}
inline std::string NewID() {
  unsigned char bytes[16];
  if (BCryptGenRandom(nullptr, bytes, sizeof(bytes), BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0)
    throw std::runtime_error("Secure random source unavailable");
  auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(
      std::chrono::system_clock::now().time_since_epoch()).count();
  for (int i = 5; i >= 0; --i) { bytes[i] = static_cast<unsigned char>(ms); ms >>= 8; }
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  std::string result;
  for (size_t i = 0; i < 16; ++i) {
    if (i == 4 || i == 6 || i == 8 || i == 10) result += '-';
    result += "0123456789abcdef"[bytes[i] >> 4];
    result += "0123456789abcdef"[bytes[i] & 15];
  }
  return result;
}
inline std::string Failure(const char* reason, const char* message, bool retryable = false) {
  Json result, error;
  result.Insert(L"ok", Value::CreateBooleanValue(false));
  Set(error, L"reason", reason); Set(error, L"message", message);
  Set(error, L"correlationId", NewID());
  error.Insert(L"retryable", Value::CreateBooleanValue(retryable));
  result.Insert(L"error", error);
  return Dump(result);
}
inline std::string Request(const std::string& space, const Json& request = Json{}) {
  Json value; Set(value, L"spaceId", space); value.Insert(L"request", request); return Dump(value);
}
inline std::filesystem::path DataRoot() {
  PWSTR path = nullptr;
  winrt::check_hresult(SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &path));
  std::filesystem::path root(path);
  CoTaskMemFree(path);
  // LocalAppData inherits this OS user's ACL; never use a temporary fallback.
  root /= L"io.delino.lane";
  std::filesystem::create_directories(root);
  return root;
}
class Handle {
 public:
  Handle() = default;
  explicit Handle(HANDLE value) : value_(value) {}
  ~Handle() { Reset(); }
  Handle(Handle&& other) noexcept : value_(other.Release()) {}
  Handle& operator=(Handle&& other) noexcept { Reset(other.Release()); return *this; }
  Handle(const Handle&) = delete;
  Handle& operator=(const Handle&) = delete;
  HANDLE Get() const { return value_; }
  explicit operator bool() const { return value_ && value_ != INVALID_HANDLE_VALUE; }
  HANDLE Release() { return std::exchange(value_, nullptr); }
  void Reset(HANDLE value = nullptr) { if (*this) CloseHandle(value_); value_ = value; }
 private:
  HANDLE value_ = nullptr;
};

inline constexpr UINT kDispatch = WM_APP + 1;
class UIQueue {
 public:
  void Attach(HWND window) { std::lock_guard<std::mutex> lock(mutex_); window_ = window; }
  bool Post(std::function<void()> task) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (!window_) return false;
    queue_.push_back(std::move(task));
    if (PostMessageW(window_, kDispatch, 0, 0)) return true;
    queue_.pop_back(); return false;
  }
  std::string Call(std::function<std::string()> function) {
    auto task = std::make_shared<std::packaged_task<std::string()>>(std::move(function));
    auto result = task->get_future();
    if (!Post([task] { (*task)(); })) throw std::runtime_error("Lane UI has stopped");
    task.reset();
    return result.get();
  }
  void Drain() {
    std::deque<std::function<void()>> tasks;
    { std::lock_guard<std::mutex> lock(mutex_); tasks.swap(queue_); }
    for (auto& task : tasks) { try { task(); } catch (...) {} }
  }
  void Stop() {
    std::lock_guard<std::mutex> lock(mutex_);
    window_ = nullptr;
    // Destroying packaged tasks wakes waiting workers with broken_promise.
    queue_.clear();
  }
 private:
  std::mutex mutex_;
  HWND window_ = nullptr;
  std::deque<std::function<void()>> queue_;
};

inline bool HTTPS(const std::string& text) {
  if (text.find('\0') != std::string::npos || text.find('\\') != std::string::npos) return false;
  for (unsigned char c : text) if (c < 32 || c == 127) return false;
  try {
    winrt::Windows::Foundation::Uri uri(winrt::to_hstring(text));
    return uri.SchemeName() == L"https" && !uri.Host().empty() &&
           uri.UserName().empty() && uri.Password().empty();
  } catch (...) { return false; }
}
inline std::string OpenURL(HWND owner, const std::string& text) {
  if (!HTTPS(text)) return Failure("ERROR_REASON_INVALID_INPUT", "Only HTTPS URLs are supported.");
  const auto value = winrt::to_hstring(text);
  const auto result = reinterpret_cast<INT_PTR>(ShellExecuteW(owner, L"open", value.c_str(), nullptr, nullptr, SW_SHOWNORMAL));
  return result > 32 ? R"({"ok":true})" : Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Windows could not open the browser.", true);
}
inline std::string CopyText(HWND owner, const std::string& text) {
  if (text.find('\0') != std::string::npos) return Failure("ERROR_REASON_INVALID_INPUT", "Clipboard text contains a null character.");
  const auto wide = winrt::to_hstring(text);
  HGLOBAL memory = GlobalAlloc(GMEM_MOVEABLE, (wide.size() + 1) * sizeof(wchar_t));
  if (!memory) return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Clipboard memory is unavailable.", true);
  void* data = GlobalLock(memory);
  if (!data) { GlobalFree(memory); return Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Clipboard memory is unavailable.", true); }
  memcpy(data, wide.c_str(), (wide.size() + 1) * sizeof(wchar_t));
  GlobalUnlock(memory);
  if (!OpenClipboard(owner)) { GlobalFree(memory); return Failure("ERROR_REASON_STORAGE_BUSY", "The clipboard is busy. Retry copying.", true); }
  const bool copied = EmptyClipboard() && SetClipboardData(CF_UNICODETEXT, memory);
  CloseClipboard();
  if (!copied) GlobalFree(memory); // Windows owns the allocation only on success.
  return copied ? R"({"ok":true})" : Failure("ERROR_REASON_DEPENDENCY_UNAVAILABLE", "Windows could not copy the text.", true);
}
}  // namespace lane::windows
