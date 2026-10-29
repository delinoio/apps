// SPDX-License-Identifier: Apache-2.0
#pragma once
#import <Foundation/Foundation.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <algorithm>
#include <chrono>
#include <cstdint>
#include <filesystem>
#include <string>
#include <vector>

// Shared with the Go CLI: the same advisory lock covers legacy cleanup, pruning and append.
// Only fixed event names enter this logger; never pass native error descriptions.
//
// The macOS host writes UTC daily buckets named native-macos-YYYYMMDD.log. A bucket is never
// appended to after its UTC day ends and expires when its bucket start plus 30 days is reached,
// regardless of modification time, because appends refresh mtime and records carry no timestamp.
// This conservative daily policy can discard the final part of a bucket (less than one day)
// early; it never keeps a record longer than 30 days. Other files in the shared directory
// (CLI and Windows-style siblings) keep modification-time expiry and the shared size budget.
namespace lane_log {

constexpr uintmax_t kBudgetBytes = 100ULL * 1024 * 1024;
constexpr int kRetentionDays = 30;
constexpr const char* kLegacyName = "native-macos.log";
constexpr const char* kLockName = ".rotation.lock";

// Removal seam so tests can exercise failed retirement/pruning without privileged filesystem setup.
using Remover = bool (*)(const std::filesystem::path&, std::error_code&);
inline bool RemoveFile(const std::filesystem::path& path, std::error_code& error) {
  return std::filesystem::remove(path, error);
}

inline std::time_t ToTime(std::chrono::system_clock::time_point point) {
  return std::chrono::system_clock::to_time_t(point);
}

// Returns the UTC bucket file name for a point in time.
inline std::string BucketName(std::chrono::system_clock::time_point now) {
  std::time_t seconds = ToTime(now);
  std::tm utc{};
  char date[16] = {0};
  if (gmtime_r(&seconds, &utc) == nullptr || strftime(date, sizeof(date), "%Y%m%d", &utc) != 8) return "";
  return std::string("native-macos-") + date + ".log";
}

// Parses native-macos-YYYYMMDD.log into its UTC bucket start; rejects impossible dates.
inline bool ParseBucketStart(const std::string& name, std::time_t* start) {
  constexpr size_t prefix = 13, digits = 8, length = prefix + digits + 4;
  if (name.size() != length || name.compare(0, prefix, "native-macos-") != 0 ||
      name.compare(prefix + digits, 4, ".log") != 0) return false;
  for (size_t i = prefix; i < prefix + digits; ++i) if (name[i] < '0' || name[i] > '9') return false;
  int year = std::stoi(name.substr(prefix, 4)), month = std::stoi(name.substr(prefix + 4, 2)),
      day = std::stoi(name.substr(prefix + 6, 2));
  std::tm utc{};
  utc.tm_year = year - 1900; utc.tm_mon = month - 1; utc.tm_mday = day;
  std::time_t seconds = timegm(&utc);
  std::tm check{};
  if (seconds == static_cast<std::time_t>(-1) || gmtime_r(&seconds, &check) == nullptr) return false;
  if (check.tm_year != year - 1900 || check.tm_mon != month - 1 || check.tm_mday != day) return false;
  *start = seconds;
  return true;
}

// True when the bucket's start plus the retention window is not after now.
inline bool BucketExpired(std::time_t start, std::chrono::system_clock::time_point now) {
  return start + static_cast<std::time_t>(kRetentionDays) * 24 * 60 * 60 <= ToTime(now);
}

// Appends one line under the shared lock. Order under the lock: retire the legacy fixed file,
// age/size prune with room reserved for the new line, then append to the current UTC bucket.
// Diagnostic failures never throw or propagate; the return value only reports whether the line
// reached the current bucket. The lock file is never removed or replaced.
inline bool AppendAt(const std::filesystem::path& logs, std::chrono::system_clock::time_point now,
                     const std::string& line, Remover remove = RemoveFile) {
  namespace fs = std::filesystem;
  std::error_code error;
  const std::string bucket = BucketName(now);
  if (bucket.empty()) return false;
  int lock = open((logs / kLockName).c_str(), O_CREAT|O_RDWR|O_CLOEXEC, 0600);
  if (lock < 0) return false;
  if (flock(lock, LOCK_EX) != 0) { close(lock); return false; }
  bool appended = false;

  // The fixed file has no per-record timestamps, so its age is unknown whatever its mtime says.
  // Remove it; never rename it into or append it to a dated bucket.
  auto legacy = logs / kLegacyName;
  auto legacyStatus = fs::symlink_status(legacy, error);
  if (!error && (fs::is_regular_file(legacyStatus) || fs::is_symlink(legacyStatus))) remove(legacy, error);

  struct Candidate { fs::path path; fs::file_time_type modified; uintmax_t bytes; };
  std::vector<Candidate> files;
  uintmax_t total = 0;
  const auto fileNow = fs::file_time_type::clock::now();
  const auto systemNow = std::chrono::system_clock::now();
  const auto mtimeCutoff = now - std::chrono::hours(24 * kRetentionDays);
  error.clear();
  fs::directory_iterator it(logs, error), end;
  while (!error && it != end) {
    const auto entry = *it;
    std::error_code entryError;
    const auto status = entry.symlink_status(entryError);
    if (!entryError && fs::is_regular_file(status) && entry.path().filename() != kLockName) {
      auto modified = fs::last_write_time(entry.path(), entryError);
      auto bytes = fs::file_size(entry.path(), entryError);
      if (!entryError) {
        std::time_t start = 0;
        bool expired;
        if (ParseBucketStart(entry.path().filename().string(), &start)) {
          expired = BucketExpired(start, now);
        } else {
          auto modifiedSystem = systemNow + std::chrono::duration_cast<std::chrono::system_clock::duration>(modified - fileNow);
          expired = modifiedSystem < mtimeCutoff;
        }
        std::error_code removeError;
        if (!(expired && remove(entry.path(), removeError))) {
          files.push_back({entry.path(), modified, bytes}); total += bytes;
        }
      }
    }
    it.increment(error);
  }

  std::sort(files.begin(), files.end(), [](const Candidate& a, const Candidate& b) {
    return a.modified != b.modified ? a.modified < b.modified : a.path < b.path;
  });
  for (const auto& file : files) {
    if (total + line.size() <= kBudgetBytes) break;
    std::error_code removeError;
    if (remove(file.path, removeError)) total = total > file.bytes ? total - file.bytes : 0;
  }

  int out = open((logs / bucket).c_str(), O_CREAT|O_WRONLY|O_APPEND|O_CLOEXEC|O_NOFOLLOW, 0600);
  if (out >= 0) {
    fchmod(out, 0600);
    appended = write(out, line.data(), line.size()) == static_cast<ssize_t>(line.size());
    close(out);
  }
  flock(lock, LOCK_UN); close(lock);
  return appended;
}

// Production builds always use Application Support; scratch roots are honored only in explicit
// native-testing builds.
inline std::filesystem::path ResolveRoot(std::filesystem::path support, const char* testRoot) {
#ifdef LANE_NATIVE_TESTING
  if (testRoot != nullptr && testRoot[0] == '/') return std::filesystem::path(testRoot);
#else
  (void)testRoot;
#endif
  support /= "io.delino.lane";
  return support;
}

}  // namespace lane_log

