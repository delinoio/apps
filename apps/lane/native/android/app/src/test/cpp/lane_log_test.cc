// SPDX-License-Identifier: Apache-2.0
#include "lane_log.h"
#include <cassert>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <string>
#include <sys/wait.h>
#include <unistd.h>

int main() {
  namespace fs = std::filesystem;
  const auto root = fs::temp_directory_path() / ("lane-log-test-" + std::to_string(getpid()));
  const auto logs = root / "logs";
  fs::create_directories(logs);
  const auto old = logs / "another-client-old.jsonl";
  std::ofstream(old) << "old";
  fs::last_write_time(old, fs::file_time_type::clock::now() - std::chrono::hours(31 * 24));
  const auto oversized = logs / "another-client-large.jsonl";
  std::ofstream(oversized) << "sparse";
  fs::resize_file(oversized, 101ULL * 1024 * 1024);
  fs::last_write_time(oversized, fs::file_time_type::clock::now() - std::chrono::hours(24));
  const auto outside = root / "outside";
  std::ofstream(outside) << "preserved";
  fs::create_symlink(outside, logs / "symlink");
  lane_log(root.string(), "test_started");
  assert(!fs::exists(old));
  assert(!fs::exists(oversized));
  assert(fs::exists(outside) && fs::is_symlink(logs / "symlink"));
  for (int child = 0; child < 4; ++child) {
    auto pid = fork();
    assert(pid >= 0);
    if (pid == 0) {
      for (int i = 0; i < 25; ++i) lane_log(root.string(), "concurrent_event");
      _exit(0);
    }
  }
  for (int child = 0; child < 4; ++child) { int status; wait(&status); assert(WIFEXITED(status) && WEXITSTATUS(status) == 0); }
  lane_log(root.string(), "invalid\"event");
  int count = 0;
  for (const auto& entry : fs::directory_iterator(logs)) {
    if (entry.path().extension() != ".jsonl") continue;
    std::ifstream input(entry.path());
    std::string line;
    while (std::getline(input, line)) {
      assert(line.front() == '{' && line.back() == '}');
      assert(line.find("invalid") == std::string::npos);
      ++count;
    }
  }
  assert(count == 101);
  assert(fs::exists(logs / ".rotation.lock"));
  fs::remove_all(root);
}
