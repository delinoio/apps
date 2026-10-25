// SPDX-License-Identifier: Apache-2.0
#pragma once
#include "platform.h"
#include <sstream>
#include <iomanip>

namespace lane::windows {
// Only fixed event/state identifiers and numeric OS categories belong here.
// Never pass task content, callback URIs, filesystem paths or exception text.
inline void Log(const char* event, const char* state = "", DWORD code = 0) noexcept {
  try {
    auto identifier = [](const char* value) {
      for (; *value; ++value) if (!((*value >= 'a' && *value <= 'z') ||
          (*value >= '0' && *value <= '9') || *value == '_')) return false;
      return true;
    };
    if (!identifier(event) || !identifier(state)) return;
    const auto directory = DataRoot() / "logs";
    std::filesystem::create_directories(directory);
    Handle lock(CreateFileW((directory / ".rotation.lock").c_str(), GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!lock) return;
    OVERLAPPED position{};
    bool acquired = false;
    for (unsigned attempt = 0; attempt != 10; ++attempt) {
      if (LockFileEx(lock.Get(), LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &position)) { acquired = true; break; }
      if (GetLastError() != ERROR_LOCK_VIOLATION) return;
      Sleep(10);
    }
    if (!acquired) return;
    // Closing lock releases the byte-range lock on every exit path.
    SYSTEMTIME time{}; GetSystemTime(&time);
    std::ostringstream date, timestamp;
    date << std::setfill('0') << std::setw(4) << time.wYear << std::setw(2) << time.wMonth << std::setw(2) << time.wDay;
    timestamp << std::setfill('0') << std::setw(4) << time.wYear << '-' << std::setw(2) << time.wMonth << '-'
              << std::setw(2) << time.wDay << 'T' << std::setw(2) << time.wHour << ':'
              << std::setw(2) << time.wMinute << ':' << std::setw(2) << time.wSecond << 'Z';
    const std::string line = "{\"time\":\"" + timestamp.str() + "\",\"event\":\"" + event +
        "\",\"state\":\"" + state + "\",\"code\":" + std::to_string(code) + "}\n";
    struct File { std::filesystem::path path; std::filesystem::file_time_type modified; uintmax_t bytes; };
    std::vector<File> files;
    uintmax_t total = 0;
    const auto now = std::filesystem::file_time_type::clock::now();
    for (const auto& entry : std::filesystem::directory_iterator(directory)) {
      if (entry.is_symlink() || !entry.is_regular_file() || entry.path().extension() == ".lock") continue;
      auto modified = entry.last_write_time();
      const auto bytes = entry.file_size();
      std::error_code error;
      if (now - modified > std::chrono::hours(24 * 30) && std::filesystem::remove(entry.path(), error)) continue;
      files.push_back({entry.path(), modified, bytes}); total += bytes;
    }
    std::sort(files.begin(), files.end(), [](const auto& a, const auto& b) { return a.modified < b.modified; });
    constexpr uintmax_t budget = 100 * 1024 * 1024;
    for (const auto& file : files) {
      if (total + line.size() <= budget) break;
      std::error_code error;
      if (std::filesystem::remove(file.path, error)) total -= file.bytes;
    }
    if (total + line.size() > budget) return;
    const auto path = directory / ("windows-host-" + date.str() + "-" + std::to_string(GetCurrentProcessId()) + ".log");
    Handle output(CreateFileW(path.c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (!output) return;
    DWORD written = 0;
    WriteFile(output.Get(), line.data(), static_cast<DWORD>(line.size()), &written, nullptr);
  } catch (...) { /* Diagnostics must not turn a successful store commit into a failure. */ }
}
}  // namespace lane::windows
