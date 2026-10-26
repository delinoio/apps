// SPDX-License-Identifier: Apache-2.0
// Synthetic regression coverage for the macOS desktop logger. It never reads real logs: every test
// uses a new scratch root under the temporary directory. Built twice by CMake: with
// LANE_NATIVE_TESTING for retention/migration coverage and without it for the production
// test-root-injection boundary.
#import <Foundation/Foundation.h>
#include "log.h"
#include <sys/wait.h>
#include <atomic>
#include <fstream>
#include <iostream>
#include <iterator>
#include <sstream>
#include <stdexcept>
#include <thread>

namespace fs = std::filesystem;
using Clock = std::chrono::system_clock;
using Hours = std::chrono::hours;

static void Check(bool value, const std::string& message) {
  if (!value) throw std::runtime_error(message);
}
static std::string Read(const fs::path& path) {
  std::ifstream in(path);
  return {std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>()};
}
static void Seed(const fs::path& path, const std::string& text, fs::file_time_type modified) {
  std::ofstream out(path);
  out << text;
  out.close();
  Check(static_cast<bool>(out), "fixture write failed");
  fs::last_write_time(path, modified);
}
static void SeedSparse(const fs::path& path, uintmax_t bytes, fs::file_time_type modified) {
  std::ofstream(path).close();
  fs::resize_file(path, bytes);
  fs::last_write_time(path, modified);
}
static fs::file_time_type Ago(int hours) { return fs::file_time_type::clock::now() - Hours(hours); }
static uintmax_t Total(const fs::path& logs) {
  uintmax_t total = 0;
  for (const auto& entry : fs::directory_iterator(logs))
    if (entry.is_regular_file() && entry.path().filename() != ".rotation.lock") total += entry.file_size();
  return total;
}
static std::string Line(const std::string& event) { return "{\"event\":\"" + event + "\"}\n"; }
static bool LockIsFree(const fs::path& logs) {
  int fd = open((logs / ".rotation.lock").c_str(), O_RDWR | O_CLOEXEC);
  if (fd < 0) return false;
  bool free = flock(fd, LOCK_EX | LOCK_NB) == 0;
  close(fd);
  return free;
}
static std::time_t Utc(int year, int month, int day, int hour = 0, int minute = 0, int second = 0) {
  std::tm utc{};
  utc.tm_year = year - 1900; utc.tm_mon = month - 1; utc.tm_mday = day;
  utc.tm_hour = hour; utc.tm_min = minute; utc.tm_sec = second;
  return timegm(&utc);
}
static std::string BucketFor(std::time_t seconds) { return lane_log::BucketName(Clock::from_time_t(seconds)); }

struct Scratch {
  fs::path root, logs;
  Scratch() {
    NSString* pattern = [NSTemporaryDirectory() stringByAppendingPathComponent:@"lane-log-test-XXXXXX"];
    std::string templ = pattern.fileSystemRepresentation;
    Check(mkdtemp(templ.data()) != nullptr, "scratch root");
    root = fs::canonical(templ);
    logs = root / "logs";
    fs::create_directories(logs);
    chmod(logs.c_str(), 0700);
  }
  ~Scratch() { std::error_code error; fs::remove_all(root, error); }
};

#ifdef LANE_NATIVE_TESTING

static void TestLegacyMigration() {
  for (int legacyAgeHours : {24 * 31, 1}) {
    Scratch scratch;
    auto legacy = scratch.logs / "native-macos.log";
    Seed(legacy, Line("synthetic_old_sentinel") + Line("synthetic_recent_sentinel"), Ago(legacyAgeHours));
    auto now = Clock::now();
    Check(lane_log::AppendAt(scratch.logs, now, Line("synthetic_new_event")), "append succeeds");
    Check(!fs::exists(legacy), "legacy file retired (age " + std::to_string(legacyAgeHours) + "h)");
    auto bucket = Read(scratch.logs / lane_log::BucketName(now));
    Check(bucket == Line("synthetic_new_event"), "bucket contains only the new event");
    Check(bucket.find("sentinel") == std::string::npos, "legacy records not copied");
    Check(fs::exists(scratch.logs / ".rotation.lock"), "lock retained");
  }
}

