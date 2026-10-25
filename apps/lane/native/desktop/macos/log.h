// SPDX-License-Identifier: Apache-2.0
#pragma once
#import <Foundation/Foundation.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <algorithm>
#include <filesystem>
#include <vector>

// Shared with the Go CLI: the same advisory lock covers append and pruning.
// Only fixed event names enter this logger; never pass native error descriptions.
inline void LaneLog(const char* event) {
  @autoreleasepool {
    std::string line = std::string("{\"event\":\"") + event + "\"}\n";
    fputs(line.c_str(), stderr);
    NSURL* support = [[NSFileManager defaultManager] URLForDirectory:NSApplicationSupportDirectory
      inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:nil];
    if (!support) return;
    std::filesystem::path root = support.fileSystemRepresentation;
    root /= "io.delino.lane";
#ifdef LANE_NATIVE_TESTING
    NSString* test = NSProcessInfo.processInfo.environment[@"LANE_NATIVE_TEST_ROOT"];
    if (test.isAbsolutePath) root = test.fileSystemRepresentation;
#endif
    std::error_code error;
    auto logs = root / "logs";
    std::filesystem::create_directories(logs, error);
    if (error) return;
    chmod(root.c_str(), 0700); chmod(logs.c_str(), 0700);
    int lock = open((logs / ".rotation.lock").c_str(), O_CREAT|O_RDWR|O_CLOEXEC, 0600);
    if (lock < 0) return;
    if (flock(lock, LOCK_EX) != 0) { close(lock); return; }
    int file = open((logs / "native-macos.log").c_str(), O_CREAT|O_WRONLY|O_APPEND|O_CLOEXEC, 0600);
    if (file >= 0) { (void)write(file, line.data(), line.size()); close(file); }
    using Entry = std::filesystem::directory_entry;
    std::vector<Entry> files;
    uintmax_t total = 0;
    for (const auto& entry : std::filesystem::directory_iterator(logs, error)) {
      if (entry.path().filename() == ".rotation.lock" || !entry.is_regular_file(error) || entry.is_symlink(error)) continue;
      files.push_back(entry); total += entry.file_size(error);
    }
    std::sort(files.begin(), files.end(), [&](const Entry& a, const Entry& b) {
      return a.last_write_time(error) < b.last_write_time(error);
    });
    auto cutoff = std::filesystem::file_time_type::clock::now() - std::chrono::hours(24*30);
    for (const auto& entry : files) {
      if (entry.last_write_time(error) >= cutoff && total <= 100ULL*1024*1024) continue;
      auto size = entry.file_size(error);
      if (std::filesystem::remove(entry.path(), error)) total = total > size ? total-size : 0;
    }
    flock(lock, LOCK_UN); close(lock);
  }
}
