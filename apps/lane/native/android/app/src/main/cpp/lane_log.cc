// SPDX-License-Identifier: Apache-2.0
#include "lane_log.h"
#include <algorithm>
#include <chrono>
#include <filesystem>
#include <fcntl.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

namespace {
struct Descriptor {
  int value;
  ~Descriptor() { if (value >= 0) close(value); }
};
}
void lane_log(const std::string& root, const std::string& event) noexcept {
  try {
    if (event.empty() || event.size() > 80 || event.find_first_not_of("abcdefghijklmnopqrstuvwxyz0123456789_") != std::string::npos) return;
    namespace fs = std::filesystem;
    auto directory = fs::path(root) / "logs";
    fs::create_directories(directory);
    chmod(directory.c_str(), 0700);
    Descriptor lock{open((directory / ".rotation.lock").c_str(), O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW, 0600)};
    if (lock.value < 0 || flock(lock.value, LOCK_EX) != 0) return;
    const auto now = std::chrono::system_clock::now();
    const auto seconds = std::chrono::duration_cast<std::chrono::seconds>(now.time_since_epoch()).count();
    const auto path = directory / ("android-" + std::to_string(seconds / 86400) + ".jsonl");
    Descriptor output{open(path.c_str(), O_CREAT | O_APPEND | O_WRONLY | O_CLOEXEC | O_NOFOLLOW, 0600)};
    if (output.value >= 0) {
      const auto line = "{\"event\":\"" + event + "\",\"platform\":\"android\",\"time\":" + std::to_string(seconds) + "}\n";
      // A partial diagnostic write is not a storage mutation success/failure signal.
      auto ignored = write(output.value, line.data(), line.size());
      (void)ignored;
    }
    struct Entry { fs::path path; uintmax_t size; fs::file_time_type time; };
    std::vector<Entry> files;
    uintmax_t total = 0;
    const auto cutoff = fs::file_time_type::clock::now() - std::chrono::hours(24 * 30);
    for (const auto& entry : fs::directory_iterator(directory)) {
      if (entry.path().filename() == ".rotation.lock" || !fs::is_regular_file(entry.symlink_status())) continue;
      auto time = entry.last_write_time();
      if (time < cutoff) { fs::remove(entry.path()); continue; }
      auto size = entry.file_size();
      total += size;
      files.push_back({entry.path(), size, time});
    }
    std::sort(files.begin(), files.end(), [](const Entry& a, const Entry& b) { return a.time < b.time; });
    for (const auto& file : files) {
      if (total <= 100ULL * 1024 * 1024) break;
      if (fs::remove(file.path)) total -= file.size;
    }
    // close() releases flock; use the same lock inode as the common engine.
  } catch (...) { /* Logging failure must never hide the operation's actual result. */ }
}