static void TestActiveRotationAndBoundary() {
  Scratch scratch;
  const std::time_t today = Utc(2026, 3, 31, 9, 30);
  const std::string expiredName = BucketFor(Utc(2026, 3, 1));   // start + 30 days == 2026-03-31 00:00.
  const std::string currentName = BucketFor(today);
  Seed(scratch.logs / expiredName, Line("synthetic_expired_event"), Ago(0));  // Active writes refreshed mtime.
  Seed(scratch.logs / currentName, Line("synthetic_current_one"), Ago(1));
  Check(lane_log::AppendAt(scratch.logs, Clock::from_time_t(today), Line("synthetic_current_two")), "append");
  Check(!fs::exists(scratch.logs / expiredName), "expired bucket removed by date despite recent mtime");
  Check(Read(scratch.logs / currentName) == Line("synthetic_current_one") + Line("synthetic_current_two"),
        "current bucket keeps both events");

  // Boundary: immediately before and exactly at bucket start plus 30 days (conservative daily rule).
  const std::time_t start = Utc(2026, 1, 10);
  const std::time_t expiry = start + 30 * 24 * 60 * 60;
  for (bool atExpiry : {false, true}) {
    Scratch boundary;
    const std::string name = BucketFor(start);
    Seed(boundary.logs / name, Line("synthetic_boundary"), Ago(0));
    lane_log::AppendAt(boundary.logs, Clock::from_time_t(atExpiry ? expiry : expiry - 1), Line("synthetic_probe"));
    Check(fs::exists(boundary.logs / name) == !atExpiry, atExpiry ? "removed at start+30d" : "retained at start+30d-1s");
  }

  // Simulated daily appends never reopen or extend an older bucket.
  Scratch daily;
  const std::string firstName = BucketFor(start);
  Seed(daily.logs / firstName, Line("synthetic_day_zero"), Ago(0));
  const std::string firstBody = Read(daily.logs / firstName);
  for (int day = 0; day <= 31; ++day) {
    const std::time_t noon = start + day * 24 * 60 * 60 + 12 * 60 * 60;
    Check(lane_log::AppendAt(daily.logs, Clock::from_time_t(noon), Line("synthetic_daily")), "daily append");
    if (day == 0) Check(Read(daily.logs / firstName) == firstBody + Line("synthetic_daily"), "same-day append joins the day bucket");
    if (day >= 30) Check(!fs::exists(daily.logs / firstName), "day-zero bucket expires on day 30 despite ongoing logging");
    if (day >= 1 && day < 30) Check(Read(daily.logs / firstName) == firstBody + Line("synthetic_daily"), "older bucket never appended after its day");
  }
}

static void TestSharedBudgetAndSiblings() {
  // Native subtotal and CLI subtotal are each under budget; the combined total plus the event is not.
  for (bool nativeOlder : {false, true}) {
    Scratch scratch;
    const auto now = Clock::now();
    const auto yesterday = BucketFor(Clock::to_time_t(now - Hours(24)));
    SeedSparse(scratch.logs / yesterday, 60ULL * 1024 * 1024, Ago(nativeOlder ? 72 : 48));
    SeedSparse(scratch.logs / "cli-20260101-4242.log", 45ULL * 1024 * 1024, Ago(nativeOlder ? 48 : 72));
    Check(lane_log::AppendAt(scratch.logs, now, Line("synthetic_budget")), "append over budget");
    Check(Total(scratch.logs) <= lane_log::kBudgetBytes, "combined bytes bounded after append");
    Check(fs::exists(scratch.logs / yesterday) == !nativeOlder, "oldest file evicted first (native)");
    Check(fs::exists(scratch.logs / "cli-20260101-4242.log") == nativeOlder, "oldest file evicted first (cli)");
    Check(fs::exists(scratch.logs / lane_log::BucketName(now)), "new event kept");
    Check(fs::exists(scratch.logs / ".rotation.lock"), "lock retained");
  }
  // Under budget: recent sibling preserved, 31-day-old sibling removed, lock retained and not replaced.
  Scratch scratch;
  Seed(scratch.logs / "cli-recent.log", Line("synthetic_cli_recent"), Ago(24));
  Seed(scratch.logs / "cli-old.log", Line("synthetic_cli_old"), Ago(24 * 31));
  Seed(scratch.logs / ".rotation.lock", "", Ago(24 * 90));  // Lock mtime must never make it eligible.
  struct stat before{}, after{};
  Check(stat((scratch.logs / ".rotation.lock").c_str(), &before) == 0, "stat lock");
  Check(lane_log::AppendAt(scratch.logs, Clock::now(), Line("synthetic_sibling")), "append");
  Check(fs::exists(scratch.logs / "cli-recent.log"), "recent sibling preserved");
  Check(!fs::exists(scratch.logs / "cli-old.log"), "31-day-old sibling removed");
  Check(stat((scratch.logs / ".rotation.lock").c_str(), &after) == 0 && before.st_ino == after.st_ino, "lock not replaced");
  struct stat bucket{};
  Check(stat((scratch.logs / lane_log::BucketName(Clock::now())).c_str(), &bucket) == 0 && (bucket.st_mode & 0777) == 0600,
        "bucket is private");
}