inline void LaneLog(const char* event, const char* operation = "", uint64_t durationMs = 0, const char* result = "") {
  @autoreleasepool {
    auto identifier = [](const char* value) {
      for (; *value; ++value) if (!((*value >= 'a' && *value <= 'z') || *value == '_')) return false;
      return true;
    };
    if (!identifier(event) || !identifier(operation) || !identifier(result)) return;
    std::string line = std::string("{\"event\":\"") + event + "\",\"operation\":\"" + operation +
      "\",\"result\":\"" + result + "\",\"durationMs\":" + std::to_string(durationMs) +
      ",\"correlationId\":\"" + NSUUID.UUID.UUIDString.UTF8String + "\"}\n";
    fputs(line.c_str(), stderr);
    NSURL* support = [[NSFileManager defaultManager] URLForDirectory:NSApplicationSupportDirectory
      inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:nil];
    if (!support) return;
    const char* testRoot = nullptr;
#ifdef LANE_NATIVE_TESTING
    NSString* test = NSProcessInfo.processInfo.environment[@"LANE_NATIVE_TEST_ROOT"];
    if (test.isAbsolutePath) testRoot = test.fileSystemRepresentation;
#endif
    std::filesystem::path root = lane_log::ResolveRoot(support.fileSystemRepresentation, testRoot);
    std::error_code error;
    auto logs = root / "logs";
    std::filesystem::create_directories(logs, error);
    if (error) return;
    chmod(root.c_str(), 0700); chmod(logs.c_str(), 0700);
    (void)lane_log::AppendAt(logs, std::chrono::system_clock::now(), line);
  }
}
