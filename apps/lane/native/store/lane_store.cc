// SPDX-License-Identifier: Apache-2.0
#include "lane_store.h"
#include "ada.h"
#include "json.hpp"
#include "lane_schema.h"
#include "picosha2.h"
#include "sqlite3.h"
#include "utf8proc.h"
#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <exception>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <functional>
#include <map>
#include <optional>
#include <regex>
#include <set>
#include <sstream>
#include <stdexcept>
#include <tuple>
#include <thread>
#include <vector>
#ifndef _WIN32
#include <fcntl.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#else
#define NOMINMAX
#include <windows.h>
#endif

namespace lane {
namespace {
using J = nlohmann::json;
using Rows = std::vector<std::vector<std::string>>;
namespace fs = std::filesystem;
constexpr int kApp = 1279348293;
int64_t now() {
  return std::chrono::duration_cast<std::chrono::microseconds>(
             std::chrono::system_clock::now().time_since_epoch())
      .count();
}
std::string uuid() {
  unsigned char b[16];
  sqlite3_randomness(16, b);
  uint64_t t = now() / 1000;
  for (int i = 5; i >= 0; --i) {
    b[i] = t & 255;
    t >>= 8;
  }
  b[6] = (b[6] & 15) | 0x70;
  b[8] = (b[8] & 63) | 0x80;
  std::ostringstream s;
  s << std::hex << std::setfill('0');
  for (int i = 0; i < 16; ++i) {
    if (i == 4 || i == 6 || i == 8 || i == 10)
      s << '-';
    s << std::setw(2) << int(b[i]);
  }
  return s.str();
}
bool validId(const std::string &s) {
  static const std::regex r(
      "[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}");
  return std::regex_match(s, r);
}
struct Error : std::runtime_error {
  std::string reason;
  J extra;
  Error(std::string r, std::string m, J e = J::object())
      : runtime_error(m), reason(std::move(r)), extra(std::move(e)) {}
};
[[noreturn]] void fail(std::string r, std::string m) {
  throw Error(std::move(r), std::move(m));
}
void require(bool b, std::string m = "Invalid request.") {
  if (!b)
    fail("INVALID_INPUT", std::move(m));
}
void id(const std::string &s) {
  require(validId(s), "A lowercase UUID v7 is required.");
}
std::string hash(const std::string &s) {
  return picosha2::hash256_hex_string(s);
}
std::string str(const J &j, const char *k, std::string def = "") {
  if (!j.contains(k) || j[k].is_null())
    return def;
  require(j[k].is_string());
  return j[k].get<std::string>();
}
J parse(const std::string &s) {
  try {
    std::vector<std::set<std::string>> keys;
    return J::parse(s, [&](int, J::parse_event_t event, J &value) {
      if (event == J::parse_event_t::object_start)
        keys.emplace_back();
      else if (event == J::parse_event_t::key) {
        require(!keys.empty() &&
                    keys.back().insert(value.get<std::string>()).second,
                "Duplicate JSON field.");
      } else if (event == J::parse_event_t::object_end)
        keys.pop_back();
      return true;
    });
  } catch (...) {
    fail("INVALID_INPUT", "Invalid JSON.");
  }
}
std::string timestamp(int64_t n = now()) {
  time_t t = n / 1000000;
  std::tm tm{};
#ifdef _WIN32
  gmtime_s(&tm, &t);
#else
  gmtime_r(&t, &tm);
#endif
  char b[48];
  std::strftime(b, sizeof b, "%Y-%m-%dT%H:%M:%S", &tm);
  char f[16];
  std::snprintf(f, sizeof f, ".%06dZ", int(n % 1000000));
  return std::string(b) + f;
}
// ProtoJSON timestamps normalize to UTC; histories retain source chronology.
int64_t parseTimestamp(const std::string &s) {
  static const std::regex pattern("([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):"
                                  "([0-9]{2}):([0-9]{2})(\\.([0-9]{1,9}))?Z");
  std::smatch m;
  require(std::regex_match(s, m, pattern), "A UTC timestamp is required.");
  std::tm tm{};
  tm.tm_year = std::stoi(m[1]) - 1900;
  tm.tm_mon = std::stoi(m[2]) - 1;
  tm.tm_mday = std::stoi(m[3]);
  tm.tm_hour = std::stoi(m[4]);
  tm.tm_min = std::stoi(m[5]);
  tm.tm_sec = std::stoi(m[6]);
  require(tm.tm_mon >= 0 && tm.tm_mon < 12 && tm.tm_mday >= 1 &&
          tm.tm_mday <= 31 && tm.tm_hour < 24 && tm.tm_min < 60 &&
          tm.tm_sec < 60);
#ifdef _WIN32
  auto seconds = _mkgmtime(&tm);
#else
  auto seconds = timegm(&tm);
#endif
  std::string frac = m[8];
  while (frac.size() < 6)
    frac += '0';
  if (frac.size() > 6)
    require(frac.substr(6).find_first_not_of('0') == std::string::npos,
            "Timestamp exceeds microsecond precision.");
  auto us = int64_t(seconds) * 1000000 + std::stoll(frac.substr(0, 6));
  require(timestamp(us).substr(0, 19) == s.substr(0, 19),
          "Invalid timestamp date.");
  return us;
}
void sqlError(int rc) {
  if (rc == SQLITE_BUSY || rc == SQLITE_LOCKED)
    fail("STORAGE_BUSY", "Storage is busy. Retry this operation.");
  if (rc == SQLITE_FULL)
    fail("STORAGE_FULL", "Storage is full.");
  if (rc == SQLITE_CONSTRAINT)
    fail("INVALID_INPUT", "Storage constraint rejected the operation.");
  fail("DEPENDENCY_UNAVAILABLE", "Storage operation failed.");
}
// App and CLI share this lock and budget. Log only operation categories,
// correlation IDs and durations; no request fields or native error strings.
void writeLog(const fs::path &root, const std::string &operation,
              const J &result, int64_t elapsed) noexcept {
  try {
    auto dir = root / "logs";
    fs::create_directories(dir);
#ifndef _WIN32
    chmod(dir.c_str(), 0700);
    int fd = ::open((dir / ".rotation.lock").c_str(), O_CREAT | O_RDWR, 0600);
    if (fd < 0)
      return;
    if (flock(fd, LOCK_EX) != 0) {
      ::close(fd);
      return;
    }
    struct Unlock {
      int fd;
      ~Unlock() {
        flock(fd, LOCK_UN);
        ::close(fd);
      }
    } unlock{fd};
#else
    HANDLE fd = CreateFileW((dir / ".rotation.lock").c_str(),
                            GENERIC_READ | GENERIC_WRITE,
                            FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                            OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (fd == INVALID_HANDLE_VALUE)
      return;
    OVERLAPPED overlapped{};
    if (!LockFileEx(fd, LOCKFILE_EXCLUSIVE_LOCK, 0, 1, 0, &overlapped)) {
      CloseHandle(fd);
      return;
    }
    struct Unlock {
      HANDLE fd;
      OVERLAPPED *o;
      ~Unlock() {
        UnlockFileEx(fd, 0, 1, 0, o);
        CloseHandle(fd);
      }
    } unlock{fd, &overlapped};
#endif
    auto date = timestamp().substr(0, 10);
    auto file = dir / ("native-" + date + ".jsonl");
    std::string category = result.value("ok", false) ? "APPLIED" : "FAILED";
    std::string correlation = uuid();
    if (result.contains("error")) {
      category = str(result["error"], "reason", "FAILED");
      correlation = str(result["error"], "correlationId", correlation);
    }
    // Caller-supplied unknown method names are deliberately not logged.
    std::string safe = operation;
    static const std::regex name("[A-Za-z]{1,48}");
    if (!std::regex_match(safe, name))
      safe = "Unknown";
    {
      std::ofstream out(file, std::ios::app);
      out << J{{"time", timestamp()},
               {"operation", safe},
               {"correlationId", correlation},
               {"result", category},
               {"durationUs", std::to_string(elapsed)}}
                 .dump()
          << '\n';
    }
#ifndef _WIN32
    chmod(file.c_str(), 0600);
#endif
    std::vector<fs::directory_entry> files;
    uintmax_t size = 0;
    auto cutoff =
        fs::file_time_type::clock::now() - std::chrono::hours(24 * 30);
    for (auto &entry : fs::directory_iterator(dir)) {
      if (entry.path().filename() == ".rotation.lock" || entry.is_symlink() ||
          !entry.is_regular_file())
        continue;
      if (entry.last_write_time() < cutoff) {
        fs::remove(entry.path());
        continue;
      }
      size += entry.file_size();
      files.push_back(entry);
    }
    std::sort(files.begin(), files.end(), [](auto &a, auto &b) {
      return a.last_write_time() < b.last_write_time();
    });
    for (auto &entry : files) {
      if (size <= 100 * 1024 * 1024)
        break;
      auto n = entry.file_size();
      fs::remove(entry.path());
      size -= n;
    }
  } catch (
      ...) { // Logging failure cannot roll back an already committed write.
  }
}
struct Db {
  sqlite3 *p = nullptr;
  explicit Db(const fs::path &path) {
    int rc = sqlite3_open_v2(path.u8string().c_str(), &p,
                             SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE |
                                 SQLITE_OPEN_FULLMUTEX,
                             nullptr);
    if (rc != SQLITE_OK) {
      if (p)
        sqlite3_close(p);
      p = nullptr;
      sqlError(rc);
    }
    sqlite3_busy_timeout(p, 5000);
    exec("PRAGMA foreign_keys=ON; PRAGMA secure_delete=ON;");
#ifndef _WIN32
    chmod(path.c_str(), 0600);
#endif
  }
  ~Db() {
    if (p)
      sqlite3_close(p);
  }
  Db(const Db &) = delete;
  void exec(const std::string &s) {
    int rc = sqlite3_exec(p, s.c_str(), nullptr, nullptr, nullptr);
    if (rc != SQLITE_OK)
      sqlError(rc & 255);
  }
  Rows run(const std::string &s, const std::vector<std::string> &args = {}) {
    sqlite3_stmt *q = nullptr;
    int rc = sqlite3_prepare_v2(p, s.c_str(), -1, &q, nullptr);
    if (rc != SQLITE_OK)
      sqlError(rc & 255);
    std::unique_ptr<sqlite3_stmt, decltype(&sqlite3_finalize)> guard(
        q, sqlite3_finalize);
    for (size_t i = 0; i < args.size(); ++i) {
      rc = sqlite3_bind_text(q, int(i + 1), args[i].data(), int(args[i].size()),
                             SQLITE_TRANSIENT);
      if (rc != SQLITE_OK)
        sqlError(rc & 255);
    }
    Rows rows;
    while ((rc = sqlite3_step(q)) == SQLITE_ROW) {
      std::vector<std::string> r;
      for (int i = 0; i < sqlite3_column_count(q); ++i) {
        auto b = sqlite3_column_text(q, i);
        r.emplace_back(b ? reinterpret_cast<const char *>(b) : "",
                       sqlite3_column_bytes(q, i));
      }
      rows.push_back(std::move(r));
    }
    if (rc != SQLITE_DONE)
      sqlError(rc & 255);
    return rows;
  }
  std::string one(const std::string &s, const std::vector<std::string> &a = {},
                  std::string def = "") {
    auto r = run(s, a);
    return r.empty() ? def : r[0][0];
  }
};
struct Tx {
  Db &d;
  bool committed = false;
  explicit Tx(Db &db, bool write = true) : d(db) {
    d.exec(write ? "BEGIN IMMEDIATE" : "BEGIN");
  }
  ~Tx() {
    if (!committed)
      sqlite3_exec(d.p, "ROLLBACK", nullptr, nullptr, nullptr);
  }
  void commit() {
    d.exec("COMMIT");
    committed = true;
  }
};
void privateDir(const fs::path &p) {
  fs::create_directories(p);
#ifndef _WIN32
  chmod(p.c_str(), 0700);
#endif
}
// Stable OS lock inode lives outside the removable database directory.
// The Go adapter uses this same flock/LockFileEx byte-range protocol.
class LifetimeLock {
#ifndef _WIN32
  int fd_ = -1;
#else
  HANDLE fd_ = INVALID_HANDLE_VALUE;
  OVERLAPPED overlapped_{};
#endif
public:
  LifetimeLock(const fs::path &path, bool exclusive) {
#ifndef _WIN32
    fd_ = ::open(path.c_str(), O_CREAT | O_RDWR, 0600);
    if (fd_ < 0)
      fail("DEPENDENCY_UNAVAILABLE", "Cannot acquire storage ownership.");
#else
    fd_ = CreateFileW(path.c_str(), GENERIC_READ | GENERIC_WRITE,
                      FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_ALWAYS,
                      FILE_ATTRIBUTE_NORMAL, nullptr);
    if (fd_ == INVALID_HANDLE_VALUE)
      fail("DEPENDENCY_UNAVAILABLE", "Cannot acquire storage ownership.");
#endif
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    for (;;) {
#ifndef _WIN32
      if (flock(fd_, (exclusive ? LOCK_EX : LOCK_SH) | LOCK_NB) == 0)
        return;
      if (errno != EWOULDBLOCK && errno != EAGAIN && errno != EINTR) {
        ::close(fd_);
        fd_ = -1;
        fail("DEPENDENCY_UNAVAILABLE", "Cannot acquire storage ownership.");
      }
#else
      if (LockFileEx(fd_,
                     LOCKFILE_FAIL_IMMEDIATELY |
                         (exclusive ? LOCKFILE_EXCLUSIVE_LOCK : 0),
                     0, 1, 0, &overlapped_))
        return;
      if (GetLastError() != ERROR_LOCK_VIOLATION) {
        CloseHandle(fd_);
        fd_ = INVALID_HANDLE_VALUE;
        fail("DEPENDENCY_UNAVAILABLE", "Cannot acquire storage ownership.");
      }
#endif
      if (std::chrono::steady_clock::now() >= deadline) {
#ifndef _WIN32
        ::close(fd_);
        fd_ = -1;
#else
        CloseHandle(fd_);
        fd_ = INVALID_HANDLE_VALUE;
#endif
        fail("STORAGE_BUSY", "Another process is using this space. Retry "
                             "removal after it drains.");
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
  }
  ~LifetimeLock() {
#ifndef _WIN32
    if (fd_ >= 0) {
      flock(fd_, LOCK_UN);
      ::close(fd_);
    }
#else
    if (fd_ != INVALID_HANDLE_VALUE) {
      UnlockFileEx(fd_, 0, 1, 0, &overlapped_);
      CloseHandle(fd_);
    }
#endif
  }
  LifetimeLock(const LifetimeLock &) = delete;
};
// Pre-release exception: only schema 3 / format 3 stores are supported. Any
// other nonempty store is refused with the explicit reset instruction and is
// never migrated, replaced or treated as empty.
constexpr const char *kStoreVersion = "3";
constexpr const char *kUnsupportedStore =
    "This Lane data store uses an unsupported format. Lane never migrates or "
    "resets it automatically. Replace it with an empty store only through the "
    "explicit development data reset (ResetDevelopmentData in the app, or "
    "lane --space <uuid> data reset --confirm-development-reset).";
void compatibility(Db &d, bool registry = false) {
  if (d.one("PRAGMA application_id") != std::to_string(kApp) ||
      d.one("PRAGMA user_version") != (registry ? "1" : kStoreVersion))
    fail("UNSUPPORTED_FORMAT", registry
                                   ? "Update Lane to open this data format."
                                   : kUnsupportedStore);
  if (!registry &&
      d.one("SELECT value FROM store_meta WHERE key='format_version'") !=
          kStoreVersion)
    fail("UNSUPPORTED_FORMAT", kUnsupportedStore);
}
void initialize(Db &d, bool registry) {
  auto v = d.one("PRAGMA user_version"), a = d.one("PRAGMA application_id");
  if (v == "0" && a == "0" &&
      d.one("SELECT count(*) FROM sqlite_master WHERE name NOT LIKE "
            "'sqlite_%'") == "0") {
    Tx t(d);
    d.exec(registry ? kRegistry : kSchema);
    d.exec(registry ? "PRAGMA application_id=1279348293; PRAGMA user_version=1;"
                    : "PRAGMA application_id=1279348293; PRAGMA user_version=3;");
    if (!registry)
      d.run("INSERT INTO store_meta VALUES('format_version','3')");
    t.commit();
  }
  compatibility(d, registry);
  d.exec("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;");
}
std::string meta(Db &d, const std::string &k, std::string def = "") {
  return d.one("SELECT value FROM store_meta WHERE key=?", {k}, def);
}
void setMeta(Db &d, const std::string &k, const std::string &v) {
  d.run("INSERT INTO store_meta(key,value) VALUES(?,?) ON CONFLICT(key) DO "
        "UPDATE SET value=excluded.value",
        {k, v});
}
std::string sequence(Db &d) {
  return d.one("SELECT COALESCE(MAX(sequence),0) FROM changes");
}
void changed(Db &d, const std::string &entity = "", int kind = 1) {
  d.run("INSERT INTO changes(entity_id,change_kind) VALUES(?,?)",
        {entity, std::to_string(kind)});
}
std::string enumValue(const J &j, const char *key, const std::string &prefix,
                      const std::vector<std::string> &values,
                      int defaultValue = 0) {
  int n = 0;
  if (j.contains(key) && !j[key].is_null()) {
    auto v = j[key];
    if (v.is_number_integer())
      n = v.get<int>();
    else if (v.is_string()) {
      auto s = v.get<std::string>();
      if (s == prefix + "UNSPECIFIED")
        n = 0;
      else {
        auto i = std::find(values.begin(), values.end(), s);
        require(i != values.end(), "Unknown enum value.");
        n = int(i - values.begin()) + 1;
      }
    } else
      require(false);
  }
  if (n == 0)
    n = defaultValue;
  require(n > 0 && n <= int(values.size()), "Missing or invalid enum value.");
  return values[n - 1];
}
const std::vector<std::string> progresses = {
    "PROGRESS_TODO", "PROGRESS_IN_PROGRESS", "PROGRESS_BLOCKED",
    "PROGRESS_DONE", "PROGRESS_CANCELED"};
const std::vector<std::string> proposals = {
    "PROPOSAL_REGULAR", "PROPOSAL_PENDING", "PROPOSAL_ACCEPTED",
    "PROPOSAL_REJECTED"};
std::string fold(const std::string &s) {
  utf8proc_uint8_t *out = nullptr;
  auto n = utf8proc_map(reinterpret_cast<const utf8proc_uint8_t *>(s.data()),
                        s.size(), &out, UTF8PROC_CASEFOLD);
  require(n >= 0, "Invalid Unicode.");
  std::string r(reinterpret_cast<char *>(out), n);
  free(out);
  return r;
}
bool hasText(const std::string &s) {
  size_t pos = 0;
  while (pos < s.size()) {
    utf8proc_int32_t cp;
    auto n = utf8proc_iterate(
        reinterpret_cast<const utf8proc_uint8_t *>(s.data() + pos),
        s.size() - pos, &cp);
    require(n > 0, "Invalid Unicode.");
    pos += n;
    auto cat = utf8proc_category(cp);
    if (cat != UTF8PROC_CATEGORY_ZS && cat != UTF8PROC_CATEGORY_ZL &&
        cat != UTF8PROC_CATEGORY_ZP && !(cp >= 9 && cp <= 13))
      return true;
  }
  return false;
}
std::string normalizeURL(std::string s) {
  require(s.find_first_of("?#\\\r\n\t ") == std::string::npos,
          "Invalid repository URL.");
  std::string authority, path, scheme;
  size_t pos = s.find("://");
  if (pos != std::string::npos) {
    scheme = s.substr(0, pos);
    require(scheme == "https" || scheme == "ssh",
            "Use HTTPS or SSH repository URLs.");
    s = s.substr(pos + 3);
    pos = s.find('/');
    require(pos != std::string::npos);
    authority = s.substr(0, pos);
    path = s.substr(pos);
  } else {
    scheme = "ssh";
    pos = s.find(':');
    require(pos != std::string::npos);
    authority = s.substr(0, pos);
    require(authority.find('@') != std::string::npos);
    path = s.substr(pos + 1);
  }
  pos = authority.rfind('@');
  if (pos != std::string::npos)
    authority = authority.substr(pos + 1);
  require(!authority.empty());
  std::string host = authority, port;
  pos = authority.rfind(':');
  if (pos != std::string::npos && authority.find(']') == std::string::npos) {
    host = authority.substr(0, pos);
    port = authority.substr(pos + 1);
    require(!port.empty() &&
            std::all_of(port.begin(), port.end(),
                        [](unsigned char c) { return c >= '0' && c <= '9'; }));
    auto pn = std::stoul(port);
    require(pn > 0 && pn <= 65535);
    port = std::to_string(pn);
    if ((scheme == "https" && port == "443") ||
        (scheme == "ssh" && port == "22"))
      port.clear();
  }
  host = ada::idna::to_ascii(host);
  require(!host.empty() && host.find_first_of("/@:%\\") == std::string::npos,
          "Invalid repository host.");
  std::transform(host.begin(), host.end(), host.begin(),
                 [](unsigned char c) { return char(std::tolower(c)); });
  while (!path.empty() && path.front() == '/')
    path.erase(path.begin());
  while (!path.empty() && path.back() == '/')
    path.pop_back();
  std::string decoded;
  auto hex = [](char c) -> int {
    if (c >= '0' && c <= '9')
      return c - '0';
    if (c >= 'a' && c <= 'f')
      return c - 'a' + 10;
    if (c >= 'A' && c <= 'F')
      return c - 'A' + 10;
    return -1;
  };
  for (size_t i = 0; i < path.size(); ++i) {
    unsigned char c = path[i];
    if (c == '%') {
      require(i + 2 < path.size());
      int a = hex(path[i + 1]), b = hex(path[i + 2]);
      require(a >= 0 && b >= 0);
      c = (a << 4) | b;
      require(c != '/' && c != '\\');
      if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
          (c >= '0' && c <= '9') || c == '-' || c == '.' || c == '_' ||
          c == '~')
        decoded += char(c);
      else {
        decoded += '%';
        decoded += "0123456789ABCDEF"[a];
        decoded += "0123456789ABCDEF"[b];
      }
      i += 2;
    } else {
      require(c >= 32 && c != 127);
      decoded += char(c);
    }
  }
  if (decoded.size() >= 4 && decoded.substr(decoded.size() - 4) == ".git")
    decoded.resize(decoded.size() - 4);
  require(!decoded.empty());
  std::istringstream segments(decoded);
  std::string seg;
  while (std::getline(segments, seg, '/'))
    require(seg != "." && seg != "..");
  fold(decoded);
  return host + (port.empty() ? "" : ":" + port) + "/" + decoded;
}
J canonical(J j) {
  if (j.is_object()) {
    J r = J::object();
    for (auto it = j.begin(); it != j.end(); ++it) {
      std::string k = it.key(), camel;
      bool upper = false;
      for (char c : k) {
        if (c == '_') {
          upper = true;
          continue;
        }
        camel += upper ? char(std::toupper(c)) : c;
        upper = false;
      }
      require(!r.contains(camel), "Duplicate ProtoJSON field.");
      if (!it.value().is_null())
        r[camel] = camel == "payload" ? it.value() : canonical(it.value());
    }
    return r;
  }
  if (j.is_array())
    for (auto &v : j)
      v = canonical(v);
  return j;
}
void fields(const J &j, std::initializer_list<const char *> allowed) {
  require(j.is_object());
  for (auto it = j.begin(); it != j.end(); ++it)
    require(std::find_if(allowed.begin(), allowed.end(),
                         [&](const char *k) { return it.key() == k; }) !=
                allowed.end(),
            "Unknown request field.");
}
J success(J r, std::string seq = "0") {
  return J{
      {"ok", true}, {"response", std::move(r)}, {"sequence", std::move(seq)}};
}
J failure(const Error &e) {
  J err = {{"reason", e.reason.rfind("ERROR_REASON_", 0) == 0
                          ? e.reason
                          : "ERROR_REASON_" + e.reason},
           {"message", e.what()},
           {"correlationId", uuid()},
           {"retryable", e.reason == "STORAGE_BUSY" ||
                             e.reason == "DEPENDENCY_UNAVAILABLE"}};
  // Preserve typed metadata only. Remote transport errors can contain raw
  // request/native details, which must not cross the JavaScript boundary.
  for (auto key : {"entityId", "currentHeadIds", "retryable", "violation"})
    if (e.extra.contains(key))
      err[key] = e.extra[key];
  return J{{"ok", false}, {"error", err}};
}
template <class F> std::string protect(F f) {
  try {
    return f().dump();
  } catch (const Error &e) {
    return failure(e).dump();
  } catch (const J::exception &) {
    return failure(Error("INVALID_INPUT", "Invalid request type.")).dump();
  } catch (...) {
    return failure(Error("DEPENDENCY_UNAVAILABLE",
                         "The operation could not be completed."))
        .dump();
  }
}
} // namespace

#include "lane_capture_helpers.h"
#include "lane_planning.h"

struct Store::Impl {
  fs::path root;
  std::unique_ptr<Db> registry;
  HostHooks hooks;
  std::exception_ptr initializationFailure;
  struct Space {
    std::unique_ptr<LifetimeLock> lifetime;
    std::unique_ptr<Db> db, observer;
    int handles = 0;
    std::string seen;
  };
  std::map<std::string, Space> spaces;
  explicit Impl(std::string r) : root(fs::u8path(r)) {
    try {
      require(!r.empty());
      privateDir(root);
      privateDir(root / "spaces");
      privateDir(root / "locks");
      registry = std::make_unique<Db>(root / "registry.sqlite");
      initialize(*registry, true);
      Tx tx(*registry);
      compatibility(*registry, true);
      if (registry->one("SELECT value FROM registry_meta WHERE key='device_id'")
              .empty())
        registry->run("INSERT INTO registry_meta VALUES('device_id',?)",
                      {uuid()});
      if (registry
              ->one(
                  "SELECT value FROM registry_meta WHERE key='active_space_id'")
              .empty()) {
        auto sid = uuid();
        registry->run("INSERT INTO spaces(id,account_key) VALUES(?,'local')",
                      {sid});
        registry->run("INSERT INTO registry_meta VALUES('active_space_id',?)",
                      {sid});
      }
      tx.commit();
    } catch (...) {
      initializationFailure = std::current_exception();
    }
  }
  void ready() {
    if (initializationFailure)
      std::rethrow_exception(initializationFailure);
  }
  bool unavailable(const std::string &s) {
    return registry->one("SELECT value FROM registry_meta WHERE key=?",
                         {"unavailable:" + s}) == "1";
  }
  std::string device() {
    return registry->one(
        "SELECT value FROM registry_meta WHERE key='device_id'");
  }
  std::string active() {
    return registry->one(
        "SELECT value FROM registry_meta WHERE key='active_space_id'");
  }
  void registered(const std::string &s) {
    id(s);
    compatibility(*registry, true);
    if (unavailable(s) ||
        registry
            ->one("SELECT id FROM spaces WHERE id=? AND account_key NOT LIKE "
                  "'removed:%'",
                  {s})
            .empty()) {
      auto it = spaces.find(s);
      if (it != spaces.end() &&
          (!it->second.db || sqlite3_get_autocommit(it->second.db->p)))
        spaces.erase(it);
      fail("NOT_FOUND", "This local space is unavailable.");
    }
  }
  // Fresh stores and development resets share this metadata seed.
  void seedMeta(Db &d, const std::string &s) {
    setMeta(d, "space_id", s);
    setMeta(d, "device_id", device());
    setMeta(d, "account_epoch", "");
    setMeta(d, "pull_cursor", "");
    setMeta(d, "notification_enabled", "0");
    setMeta(d, "notification_content", "DETAIL");
    setMeta(d, "sync_owner", "");
    setMeta(d, "sync_owner_expires_at_us", "0");
  }
  Db &db(const std::string &s) {
    registered(s);
    auto &slot = spaces[s];
    if (!slot.db) {
      // An interrupted development reset leaves partially removed data. Never
      // open it as valid; repeating the reset completes it.
      if (!registry->one("SELECT value FROM registry_meta WHERE key=?",
                         {"reset:" + s})
               .empty())
        fail("STORAGE_BUSY", "A development reset was interrupted. Repeat the "
                             "reset to finish it.");
      auto lifetime = std::make_unique<LifetimeLock>(
          root / "locks" / (s + ".handles.lock"), false);
      if (unavailable(s))
        fail("NOT_FOUND", "This local space is unavailable.");
      auto dir = root / "spaces" / s;
      privateDir(dir);
      // Install the connections only after compatibility is established so an
      // unsupported store never keeps its lifetime lease or a half-open slot.
      auto primary = std::make_unique<Db>(dir / "lane.sqlite");
      initialize(*primary, false);
      {
        Tx t(*primary);
        if (meta(*primary, "space_id").empty())
          seedMeta(*primary, s);
        require(meta(*primary, "space_id") == s, "Storage routing mismatch.");
        t.commit();
      }
      auto observer = std::make_unique<Db>(dir / "lane.sqlite");
      initialize(*observer, false);
      slot.lifetime = std::move(lifetime);
      slot.db = std::move(primary);
      slot.observer = std::move(observer);
    }
    compatibility(*slot.db);
    return *slot.db;
  }
  bool enabled(const std::string &s) {
    return registry->one("SELECT sync_enabled FROM spaces WHERE id=?", {s}) ==
           "1";
  }
  J removeData(const std::string &s) {
    id(s);
    compatibility(*registry, true);
    bool marked = unavailable(s);
    if (!marked)
      registered(s);
    if (hooks.disconnect)
      hooks.disconnect(s);
    {
      Tx t(*registry);
      registry->run("INSERT INTO registry_meta(key,value) VALUES(?,'1') ON "
                    "CONFLICT(key) DO UPDATE SET value='1'",
                    {"unavailable:" + s});
      registry->run("UPDATE spaces SET sync_enabled=0,account_key=? WHERE id=?",
                    {"removed:" + s, s});
      t.commit();
    }
    // Closing both physical connections releases the SH lifetime lock last.
    spaces.erase(s);
    LifetimeLock exclusive(root / "locks" / (s + ".handles.lock"), true);
    std::error_code ec;
    fs::remove_all(root / "spaces" / s, ec);
    if (ec)
      fail("DEPENDENCY_UNAVAILABLE",
           "Could not remove this space. Retry cleanup.");
    {
      Tx t(*registry);
      registry->run("DELETE FROM spaces WHERE id=?", {s});
      if (active() == s) {
        auto local =
            registry->one("SELECT id FROM spaces WHERE account_key='local'");
        if (local.empty()) {
          local = uuid();
          registry->run("INSERT INTO spaces(id,account_key) VALUES(?,'local')",
                        {local});
        }
        registry->run(
            "UPDATE registry_meta SET value=? WHERE key='active_space_id'",
            {local});
      }
      t.commit();
    }
    return success({{"completed", true}});
  }
  // Explicit development reset of one existing local space. Never implicit:
  // the caller already holds the user's typed confirmation.
  J resetData(std::string s) {
    id(s);
    compatibility(*registry, true);
    registered(s);
    // Release this process's own handles; another process or a running sync
    // worker keeps its shared lease and makes the exclusive wait fail.
    auto slot = spaces.find(s);
    if (slot != spaces.end()) {
      slot->second.observer.reset();
      slot->second.db.reset();
      slot->second.lifetime.reset();
    }
    LifetimeLock exclusive(root / "locks" / (s + ".handles.lock"), true);
    const auto dir = root / "spaces" / s;
    const std::string markerKey = "reset:" + s;
    // The marker records the binding to keep so a repeated reset after an
    // interruption behaves exactly like the first attempt.
    auto marker = registry->one("SELECT value FROM registry_meta WHERE key=?", {markerKey});
    J binding;
    if (!marker.empty()) {
      binding = parse(marker);
    } else {
      const bool account =
          registry->one("SELECT account_key FROM spaces WHERE id=?", {s}) != "local";
      std::string epoch;
      if (account) {
        // Keep the retained epoch when it is readable so reconnect follows the
        // same confirmation path as an epoch change.
        sqlite3 *raw = nullptr;
        auto path = (dir / "lane.sqlite").u8string();
        if (fs::exists(dir / "lane.sqlite") &&
            sqlite3_open_v2(path.c_str(), &raw, SQLITE_OPEN_READONLY, nullptr) == SQLITE_OK) {
          sqlite3_stmt *q = nullptr;
          if (sqlite3_prepare_v2(raw, "SELECT value FROM store_meta WHERE key='account_epoch'", -1, &q, nullptr) == SQLITE_OK) {
            if (sqlite3_step(q) == SQLITE_ROW && sqlite3_column_text(q, 0))
              epoch = reinterpret_cast<const char *>(sqlite3_column_text(q, 0));
            sqlite3_finalize(q);
          }
        }
        if (raw)
          sqlite3_close(raw);
      }
      binding = {{"account", account}, {"epoch", epoch}};
      Tx t(*registry);
      registry->run("INSERT INTO registry_meta(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    {markerKey, binding.dump()});
      t.commit();
    }
    std::error_code ec;
    fs::remove_all(dir, ec); // Same removal path as RemoveDeviceData.
    if (ec)
      fail("DEPENDENCY_UNAVAILABLE", "Could not remove this space. Repeat the reset.");
    privateDir(dir);
    {
      Db fresh(dir / "lane.sqlite");
      initialize(fresh, false);
      Tx t(fresh);
      seedMeta(fresh, s);
      if (binding.value("account", false)) {
        setMeta(fresh, "account_epoch", str(binding, "epoch"));
        setMeta(fresh, "reconnect_required", "1");
        setMeta(fresh, "sync_error_reason", "ERROR_REASON_STALE_EPOCH");
      }
      t.commit();
    }
    {
      Tx t(*registry);
      if (binding.value("account", false))
        registry->run("UPDATE spaces SET sync_enabled=0 WHERE id=?", {s});
      registry->run("DELETE FROM registry_meta WHERE key=?", {markerKey});
      t.commit();
    }
    // Force the next observer poll to report the new database.
    slot = spaces.find(s);
    if (slot != spaces.end())
      slot->second.seen = "reset";
    return success({{"completed", true}});
  }
  J origin(const std::string &s) {
    return J{{"kind", "ORIGIN_KIND_APP"},
             {"deviceId", device()},
             {"deviceName", "Lane"},
             {"accountId",
              registry->one("SELECT account_id FROM spaces WHERE id=?", {s})}};
  }
  std::string mapped(Db &d, std::string s) {
    std::set<std::string> seen;
    while (true) {
      require(seen.insert(s).second, "Invalid repository mapping.");
      auto r = d.one(
          "SELECT canonical_id FROM repository_mappings WHERE local_id=?", {s});
      if (r.empty() || r == s)
        return s;
      s = r;
    }
  }
  std::vector<std::string> aliases(Db &d, const std::string &e) {
    auto canonical = mapped(d, e);
    std::vector<std::string> ids{canonical};
    for (auto &r : d.run("SELECT local_id FROM repository_mappings"))
      if (mapped(d, r[0]) == canonical && r[0] != canonical)
        ids.push_back(r[0]);
    return ids;
  }
  std::vector<J> heads(Db &d, const std::string &e) {
    std::vector<J> h;
    for (auto &alias : aliases(d, e))
      for (auto &r :
           d.run("SELECT v.envelope_json FROM heads h JOIN versions v ON "
                 "h.version_id=v.version_id AND h.entity_id=v.entity_id WHERE "
                 "h.entity_id=? ORDER BY h.version_id",
                 {alias}))
        h.push_back(parse(r[0]));
    // A canonical tombstone dominates every retained alias.
    for (auto &v : h)
      if (v.value("deleted", false))
        return {v};
    return h;
  }
  J result(Db &d, const std::string &e,
           std::string kind = "RESULT_KIND_APPLIED") {
    auto h = heads(d, e);
    if (h.empty())
      fail("NOT_FOUND", "Entity not found.");
    if (h[0].value("deleted", false))
      kind = "RESULT_KIND_DELETED";
    else if (h.size() > 1)
      kind = "RESULT_KIND_CONFLICT";
    J r = {{"result", kind}, {"version", h[0]}, {"alternatives", J::array()}};
    if (h.size() > 1)
      r["alternatives"] = h;
    return r;
  }
  J current(Db &d, const std::string &e, int kind = 0) {
    auto h = heads(d, e);
    if (h.empty())
      fail("NOT_FOUND", "Entity not found.");
    if (h[0].value("deleted", false))
      fail("DELETED", "Entity was permanently deleted.");
    if (kind)
      require(h[0]["kind"] == kindName(kind));
    if (h.size() > 1)
      fail("CONFLICT_READ_ONLY", "Resolve the conflict before editing.");
    return h[0];
  }
  void repository(Db &d, const std::string &e) {
    auto h = heads(d, mapped(d, e));
    if (h.empty())
      fail("NOT_FOUND", "Repository not found.");
    if (h[0].value("deleted", false))
      fail("DELETED", "Repository was deleted.");
    require(h[0]["kind"] == "ENTITY_KIND_REPOSITORY");
  }
  void uniqueURL(Db &d, const std::string &url, const std::string &except) {
    for (auto &r :
         d.run("SELECT v.entity_id,v.envelope_json FROM heads h JOIN versions "
               "v ON v.version_id=h.version_id JOIN entities e ON "
               "e.id=v.entity_id WHERE e.kind=1 AND e.deleted=0")) {
      auto v = parse(r[1]);
      if (mapped(d, r[0]) != mapped(d, except) && v.contains("repository") &&
          v["repository"].value("normalizedUrl", "") == url)
        fail("INVALID_INPUT", "Repository URL is already registered.");
    }
  }
  J connectionBody(Db &d, const std::string &e, J b, bool unique = true) {
    fields(b, {"prefix", "repositoryId"});
    require(normalizePrefix(str(b, "prefix")) == str(b, "prefix"), "URL prefix is not normalized.");
    id(str(b, "repositoryId"));
    repository(d, str(b, "repositoryId"));
    if (unique) {
      current(d, mapped(d, str(b, "repositoryId")), 1);
      for (auto &r : d.run("SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id JOIN entities e ON e.id=h.entity_id WHERE e.kind=3 AND e.deleted=0 AND e.id<>?", {e}))
        require(parse(r[0])["urlConnection"]["prefix"] != b["prefix"], "This URL prefix already has a connection.");
    }
    return b;
  }
  void captureIdentity(Db &d, const std::string &task, const J &capture) {
    captureManifest(capture);
    auto aid = str(capture, "attachmentId");
    std::vector<std::string> identity{task, str(capture,"sha256"),
        capture["width"].dump(), capture["height"].dump(), str(capture,"byteSize")};
    auto existing = d.run("SELECT task_id,sha256,width,height,byte_size FROM attachment_identities WHERE id=?", {aid});
    if (!existing.empty())
      require(existing[0] == identity, "Attachment identity cannot change owner or content.");
    else {
      identity.insert(identity.begin(),aid);
      d.run("INSERT INTO attachment_identities(id,task_id,sha256,width,height,byte_size) VALUES(?,?,?,?,?,?)", identity);
    }
  }
  void captureReferences(Db &d, const std::string &task, const J &captures, bool pending = false) {
    require(captures.is_array());
    std::set<std::string> seen;
    for (const auto &c : captures) {
      captureManifest(c);
      auto aid = str(c, "attachmentId");
      require(seen.insert(aid).second, "Capture identities must be unique.");
      captureIdentity(d, task, c);
      auto rows = d.run("SELECT task_id,sha256,width,height,byte_size FROM attachments WHERE id=?", {aid});
      if (rows.empty()) {
        if (!pending) fail("NOT_FOUND", "Attachment does not belong to this task.");
        d.run("INSERT INTO entities(id,kind) VALUES(?,2) ON CONFLICT DO NOTHING", {task});
        d.run("INSERT INTO attachments(id,task_id,sha256,width,height,byte_size) VALUES(?,?,?,?,?,?)", {aid,task,str(c,"sha256"),c["width"].dump(),c["height"].dump(),str(c,"byteSize")});
      } else {
        require(rows[0] == std::vector<std::string>{task,str(c,"sha256"),c["width"].dump(),c["height"].dump(),str(c,"byteSize")}, "Immutable attachment ownership or content cannot change.");
      }
    }
  }
  J insertCaptures(Db &d, const std::string &task, const J &images) {
    require(images.is_array() && !images.empty(), "At least one final image is required.");
    J captures = J::array();
    for (const auto &image : images) {
      fields(image, {"capture", "png"});
      const auto &c = image.at("capture");
      validatePNG(c, decodeBase64(str(image,"png")));
      require(d.one("SELECT id FROM attachment_identities WHERE id=?", {str(c,"attachmentId")}).empty(), "Attachment identity is already in use.");
      captureReferences(d,task,J::array({c}),true);
      d.run("UPDATE attachments SET png_base64=? WHERE id=?", {str(image,"png"),str(c,"attachmentId")});
      captures.push_back(c);
    }
    return captures;
  }
  J taskBody(Db &d, J t) {
    fields(t, {"repositoryId", "title", "description", "prompt", "progress",
               "proposal", "captures", "priority", "type", "labelIds",
               "startDate", "dueDate", "storyPoints", "milestoneId", "cycleId",
               "parentTaskId", "dependsOnTaskIds", "relatedTaskIds",
               "duplicateOfTaskId"});
    id(str(t, "repositoryId"));
    repository(d, str(t, "repositoryId"));
    require(hasText(str(t, "title")), "A title is required.");
    J r = {{"repositoryId", str(t, "repositoryId")},
           {"title", str(t, "title")},
           {"description", str(t, "description")},
           {"progress", enumValue(t, "progress", "PROGRESS_", progresses)},
           {"proposal", enumValue(t, "proposal", "PROPOSAL_", proposals)}};
    if (t.contains("prompt"))
      r["prompt"] = str(t, "prompt");
    if (t.contains("captures")) {
      require(t["captures"].is_array());
      for (const auto &c : t["captures"]) captureManifest(c);
      r["captures"] = t["captures"];
    }
    planningFieldsShape(t, r);
    return r;
  }
  J version(const std::string &s, const std::string &e, int k, J body,
            J parents = J::array(), bool deleted = false) {
    std::sort(parents.begin(), parents.end());
    J v = {{"formatVersion", 3},
           {"entityId", e},
           {"kind", kindName(k)},
           {"versionId", uuid()},
           {"parentVersionIds", parents},
           {"createdAt", timestamp()},
           {"origin", origin(s)},
           {"deleted", deleted}};
    if (!deleted)
      v[bodyKey(k)] = body;
    return v;
  }
  J comparableVersion(J v) {
    v = withoutEmptyCaptures(v);
    v["deleted"] = v.value("deleted", false);
    v["promptRestoration"] = v.value("promptRestoration", false);
    v["parentVersionIds"] = v.value("parentVersionIds", J::array());
    auto &o = v["origin"];
    for (auto key : {"deviceId", "deviceName", "accountId"})
      o[key] = str(o, key);
    if (v.contains("task"))
      v["task"]["description"] = str(v["task"], "description");
    return v;
  }
  void persist(Db &d, const J &v, bool prompt, bool replace,
               bool projection = true) {
    auto e = str(v, "entityId"), vid = str(v, "versionId");
    int k = versionKind(v);
    bool deleted = v.value("deleted", false);
    auto existingKind = d.one("SELECT kind FROM entities WHERE id=?", {e});
    require(existingKind.empty() || existingKind == std::to_string(k),
            "Entity kind cannot change.");
    d.run(
        "INSERT INTO entities(id,kind) VALUES(?,?) ON CONFLICT(id) DO NOTHING",
        {e, std::to_string(k)});
    auto existing =
        d.one("SELECT envelope_json FROM versions WHERE version_id=?", {vid});
    if (!existing.empty()) {
      require(comparableVersion(parse(existing)) == comparableVersion(v),
              "Version identity was reused.");
      return;
    }
    d.run("INSERT INTO versions VALUES(?,?,?,?,?)",
          {vid, e, v.dump(),
           std::to_string(parseTimestamp(str(v, "createdAt"))),
           prompt ? "1" : "0"});
    if (replace)
      for (auto &alias : aliases(d, e))
        d.run("DELETE FROM heads WHERE entity_id=?", {alias});
    d.run("INSERT INTO heads VALUES(?,?)", {e, vid});
    if (projection && !(k == 1 && mapped(d, e) != e))
      d.run(
          "UPDATE entities SET repository_id=?,normalized_url=?,deleted=? "
          "WHERE id=?",
          {k != 1 && !deleted ? mapped(d, str(v[bodyKey(k)], "repositoryId")) : "",
           k == 1 && !deleted ? str(v["repository"], "normalizedUrl") : "",
           deleted ? "1" : "0", e});
  }
  // `at` overrides the upload position (microseconds) of the queued mutation;
  // a version that supersedes a rejected chain keeps the chain's position.
  void outbox(Db &d, const J &v, const std::string &kind, int64_t at = 0) {
    auto rid = uuid();
    J m = {{"meta", {{"requestId", rid}, {"origin", v["origin"]}}},
           {"version", v},
           {"expectedHeadIds", v["parentVersionIds"]},
           {"kind", kind}};
    d.run("INSERT INTO outbox VALUES(?,?,?,1,NULL,?)",
          {rid, str(v, "entityId"), m.dump(),
           std::to_string(at ? at : now())});
  }
  struct Supersession {
    J parents;        // parents of the oldest version in the replaced chain
    int64_t position; // upload position of the oldest replaced version
  };
  // An ordinary edit of an entity whose head chain holds a version the server
  // rejected for a constraint violation replaces that unsynchronized chain: the
  // correction takes the chain's parents (the last accepted heads, or none for
  // a creation) and its upload position, because PushChanges requires every
  // parent to exist on the server. The rejected immutable versions stay in
  // local history; their outbox rows are dropped so they are never uploaded or
  // retried. Call only after every validation of the edit has passed.
  std::optional<Supersession> supersedeBlocked(Db &d, const J &head) {
    struct Row { std::string request; bool rejected; int64_t at; };
    std::map<std::string, Row> pending;
    for (auto &r : d.run("SELECT request_id,request_json,state,COALESCE(error_reason,''),"
                         "created_at_us FROM outbox WHERE entity_id=?",
                         {str(head, "entityId")}))
      pending[str(parse(r[1])["version"], "versionId")] = {
          r[0],
          r[2] == "2" && r[3].rfind("CONSTRAINT_VIOLATION:", 0) == 0,
          std::stoll(r[4])};
    // Walk the single-parent chain of unsynchronized versions from the head.
    std::vector<J> chain;
    J current = head;
    while (pending.count(str(current, "versionId"))) {
      chain.push_back(current);
      auto parents = current.value("parentVersionIds", J::array());
      if (parents.size() != 1)
        break;
      auto raw = d.one("SELECT envelope_json FROM versions WHERE version_id=?",
                       {parents[0].get<std::string>()});
      if (raw.empty())
        break;
      current = parse(raw);
    }
    const bool rejected = std::any_of(chain.begin(), chain.end(), [&](const J &v) {
      return pending.at(str(v, "versionId")).rejected;
    });
    if (!rejected)
      return std::nullopt;
    Supersession s{chain.back().value("parentVersionIds", J::array()), INT64_MAX};
    for (auto &v : chain) {
      auto &row = pending.at(str(v, "versionId"));
      s.position = std::min(s.position, row.at);
      d.run("DELETE FROM outbox WHERE request_id=?", {row.request});
    }
    return s;
  }
  static std::string mutationKind(const J &parents) {
    return parents.empty()      ? "MUTATION_KIND_CREATE"
           : parents.size() > 1 ? "MUTATION_KIND_RESOLVE"
                                : "MUTATION_KIND_SAVE";
  }
  // Every other entity a version refers to: its repository and, for a task, its
  // planning items and related tasks. Used to order dependent uploads.
  static std::vector<std::string> referencedIds(const J &version) {
    std::vector<std::string> out;
    for (auto field : {"task", "urlConnection", "label", "milestone", "cycle"}) {
      if (!version.contains(field) || !version[field].is_object())
        continue;
      const J &body = version[field];
      for (auto key : {"repositoryId", "milestoneId", "cycleId", "parentTaskId", "duplicateOfTaskId"})
        if (body.contains(key) && body[key].is_string())
          out.push_back(body[key].get<std::string>());
      for (auto key : {"labelIds", "dependsOnTaskIds", "relatedTaskIds"})
        if (body.contains(key) && body[key].is_array())
          for (auto &x : body[key])
            if (x.is_string())
              out.push_back(x.get<std::string>());
    }
    return out;
  }
  struct PendingRow {
    std::string entity;
    J mutation;
    int state;
    // Targets of parent, dependency and duplicate-of edges this version removes
    // relative to its parents, and whether it adds any such edge.
    std::set<std::string> dropped;
    bool adds = false;
  };
  static std::set<std::string> edgeTargets(const J &task) {
    std::set<std::string> out;
    for (auto key : {"parentTaskId", "duplicateOfTaskId"})
      if (task.contains(key) && task[key].is_string())
        out.insert(task[key].get<std::string>());
    if (task.contains("dependsOnTaskIds") && task["dependsOnTaskIds"].is_array())
      for (auto &x : task["dependsOnTaskIds"])
        if (x.is_string())
          out.insert(x.get<std::string>());
    return out;
  }
  // Records the graph edges a pending task version removes and adds by comparing
  // it with its parent versions.
  void edgeChanges(Db &d, PendingRow &row) {
    const J &v = row.mutation["version"];
    if (v.value("deleted", false) || !v.contains("task"))
      return;
    const auto now = edgeTargets(v["task"]);
    std::set<std::string> before;
    for (auto &parent : v.value("parentVersionIds", J::array())) {
      auto raw = d.one("SELECT envelope_json FROM versions WHERE version_id=?",
                       {parent.get<std::string>()});
      if (raw.empty())
        continue;
      auto stored = parse(raw);
      if (stored.contains("task"))
        for (auto &x : edgeTargets(stored["task"]))
          before.insert(x);
    }
    for (auto &x : before)
      if (!now.count(x))
        row.dropped.insert(x);
    for (auto &x : now)
      row.adds = row.adds || !before.count(x);
  }
  // Orders outbox rows so that every row follows the earlier row of its own
  // entity and the pending creation of every entity it references, keeping the
  // stored order otherwise. A corrected version that took an older upload
  // position therefore still uploads after a label created after it. Reference
  // cycles are cut at the edge that closes them.
  static std::vector<size_t> dependencyOrder(const std::vector<PendingRow> &rows) {
    std::map<std::string, size_t> creations, last;
    for (size_t i = 0; i < rows.size(); ++i)
      if (rows[i].mutation.value("kind", "") == "MUTATION_KIND_CREATE")
        creations.emplace(rows[i].entity, i);
    std::vector<std::vector<size_t>> deps(rows.size());
    std::map<std::string, std::vector<size_t>> byEntity;
    for (size_t i = 0; i < rows.size(); ++i)
      byEntity[rows[i].entity].push_back(i);
    // The server validates each item against the state it has accepted so far.
    // A version that adds a graph edge must therefore follow a version of
    // another task that removes an edge pointing at the first task, or the
    // old edge still closes a cycle (B depended on A; A now depends on B and B
    // no longer depends on A).
    for (size_t removal = 0; removal < rows.size(); ++removal)
      for (auto &target : rows[removal].dropped) {
        auto addition = byEntity.find(target);
        if (addition == byEntity.end())
          continue;
        for (auto index : addition->second)
          if (index != removal && rows[index].adds && rows[index].entity != rows[removal].entity)
            deps[index].push_back(removal);
      }
    for (size_t i = 0; i < rows.size(); ++i) {
      auto previous = last.find(rows[i].entity);
      if (previous != last.end())
        deps[i].push_back(previous->second);
      last[rows[i].entity] = i;
      for (auto &ref : referencedIds(rows[i].mutation["version"])) {
        auto created = creations.find(ref);
        if (created != creations.end() && created->second != i)
          deps[i].push_back(created->second);
      }
    }
    std::vector<int> state(rows.size(), 0); // 1 = on the current path, 2 = placed
    std::vector<size_t> order;
    for (size_t start = 0; start < rows.size(); ++start) {
      if (state[start])
        continue;
      state[start] = 1;
      std::vector<std::pair<size_t, size_t>> stack{{start, 0}};
      while (!stack.empty()) {
        auto &[node, next] = stack.back();
        if (next < deps[node].size()) {
          auto dep = deps[node][next++];
          if (!state[dep]) {
            state[dep] = 1;
            stack.emplace_back(dep, 0);
          }
        } else {
          state[node] = 2;
          order.push_back(node);
          stack.pop_back();
        }
      }
    }
    return order;
  }
  // The next upload batch in causal order. A pending version is held back
  // while an earlier version of the same entity is blocked, because the server
  // never accepted its parent, and while it references an entity whose
  // creation is blocked or itself held back. Both resume after the blocked
  // item is retried or superseded by a correction.
  J uploadBatch(Db &d) {
    std::vector<PendingRow> rows;
    for (auto &r : d.run("SELECT entity_id,request_json,state FROM outbox ORDER BY "
                         "created_at_us,rowid"))
      rows.push_back({r[0], parse(r[1]), std::stoi(r[2]), {}, false});
    for (auto &row : rows)
      edgeChanges(d, row);
    std::set<std::string> blocked, blockedCreations;
    J out = J::array();
    for (auto index : dependencyOrder(rows)) {
      auto &row = rows[index];
      if (row.state != 1 && row.state != 2)
        continue;
      bool held = row.state == 2 || blocked.count(row.entity);
      for (auto &ref : referencedIds(row.mutation["version"]))
        held = held || blockedCreations.count(ref);
      if (!held) {
        out.push_back(row.mutation);
        continue;
      }
      if (row.mutation.value("kind", "") == "MUTATION_KIND_CREATE")
        blockedCreations.insert(row.entity);
      blocked.insert(row.entity);
    }
    return out;
  }
  void cancelNotification(Db &d, const std::string &e) {
    d.run("UPDATE notification_receipts SET state=3 WHERE proposal_id=? AND "
          "state=1",
          {e});
  }
  void tombstone(Db &d, J v, bool queue) {
    v.erase("promptRestoration");
    auto e = str(v, "entityId");
    auto h = heads(d, e);
    if (!h.empty() && h[0].value("deleted", false))
      return;
    bool repo = v["kind"] == "ENTITY_KIND_REPOSITORY";
    if (repo) {
      for (auto &r : d.run("SELECT id FROM entities WHERE kind IN (2,3,4,5,6) AND deleted=0 "
                           "AND repository_id=?",
                           {e})) {
        auto old = heads(d, r[0]);
        J child = v;
        child["entityId"] = r[0];
        child["versionId"] = uuid();
        child["kind"] = old[0]["kind"];
        child["parentVersionIds"] = J::array();
        for (auto &a : old)
          child["parentVersionIds"].push_back(a["versionId"]);
        tombstone(d, child, queue);
      }
      for (auto &r : d.run("SELECT kind,id,body_json FROM remote_objects WHERE "
                           "deleted=0")) {
        auto b = parse(r[2]);
        if (mapped(d, str(b, "repositoryId")) == mapped(d, e))
          remoteDelete(d, std::stoi(r[0]), r[1]);
      }
    }
    auto related = aliases(d, e);
    for (auto &alias : related) {
      d.run("DELETE FROM attachments WHERE task_id=?", {alias});
      d.run("DELETE FROM heads WHERE entity_id=?", {alias});
      d.run("DELETE FROM versions WHERE entity_id=?", {alias});
      d.run("DELETE FROM outbox WHERE entity_id=?", {alias});
      d.run("UPDATE entities SET "
            "deleted=1,repository_id=NULL,normalized_url=NULL WHERE id=?",
            {alias});
    }
    persist(d, v, false, true);
    for (auto &alias : related)
      d.run("UPDATE receipts SET result_json=? WHERE entity_id=?",
            {J{{"result", result(d, e)}}.dump(), alias});
    if (repo) {
      for (auto &row :
           d.run("SELECT method,request_id,result_json FROM receipts")) {
        auto saved = parse(row[2]);
        if (saved.contains("repository") &&
            saved["repository"].contains("version") &&
            mapped(d, str(saved["repository"]["version"], "entityId")) ==
                mapped(d, e)) {
          saved["repository"] = result(d, e);
          d.run("UPDATE receipts SET result_json=? WHERE method=? AND "
                "request_id=?",
                {saved.dump(), row[0], row[1]});
        }
      }
    }
    d.run("DELETE FROM store_meta WHERE key=?", {"notification_claim:" + e});
    cancelNotification(d, e);
    if (queue)
      outbox(d, v, "MUTATION_KIND_DELETE");
    changed(d, e);
  }
  J prefs(Db &d) {
    return J{{"enabled", meta(d, "notification_enabled", "0") == "1"},
             {"content", "NOTIFICATION_CONTENT_" +
                             meta(d, "notification_content", "DETAIL")},
             {"permission", meta(d, "notification_permission",
                                 "NOTIFICATION_PERMISSION_NOT_REQUESTED")}};
  }
  J status(Db &d, const std::string &s) {
    J errors = J::array();
    for (auto &r : d.run("SELECT request_id,entity_id,error_reason FROM outbox "
                         "WHERE state=2 ORDER BY created_at_us"))
      errors.push_back(
          {{"requestId", r[0]}, {"entityId", r[1]}, {"reason", r[2]}});
    J r = {{"spaceId", s},
           {"deviceId", device()},
           {"syncEnabled", enabled(s)},
           {"accountId",
            registry->one("SELECT account_id FROM spaces WHERE id=?", {s})},
           {"reconnectRequired", meta(d, "reconnect_required", "0") == "1"},
           {"connected", enabled(s) && meta(d, "connected") == "1"},
           {"pendingItemCount", d.one("SELECT (SELECT count(*) FROM outbox)+(SELECT count(*) FROM attachments WHERE synchronized=0)")},
           {"itemErrors", errors},
           {"notificationPreferences", prefs(d)}};
    auto syncError = meta(d, "sync_error_reason");
    if (!syncError.empty())
      r["syncErrorReason"] = syncError;
    auto last = meta(d, "last_sync_at");
    if (!last.empty())
      r["lastSyncAt"] = last;
    return r;
  }
  // Replaces the planning fields of `target` (a CreateTask request or a Task
  // body) with their normalized form and normalizes the relationship edits that
  // ride on `request`. Equal content therefore yields equal fingerprints.
  static void normalizePlanning(J &target, J &request) {
    J shaped = J::object();
    planningFieldsShape(target, shaped);
    for (auto key : planningTaskFields)
      target.erase(key);
    for (auto it = shaped.begin(); it != shaped.end(); ++it)
      target[it.key()] = it.value();
    if (request.contains("relationshipEdits")) {
      auto edits = normalizedEdits(request["relationshipEdits"]);
      if (edits.empty())
        request.erase("relationshipEdits");
      else
        request["relationshipEdits"] = edits;
    }
  }
  J normalizeRequest(const std::string &m, J q, const std::string &s) {
    if (q.contains("meta")) {
      fields(q["meta"], {"requestId", "origin"});
      id(str(q["meta"], "requestId"));
      q["meta"] = {{"requestId", str(q["meta"], "requestId")},
                   {"origin", origin(s)}};
    }
    if (m == "CreateRepository" || m == "SaveRepository") {
      fields(q, {"meta", "id", "url", "displayName", "expectedVersionId"});
      q["url"] = normalizeURL(str(q, "url"));
      if (q.contains("displayName"))
        q["displayName"] = str(q, "displayName");
    }
    if (m == "CreateTask") {
      fields(q, {"meta", "id", "repository", "title", "description", "prompt",
                 "progress", "proposal", "priority", "type", "labelIds",
                 "startDate", "dueDate", "storyPoints", "milestoneId",
                 "cycleId", "parentTaskId", "dependsOnTaskIds",
                 "relatedTaskIds", "duplicateOfTaskId", "relationshipEdits"});
      normalizePlanning(q, q);
      fields(q.value("repository", J::object()), {"id", "url"});
      require(q["repository"].size() == 1);
      if (q["repository"].contains("url"))
        q["repository"]["url"] = normalizeURL(str(q["repository"], "url"));
      else
        id(str(q["repository"], "id"));
      q["description"] = str(q, "description");
      q["progress"] = enumValue(q, "progress", "PROGRESS_", progresses, 1);
      q["proposal"] = enumValue(q, "proposal", "PROPOSAL_", proposals, 1);
      require(q["proposal"] == "PROPOSAL_REGULAR" ||
              q["proposal"] == "PROPOSAL_PENDING");
      require(hasText(str(q, "title")));
      if (q.contains("prompt"))
        q["prompt"] = str(q, "prompt");
    }
    if (m == "CreateCaptureTask") {
      fields(q, {"meta","id","repositoryId","title","description","images"});
      id(str(q,"repositoryId"));
      require(hasText(str(q,"title")), "A title is required.");
      q["description"] = str(q,"description");
    }
    if (m == "CreateURLConnection" || m == "SaveURLConnection") {
      fields(q, {"meta","id","connection","expectedVersionId"});
      fields(q.at("connection"), {"prefix","repositoryId"});
      q["connection"]["prefix"] = normalizePrefix(str(q["connection"],"prefix"));
      id(str(q["connection"],"repositoryId"));
    }
    if (m == "DeleteURLConnection") fields(q, {"meta","id"});
    if (m == "CreatePlanningItem" || m == "SavePlanningItem") {
      if (m == "CreatePlanningItem") fields(q, {"meta", "id", "item"});
      else fields(q, {"meta", "id", "expectedVersionId", "item"});
      require(q.contains("item") && q["item"].is_object() && q["item"].size() == 1,
              "Exactly one of label, milestone or cycle is required.");
      auto it = q["item"].begin();
      int kind = it.key() == "label" ? int(kLabel) : it.key() == "milestone" ? int(kMilestone)
                 : it.key() == "cycle" ? int(kCycle) : 0;
      require(kind != 0, "Exactly one of label, milestone or cycle is required.");
      q["item"] = {{it.key(), planningShape(kind, it.value())}};
    }
    if (m == "DeletePlanningItem") fields(q, {"meta", "id"});
    if (m == "SetProgress") {
      fields(q, {"meta", "id", "expectedVersionId", "progress"});
      q["progress"] = enumValue(q, "progress", "PROGRESS_", progresses);
    }
    if (m == "SetProposal") {
      fields(q, {"meta", "id", "expectedVersionId", "proposal"});
      q["proposal"] = enumValue(q, "proposal", "PROPOSAL_", proposals);
    }
    if (m == "SaveTask") {
      fields(q, {"meta", "id", "expectedVersionId", "task", "relationshipEdits"});
      auto &t = q["task"];
      normalizePlanning(t, q);
      t["description"] = str(t, "description");
      t["progress"] = enumValue(t, "progress", "PROGRESS_", progresses);
      t["proposal"] = enumValue(t, "proposal", "PROPOSAL_", proposals);
    }
    if (m == "RestorePrompt")
      fields(q, {"meta", "id", "expectedVersionId", "sourceVersionId"});
    if (m == "DeleteTask" || m == "DeleteRepository")
      fields(q, {"meta", "id"});
    if (m == "ResolveConflict") {
      fields(q, {"meta", "id", "expectedHeadIds", "resolved"});
      require(q.value("expectedHeadIds", J::array()).is_array());
      std::vector<std::string> hs =
          q.value("expectedHeadIds", std::vector<std::string>{});
      for (auto &a : hs)
        id(a);
      std::sort(hs.begin(), hs.end());
      hs.erase(std::unique(hs.begin(), hs.end()), hs.end());
      q["expectedHeadIds"] = hs;
    }
    return q;
  }
  // ---- Task planning: references, relationship graph and cleanup ----------
  struct TaskNode {
    J head;              // first accepted head (the entity result's version)
    J orig, cur;         // task body before and after this operation
    std::vector<J> extra; // task bodies of other conflict heads
    bool conflicted = false;
    std::string repo;    // canonical repository
  };
  using NodeMap = std::map<std::string, std::unique_ptr<TaskNode>>;
  // Lazily loads a live task; nullptr means missing, deleted or not a task.
  TaskNode *taskNode(Db &d, NodeMap &nodes, const std::string &taskId) {
    auto it = nodes.find(taskId);
    if (it == nodes.end()) {
      std::unique_ptr<TaskNode> n;
      auto h = heads(d, taskId);
      if (!h.empty() && !h[0].value("deleted", false) &&
          h[0]["kind"] == "ENTITY_KIND_TASK") {
        n = std::make_unique<TaskNode>();
        n->head = h[0];
        n->orig = n->cur = h[0]["task"];
        n->conflicted = h.size() > 1;
        for (size_t i = 1; i < h.size(); ++i)
          n->extra.push_back(h[i]["task"]);
        n->repo = mapped(d, str(n->orig, "repositoryId"));
      }
      it = nodes.emplace(taskId, std::move(n)).first;
    }
    return it->second.get();
  }
  // Every reference to `target` is removed from the task body. Reports change.
  static bool removeTaskReference(J &t, const std::string &target) {
    bool changed = false;
    for (auto key : {"parentTaskId", "duplicateOfTaskId"})
      if (t.contains(key) && t[key] == target) {
        t.erase(key);
        changed = true;
      }
    for (auto key : {"dependsOnTaskIds", "relatedTaskIds"}) {
      if (!t.contains(key))
        continue;
      std::vector<std::string> list = t[key];
      auto end = std::remove(list.begin(), list.end(), target);
      if (end == list.end())
        continue;
      list.erase(end, list.end());
      changed = true;
      if (list.empty())
        t.erase(key);
      else
        t[key] = list;
    }
    return changed;
  }
  static bool removePlanningReference(J &t, const std::string &target) {
    bool changed = false;
    for (auto key : {"milestoneId", "cycleId"})
      if (t.contains(key) && t[key] == target) {
        t.erase(key);
        changed = true;
      }
    if (t.contains("labelIds")) {
      std::vector<std::string> list = t["labelIds"];
      auto end = std::remove(list.begin(), list.end(), target);
      if (end != list.end()) {
        list.erase(end, list.end());
        changed = true;
        if (list.empty())
          t.erase("labelIds");
        else
          t["labelIds"] = list;
      }
    }
    return changed;
  }
  // Live tasks whose accepted heads textually mention `target`. A pre-filter
  // only: callers confirm with the typed reference check.
  std::vector<std::string> mentioning(Db &d, const std::string &target) {
    std::vector<std::string> ids;
    for (auto &r : d.run("SELECT DISTINCT e.id FROM entities e JOIN heads h ON "
                         "h.entity_id=e.id JOIN versions v ON "
                         "v.version_id=h.version_id WHERE e.kind=2 AND "
                         "e.deleted=0 AND instr(v.envelope_json,?)>0 ORDER BY e.id",
                         {target}))
      ids.push_back(r[0]);
    return ids;
  }
  // Applies `detach` to every task that references `target`, updating node
  // bodies. A conflicted task cannot receive a cleanup version.
  void detachReferences(Db &d, NodeMap &nodes, const std::string &target,
                        const std::string &skip,
                        const std::function<bool(J &)> &detach) {
    for (auto &taskId : mentioning(d, target)) {
      if (taskId == skip)
        continue;
      auto *n = taskNode(d, nodes, taskId);
      if (!n)
        continue;
      bool any = detach(n->cur);
      for (auto body : n->extra)
        any = detach(body) || any;
      if (any && n->conflicted)
        fail("CONFLICT_READ_ONLY", "Resolve the conflict before editing.");
    }
  }
  // Live planning item of the wanted kind in the task's repository.
  void planningReference(Db &d, const std::string &itemId, int kind,
                         const std::string &repo) {
    auto h = heads(d, itemId);
    if (h.empty() || h[0].value("deleted", false) ||
        h[0]["kind"] != kindName(kind))
      violation("REFERENCE_NOT_FOUND", itemId,
                "A referenced label, milestone or cycle does not exist.");
    if (mapped(d, str(h[0].at(bodyKey(kind)), "repositoryId")) != repo)
      violation("CROSS_REPOSITORY", itemId,
                "A referenced item belongs to another repository.");
  }
  static std::vector<std::string> list(const J &t, const char *key) {
    std::vector<std::string> out;
    if (t.contains(key))
      out = t[key].get<std::vector<std::string>>();
    return out;
  }
  static void putSet(J &t, const char *key, std::vector<std::string> set) {
    set = sortedUnique(std::move(set));
    if (set.empty())
      t.erase(key);
    else
      t[key] = set;
  }
  // Does a cycle through `start` exist along `key` edges? Conflicted tasks
  // contribute the union of every head's references.
  bool cyclesThrough(Db &d, NodeMap &nodes, const std::string &start,
                     const char *key) {
    auto successors = [&](const std::string &taskId) {
      std::vector<std::string> out;
      auto *n = taskNode(d, nodes, taskId);
      if (!n)
        return out;
      auto add = [&](const J &body) {
        if (!body.contains(key))
          return;
        if (body[key].is_string())
          out.push_back(body[key].get<std::string>());
        else
          for (auto &v : body[key])
            out.push_back(v.get<std::string>());
      };
      add(n->cur);
      for (auto &body : n->extra)
        add(body);
      return out;
    };
    std::set<std::string> seen;
    std::vector<std::string> todo = successors(start);
    while (!todo.empty()) {
      auto x = todo.back();
      todo.pop_back();
      if (x == start)
        return true;
      if (!seen.insert(x).second)
        continue;
      for (auto &next : successors(x))
        todo.push_back(next);
    }
    return false;
  }
  std::vector<std::string> changedTasks(NodeMap &nodes, const std::string &skip,
                                        bool enforce) {
    std::vector<std::string> ids;
    for (auto &entry : nodes)
      if (entry.second && entry.first != skip &&
          entry.second->cur != entry.second->orig) {
        if (enforce && entry.second->conflicted)
          fail("CONFLICT_READ_ONLY", "Resolve the conflict before editing.");
        ids.push_back(entry.first);
      }
    return ids; // std::map order is ascending entity ID
  }
  // Domain rules for creating or saving task `taskId` with normalized body
  // `body`. `previousRepo` is empty for a creation. Validates references and
  // the relationship graph against current heads, applies symmetric related
  // links and relationship edits, and clears references on a move. Returns the
  // IDs of the other tasks that need new versions. With `others` false (a
  // stale save preserved only as a conflict alternative) nothing else changes:
  // relationship edits and symmetric related links are discarded and the
  // alternative is checked against the graph that stays committed, the other
  // tasks unchanged and this task with its existing heads plus the alternative.
  std::vector<std::string> planTaskWrite(Db &d, NodeMap &nodes,
                                         const std::string &taskId, J &body,
                                         const J &requestedEdits,
                                         const std::string &previousRepo,
                                         bool others) {
    static const J noEdits = J::object();
    const J &edits = others ? requestedEdits : noEdits;
    const auto repo = mapped(d, str(body, "repositoryId"));
    auto *previous = previousRepo.empty() ? nullptr : taskNode(d, nodes, taskId);
    if (!previousRepo.empty() && previousRepo != repo) {
      // Move: discard the references sent with it and detach incoming ones.
      for (auto key : {"labelIds", "milestoneId", "cycleId", "parentTaskId",
                       "dependsOnTaskIds", "relatedTaskIds", "duplicateOfTaskId"})
        body.erase(key);
      auto &slot = nodes[taskId];
      if (!slot) {
        slot = std::make_unique<TaskNode>();
        slot->orig = J();
      }
      slot->cur = body;
      if (others)
        detachReferences(d, nodes, taskId, taskId,
                         [&](J &t) { return removeTaskReference(t, taskId); });
      return others ? changedTasks(nodes, taskId, true)
                    : std::vector<std::string>{};
    }
    for (auto &label : list(body, "labelIds"))
      planningReference(d, label, kLabel, repo);
    if (body.contains("milestoneId"))
      planningReference(d, str(body, "milestoneId"), kMilestone, repo);
    if (body.contains("cycleId"))
      planningReference(d, str(body, "cycleId"), kCycle, repo);
    std::vector<std::string> links;
    for (auto key : {"dependsOnTaskIds", "relatedTaskIds"})
      for (auto &x : list(body, key))
        links.push_back(x);
    for (auto key : {"parentTaskId", "duplicateOfTaskId"})
      if (body.contains(key))
        links.push_back(str(body, key));
    for (auto key : {"addSubtaskIds", "addBlockedTaskIds"})
      for (auto &x : list(edits, key))
        links.push_back(x);
    for (auto &x : links)
      if (x == taskId)
        violation("SELF_LINK", taskId, "A task cannot be linked to itself.");
    for (auto &x : links)
      if (!taskNode(d, nodes, x))
        violation("REFERENCE_NOT_FOUND", x, "A linked task does not exist.");
    for (auto &x : links)
      if (taskNode(d, nodes, x)->repo != repo)
        violation("CROSS_REPOSITORY", x, "A linked task belongs to another repository.");
    // The primary node carries the new body from here on.
    auto &primary = nodes[taskId];
    if (!primary) {
      primary = std::make_unique<TaskNode>();
      primary->orig = J();
    }
    std::vector<std::string> related = list(body, "relatedTaskIds");
    if (previous)
      for (auto &x : list(previous->orig, "relatedTaskIds"))
        related.push_back(x);
    if (others) {
      primary->cur = body;
      primary->extra.clear(); // This write supersedes every conflicting head.
      primary->conflicted = false;
    } else if (primary->orig.is_null()) {
      primary->cur = body;
    } else {
      // The alternative is stored next to the existing heads, so cycle checks
      // see the union of their references.
      primary->extra.push_back(body);
    }
    // Edits to another task apply to every head of a conflicted task; changing
    // any of them fails the whole operation instead of resolving the conflict.
    auto touch = [&](TaskNode *n, const std::function<void(J &)> &edit) {
      bool any = false;
      J before = n->cur;
      edit(n->cur);
      any = n->cur != before;
      for (auto body : n->extra) {
        J copy = body;
        edit(copy);
        any = any || copy != body;
      }
      if (any && n->conflicted)
        fail("CONFLICT_READ_ONLY", "Resolve the conflict before editing.");
    };
    const auto mine = list(body, "relatedTaskIds");
    for (auto &x : sortedUnique(related)) {
      auto *other = taskNode(d, nodes, x);
      if (!other || !others)
        continue;
      const bool linked = std::find(mine.begin(), mine.end(), x) != mine.end();
      touch(other, [&](J &t) {
        auto set = list(t, "relatedTaskIds");
        set.erase(std::remove(set.begin(), set.end(), taskId), set.end());
        if (linked)
          set.push_back(taskId);
        putSet(t, "relatedTaskIds", set);
      });
    }
    for (auto &x : list(edits, "addSubtaskIds"))
      touch(taskNode(d, nodes, x), [&](J &t) { t["parentTaskId"] = taskId; });
    for (auto &x : list(edits, "removeSubtaskIds"))
      if (auto *n = taskNode(d, nodes, x))
        touch(n, [&](J &t) {
          if (t.contains("parentTaskId") && t["parentTaskId"] == taskId)
            t.erase("parentTaskId");
        });
    for (auto &x : list(edits, "addBlockedTaskIds"))
      touch(taskNode(d, nodes, x), [&](J &t) {
        auto set = list(t, "dependsOnTaskIds");
        set.push_back(taskId);
        putSet(t, "dependsOnTaskIds", set);
      });
    for (auto &x : list(edits, "removeBlockedTaskIds"))
      if (auto *n = taskNode(d, nodes, x))
        touch(n, [&](J &t) {
          auto set = list(t, "dependsOnTaskIds");
          set.erase(std::remove(set.begin(), set.end(), taskId), set.end());
          putSet(t, "dependsOnTaskIds", set);
        });
    // A new cycle must pass through a task this write changed.
    std::vector<std::string> touched{taskId};
    for (auto &x : changedTasks(nodes, taskId, false))
      touched.push_back(x);
    for (auto &entry : {std::make_pair("parentTaskId", "PARENT_CYCLE"),
                        std::make_pair("dependsOnTaskIds", "DEPENDENCY_CYCLE"),
                        std::make_pair("duplicateOfTaskId", "DUPLICATE_CYCLE")})
      for (auto &x : touched)
        if (cyclesThrough(d, nodes, x, entry.first))
          violation(entry.second, taskId, "The relationship forms a cycle.");
    return others ? changedTasks(nodes, taskId, true)
                  : std::vector<std::string>{};
  }
  // Writes one cleanup or relationship version for another task, in the
  // caller's transaction and with the acting origin.
  void writeOtherTask(Db &d, const std::string &s, const std::string &taskId,
                      TaskNode &node) {
    auto v = version(s, taskId, kTask, node.cur, J::array({node.head["versionId"]}));
    persist(d, v, false, true);
    outbox(d, v, "MUTATION_KIND_SAVE");
    changed(d, taskId);
  }
  // Duplicate-name, repository and immutability rules of a planning item.
  void planningWrite(Db &d, const std::string &itemId, int kind, const J &body,
                     const std::vector<J> &existing) {
    const auto repo = mapped(d, str(body, "repositoryId"));
    repository(d, repo);
    for (auto &h : existing)
      require(h["kind"] == kindName(kind) &&
                  mapped(d, str(h.at(bodyKey(kind)), "repositoryId")) == repo,
              "An item's kind and repository cannot change.");
    const auto name = lowerSimple(str(body, "name"));
    for (auto &r : d.run("SELECT id FROM entities WHERE kind=? AND deleted=0 AND "
                         "repository_id=? ORDER BY id",
                         {std::to_string(kind), repo})) {
      if (r[0] == itemId)
        continue;
      for (auto &h : heads(d, r[0]))
        if (!h.value("deleted", false) && h.contains(bodyKey(kind)) &&
            lowerSimple(trimmed(str(h[bodyKey(kind)], "name"))) == name)
          violation("DUPLICATE_NAME", itemId,
                    "A " + std::string(bodyKey(kind)) +
                        " with this name already exists in the repository.");
    }
  }
  J mutate(Db &d, const std::string &s, const std::string &m, J q) {
    q = normalizeRequest(m, q, s);
    require(q.contains("meta"));
    auto rid = str(q["meta"], "requestId"), e = str(q, "id");
    id(e);
    auto fingerprint = hash(withoutEmptyCaptures(q).dump());
    auto receipt = d.run("SELECT fingerprint,result_json,entity_id FROM "
                         "receipts WHERE method=? AND request_id=?",
                         {m, rid});
    if (!receipt.empty()) {
      if (receipt[0][0] != fingerprint)
        fail("REQUEST_ID_REUSED", "Request ID was used for different content.");
      auto r = parse(receipt[0][1]);
      // Rust receipts historically store EntityResult directly. Native receipts
      // store the response wrapper; preserve both durable formats on replay.
      if (r.contains("result") && r["result"].is_string())
        r = J{{"result", r}};
      // Affected tasks replay from their stored versions, so a receipt never
      // retains content of a task that was deleted after the original call. A
      // version erased by that deletion is skipped.
      std::vector<std::string> affectedIds;
      if (r.contains("affectedVersionIds"))
        affectedIds = r["affectedVersionIds"].get<std::vector<std::string>>();
      for (auto &embedded : r.value("affected", J::array()))
        if (embedded.contains("version"))
          affectedIds.push_back(str(embedded["version"], "versionId"));
      r.erase("affectedVersionIds");
      r.erase("affected");
      if (!affectedIds.empty()) {
        r["affected"] = J::array();
        for (auto &versionId : affectedIds) {
          auto raw = d.one("SELECT envelope_json FROM versions WHERE version_id=?", {versionId});
          if (!raw.empty())
            r["affected"].push_back({{"result", "RESULT_KIND_REPLAYED"}, {"version", parse(raw)}, {"alternatives", J::array()}});
        }
        if (r["affected"].empty())
          r.erase("affected");
      }
      auto kind = r["result"].value("result", "");
      if (kind == "RESULT_KIND_CONFLICT") {
        J ids = J::array();
        for (const auto &head : heads(d, e)) ids.push_back(head["versionId"]);
        r["_error"] = {{"reason", "ERROR_REASON_STALE_VERSION"}, {"currentHeadIds", ids}};
      } else if (kind != "RESULT_KIND_DELETED")
        r["result"]["result"] = "RESULT_KIND_REPLAYED";
      return r;
    }
    bool create = m == "CreateTask" || m == "CreateRepository" || m == "CreateCaptureTask" || m == "CreateURLConnection" || m == "CreatePlanningItem",
         del = m == "DeleteTask" || m == "DeleteRepository" || m == "DeleteURLConnection" || m == "DeletePlanningItem",
         resolve = m == "ResolveConflict",
         planning = m.find("PlanningItem") != std::string::npos;
    // Planning methods learn their kind from the item (create) or the stored head.
    int itemKind = 0;
    if (planning && m != "DeletePlanningItem") {
      auto key = q["item"].begin().key();
      itemKind = key == "label" ? int(kLabel) : key == "milestone" ? int(kMilestone) : int(kCycle);
    }
    int k = planning ? (create ? itemKind : 0)
            : (m.find("Repository") != std::string::npos) ? 1 : m.find("URLConnection") != std::string::npos ? 3 : 2;
    auto hs = heads(d, e);
    if (!hs.empty() && hs[0].value("deleted", false)) {
      J r = {{"result", result(d, e)}};
      d.run("INSERT INTO receipts VALUES(?,?,?,?,?)",
            {m, rid, fingerprint, e, r.dump()});
      return r;
    }
    if (create)
      require(hs.empty(), "Entity ID is already used.");
    else if (hs.empty())
      fail("NOT_FOUND", "Entity not found.");
    if (!create) {
      int actualKind = versionKind(hs[0]);
      require(resolve || (planning ? isPlanningKind(actualKind) : actualKind == k), "Method does not match entity kind.");
      require(!planning || del || itemKind == actualKind, "An item's kind cannot change.");
      k = actualKind;
    }
    J body = J::object(), parents = J::array(), repoResult;
    bool stale = false, prompt = false;
    NodeMap nodes;
    std::vector<std::string> otherIds;
    if (del) {
      for (auto &h : hs)
        parents.push_back(h["versionId"]);
      // Remove assignments and incoming relationships from other tasks in this
      // same transaction; a failure rolls the whole operation back.
      if (k == kTask) {
        detachReferences(d, nodes, e, e, [&](J &t) { return removeTaskReference(t, e); });
        otherIds = changedTasks(nodes, e, true);
      } else if (isPlanningKind(k)) {
        detachReferences(d, nodes, e, "", [&](J &t) { return removePlanningReference(t, e); });
        otherIds = changedTasks(nodes, e, true);
      }
      for (auto &other : otherIds)
        writeOtherTask(d, s, other, *nodes[other]);
      auto v = version(s, e, k, {}, parents, true);
      tombstone(d, v, true);
    } else {
      if (create) {
        if (planning) {
          body = q["item"].begin().value();
          planningWrite(d, e, k, body, {});
        } else if (k == 3) {
          body = connectionBody(d,e,q.at("connection"));
        } else if (m == "CreateCaptureTask") {
          body = taskBody(d, {{"repositoryId",q["repositoryId"]},{"title",q["title"]},{"description",""},{"prompt",q["description"]},{"progress","PROGRESS_TODO"},{"proposal","PROPOSAL_REGULAR"}});
          body["captures"] = insertCaptures(d,e,q.at("images"));
          prompt = true;
        } else if (k == 1) {
          body = {{"normalizedUrl", q["url"]}};
          if (q.contains("displayName"))
            body["displayName"] = q["displayName"];
          uniqueURL(d, str(body, "normalizedUrl"), e);
        } else {
          std::string repo;
          if (q["repository"].contains("id"))
            repo = str(q["repository"], "id");
          else {
            auto url = str(q["repository"], "url");
            repo = d.one("SELECT id FROM entities WHERE kind=1 AND deleted=0 "
                         "AND normalized_url=?",
                         {url});
            if (repo.empty()) {
              uniqueURL(d, url, "");
              repo = uuid();
              auto rv = version(s, repo, 1, {{"normalizedUrl", url}});
              persist(d, rv, false, true);
              outbox(d, rv, "MUTATION_KIND_CREATE");
              changed(d, repo);
            }
            repoResult = result(d, repo);
          }
          body = {{"repositoryId", repo},
                  {"title", q["title"]},
                  {"description", q["description"]},
                  {"progress", q["progress"]},
                  {"proposal", q["proposal"]}};
          if (q.contains("prompt"))
            body["prompt"] = q["prompt"];
          for (auto key : planningTaskFields)
            if (q.contains(key))
              body[key] = q[key];
          body = taskBody(d, body);
          prompt = body.contains("prompt");
          otherIds = planTaskWrite(d, nodes, e, body, q.value("relationshipEdits", J::object()), "", true);
        }
      } else if (resolve) {
        std::vector<std::string> actual;
        for (auto &h : hs)
          actual.push_back(str(h, "versionId"));
        std::sort(actual.begin(), actual.end());
        if (hs.size() < 2 || q["expectedHeadIds"] != J(actual))
          fail("STALE_RESOLUTION", "Conflict heads changed. Read them again.");
        parents = actual;
        auto resolved = q.value("resolved", J::object());
        require(!resolved.value("deleted", false),
                "Use permanent deletion to delete a conflict.");
        require(!resolved.contains("entityId") ||
                str(resolved, "entityId") == e);
        require(!resolved.contains("kind") ||
                resolved["kind"] == hs[0]["kind"]);
        if (isPlanningKind(k)) {
          body = resolved.at(bodyKey(k));
          body = planningShape(k, body);
          planningWrite(d, e, k, body, hs);
        } else if (k == 3) {
          body = connectionBody(d,e,resolved.at("urlConnection"));
        } else if (k == 1) {
          body = resolved.at("repository");
          fields(body, {"normalizedUrl", "displayName"});
          require(str(body, "normalizedUrl") ==
                  normalizeURL("https://" + str(body, "normalizedUrl")));
          uniqueURL(d, str(body, "normalizedUrl"), e);
        } else {
          body = taskBody(d, resolved.at("task"));
          bool regular = std::any_of(hs.begin(), hs.end(), [](const J &h) {
            return h["task"]["proposal"] == "PROPOSAL_REGULAR";
          });
          require(!regular || body["proposal"] == "PROPOSAL_REGULAR",
                  "A regular task cannot become a proposal.");
          prompt = true;
          // A resolution validates against the union of every head's links and
          // applies move cleanup when it changes the accepted repository.
          otherIds = planTaskWrite(d, nodes, e, body, J::object(),
                                   mapped(d, str(hs[0]["task"], "repositoryId")), true);
        }
      } else {
        if (hs.size() > 1)
          fail("CONFLICT_READ_ONLY", "Resolve the conflict before editing.");
        auto expected = str(q, "expectedVersionId");
        id(expected);
        auto base =
            d.one("SELECT envelope_json FROM versions WHERE version_id=?",
                  {expected});
        if (!base.empty())
          require(mapped(d, str(parse(base), "entityId")) == mapped(d, e),
                  "Version belongs to another entity.");
        if (base.empty())
          fail("STALE_VERSION", "Expected version is not available.");
        J old = parse(base);
        stale = expected != str(hs[0], "versionId");
        parents.push_back(expected);
        if (planning) {
          body = q["item"].begin().value();
          planningWrite(d, e, k, body, hs);
        } else if (k == 3) {
          body = connectionBody(d,e,q.at("connection"));
        } else if (k == 1) {
          body = {{"normalizedUrl", q["url"]}};
          if (q.contains("displayName"))
            body["displayName"] = q["displayName"];
          uniqueURL(d, str(body, "normalizedUrl"), e);
        } else {
          body = old.at("task");
          if (m == "SaveTask") {
            body = taskBody(d, q.at("task"));
            require(body["proposal"] == old["task"]["proposal"],
                    "Use SetProposal to change proposal disposition.");
            otherIds = planTaskWrite(d, nodes, e, body, q.value("relationshipEdits", J::object()),
                                     mapped(d, str(old["task"], "repositoryId")), !stale);
          } else if (m == "SetProgress")
            body["progress"] = q["progress"];
          else if (m == "SetProposal") {
            require(q["proposal"] == "PROPOSAL_ACCEPTED" ||
                    q["proposal"] == "PROPOSAL_REJECTED");
            require(body["proposal"] != "PROPOSAL_REGULAR",
                    "Regular tasks are not proposals.");
            body["proposal"] = q["proposal"];
          } else if (m == "RestorePrompt") {
            auto source = str(q, "sourceVersionId");
            id(source);
            auto saved = d.one("SELECT envelope_json FROM versions WHERE "
                               "version_id=? AND entity_id=?",
                               {source, e});
            if (saved.empty())
              fail("NOT_FOUND", "Prompt version not found.");
            auto src = parse(saved).at("task");
            body.erase("captures");
            if (src.contains("captures")) body["captures"] = src["captures"];
            body.erase("prompt");
            if (src.contains("prompt"))
              body["prompt"] = src["prompt"];
          } else
            fail("INVALID_INPUT", "Unknown mutation.");
          body = taskBody(d, body);
          // SetProgress, SetProposal and RestorePrompt copy the planning fields
          // of the requested base. A stale base becomes a conflict alternative
          // next to the current heads, so validate it like a stale save: no
          // cascade and the union of every head's references.
          if (stale && m != "SaveTask")
            planTaskWrite(d, nodes, e, body, J::object(),
                          mapped(d, str(hs[0]["task"], "repositoryId")), false);
          prompt = body.value("captures",J::array()) != old["task"].value("captures",J::array()) || m == "RestorePrompt" ||
                   body.contains("prompt") != old["task"].contains("prompt") ||
                   (body.contains("prompt") &&
                    body["prompt"] != old["task"].value("prompt", J()));
        }
      }
      if (k == 2) captureReferences(d,e,body.value("captures",J::array()));
      // A correction of a rejected, never-accepted chain replaces that chain.
      std::optional<Supersession> superseded;
      if (!create && !resolve && !stale)
        superseded = supersedeBlocked(d, hs[0]);
      if (superseded)
        parents = superseded->parents;
      auto v = version(s, e, k, body, parents);
      if (m == "RestorePrompt")
        v["promptRestoration"] = true;
      persist(d, v, prompt, !stale, !stale);
      outbox(d, v,
             superseded ? mutationKind(parents)
             : create    ? "MUTATION_KIND_CREATE"
             : resolve ? "MUTATION_KIND_RESOLVE"
                       : "MUTATION_KIND_SAVE",
             superseded ? superseded->position : 0);
      changed(d, e);
      if (k == 2 && body["proposal"] != "PROPOSAL_PENDING")
        cancelNotification(d, e);
      for (auto &other : otherIds)
        writeOtherTask(d, s, other, *nodes[other]);
    }
    J r = {{"result", result(d, e)}};
    J affectedVersionIds = J::array();
    if ((m == "CreateTask" || m == "SaveTask") && !otherIds.empty()) {
      r["affected"] = J::array();
      for (auto &other : otherIds) {
        r["affected"].push_back(result(d, other));
        affectedVersionIds.push_back(r["affected"].back()["version"]["versionId"]);
      }
    }
    if (!repoResult.is_null())
      r["repository"] = repoResult;
    if (stale) {
      J h = J::array();
      for (auto &a : heads(d, e))
        h.push_back(a["versionId"]);
      r["_error"] = {{"reason", "ERROR_REASON_STALE_VERSION"},
                     {"currentHeadIds", h}};
    }
    // The durable receipt keeps the IDs of the affected versions, not their
    // content: deleting an affected task erases its versions, and the receipt
    // of the call that changed it must not keep returning its snapshot.
    J saved = r;
    if (!affectedVersionIds.empty()) {
      saved.erase("affected");
      saved["affectedVersionIds"] = affectedVersionIds;
    }
    d.run("INSERT INTO receipts VALUES(?,?,?,?,?)",
          {m, rid, fingerprint, e, saved.dump()});
    return r;
  }
  static int pageSize(const J &q) {
    auto p = q.value("page", J::object());
    fields(p, {"size", "cursor"});
    int size = p.value("size", 100);
    if (size == 0)
      size = 100;
    require(size > 0 && size <= 1000);
    return size;
  }
  std::pair<size_t, size_t> page(const J &q, const std::string &s,
                                 const std::string &m) {
    auto p = q.value("page", J::object());
    int size = pageSize(q);
    auto cursor = str(p, "cursor");
    size_t offset = 0;
    if (!cursor.empty()) {
      auto pos = cursor.find(':');
      require(pos != std::string::npos);
      J key = q;
      key.erase("page");
      require(cursor.substr(pos + 1) == hash(s + m + key.dump()));
      auto n = cursor.substr(0, pos);
      require(!n.empty() &&
              n.find_first_not_of("0123456789") == std::string::npos);
      offset = std::stoull(n);
    }
    return {size_t(size), offset};
  }
  J paginated(J items, const J &q, const std::string &s, const std::string &m,
              const char *field = "items") {
    auto [size, off] = page(q, s, m);
    J result = J::array();
    for (size_t i = off; i < items.size() && i - off < size; ++i)
      result.push_back(items[i]);
    std::string cursor;
    if (off < items.size() && items.size() - off > size) {
      J key = q;
      key.erase("page");
      cursor = std::to_string(off + size) + ":" + hash(s + m + key.dump());
    }
    return J{{field, result}, {"nextCursor", cursor}};
  }
  // ListTasks: existing search/progress/proposal filters AND with the planning
  // filter; one sort key; keyset pagination whose cursor encodes the last sort
  // tuple and task ID so concatenated pages equal the unpaged order.
  J listTasks(Db &d, const std::string &s, const J &q) {
    fields(q, {"page", "repositoryId", "progress", "proposal", "search",
               "filter", "sortKey", "sortDirection"});
    auto wanted = str(q, "repositoryId"), search = fold(str(q, "search"));
    std::set<std::string> pg, pr;
    for (auto &v : q.value("progress", J::array()))
      pg.insert(enumValue(J{{"v", v}}, "v", "PROGRESS_", progresses));
    for (auto &v : q.value("proposal", J::array()))
      pr.insert(enumValue(J{{"v", v}}, "v", "PROPOSAL_", proposals));
    const J filter = q.value("filter", J::object());
    require(filter.is_object(), "Invalid task filter.");
    fields(filter, {"priorities", "includeUnassignedPriority", "types",
                    "includeUnassignedType", "labelIds", "includeUnlabeled",
                    "startDate", "dueDate", "storyPoints", "milestoneIds",
                    "includeNoMilestone", "cycleIds", "includeNoCycle",
                    "relationships", "includeNoRelationships"});
    auto flag = [&](const J &j, const char *key) {
      if (!j.contains(key) || j[key].is_null())
        return false;
      require(j[key].is_boolean(), "Invalid task filter.");
      return j[key].get<bool>();
    };
    auto enums = [&](const char *key, const std::string &prefix,
                     const std::vector<std::string> &values) {
      std::vector<int> out;
      if (filter.contains(key)) {
        require(filter[key].is_array(), "Invalid task filter.");
        for (auto &v : filter[key])
          out.push_back(optionalEnum(J{{"v", v}}, "v", prefix, values));
      }
      return out;
    };
    auto priorityFilter = enums("priorities", "TASK_PRIORITY_", priorities);
    auto typeFilter = enums("types", "TASK_TYPE_", taskTypes);
    auto relationshipFilter = enums("relationships", "RELATIONSHIP_KIND_", relationshipKinds);
    auto labelFilter = idSet(filter, "labelIds");
    auto milestoneFilter = idSet(filter, "milestoneIds");
    auto cycleFilter = idSet(filter, "cycleIds");
    struct Range { bool present = false, from = false, to = false, unassigned = false; std::string lo, hi; };
    auto dateRange = [&](const char *key) {
      Range r;
      if (!filter.contains(key) || filter[key].is_null())
        return r;
      const J &j = filter[key];
      fields(j, {"from", "to", "includeUnassigned"});
      r.unassigned = flag(j, "includeUnassigned");
      if (j.contains("from") && !j["from"].is_null()) { r.from = true; r.lo = dateField(j, "from"); }
      if (j.contains("to") && !j["to"].is_null()) { r.to = true; r.hi = dateField(j, "to"); }
      require(!r.from || !r.to || r.lo <= r.hi, "A date range must not end before it starts.");
      r.present = r.from || r.to || r.unassigned;
      return r;
    };
    auto inDates = [](const Range &r, const J &t, const char *key) {
      if (!r.present)
        return true;
      if (!t.contains(key))
        return r.unassigned;
      if (!r.from && !r.to)
        return false;
      auto value = t[key].get<std::string>();
      return (!r.from || value >= r.lo) && (!r.to || value <= r.hi);
    };
    auto startRange = dateRange("startDate"), dueRange = dateRange("dueDate");
    struct Points { bool present = false, min = false, max = false, unassigned = false; uint64_t lo = 0, hi = 0; } points;
    if (filter.contains("storyPoints") && !filter["storyPoints"].is_null()) {
      const J &j = filter["storyPoints"];
      fields(j, {"min", "max", "includeUnassigned"});
      points.unassigned = flag(j, "includeUnassigned");
      if (j.contains("min") && !j["min"].is_null()) { points.min = true; points.lo = storyPoints(j["min"]); }
      if (j.contains("max") && !j["max"].is_null()) { points.max = true; points.hi = storyPoints(j["max"]); }
      require(!points.min || !points.max || points.lo <= points.hi, "A points range must not end before it starts.");
      points.present = points.min || points.max || points.unassigned;
    }
    const bool prioritiesOn = !priorityFilter.empty() || flag(filter, "includeUnassignedPriority"),
               typesOn = !typeFilter.empty() || flag(filter, "includeUnassignedType");
    const bool unlabeled = flag(filter, "includeUnlabeled"),
               noMilestone = flag(filter, "includeNoMilestone"),
               noCycle = flag(filter, "includeNoCycle"),
               noRelationships = flag(filter, "includeNoRelationships");
    const bool unassignedPriority = flag(filter, "includeUnassignedPriority"),
               unassignedType = flag(filter, "includeUnassignedType");
    const int sortKey = optionalEnum(q, "sortKey", "TASK_SORT_KEY_", sortKeys);
    const int direction = optionalEnum(q, "sortDirection", "SORT_DIRECTION_", sortDirections);
    const bool descending = direction == 2;
    // `task` is the body of `item` with its references normalized against the
    // live entities of the task's repository; `item` itself stays untouched.
    struct Row { std::string id; J item; SortValue value; J task; std::string repo; };
    std::vector<Row> rows;
    std::map<std::string, std::string> taskRepos;
    for (auto &r : d.run("SELECT id FROM entities WHERE kind=2 AND deleted=0 ORDER BY id")) {
      Row row{r[0], result(d, r[0]), {}, {}, {}};
      row.task = row.item["version"]["task"];
      row.repo = mapped(d, str(row.task, "repositoryId"));
      taskRepos[row.id] = row.repo;
      rows.push_back(std::move(row));
    }
    // Cleanup after a deletion or move skips a conflicted task, so its accepted
    // body can keep a reference to an entity that no longer exists (or that
    // lives in another repository). Such a reference is not an assignment:
    // ignore it for filters, sort keys and the derived relationship views.
    std::map<std::string, bool> liveItems;
    auto liveItem = [&](const std::string &itemId, int kind, const std::string &repo) {
      const auto key = std::to_string(kind) + itemId + "@" + repo;
      auto it = liveItems.find(key);
      if (it == liveItems.end()) {
        auto h = heads(d, itemId);
        bool live = !h.empty() && !h[0].value("deleted", false) &&
                    h[0]["kind"] == kindName(kind) && h[0].contains(bodyKey(kind)) &&
                    mapped(d, str(h[0][bodyKey(kind)], "repositoryId")) == repo;
        it = liveItems.emplace(key, live).first;
      }
      return it->second;
    };
    auto liveTask = [&](const std::string &taskId, const std::string &repo) {
      auto it = taskRepos.find(taskId);
      return it != taskRepos.end() && it->second == repo;
    };
    std::set<std::string> parents, blockers, relatedBack;
    for (auto &row : rows) {
      J &t = row.task;
      auto keep = [&](const char *key, const std::function<bool(const std::string &)> &live) {
        if (!t.contains(key))
          return;
        if (t[key].is_string()) {
          if (!live(t[key].get<std::string>()))
            t.erase(key);
          return;
        }
        std::vector<std::string> kept;
        for (auto &x : list(t, key))
          if (live(x))
            kept.push_back(x);
        putSet(t, key, kept);
      };
      keep("labelIds", [&](const std::string &x) { return liveItem(x, kLabel, row.repo); });
      keep("milestoneId", [&](const std::string &x) { return liveItem(x, kMilestone, row.repo); });
      keep("cycleId", [&](const std::string &x) { return liveItem(x, kCycle, row.repo); });
      for (auto key : {"parentTaskId", "duplicateOfTaskId", "dependsOnTaskIds", "relatedTaskIds"})
        keep(key, [&](const std::string &x) { return liveTask(x, row.repo); });
    }
    for (auto &row : rows) {
      const J &t = row.task;
      if (t.contains("parentTaskId")) parents.insert(str(t, "parentTaskId"));
      for (auto &x : list(t, "dependsOnTaskIds")) blockers.insert(x);
      for (auto &x : list(t, "relatedTaskIds")) if (x != row.id) relatedBack.insert(x);
    }
    std::map<std::string, std::string> milestoneNames, cycleStarts;
    auto milestoneName = [&](const std::string &mid) {
      auto it = milestoneNames.find(mid);
      if (it == milestoneNames.end()) {
        auto h = heads(d, mid);
        std::string name;
        if (!h.empty() && h[0].contains("milestone")) name = lowerSimple(str(h[0]["milestone"], "name"));
        it = milestoneNames.emplace(mid, name).first;
      }
      return it->second;
    };
    auto cycleStart = [&](const std::string &cid) {
      auto it = cycleStarts.find(cid);
      if (it == cycleStarts.end()) {
        auto h = heads(d, cid);
        std::string start;
        if (!h.empty() && h[0].contains("cycle")) start = str(h[0]["cycle"], "startDate");
        it = cycleStarts.emplace(cid, start).first;
      }
      return it->second;
    };
    std::vector<Row> matched;
    for (auto &row : rows) {
      const J &t = row.task;
      if (!wanted.empty() && row.repo != mapped(d, wanted)) continue;
      if (!pg.empty() && !pg.count(str(t, "progress"))) continue;
      if (!pr.empty() && !pr.count(str(t, "proposal"))) continue;
      if (!search.empty() && fold(str(t, "title")).find(search) == std::string::npos &&
          fold(str(t, "description")).find(search) == std::string::npos) continue;
      const int priority = int(std::find(priorities.begin(), priorities.end(), str(t, "priority")) - priorities.begin()) + 1;
      const bool hasPriority = t.contains("priority");
      if (prioritiesOn && !(hasPriority ? std::find(priorityFilter.begin(), priorityFilter.end(), priority) != priorityFilter.end() : unassignedPriority)) continue;
      const int type = int(std::find(taskTypes.begin(), taskTypes.end(), str(t, "type")) - taskTypes.begin()) + 1;
      const bool hasType = t.contains("type");
      if (typesOn && !(hasType ? std::find(typeFilter.begin(), typeFilter.end(), type) != typeFilter.end() : unassignedType)) continue;
      if (!labelFilter.empty() || unlabeled) {
        auto own = list(t, "labelIds");
        bool any = std::any_of(own.begin(), own.end(), [&](const std::string &l) {
          return std::binary_search(labelFilter.begin(), labelFilter.end(), l); });
        if (!any && !(unlabeled && own.empty())) continue;
      }
      if (!inDates(startRange, t, "startDate") || !inDates(dueRange, t, "dueDate")) continue;
      if (points.present) {
        if (!t.contains("storyPoints")) { if (!points.unassigned) continue; }
        else if (!points.min && !points.max) continue;
        else {
          auto value = t["storyPoints"].get<uint64_t>();
          if ((points.min && value < points.lo) || (points.max && value > points.hi)) continue;
        }
      }
      if (!milestoneFilter.empty() || noMilestone) {
        bool ok = t.contains("milestoneId")
            ? std::binary_search(milestoneFilter.begin(), milestoneFilter.end(), str(t, "milestoneId")) : noMilestone;
        if (!ok) continue;
      }
      if (!cycleFilter.empty() || noCycle) {
        bool ok = t.contains("cycleId")
            ? std::binary_search(cycleFilter.begin(), cycleFilter.end(), str(t, "cycleId")) : noCycle;
        if (!ok) continue;
      }
      if (!relationshipFilter.empty() || noRelationships) {
        std::set<int> kinds;
        if (t.contains("parentTaskId")) kinds.insert(1);
        if (parents.count(row.id)) kinds.insert(2);
        if (t.contains("dependsOnTaskIds")) kinds.insert(3);
        if (blockers.count(row.id)) kinds.insert(4);
        if (t.contains("relatedTaskIds") || relatedBack.count(row.id)) kinds.insert(5);
        if (t.contains("duplicateOfTaskId")) kinds.insert(6);
        bool any = std::any_of(relationshipFilter.begin(), relationshipFilter.end(), [&](int k) { return kinds.count(k) > 0; });
        if (!any && !(noRelationships && kinds.empty())) continue;
      }
      SortValue sv;
      switch (sortKey) {
      case 1: sv = {hasPriority, hasPriority ? priority : 0, ""}; break;
      case 2: sv = {hasType, hasType ? type : 0, ""}; break;
      case 3: if (t.contains("startDate")) sv = {true, 0, str(t, "startDate")}; break;
      case 4: if (t.contains("dueDate")) sv = {true, 0, str(t, "dueDate")}; break;
      case 5: if (t.contains("storyPoints")) sv = {true, int64_t(t["storyPoints"].get<uint64_t>()), ""}; break;
      case 6: if (t.contains("milestoneId")) sv = {true, 0, milestoneName(str(t, "milestoneId"))}; break;
      case 7: if (t.contains("cycleId")) sv = {true, 0, cycleStart(str(t, "cycleId"))}; break;
      default: break;
      }
      row.value = sv;
      matched.push_back(row);
    }
    std::sort(matched.begin(), matched.end(), [&](const Row &a, const Row &b) {
      return compareTuples(a.value, b.value, a.id, b.id, descending) < 0; });
    // Keyset cursor: scoped to this account space, method and query.
    J scope = q;
    scope.erase("page");
    const auto scoped = hash(s + "ListTasks" + scope.dump());
    const auto cursor = str(q.value("page", J::object()), "cursor");
    SortValue after; std::string afterId; bool resume = false;
    if (!cursor.empty()) {
      auto pos = cursor.find(':');
      require(pos != std::string::npos && cursor.substr(0, pos) == scoped, "Invalid page cursor.");
      J key = parse(hexDecode(cursor.substr(pos + 1)));
      require(key.is_object() && key.contains("i") && key["i"].is_string(), "Invalid page cursor.");
      after = {key.value("a", false), key.value("n", int64_t(0)), key.value("t", "")};
      afterId = key["i"].get<std::string>();
      resume = true;
    }
    const size_t size = size_t(pageSize(q));
    J items = J::array();
    std::string next;
    const Row *last = nullptr;
    size_t remaining = 0;
    for (auto &row : matched) {
      if (resume && compareTuples(row.value, after, row.id, afterId, descending) <= 0)
        continue;
      if (items.size() < size) { items.push_back(row.item); last = &row; }
      else { remaining = 1; break; }
    }
    if (remaining && last)
      next = scoped + ":" + hexEncode(J{{"a", last->value.assigned}, {"n", last->value.number},
                                        {"t", last->value.text}, {"i", last->id}}.dump());
    return J{{"items", items}, {"nextCursor", next}};
  }
  J read(Db &d, const std::string &s, const std::string &m, const J &q) {
    if (m == "GetAttachment") {
      fields(q,{"taskId","attachmentId"});
      auto task = str(q,"taskId"), aid = str(q,"attachmentId");
      id(task); id(aid);
      auto h = heads(d,task);
      if (h.empty() || h[0]["kind"] != "ENTITY_KIND_TASK") fail("NOT_FOUND","Task does not exist.");
      if (h[0].value("deleted",false)) return J{{"state","ATTACHMENT_STATE_DELETED"}};
      auto rows = d.run("SELECT png_base64 FROM attachments WHERE task_id=? AND id=?", {task,aid});
      if (rows.empty()) fail("NOT_FOUND","Attachment does not belong to this task.");
      if (rows[0][0].empty()) return J{{"state","ATTACHMENT_STATE_WAITING_FOR_SYNC"}};
      return J{{"state","ATTACHMENT_STATE_AVAILABLE"},{"png",rows[0][0]}};
    }
    if (m == "ListURLConnections") {
      fields(q,{"page"});
      J all = J::array();
      for (auto &r : d.run("SELECT id FROM entities WHERE kind=3 AND deleted=0 ORDER BY id")) all.push_back(result(d,r[0]));
      return paginated(all,q,s,m,"items");
    }
    if (m == "GetLocalStatus")
      return status(d, s);
    if (m == "GetNotificationPreferences")
      return prefs(d);
    if (m == "GetTask" || m == "GetRepository") {
      fields(q, {"id"});
      auto e = str(q, "id");
      id(e);
      auto r = result(d, e);
      require(r["version"]["kind"] ==
              (m == "GetTask" ? "ENTITY_KIND_TASK" : "ENTITY_KIND_REPOSITORY"));
      return J{{"result", r}};
    }
    if (m == "ListHistory") {
      fields(q, {"id", "page", "promptsOnly"});
      auto e = str(q, "id");
      id(e);
      auto h = heads(d, e);
      if (h.empty())
        fail("NOT_FOUND", "Entity not found.");
      J all = J::array();
      if (!h[0].value("deleted", false))
        for (auto &r :
             d.run("SELECT envelope_json,prompt_version,entity_id FROM "
                   "versions ORDER BY created_at_us,version_id")) {
          if (mapped(d, r[2]) != mapped(d, e))
            continue;
          if (!q.value("promptsOnly", false) || r[1] == "1")
            all.push_back(parse(r[0]));
        }
      return paginated(all, q, s, m, "versions");
    }
    if (m == "ListTasks")
      return listTasks(d, s, q);
    if (m == "ListRepositories") {
      fields(q, {"page", "repositoryId", "progress", "proposal", "search"});
      J all = J::array();
      for (auto &r : d.run("SELECT id FROM entities WHERE kind=1 AND deleted=0 "
                           "ORDER BY id")) {
        if (mapped(d, r[0]) != r[0])
          continue;
        all.push_back(result(d, r[0]));
      }
      return paginated(all, q, s, m);
    }
    if (m == "GetPlanningItem") {
      fields(q, {"id"});
      auto e = str(q, "id");
      id(e);
      auto r = result(d, e);
      require(isPlanningKind(versionKind(r["version"])));
      return J{{"result", r}};
    }
    if (m == "ListPlanningItems") {
      fields(q, {"page", "repositoryId", "kind"});
      int kind = optionalEnum(q, "kind", "ENTITY_KIND_",
                              {"ENTITY_KIND_REPOSITORY", "ENTITY_KIND_TASK",
                               "ENTITY_KIND_URL_CONNECTION", "ENTITY_KIND_LABEL",
                               "ENTITY_KIND_MILESTONE", "ENTITY_KIND_CYCLE"});
      require(kind == 0 || isPlanningKind(kind),
              "Only label, milestone and cycle kinds can be listed.");
      auto wanted = str(q, "repositoryId");
      if (!wanted.empty())
        id(wanted);
      J all = J::array();
      for (auto &r : d.run("SELECT id,repository_id FROM entities WHERE kind IN "
                           "(4,5,6) AND deleted=0 ORDER BY id")) {
        auto item = result(d, r[0]);
        if (kind && versionKind(item["version"]) != kind)
          continue;
        if (!wanted.empty() && mapped(d, r[1]) != mapped(d, wanted))
          continue;
        all.push_back(item);
      }
      return paginated(all, q, s, m);
    }
    if (m == "ListEvents" || m == "ListReceivers") {
      J all = J::array();
      int kind = m == "ListEvents" ? 2 : 1;
      for (auto &r : d.run("SELECT body_json FROM remote_objects WHERE kind=? "
                           "AND deleted=0 ORDER BY id",
                           {std::to_string(kind)})) {
        auto v = parse(r[0]);
        if (q.contains("repositoryId") && mapped(d, str(q, "repositoryId")) !=
                                              mapped(d, str(v, "repositoryId")))
          continue;
        auto states = q.value("states", J::array());
        if (!states.empty() && std::find(states.begin(), states.end(),
                                         v.value("state", J())) == states.end())
          continue;
        all.push_back(v);
      }
      return paginated(all, q, s, m);
    }
    if (m == "GetEvent") {
      auto e = str(q, "id");
      id(e);
      auto row = d.run(
          "SELECT body_json,deleted FROM remote_objects WHERE kind=2 AND id=?",
          {e});
      if (row.empty())
        fail("NOT_FOUND", "Event not found.");
      if (row[0][1] == "1")
        fail("DELETED", "Event was deleted.");
      J processing = J::array();
      for (auto &r : d.run("SELECT body_json FROM remote_objects WHERE kind=3 "
                           "AND deleted=0 ORDER BY id")) {
        auto v = parse(r[0]);
        if (str(v, "eventId") == e)
          processing.push_back(v);
      }
      return J{{"event", parse(row[0][0])}, {"processing", processing}};
    }
    fail("INVALID_INPUT", "Unknown local query.");
  }
  J remote(const std::string &s, const std::string &m, J q) {
    if (!enabled(s) || !hooks.remote)
      fail("ONLINE_REQUIRED", "This operation requires an online account.");
    if (q.contains("meta"))
      q["meta"]["origin"] = origin(s);
    J r;
    try {
      r = parse(hooks.remote(s, m, q.dump()));
    } catch (const Error &) {
      throw;
    } catch (...) {
      fail("DEPENDENCY_UNAVAILABLE",
           "The online service could not be reached.");
    }
    if (r.contains("ok")) {
      if (!r.value("ok", false)) {
        auto e = r.value("error", J::object());
        throw Error(str(e, "reason", "DEPENDENCY_UNAVAILABLE"),
                    "The online operation failed.", e);
      }
      return r.value("response", J::object());
    }
    return r;
  }
  // Text kept with a rejected outbox item. A server constraint rejection keeps
  // the local version blocked as CONSTRAINT_VIOLATION:<VIOLATION>.
  static std::string rejectionReason(const J &error) {
    static const std::vector<std::string> reasons = {
        "INVALID_INPUT", "NOT_FOUND", "STALE_VERSION", "CONFLICT_READ_ONLY",
        "STALE_RESOLUTION", "REQUEST_ID_REUSED", "DELETED", "UNSUPPORTED_FORMAT",
        "STORAGE_BUSY", "STORAGE_FULL", "ONLINE_REQUIRED", "UNAUTHENTICATED",
        "FORBIDDEN", "DEPENDENCY_UNAVAILABLE", "ITEM_TOO_LARGE", "RATE_LIMITED",
        "STALE_EPOCH", "LEASE_INVALID", "CONSTRAINT_VIOLATION"};
    std::string reason = "ERROR_REASON_UNSPECIFIED";
    if (error.contains("reason") && error["reason"].is_number_integer()) {
      auto n = error["reason"].get<int64_t>();
      if (n >= 1 && n <= int64_t(reasons.size()))
        reason = "ERROR_REASON_" + reasons[size_t(n - 1)];
    } else
      reason = str(error, "reason", reason);
    if (reason != "ERROR_REASON_CONSTRAINT_VIOLATION")
      return reason;
    std::string name = "UNSPECIFIED";
    if (error.contains("violation") && !error["violation"].is_null()) {
      int n = optionalEnum(error, "violation", "CONSTRAINT_VIOLATION_", constraintViolations);
      if (n)
        name = constraintViolations[n - 1].substr(sizeof("CONSTRAINT_VIOLATION_") - 1);
    }
    return "CONSTRAINT_VIOLATION:" + name;
  }
  void lease(Db &d, const std::string &owner) {
    if (meta(d, "sync_owner") != owner ||
        std::stoll(meta(d, "sync_owner_expires_at_us", "0")) <= now())
      fail("LEASE_INVALID", "Synchronization ownership expired.");
  }
  void cache(Db &d, int kind, const J &b) {
    auto e = str(b, "id");
    id(e);
    auto old = d.run(
        "SELECT deleted,body_json FROM remote_objects WHERE kind=? AND id=?",
        {std::to_string(kind), e});
    if (!old.empty() && old[0][0] == "1")
      return;
    bool deleted = b.value("deleted", false) ||
                   b.value("state", "") == "EVENT_STATE_DELETED";
    auto repo = str(b, "repositoryId");
    if (!repo.empty()) {
      auto h = heads(d, mapped(d, repo));
      if (!h.empty() && h[0].value("deleted", false))
        deleted = true;
    }
    d.run("INSERT INTO remote_objects VALUES(?,?,?,?) ON CONFLICT(kind,id) DO "
          "UPDATE SET body_json=excluded.body_json,deleted=excluded.deleted",
          {std::to_string(kind), e, deleted ? "" : b.dump(),
           deleted ? "1" : "0"});
    changed(d, e, 2);
  }
  void remoteDelete(Db &d, int kind, const std::string &e) {
    d.run("INSERT INTO remote_objects VALUES(?,?,NULL,1) ON CONFLICT(kind,id) "
          "DO UPDATE SET body_json=NULL,deleted=1",
          {std::to_string(kind), e});
    if (kind == 2)
      for (auto &r : d.run("SELECT id,body_json FROM remote_objects WHERE "
                           "kind=3 AND deleted=0"))
        if (str(parse(r[1]), "eventId") == e)
          d.run("UPDATE remote_objects SET deleted=1,body_json=NULL WHERE "
                "kind=3 AND id=?",
                {r[0]});
    changed(d, e, 2);
  }
  // Readers accept envelope formats 1, 2 and 3; absent planning fields mean
  // unassigned. Planning data and kinds require format 3, captures format 2.
  void validateVersion(Db &d, const J &v) {
    const int format = v.value("formatVersion", 0);
    if (format < 1 || format > 3)
      fail("UNSUPPORTED_FORMAT", "Update Lane to read this entity version.");
    fields(v, {"formatVersion", "entityId", "kind", "versionId",
               "parentVersionIds", "createdAt", "origin", "deleted",
               "promptRestoration", "task", "repository", "urlConnection",
               "label", "milestone", "cycle"});
    parseTimestamp(str(v, "createdAt"));
    auto o = v.value("origin", J::object());
    fields(o, {"kind", "deviceId", "deviceName", "accountId"});
    enumValue(o, "kind", "ORIGIN_KIND_",
              {"ORIGIN_KIND_APP", "ORIGIN_KIND_CLI", "ORIGIN_KIND_MCP"});
    if (!str(o, "deviceId").empty())
      id(str(o, "deviceId"));
    id(str(v, "entityId"));
    id(str(v, "versionId"));
    const int kind = kindNumber(v.value("kind", J()));
    require(kind != 0);
    auto ps = v.value("parentVersionIds", J::array());
    require(ps.is_array());
    std::string prev;
    for (auto &p : ps) {
      require(p.is_string());
      auto x = p.get<std::string>();
      id(x);
      require(prev.empty() || prev < x);
      prev = x;
      require(x != str(v, "versionId"), "Version cannot parent itself.");
      auto parentEntity =
          d.one("SELECT entity_id FROM versions WHERE version_id=?", {x});
      require(parentEntity.empty() ||
                  mapped(d, parentEntity) == mapped(d, str(v, "entityId")),
              "Parent belongs to another entity.");
    }
    if (v.value("promptRestoration", false))
      require(kind == kTask && !v.value("deleted", false),
              "Restoration marker requires a live task.");
    require(format >= 2 || (kind != kUrlConnection && (!v.contains("task") || v["task"].value("captures",J::array()).empty())), "Capture entities require format 2.");
    require(format == 3 || (!isPlanningKind(kind) && (!v.contains("task") || !hasPlanningFields(v["task"]))), "Planning data requires format 3.");
    // Exactly the body matching the kind may be present; tombstones have none.
    for (int other = kRepository; other <= kCycle; ++other)
      if (v.contains(bodyKey(other)))
        require(!v.value("deleted", false) && other == kind,
                "The entity body does not match its kind.");
    if (v.value("deleted", false))
      return;
    require(v.contains(bodyKey(kind)), "An entity body is required.");
    if (kind == kUrlConnection) {
      connectionBody(d,str(v,"entityId"),v.at("urlConnection"),false);
    } else if (kind == kTask) {
      taskBody(d, v.at("task"));
    } else if (isPlanningKind(kind)) {
      // Shape only: uniqueness and ownership were validated where the version
      // was written and are authoritative on the server.
      planningShape(kind, v.at(bodyKey(kind)));
    } else {
      auto b = v.at("repository");
      fields(b, {"normalizedUrl", "displayName"});
      require(normalizeURL("https://" + str(b, "normalizedUrl")) ==
              str(b, "normalizedUrl"));
    }
  }
  bool ancestor(Db &d, const std::string &a, const std::string &b) {
    if (a == b)
      return true;
    std::set<std::string> visited;
    std::vector<std::string> todo{b};
    while (!todo.empty()) {
      auto x = todo.back();
      todo.pop_back();
      if (!visited.insert(x).second)
        continue;
      auto raw =
          d.one("SELECT envelope_json FROM versions WHERE version_id=?", {x});
      if (raw.empty())
        continue;
      for (auto &p : parse(raw).value("parentVersionIds", J::array())) {
        auto parent = p.get<std::string>();
        if (parent == a)
          return true;
        todo.push_back(parent);
      }
    }
    return false;
  }
  void applyEntity(Db &d, const J &r) {
    std::vector<J> incoming;
    if (r.contains("version"))
      incoming.push_back(r["version"]);
    for (auto &v : r.value("alternatives", J::array()))
      if (std::none_of(incoming.begin(), incoming.end(), [&](auto &a) {
            return a["versionId"] == v["versionId"];
          }))
        incoming.push_back(v);
    for (auto v : incoming) {
      if (v.contains("kind"))
        v["kind"] = enumValue(v, "kind", "ENTITY_KIND_",
                              {"ENTITY_KIND_REPOSITORY", "ENTITY_KIND_TASK", "ENTITY_KIND_URL_CONNECTION",
                               "ENTITY_KIND_LABEL", "ENTITY_KIND_MILESTONE", "ENTITY_KIND_CYCLE"});
      if (v.contains("origin"))
        v["origin"]["kind"] = enumValue(
            v["origin"], "kind", "ORIGIN_KIND_",
            {"ORIGIN_KIND_APP", "ORIGIN_KIND_CLI", "ORIGIN_KIND_MCP"});
      if (v.contains("task")) {
        v["task"]["progress"] =
            enumValue(v["task"], "progress", "PROGRESS_", progresses);
        v["task"]["proposal"] =
            enumValue(v["task"], "proposal", "PROPOSAL_", proposals);
        // Planning enums arrive by name or number; UNSPECIFIED means unassigned.
        auto planningEnum = [&](const char *key, const char *prefix,
                                const std::vector<std::string> &values) {
          int n = optionalEnum(v["task"], key, prefix, values);
          if (n)
            v["task"][key] = values[n - 1];
          else
            v["task"].erase(key);
        };
        planningEnum("priority", "TASK_PRIORITY_", priorities);
        planningEnum("type", "TASK_TYPE_", taskTypes);
      }
      if (v.value("formatVersion", 0) < 1 || v.value("formatVersion", 0) > 3)
        fail("UNSUPPORTED_FORMAT", "Update Lane to read this entity version.");
      auto e = str(v, "entityId");
      auto old = heads(d, e);
      if (!old.empty() && old[0].value("deleted", false))
        continue;
      if (v.value("deleted", false)) {
        validateVersion(d, v);
        tombstone(d, v, false);
        continue;
      }
      int incomingKind = kindNumber(v.value("kind", J()));
      if (incomingKind >= kTask && v.contains(bodyKey(incomingKind))) {
        auto repo = mapped(d, str(v.at(bodyKey(incomingKind)), "repositoryId"));
        auto rh = heads(d, repo);
        if (!rh.empty() && rh[0].value("deleted", false)) {
          // A historical version can still reference the former repository.
          // Its deletion must not erase an entity that has moved elsewhere.
          auto currentRepository =
              d.one("SELECT repository_id FROM entities WHERE id=?", {e});
          if (!currentRepository.empty() && mapped(d, currentRepository) != repo)
            continue;
          J dead = v;
          dead.erase(bodyKey(incomingKind));
          dead.erase("promptRestoration");
          dead["deleted"] = true;
          dead["versionId"] = uuid();
          tombstone(d, dead, false);
          continue;
        }
      }
      validateVersion(d, v);
      for (auto &parent : v.value("parentVersionIds", J::array())) {
        if (d.one("SELECT version_id FROM versions WHERE version_id=?",
                  {parent.get<std::string>()})
                .empty())
          fail("DEPENDENCY_UNAVAILABLE",
               "A parent version must be synchronized first.");
      }
      auto exists =
          d.one("SELECT envelope_json FROM versions WHERE version_id=?",
                {str(v, "versionId")});
      if (!exists.empty()) {
        require(comparableVersion(parse(exists)) == comparableVersion(v),
                "Version identity was reused.");
        continue;
      }
      bool prompt = v.contains("task") && v["task"].contains("prompt");
      auto parents = v.value("parentVersionIds", J::array());
      if (parents.size() == 1) {
        auto raw = d.one("SELECT envelope_json FROM versions WHERE "
                         "version_id=? AND entity_id=?",
                         {parents[0].get<std::string>(), e});
        if (!raw.empty()) {
          auto base = parse(raw);
          if (v.contains("task"))
            prompt = base["task"].value("prompt", J()) !=
                     v["task"].value("prompt", J()) || base["task"].value("captures",J::array()) != v["task"].value("captures",J::array());
        }
      }
      prompt = prompt || v.value("promptRestoration", false);
      if (v["kind"] == "ENTITY_KIND_REPOSITORY")
        uniqueURL(d, str(v["repository"], "normalizedUrl"), e);
      if (v.contains("task")) captureReferences(d,e,v["task"].value("captures",J::array()),true);
      persist(d, v, prompt, false, old.empty());
      bool isAncestor = false;
      for (auto &h : old) {
        if (ancestor(d, str(h, "versionId"), str(v, "versionId")))
          d.run("DELETE FROM heads WHERE entity_id=? AND version_id=?",
                {str(h, "entityId"), str(h, "versionId")});
        else if (ancestor(d, str(v, "versionId"), str(h, "versionId")))
          isAncestor = true;
      }
      if (isAncestor)
        d.run("DELETE FROM heads WHERE entity_id=? AND version_id=?",
              {e, str(v, "versionId")});
      if (parents.size() > 1 &&
          r.value("result", "") != "RESULT_KIND_CONFLICT") {
        // Server acceptance is authoritative for competing provisional
        // resolutions. Keep rejected snapshots as history and stop uploading
        // those candidates.
        for (auto &h : old)
          if (h.value("parentVersionIds", J::array()) == parents &&
              str(h, "versionId") != str(v, "versionId")) {
            d.run("DELETE FROM heads WHERE entity_id=? AND version_id=?",
                  {str(h, "entityId"), str(h, "versionId")});
            for (auto &pending : d.run("SELECT request_id,request_json FROM "
                                       "outbox WHERE entity_id=?",
                                       {e}))
              if (parse(pending[1])["version"]["versionId"] == h["versionId"])
                d.run("UPDATE outbox SET "
                      "state=2,error_reason='ERROR_REASON_STALE_RESOLUTION' "
                      "WHERE request_id=?",
                      {pending[0]});
          }
      }
      auto hs = heads(d, e);
      if (hs.size() == 1) {
        auto only = hs[0];
        int k = versionKind(only);
        d.run("UPDATE entities SET repository_id=?,normalized_url=? WHERE id=?",
              {k != 1 ? mapped(d, str(only[bodyKey(k)], "repositoryId")) : "",
               k == 1 ? str(only["repository"], "normalizedUrl") : "", e});
      }
      if (v.contains("task")) {
        bool pending = v["task"]["proposal"] == "PROPOSAL_PENDING";
        if (!pending)
          cancelNotification(d, e);
        else if (old.empty() && parents.empty() &&
                 v.at("origin").value("kind", "") == "ORIGIN_KIND_MCP" &&
                 str(v["origin"], "deviceId") != device())
          d.run("INSERT INTO notification_receipts VALUES(?,?,1) ON CONFLICT "
                "DO NOTHING",
                {e, device()});
      }
      changed(d, e);
    }
  }
  void mappings(Db &d, const J &list) {
    for (auto &m : list) {
      auto local = str(m, "localId"), canonical = str(m, "canonicalId");
      id(local);
      id(canonical);
      if (local == canonical)
        continue;
      auto existing =
          d.one("SELECT canonical_id FROM repository_mappings WHERE local_id=?",
                {local});
      require(existing.empty() || existing == canonical);
      require(mapped(d, canonical) != local);
      d.run("INSERT INTO repository_mappings VALUES(?,?) ON CONFLICT(local_id) "
            "DO UPDATE SET canonical_id=excluded.canonical_id",
            {local, canonical});
      d.run("UPDATE entities SET repository_id=? WHERE kind IN (2,3,4,5,6) AND "
            "repository_id=?",
            {canonical, local}); // Immutable envelopes and fingerprints keep
                                 // their original references.
      d.run("UPDATE entities SET normalized_url=NULL WHERE id=?", {local});
      changed(d, local);
    }
  }
};
Store::Store(std::string root)
    : impl_(std::make_unique<Impl>(std::move(root))) {}
Store::~Store() = default;
void Store::setHostHooks(HostHooks hooks) { impl_->hooks = std::move(hooks); }
std::string Store::sqliteSourceId() { return sqlite3_sourceid(); }
std::string Store::open(std::string text) {
  return protect([&] {
    impl_->ready();
    auto q = canonical(parse(text));
    fields(q, {"formatVersion", "spaceId"});
    if (q.value("formatVersion", 0) != 3)
      fail("UNSUPPORTED_FORMAT", "Update Lane to use this storage format.");
    auto s = str(q, "spaceId");
    if (s.empty())
      s = impl_->active();
    auto &d = impl_->db(s);
    ++impl_->spaces[s].handles;
    auto seq = sequence(d);
    return success({{"spaceId", s},
                    {"deviceId", impl_->device()},
                    {"schemaVersion", 3},
                    {"formatVersion", 3},
                    {"sequence", seq}},
                   seq);
  });
}
std::string Store::reset(std::string s) {
  auto started = now();
  auto response = protect([&] {
    impl_->ready();
    return impl_->resetData(s);
  });
  writeLog(impl_->root, "ResetDevelopmentData", parse(response), now() - started);
  return response;
}
std::string Store::close(std::string s) {
  return protect([&] {
    impl_->ready();
    impl_->registered(s);
    auto it = impl_->spaces.find(s);
    if (it != impl_->spaces.end() && it->second.handles > 0)
      --it->second.handles;
    if (it != impl_->spaces.end() && it->second.handles == 0 &&
        !impl_->enabled(s))
      impl_->spaces.erase(it);
    return success({{"completed", true}});
  });
}
std::string Store::poll() {
  J events = J::array();
  try {
    impl_->ready();
    for (auto &r :
         impl_->registry->run("SELECT id FROM spaces WHERE sync_enabled=1")) {
      try {
        impl_->db(r[0]);
      } catch (...) {
      }
    }
    std::vector<std::string> ids;
    for (auto &entry : impl_->spaces)
      ids.push_back(entry.first);
    for (auto &s : ids)
      try {
        impl_->registered(s);
        auto &slot = impl_->spaces.at(s);
        if (slot.handles == 0 && !impl_->enabled(s))
          continue;
        impl_->db(s); // Reopen after a development reset released the handles.
        if (!slot.observer)
          continue;
        compatibility(*slot.observer);
        auto seq = sequence(*slot.observer);
        if (seq != slot.seen) {
          slot.seen = seq;
          events.push_back({{"spaceId", s}, {"sequence", seq}});
        }
      } catch (...) {
      } // Marked handles drain; other spaces still receive invalidations.
  } catch (...) {
  } // Retry transient registry failures on the next host tick.
  return events.dump();
}
std::string Store::invoke(std::string m, std::string text) {
  auto started = now();
  auto response = protect([&] {
    impl_->ready();
    auto outer = canonical(parse(text));
    fields(outer, {"spaceId", "request"});
    auto s = str(outer, "spaceId");
    auto request = outer.value("request", J::object());
    if (m == "RemoveDeviceData") {
      fields(request, {});
      return impl_->removeData(s);
    }
    if (m == "ResetDevelopmentData") {
      // Works without an open handle so an unsupported store can be replaced.
      fields(request, {"confirmed"});
      require(request.value("confirmed", J()) == J(true),
              "Confirm the development data reset.");
      return impl_->resetData(s.empty() ? impl_->active() : s);
    }
    if (m == "DisconnectSync" &&
        enumValue(
            request, "retention", "LOCAL_RETENTION_",
            {"LOCAL_RETENTION_KEEP_LOCAL", "LOCAL_RETENTION_REMOVE_LOCAL"}) ==
            "LOCAL_RETENTION_REMOVE_LOCAL")
      return impl_->removeData(s);
    impl_->registered(s);
    auto it = impl_->spaces.find(s);
    if (it == impl_->spaces.end() || it->second.handles <= 0)
      fail("NOT_FOUND", "Open this space before using it.");
    auto &d = impl_->db(s);
    auto q = outer.value("request", J::object());
    require(q.is_object());
    static const std::set<std::string> writes = {
        "CreateRepository", "SaveRepository", "DeleteRepository",
        "CreateTask",       "SaveTask",       "SetProgress",
        "SetProposal",      "RestorePrompt",  "DeleteTask",
        "ResolveConflict", "CreateCaptureTask", "CreateURLConnection", "SaveURLConnection", "DeleteURLConnection",
        "CreatePlanningItem", "SavePlanningItem", "DeletePlanningItem"};
    static const std::set<std::string> reads = {"GetAttachment", "ListURLConnections", "GetRepository",
                                                "GetPlanningItem", "ListPlanningItems",
                                                "GetTask",
                                                "ListRepositories",
                                                "ListTasks",
                                                "ListHistory",
                                                "GetLocalStatus",
                                                "GetNotificationPreferences",
                                                "ListEvents",
                                                "GetEvent"};
    if (writes.count(m)) {
      Tx tx(d);
      compatibility(d);
      impl_->registered(s);
      auto r = impl_->mutate(d, s, m, q);
      tx.commit();
      auto seq = sequence(d);
      if (r.contains("_error")) {
        auto e = r["_error"];
        r.erase("_error");
        auto err = failure(
            Error("STALE_VERSION", "The edit was preserved as a conflict.", e));
        err["response"] = r;
        err["sequence"] = seq;
        return err;
      }
      return success(r, seq);
    }
    if (reads.count(m)) {
      Tx tx(d, false);
      auto r = impl_->read(d, s, m, q);
      auto seq = sequence(d);
      tx.commit();
      return success(r, seq);
    }
    if (m == "RetryBlockedItems") {
      // App action equivalent of `lane sync run --retry-blocked`: rejected
      // items return to the pending state with their original request IDs.
      fields(q, {});
      Tx tx(d);
      compatibility(d);
      d.run("UPDATE outbox SET state=1,error_reason=NULL WHERE state=2");
      changed(d, "", 4);
      tx.commit();
      return success({{"completed", true}}, sequence(d));
    }
    if (m == "SetNotificationPreferences") {
      fields(q, {"enabled", "content"});
      require(!q.contains("enabled") || q["enabled"].is_boolean());
      bool enabled = q.value("enabled", false);
      auto content = enumValue(
          q, "content", "NOTIFICATION_CONTENT_",
          {"NOTIFICATION_CONTENT_DETAIL", "NOTIFICATION_CONTENT_GENERIC"}, 1);
      auto permission =
          meta(d, "notification_permission",
               "NOTIFICATION_PERMISSION_NOT_REQUESTED"); // Prompt before taking
                                                         // the SQL write lock.
      if (enabled && meta(d, "notification_enabled", "0") != "1")
        permission = impl_->hooks.requestNotificationPermission
                         ? impl_->hooks.requestNotificationPermission()
                         : "NOTIFICATION_PERMISSION_UNSUPPORTED";
      permission = enumValue(J{{"permission", permission}}, "permission",
                             "NOTIFICATION_PERMISSION_",
                             {"NOTIFICATION_PERMISSION_NOT_REQUESTED",
                              "NOTIFICATION_PERMISSION_GRANTED",
                              "NOTIFICATION_PERMISSION_DENIED",
                              "NOTIFICATION_PERMISSION_UNSUPPORTED"});
      Tx tx(d);
      compatibility(d);
      setMeta(d, "notification_enabled", enabled ? "1" : "0");
      setMeta(d, "notification_content",
              content == "NOTIFICATION_CONTENT_GENERIC" ? "GENERIC" : "DETAIL");
      setMeta(d, "notification_permission", permission);
      changed(d, "", 3);
      tx.commit();
      return success(impl_->prefs(d), sequence(d));
    }
    if (m == "ConnectSync") {
      fields(q, {"accountId"});
      if (!impl_->hooks.authorize)
        fail("DEPENDENCY_UNAVAILABLE",
             "Account approval is unavailable on this host.");
      {
        Tx t(d);
        compatibility(d);
        setMeta(d, "reconnect_intent", meta(d, "reconnect_required", "0"));
        setMeta(d, "reconnect_entity_sequence",
                d.one("SELECT COALESCE(MAX(sequence),0) FROM changes WHERE "
                      "change_kind IN (1,2)"));
        setMeta(d, "reconnect_from_epoch", meta(d, "account_epoch"));
        t.commit();
      }
      auto r = parse(impl_->hooks.authorize(str(q, "accountId")));
      if (r.contains("ok")) {
        if (!r.value("ok", false))
          throw Error(str(r["error"], "reason", "DEPENDENCY_UNAVAILABLE"),
                      "Account approval failed.", r["error"]);
        r = r["response"];
      }
      return success(r, sequence(d));
    }
    if (m == "DisconnectSync" || m == "RemoveDeviceData") {
      std::string retention =
          m == "RemoveDeviceData"
              ? "LOCAL_RETENTION_REMOVE_LOCAL"
              : enumValue(q, "retention", "LOCAL_RETENTION_",
                          {"LOCAL_RETENTION_KEEP_LOCAL",
                           "LOCAL_RETENTION_REMOVE_LOCAL"});
      if (impl_->hooks.disconnect)
        impl_->hooks.disconnect(s);
      {
        Tx rt(*impl_->registry);
        compatibility(*impl_->registry, true);
        impl_->registry->run("UPDATE spaces SET sync_enabled=0 WHERE id=?",
                             {s});
        rt.commit();
      }
      {
        Tx t(d);
        compatibility(d);
        setMeta(d, "connected", "0");
        setMeta(d, "sync_owner", "");
        setMeta(d, "sync_owner_expires_at_us", "0");
        changed(d, "", 3);
        t.commit();
      }
      (void)retention; // REMOVE_LOCAL is handled before acquiring a space
                       // connection.
      return success({{"completed", true}});
    }
    static const std::set<std::string> online = {
        "ListReceivers",     "CreateReceiver",    "SaveReceiver",
        "RotateReceiverKey", "DeleteReceiver",    "DeleteEvent",
        "ClaimEvent",        "RenewEventLease",   "ReleaseEventLease",
        "CompleteEvent",     "ReopenEvent",       "ListDevices",
        "DisconnectDevice",  "ListConnections",   "RevokeConnection",
        "RegisterPush",      "BeginDataDeletion", "DeleteAccountData"};
    if (online.count(m)) {
      if (m == "BeginDataDeletion" || m == "DeleteAccountData") {
        auto account = impl_->registry->one(
            "SELECT account_id FROM spaces WHERE id=?", {s});
        require(!account.empty() && str(q, "confirmedAccountId") == account,
                "Confirm the pinned account before deleting data.");
        q["mode"] = enumValue(q, "mode", "DATA_DELETION_MODE_",
                              {"DATA_DELETION_MODE_DELETE_EVERYWHERE",
                               "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"});
      }
      if (m == "RegisterPush")
        q["deviceId"] = impl_->device();
      auto r = impl_->remote(
          s, m, q); // Network happens with no SQL transaction held.
      Tx tx(d);
      compatibility(d);
      if (r.contains("event"))
        impl_->cache(d, 2, r["event"]);
      if (r.contains("receiver"))
        impl_->cache(d, 1, r["receiver"]);
      if (r.contains("processing"))
        impl_->cache(d, 3, r["processing"]);
      if (r.contains("deletion")) {
        auto v = r["deletion"];
        impl_->remoteDelete(
            d,
            v.value("kind", "") == "ENTITY_KIND_OR_REMOTE_KIND_EVENT" ? 2 : 1,
            str(v, "id"));
      }
      for (auto &v : r.value("tasks", J::array()))
        impl_->applyEntity(d, v);
      bool disconnected =
          m == "DeleteAccountData" ||
          (m == "DisconnectDevice" && str(q, "deviceId") == impl_->device());
      if (disconnected) {
        setMeta(d, "connected", "0");
        setMeta(d, "sync_owner", "");
        setMeta(d, "sync_owner_expires_at_us", "0");
        if (m == "DeleteAccountData") {
          if (str(q, "mode") == "DATA_DELETION_MODE_DELETE_EVERYWHERE") {
            d.exec("DELETE FROM attachments;DELETE FROM heads;DELETE FROM versions;DELETE FROM "
                   "entities;DELETE FROM receipts;DELETE FROM outbox;DELETE "
                   "FROM remote_objects;DELETE FROM "
                   "notification_receipts;DELETE FROM repository_mappings;");
            d.run(
                "DELETE FROM store_meta WHERE key LIKE 'notification_claim:%'");
            setMeta(d, "account_deleted", "1");
          } else {
            setMeta(d, "reconnect_required", "1");
            setMeta(d, "sync_error_reason", "ERROR_REASON_STALE_EPOCH");
          }
        }
        changed(d, "", 4);
      }
      tx.commit();
      if (disconnected) {
        Tx registryTx(*impl_->registry);
        impl_->registry->run("UPDATE spaces SET sync_enabled=0 WHERE id=?",
                             {s});
        registryTx.commit();
        if (impl_->hooks.disconnect)
          impl_->hooks.disconnect(s);
      }
      return success(r, sequence(d));
    }
    fail("INVALID_INPUT", "Unknown Lane method.");
  });
  writeLog(impl_->root, m == "ResetDevelopmentData" ? m : "Invoke", parse(response),
           now() - started);
  return response;
}
std::string Store::approveAccount(std::string text, bool merge) {
  return protect([&] {
    impl_->ready();
    auto q = canonical(parse(text));
    fields(q, {"issuer", "accountId", "tenantId", "epoch", "deviceId"});
    auto issuer = str(q, "issuer"), account = str(q, "accountId"),
         tenant = str(q, "tenantId");
    require(issuer.rfind("https://", 0) == 0 && !account.empty() &&
            !tenant.empty());
    id(str(q, "epoch"));
    if (q.contains("deviceId"))
      require(str(q, "deviceId") == impl_->device(),
              "Authorization device does not match this installation.");
    auto key = hash(issuer + std::string(1, '\0') + account +
                    std::string(1, '\0') + tenant);
    std::string s;
    {
      Tx rt(*impl_->registry);
      compatibility(*impl_->registry, true);
      s = impl_->registry->one("SELECT id FROM spaces WHERE account_key=?",
                               {key});
      if (s.empty()) {
        s = uuid();
        impl_->registry->run("INSERT INTO spaces VALUES(?,?,?,?,?,1)",
                             {s, key, account, tenant, issuer});
      } else
        impl_->registry->run("UPDATE spaces SET sync_enabled=1 WHERE id=?",
                             {s});
      rt.commit();
    }
    auto &d = impl_->db(s);
    auto local =
        impl_->registry->one("SELECT id FROM spaces WHERE account_key='local'");
    if (merge && !local.empty() && local != s) {
      auto &source = impl_->db(local);
      Tx sourceRead(source, false);
      Tx t(d);
      compatibility(d);
      // Import current ownership before tombstones. Replaying old task versions
      // would incorrectly restore their former repository projection.
      for (auto &r : source.run("SELECT id,kind,repository_id FROM entities")) {
        auto kind = d.one("SELECT kind FROM entities WHERE id=?", {r[0]});
        require(kind.empty() || kind == r[1], "Entity kind cannot change during import.");
        d.run("INSERT INTO entities(id,kind,repository_id) VALUES(?,?,?) ON CONFLICT DO NOTHING",
              {r[0],r[1],impl_->mapped(d,impl_->mapped(source,r[2]))});
      }
      for (auto &r : source.run("SELECT local_id,canonical_id FROM repository_mappings"))
        impl_->mappings(d,J::array({J{{"localId",r[0]},{"canonicalId",r[1]}}}));
      for (auto &r : source.run("SELECT id,normalized_url FROM entities WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL")) {
        auto other = d.one("SELECT id FROM entities WHERE kind=1 AND normalized_url=? AND deleted=0",{r[1]});
        if (!other.empty() && other != r[0])
          impl_->mappings(d,J::array({J{{"localId",r[0]},{"canonicalId",other}}}));
      }
      // Copy immutable non-head history without validation against today's
      // repositories or writes to heads/projections. Source is a compatible
      // local store; current heads below still use normal merge/deletion rules.
      for (auto &r : source.run("SELECT v.version_id,v.entity_id,v.envelope_json,v.created_at_us,v.prompt_version FROM versions v WHERE NOT EXISTS(SELECT 1 FROM heads h WHERE h.version_id=v.version_id) ORDER BY v.created_at_us,v.version_id")) {
        auto h = impl_->heads(d,r[1]);
        if (!h.empty() && h[0].value("deleted",false)) continue;
        auto previous = d.one("SELECT envelope_json FROM versions WHERE version_id=?",{r[0]});
        require(previous.empty() || impl_->comparableVersion(parse(previous)) == impl_->comparableVersion(parse(r[2])), "Version identity differs between datasets.");
        d.run("INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us,prompt_version) VALUES(?,?,?,?,?) ON CONFLICT DO NOTHING",r);
      }
      // Retain identities even when their bytes and owner have been deleted.
      for (auto &r : source.run("SELECT id,task_id,sha256,width,height,byte_size FROM attachment_identities"))
        impl_->captureIdentity(d,r[1],{{"attachmentId",r[0]},{"sha256",r[2]},
            {"width",std::stoll(r[3])},{"height",std::stoll(r[4])},{"byteSize",r[5]}});
      for (auto &r : source.run("SELECT id,task_id,sha256,width,height,byte_size FROM attachments")) {
        auto h = impl_->heads(d,r[1]);
        if (!h.empty() && h[0].value("deleted",false)) continue;
        impl_->captureReferences(d,r[1],J::array({J{{"attachmentId",r[0]},
            {"sha256",r[2]},{"width",std::stoll(r[3])},{"height",std::stoll(r[4])},{"byteSize",r[5]}}}),true);
      }
      for (auto &r : source.run("SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id JOIN entities e ON e.id=h.entity_id ORDER BY CASE WHEN e.kind=1 AND e.deleted=0 THEN 0 WHEN e.deleted=0 THEN 1 ELSE 2 END,v.created_at_us,v.version_id"))
        impl_->applyEntity(d,{{"version",parse(r[0])}});
      for (auto &r : source.run("SELECT id,task_id,sha256,width,height,byte_size,png_base64 FROM attachments")) {
        auto retained = d.run("SELECT task_id,sha256,width,height,byte_size FROM attachments WHERE id=?", {r[0]});
        if (retained.empty()) continue; // A destination tombstone must not regain bytes.
        require(retained[0] == std::vector<std::string>(r.begin()+1,r.begin()+6), "Attachment differs between datasets.");
        if (!r[6].empty()) d.run("UPDATE attachments SET png_base64=? WHERE id=? AND png_base64 IS NULL", {r[6],r[0]});
      }
      for (auto &r : source.run(
               "SELECT method,request_id,fingerprint,entity_id,result_json "
               "FROM receipts")) {
        auto prev = d.run(
            "SELECT fingerprint FROM receipts WHERE method=? AND request_id=?",
            {r[0], r[1]});
        require(prev.empty() || prev[0][0] == r[2],
                "Request identity differs between datasets.");
        d.run("INSERT INTO receipts VALUES(?,?,?,?,?) ON CONFLICT DO NOTHING",
              r);
      }
      for (auto &r : source.run("SELECT "
                                "request_id,entity_id,request_json,state,error_"
                                "reason,created_at_us FROM outbox"))
        d.run("INSERT INTO outbox VALUES(?,?,?,?,?,?) ON CONFLICT DO NOTHING",
              r);
      changed(d, "", 4);
      t.commit();
      sourceRead.commit();
    }
    {
      Tx t(d);
      // Reapproval provides credentials, not permission to overwrite retained
      // generation identity. Probe the old epoch before pushing anything.
      // A space waiting for reconnect (after an epoch change or a development
      // reset) must confirm explicitly even when it has no retained epoch.
      if (meta(d, "account_epoch").empty() && meta(d, "reconnect_required", "0") != "1")
        setMeta(d, "account_epoch", str(q, "epoch"));
      else if (meta(d, "account_epoch") != str(q, "epoch"))
        setMeta(d, "sync_error_reason", "ERROR_REASON_STALE_EPOCH");
      setMeta(d, "approved_epoch", str(q, "epoch"));
      setMeta(d, "connected", "0");
      changed(d, "", 3);
      t.commit();
    }
    {
      Tx t(*impl_->registry);
      impl_->registry->run(
          "UPDATE registry_meta SET value=? WHERE key='active_space_id'", {s});
      t.commit();
    }
    return success({{"spaceId", s}, {"deviceId", impl_->device()}},
                   sequence(d));
  });
}
std::string Store::acquireSync(std::string s, std::string owner, int seconds) {
  return protect([&] {
    impl_->ready();
    id(owner);
    require(seconds >= 0 && seconds <= 300);
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    if (!impl_->enabled(s))
      fail("ONLINE_REQUIRED", "Synchronization is not enabled.");
    bool acquired = false;
    auto old = meta(d, "sync_owner");
    auto expiry = std::stoll(meta(d, "sync_owner_expires_at_us", "0"));
    if (old == owner || old.empty() || expiry <= now()) {
      setMeta(d, "sync_owner", seconds == 0 ? "" : owner);
      setMeta(d, "sync_owner_expires_at_us",
              std::to_string(
                  seconds == 0 ? 0 : now() + int64_t(seconds) * 1000000));
      acquired = seconds > 0;
    }
    t.commit();
    return success({{"acquired", acquired}}, sequence(d));
  });
}
std::string Store::pendingSync(std::string s, std::string owner) {
  return protect([&] {
    impl_->ready();
    auto &d = impl_->db(s);
    Tx t(d, false);
    impl_->lease(d, owner);
    J mutations = J::array();
    // Returning no uploads during an epoch mismatch makes stale reupload
    // impossible even if a host forgets its GetSyncInfo comparison.
    bool epochReady = meta(d, "approved_epoch", meta(d, "account_epoch")) ==
                          meta(d, "account_epoch") &&
                      meta(d, "reconnect_required", "0") != "1";
    // Blocked (rejected) items are never re-sent implicitly; the user corrects
    // them and retries explicitly (RetryBlockedItems).
    if (epochReady)
      mutations = impl_->uploadBatch(d);
    J attachments = J::array();
    if (epochReady) {
      for (auto &row : d.run("SELECT a.id,a.task_id,a.sha256,a.width,a.height,a.byte_size,a.png_base64 FROM attachments a JOIN entities e ON e.id=a.task_id WHERE a.synchronized=0 AND a.png_base64 IS NOT NULL AND e.deleted=0 ORDER BY a.id")) {
        attachments.push_back({{"taskId",row[1]},
          {"capture",{{"attachmentId",row[0]},{"sha256",row[2]},
                      {"width",std::stoll(row[3])},{"height",std::stoll(row[4])},{"byteSize",row[5]}}},
          {"png",row[6]}});
      }
    }
    J r = {{"formatVersion", 3},
           {"epoch", meta(d, "account_epoch")},
           {"deviceId", impl_->device()},
           {"mutations", mutations},
           {"attachments", attachments},
           {"cursor", meta(d, "pull_cursor")}};
    t.commit();
    return success(r, sequence(d));
  });
}
std::string Store::acknowledgeAttachment(std::string s, std::string owner,
    std::string epoch, std::string task, std::string attachment) {
  return protect([&] {
    impl_->ready();
    id(task); id(attachment);
    auto &d = impl_->db(s);
    Tx tx(d);
    compatibility(d);
    impl_->lease(d,owner);
    if (meta(d,"account_epoch") != epoch ||
        meta(d,"approved_epoch",meta(d,"account_epoch")) != epoch ||
        meta(d,"reconnect_required","0") == "1")
      fail("STALE_EPOCH","Attachment upload belongs to an old synchronization epoch.");
    require(!d.one("SELECT id FROM attachments WHERE id=? AND task_id=? AND png_base64 IS NOT NULL",{attachment,task}).empty(), "Attachment upload is no longer available.");
    d.run("UPDATE attachments SET synchronized=1 WHERE id=? AND task_id=?",{attachment,task});
    changed(d,task);
    tx.commit();
    return success({{"completed",true}},sequence(d));
  });
}
std::string Store::cacheAttachment(std::string s, std::string task,
                                    std::string attachment, std::string png) {
  auto started = now();
  auto response = protect([&] {
    impl_->ready();
    impl_->registered(s);
    id(task); id(attachment);
    auto &d = impl_->db(s);
    Tx tx(d);
    compatibility(d);
    auto h = impl_->heads(d,task);
    if (h.empty() || h[0]["kind"] != "ENTITY_KIND_TASK") fail("NOT_FOUND","Task does not exist.");
    if (h[0].value("deleted",false)) {
      tx.commit();
      return success({{"state","ATTACHMENT_STATE_DELETED"}},sequence(d));
    }
    auto rows = d.run("SELECT sha256,width,height,byte_size FROM attachments WHERE id=? AND task_id=?",{attachment,task});
    if (rows.empty()) fail("NOT_FOUND","Attachment does not belong to this task.");
    auto &r = rows[0];
    J manifest = {{"attachmentId",attachment},{"sha256",r[0]},
                  {"width",std::stoll(r[1])},{"height",std::stoll(r[2])},{"byteSize",r[3]}};
    validatePNG(manifest,decodeBase64(png));
    d.run("UPDATE attachments SET png_base64=?,synchronized=1 WHERE id=? AND task_id=?",{png,attachment,task});
    changed(d,task);
    auto result = impl_->read(d,s,"GetAttachment",{{"taskId",task},{"attachmentId",attachment}});
    tx.commit();
    return success(result,sequence(d));
  });
  writeLog(impl_->root,"CacheAttachment",parse(response),now()-started);
  return response;
}
std::string Store::applySync(std::string s, std::string text) {
  return protect([&] {
    impl_->ready();
    auto q = canonical(parse(text));
    fields(q, {"formatVersion", "epoch", "owner", "push", "pull"});
    if (q.value("formatVersion", 0) != 3)
      fail("UNSUPPORTED_FORMAT",
           "Update Lane before synchronization can continue.");
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    impl_->lease(d, str(q, "owner"));
    if (str(q, "epoch") != meta(d, "account_epoch"))
      fail("STALE_EPOCH", "Reconnect explicitly to this account.");
    if (q.contains("push")) {
      auto p = q["push"];
      impl_->mappings(d, p.value("repositoryMappings", J::array()));
      for (auto &r : p.value("results", J::array())) {
        auto rid = str(r, "requestId");
        auto pending =
            d.one("SELECT request_json FROM outbox WHERE request_id=?", {rid});
        if (pending.empty())
          continue;
        if (r.contains("error") &&
            Impl::rejectionReason(r["error"]) != "ERROR_REASON_UNSPECIFIED") {
          d.run("UPDATE outbox SET state=2,error_reason=? WHERE request_id=?",
                {Impl::rejectionReason(r["error"]), rid});
        } else {
          if (r.contains("result"))
            impl_->applyEntity(d, r["result"]);
          d.run("DELETE FROM outbox WHERE request_id=?", {rid});
        }
      }
    }
    if (q.contains("pull")) {
      auto p = q["pull"];
      for (auto &c : p.value("changes", J::array())) {
        if (c.contains("entity"))
          impl_->applyEntity(d, c["entity"]);
        else if (c.contains("receiver"))
          impl_->cache(d, 1, c["receiver"]);
        else if (c.contains("event"))
          impl_->cache(d, 2, c["event"]);
        else if (c.contains("processing"))
          impl_->cache(d, 3, c["processing"]);
        else if (c.contains("deletion")) {
          auto v = c["deletion"];
          auto kind = str(v, "kind"), e = str(v, "id");
          if (kind == "ENTITY_KIND_OR_REMOTE_KIND_EVENT" ||
              kind == "ENTITY_KIND_OR_REMOTE_KIND_RECEIVER")
            impl_->remoteDelete(
                d, kind == "ENTITY_KIND_OR_REMOTE_KIND_EVENT" ? 2 : 1, e);
          else if (kind == "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT") {
            auto account = impl_->registry->one(
                "SELECT account_id FROM spaces WHERE id=?", {s});
            if (account.empty() || e != account)
              fail("FORBIDDEN",
                   "Account deletion does not belong to this space.");
            d.exec("DELETE FROM attachments;DELETE FROM heads;DELETE FROM versions;DELETE FROM "
                   "entities;DELETE FROM receipts;DELETE FROM outbox;DELETE "
                   "FROM remote_objects;DELETE FROM "
                   "notification_receipts;DELETE FROM repository_mappings;");
            setMeta(d, "connected", "0");
            setMeta(d, "account_deleted", "1");
            setMeta(d, "sync_owner", "");
            setMeta(d, "sync_owner_expires_at_us", "0");
            changed(d, "", 4);
          } else {
            auto h = impl_->heads(d, e);
            J v = impl_->version(
                s, e, kind == "ENTITY_KIND_OR_REMOTE_KIND_REPOSITORY" ? 1 : kind == "ENTITY_KIND_OR_REMOTE_KIND_URL_CONNECTION" ? 3 : 2,
                {}, J::array(), true);
            v["versionId"] = str(c["deletion"], "deletionVersionId");
            impl_->tombstone(d, v, false);
          }
        }
      }
      setMeta(d, "pull_cursor", str(p, "nextCursor"));
    }
    bool deleted = meta(d, "account_deleted") == "1";
    setMeta(d, "connected", deleted ? "0" : "1");
    setMeta(d, "sync_error_reason", "");
    setMeta(d, "last_sync_at", timestamp());
    changed(d, "", 4);
    t.commit();
    if (deleted) {
      Tx rt(*impl_->registry);
      impl_->registry->run("UPDATE spaces SET sync_enabled=0 WHERE id=?", {s});
      rt.commit();
      if (impl_->hooks.disconnect)
        impl_->hooks.disconnect(s);
    }
    return success({{"completed", true}}, sequence(d));
  });
}
namespace {
template <class I> J notificationItems(I &impl, Db &d) {
  J items = J::array();
  if (meta(d, "notification_enabled", "0") != "1" ||
      meta(d, "notification_permission") != "NOTIFICATION_PERMISSION_GRANTED")
    return items;
  for (auto &r : d.run("SELECT proposal_id FROM notification_receipts WHERE "
                       "device_id=? AND state=1 ORDER BY proposal_id",
                       {impl.device()})) {
    auto h = impl.heads(d, r[0]);
    if (h.size() != 1 || h[0].value("deleted", false) ||
        !h[0].contains("task") ||
        h[0]["task"]["proposal"] != "PROPOSAL_PENDING")
      continue;
    auto v = h[0], task = v["task"];
    auto repo = impl.heads(d, impl.mapped(d, str(task, "repositoryId")));
    if (repo.empty() || repo[0].value("deleted", false))
      continue;
    auto b = repo[0]["repository"];
    bool generic = meta(d, "notification_content", "DETAIL") == "GENERIC";
    items.push_back(
        {{"proposalId", r[0]},
         {"deviceId", impl.device()},
         {"title", generic ? "New Lane proposal" : str(task, "title")},
         {"repositoryName",
          generic ? "" : str(b, "displayName", str(b, "normalizedUrl"))},
         {"content", generic ? "NOTIFICATION_CONTENT_GENERIC"
                             : "NOTIFICATION_CONTENT_DETAIL"}});
  }
  return items;
}
} // namespace
std::string Store::pendingNotifications(std::string s) {
  return protect([&] {
    impl_->ready();
    auto &d = impl_->db(s);
    Tx t(d, false);
    auto items = notificationItems(*impl_, d);
    t.commit();
    return success({{"items", items}}, sequence(d));
  });
}
std::string Store::claimNotifications(std::string s, std::string claimant) {
  return protect([&] {
    impl_->ready();
    id(claimant);
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    J claimed = J::array();
    for (auto &item : notificationItems(*impl_, d)) {
      auto key = "notification_claim:" + str(item, "proposalId");
      auto previous = meta(d, key);
      if (!previous.empty()) {
        auto p = parse(previous);
        if (p.value("expires", int64_t(0)) > now())
          continue;
      }
      setMeta(d, key,
              J{{"owner", claimant}, {"expires", now() + 60000000}}.dump());
      claimed.push_back(item);
    }
    t.commit();
    return success({{"items", claimed}}, sequence(d));
  });
}
std::string Store::acknowledgeNotification(std::string s,
                                           std::string proposal) {
  return protect([&] {
    impl_->ready();
    id(proposal);
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    d.run("UPDATE notification_receipts SET state=2 WHERE proposal_id=? AND "
          "device_id=? AND state=1",
          {proposal, impl_->device()});
    d.run("DELETE FROM store_meta WHERE key=?",
          {"notification_claim:" + proposal});
    t.commit();
    return success({{"completed", true}}, sequence(d));
  });
}
std::string Store::recordNotificationPermission(std::string s,
                                                std::string permission) {
  return protect([&] {
    impl_->ready();
    permission = enumValue(
        J{{"v", permission}}, "v", "NOTIFICATION_PERMISSION_",
        {"NOTIFICATION_PERMISSION_NOT_REQUESTED",
         "NOTIFICATION_PERMISSION_GRANTED", "NOTIFICATION_PERMISSION_DENIED",
         "NOTIFICATION_PERMISSION_UNSUPPORTED"});
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    setMeta(d, "notification_permission", permission);
    changed(d, "", 3);
    t.commit();
    return success(impl_->prefs(d), sequence(d));
  });
}
std::string Store::recordSyncFailure(std::string s, std::string reason) {
  return protect([&] {
    impl_->ready();
    require(reason.rfind("ERROR_REASON_", 0) == 0);
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    setMeta(d, "connected", "0");
    setMeta(d, "sync_error_reason", reason);
    if (reason == "ERROR_REASON_STALE_EPOCH")
      setMeta(d, "reconnect_required", "1");
    changed(d, "", 4);
    t.commit();
    return success(impl_->status(d, s), sequence(d));
  });
}
std::string Store::spacesForAccount(std::string account) {
  return protect([&] {
    impl_->ready();
    require(!account.empty());
    J items = J::array();
    for (auto &r : impl_->registry->run(
             "SELECT id FROM spaces WHERE account_id=? AND sync_enabled=1 AND "
             "account_key NOT LIKE 'removed:%' ORDER BY id",
             {account}))
      items.push_back(r[0]);
    return success({{"items", items}});
  });
}
std::string Store::confirmSyncEpoch(std::string s, std::string epoch) {
  return protect([&] {
    impl_->ready();
    id(epoch);
    auto &d = impl_->db(s);
    Tx t(d);
    compatibility(d);
    require(meta(d, "approved_epoch") == epoch,
            "Epoch must come from a validated account approval.");
    require(meta(d, "reconnect_required", "0") == "1" &&
                meta(d, "reconnect_intent", "0") == "1",
            "Explicit reconnect approval is required.");
    require(meta(d, "reconnect_from_epoch") == meta(d, "account_epoch") &&
                meta(d, "reconnect_entity_sequence") ==
                    d.one("SELECT COALESCE(MAX(sequence),0) FROM changes WHERE "
                          "change_kind IN (1,2)"),
            "Local data changed during approval. Confirm again.");
    if (meta(d, "account_epoch") != epoch) {
      d.exec("DELETE FROM outbox; UPDATE attachments SET synchronized=0;");
      // Only versions reachable from a current head are uploaded. A rejected
      // version that an ordinary edit superseded stays in local history but is
      // neither a head nor an ancestor of one.
      std::set<std::string> reachable;
      std::map<std::string, J> stored;
      for (auto &r : d.run("SELECT version_id,envelope_json FROM versions"))
        stored[r[0]] = parse(r[1]).value("parentVersionIds", J::array());
      std::vector<std::string> frontier;
      for (auto &r : d.run("SELECT version_id FROM heads"))
        if (reachable.insert(r[0]).second)
          frontier.push_back(r[0]);
      while (!frontier.empty()) {
        auto version = frontier.back();
        frontier.pop_back();
        auto it = stored.find(version);
        if (it == stored.end())
          continue;
        for (auto &parent : it->second)
          if (parent.is_string() && reachable.insert(parent.get<std::string>()).second)
            frontier.push_back(parent.get<std::string>());
      }
      for (auto &r : d.run("SELECT envelope_json FROM versions ORDER BY "
                           "created_at_us,version_id")) {
        auto v = parse(r[0]);
        if (!reachable.count(str(v, "versionId")))
          continue;
        auto parents = v.value("parentVersionIds", J::array());
        impl_->outbox(d, v,
                      v.value("deleted", false) ? "MUTATION_KIND_DELETE"
                      : parents.empty()         ? "MUTATION_KIND_CREATE"
                      : parents.size() > 1      ? "MUTATION_KIND_RESOLVE"
                                                : "MUTATION_KIND_SAVE");
      }
      setMeta(d, "account_epoch", epoch);
      setMeta(d, "pull_cursor", "");
    }
    setMeta(d, "account_deleted", "0");
    setMeta(d, "reconnect_required", "0");
    setMeta(d, "reconnect_intent", "0");
    setMeta(d, "sync_error_reason", "");
    setMeta(d, "connected", "0");
    changed(d, "", 4);
    t.commit();
    return success({{"completed", true}}, sequence(d));
  });
}
} // namespace lane