static void TestConcurrentWriters() {
  Scratch scratch;
  constexpr int threads = 6, perThread = 20, children = 2;
  std::vector<pid_t> pids;
  for (int child = 0; child < children; ++child) {
    pid_t pid = fork();
    Check(pid >= 0, "fork");
    if (pid == 0) {
      bool ok = true;
      for (int i = 0; i < perThread; ++i) ok &= lane_log::AppendAt(scratch.logs, Clock::now(), Line("synthetic_process_writer"));
      _exit(ok ? 0 : 1);
    }
    pids.push_back(pid);
  }
  std::atomic<int> failures{0};
  std::vector<std::thread> workers;
  for (int t = 0; t < threads; ++t) workers.emplace_back([&] {
    for (int i = 0; i < perThread; ++i)
      if (!lane_log::AppendAt(scratch.logs, Clock::now(), Line("synthetic_thread_writer"))) ++failures;
  });
  for (auto& worker : workers) worker.join();
  for (pid_t pid : pids) {
    int status = 0;
    Check(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0, "child writer succeeded");
  }
  Check(failures == 0, "thread writers succeeded");
  std::istringstream lines(Read(scratch.logs / lane_log::BucketName(Clock::now())));
  int count = 0;
  for (std::string line; std::getline(lines, line); ++count)
    Check(line == "{\"event\":\"synthetic_process_writer\"}" || line == "{\"event\":\"synthetic_thread_writer\"}", "no interleaved lines");
  Check(count == (threads + children) * perThread, "every concurrent event preserved exactly once");
  Check(LockIsFree(scratch.logs), "lock released");
}

static std::vector<std::string> failedPaths;
static bool FailingRemove(const fs::path& path, std::error_code& error) {
  failedPaths.push_back(path.filename().string());
  error = std::make_error_code(std::errc::permission_denied);
  return false;
}

static void TestFailuresStayBounded() {
  {  // Failed legacy removal must not move legacy metadata into a current bucket.
    Scratch scratch;
    Seed(scratch.logs / "native-macos.log", Line("synthetic_legacy_sentinel"), Ago(1));
    const auto now = Clock::now();
    failedPaths.clear();
    Check(lane_log::AppendAt(scratch.logs, now, Line("synthetic_after_failed_removal"), FailingRemove), "append still succeeds");
    Check(!failedPaths.empty() && failedPaths.front() == "native-macos.log", "legacy removal attempted");
    Check(Read(scratch.logs / "native-macos.log") == Line("synthetic_legacy_sentinel"), "legacy file untouched");
    Check(Read(scratch.logs / lane_log::BucketName(now)) == Line("synthetic_after_failed_removal"), "legacy not copied to bucket");
    Check(LockIsFree(scratch.logs), "lock released after failed removal");
  }
  {  // Failed pruning of an expired bucket must not relabel it as current.
    Scratch scratch;
    const auto now = Clock::from_time_t(Utc(2026, 5, 20, 8));
    const auto expired = BucketFor(Utc(2026, 4, 1));
    Seed(scratch.logs / expired, Line("synthetic_expired_sentinel"), Ago(0));
    Check(lane_log::AppendAt(scratch.logs, now, Line("synthetic_current"), FailingRemove), "append succeeds");
    Check(Read(scratch.logs / expired) == Line("synthetic_expired_sentinel"), "expired bucket not rewritten");
    Check(Read(scratch.logs / lane_log::BucketName(now)) == Line("synthetic_current"), "current bucket has only the new event");
    Check(LockIsFree(scratch.logs), "lock released");
  }
  {  // Failed append (a directory occupies the bucket path): legacy still retired, nothing copied.
    Scratch scratch;
    const auto now = Clock::now();
    Seed(scratch.logs / "native-macos.log", Line("synthetic_legacy_sentinel"), Ago(1));
    fs::create_directory(scratch.logs / lane_log::BucketName(now));
    Check(!lane_log::AppendAt(scratch.logs, now, Line("synthetic_unwritable")), "append reports failure without throwing");
    Check(!fs::exists(scratch.logs / "native-macos.log"), "legacy retired");
    Check(fs::is_directory(scratch.logs / lane_log::BucketName(now)), "bucket path unchanged");
    Check(LockIsFree(scratch.logs), "lock released after failed append");
  }
  {  // Business action continues: LaneLog returns normally when the logs root is unusable.
    Scratch scratch;
    fs::create_directory(scratch.logs / lane_log::BucketName(Clock::now()));
    setenv("LANE_NATIVE_TEST_ROOT", scratch.root.c_str(), 1);
    LaneLog("synthetic_failure_probe", "append", 1, "failed");
  }
}

static std::string Snapshot(const fs::path& logs) {
  std::string out;
  std::vector<fs::path> paths;
  for (const auto& entry : fs::directory_iterator(logs)) paths.push_back(entry.path());
  std::sort(paths.begin(), paths.end());
  for (const auto& path : paths) out += path.filename().string() + ":" + Read(path) + "\n";
  return out;
}

static void TestIsolationAndFiltering() {
  Scratch target, bystander;
  Seed(bystander.logs / "native-macos.log", Line("synthetic_bystander_legacy"), Ago(24 * 40));
  Seed(bystander.logs / "cli-old.log", Line("synthetic_bystander_old"), Ago(24 * 40));
  const std::string bystanderBefore = Snapshot(bystander.logs);
  setenv("LANE_NATIVE_TEST_ROOT", target.root.c_str(), 1);
  LaneLog("synthetic_isolation", "append", 7, "completed");
  Check(Snapshot(bystander.logs) == bystanderBefore, "other scratch root unchanged");
  const auto bucket = target.logs / lane_log::BucketName(Clock::now());
  Check(fs::exists(bucket), "event written under the injected root");
  const std::string body = Read(bucket);
  Check(body.find("\"event\":\"synthetic_isolation\"") != std::string::npos &&
        body.find("\"operation\":\"append\"") != std::string::npos &&
        body.find("\"durationMs\":7") != std::string::npos && body.find("correlationId") != std::string::npos,
        "metadata-only event shape");
  struct stat info{};
  Check(stat(target.root.c_str(), &info) == 0 && (info.st_mode & 0777) == 0700, "root private");
  Check(stat(target.logs.c_str(), &info) == 0 && (info.st_mode & 0777) == 0700, "logs private");

  // Invalid identifiers are dropped without touching the filesystem.
  const std::string before = Snapshot(target.logs);
  LaneLog("Bad-Event", "append", 1, "completed");
  LaneLog("synthetic_ok", "has space", 1, "completed");
  LaneLog("synthetic_ok", "append", 1, "result\"injection");
  LaneLog("synthetic_ok", "line\nbreak", 1, "completed");
  Check(Snapshot(target.logs) == before, "invalid identifiers rejected");
}

#else  // Production configuration.

static void TestProductionIgnoresTestRoot() {
  Scratch support, injected;
  // Source/build boundary: a production build resolves only the platform directory.
  auto resolved = lane_log::ResolveRoot(support.root, injected.root.c_str());
  Check(resolved == support.root / "io.delino.lane", "production ignores injected test root");

  // Isolated platform-directory fixture: CoreFoundation honors CFFIXED_USER_HOME, so Application
  // Support resolves under the scratch home and real user logs are never touched.
  setenv("CFFIXED_USER_HOME", support.root.c_str(), 1);
  setenv("LANE_NATIVE_TEST_ROOT", injected.root.c_str(), 1);
  LaneLog("synthetic_production_probe", "append", 1, "completed");
  Check(!fs::exists(injected.logs / lane_log::BucketName(Clock::now())), "production LaneLog ignored LANE_NATIVE_TEST_ROOT");
  Check(fs::is_empty(injected.logs), "injected root untouched");
  auto platformLogs = support.root / "Library" / "Application Support" / "io.delino.lane" / "logs";
  Check(fs::exists(platformLogs / lane_log::BucketName(Clock::now())), "event written under the isolated platform directory");
}

#endif

int main() {
  @autoreleasepool {
    try {
#ifdef LANE_NATIVE_TESTING
      TestLegacyMigration();
      TestActiveRotationAndBoundary();
      TestSharedBudgetAndSiblings();
      TestConcurrentWriters();
      TestFailuresStayBounded();
      TestIsolationAndFiltering();
#else
      TestProductionIgnoresTestRoot();
#endif
      std::cout << "Lane macOS log tests passed\n";
      return 0;
    } catch (const std::exception& error) {
      std::cerr << "Lane macOS log test failed: " << error.what() << "\n";
      return 1;
    }
  }
}
