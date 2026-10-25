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
#include <chrono>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <map>
#include <regex>
#include <set>
#include <sstream>
#include <stdexcept>
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
void compatibility(Db &d, bool registry = false) {
  if (d.one("PRAGMA application_id") != std::to_string(kApp) ||
      d.one("PRAGMA user_version") != "1")
    fail("UNSUPPORTED_FORMAT", "Update Lane to open this data format.");
  if (!registry &&
      d.one("SELECT value FROM store_meta WHERE key='format_version'") != "1")
    fail("UNSUPPORTED_FORMAT", "Update Lane to open this data format.");
}
void initialize(Db &d, bool registry) {
  auto v = d.one("PRAGMA user_version"), a = d.one("PRAGMA application_id");
  if (v == "0" && a == "0" &&
      d.one("SELECT count(*) FROM sqlite_master WHERE name NOT LIKE "
            "'sqlite_%'") == "0") {
    Tx t(d);
    d.exec(registry ? kRegistry : kSchema);
    d.exec("PRAGMA application_id=1279348293; PRAGMA user_version=1;");
    if (!registry)
      d.run("INSERT INTO store_meta VALUES('format_version','1')");
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
  for (auto key : {"entityId", "currentHeadIds", "retryable"})
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

struct Store::Impl {
  fs::path root;
  std::unique_ptr<Db> registry;
  HostHooks hooks;
  struct Space {
    std::unique_ptr<Db> db, observer;
    int handles = 0;
    std::string seen;
  };
  std::map<std::string, Space> spaces;
  explicit Impl(std::string r) : root(fs::u8path(r)) {
    require(!r.empty());
    privateDir(root);
    privateDir(root / "spaces");
    registry = std::make_unique<Db>(root / "registry.sqlite");
    initialize(*registry, true);
    Tx tx(*registry);
    compatibility(*registry, true);
    if (registry->one("SELECT value FROM registry_meta WHERE key='device_id'")
            .empty())
      registry->run("INSERT INTO registry_meta VALUES('device_id',?)",
                    {uuid()});
    if (registry
            ->one("SELECT value FROM registry_meta WHERE key='active_space_id'")
            .empty()) {
      auto sid = uuid();
      registry->run("INSERT INTO spaces(id,account_key) VALUES(?,'local')",
                    {sid});
      registry->run("INSERT INTO registry_meta VALUES('active_space_id',?)",
                    {sid});
    }
    tx.commit();
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
    if (registry
            ->one("SELECT id FROM spaces WHERE id=? AND account_key NOT LIKE "
                  "'removed:%'",
                  {s})
            .empty())
      fail("NOT_FOUND", "This local space is unavailable.");
  }
  Db &db(const std::string &s) {
    registered(s);
    auto &slot = spaces[s];
    if (!slot.db) {
      auto dir = root / "spaces" / s;
      privateDir(dir);
      slot.db = std::make_unique<Db>(dir / "lane.sqlite");
      initialize(*slot.db, false);
      Tx t(*slot.db);
      if (meta(*slot.db, "space_id").empty()) {
        setMeta(*slot.db, "space_id", s);
        setMeta(*slot.db, "device_id", device());
        setMeta(*slot.db, "account_epoch", "");
        setMeta(*slot.db, "pull_cursor", "");
        setMeta(*slot.db, "notification_enabled", "0");
        setMeta(*slot.db, "notification_content", "DETAIL");
        setMeta(*slot.db, "sync_owner", "");
        setMeta(*slot.db, "sync_owner_expires_at_us", "0");
      }
      require(meta(*slot.db, "space_id") == s, "Storage routing mismatch.");
      t.commit();
      slot.observer = std::make_unique<Db>(dir / "lane.sqlite");
      initialize(*slot.observer, false);
    }
    compatibility(*slot.db);
    return *slot.db;
  }
  bool enabled(const std::string &s) {
    return registry->one("SELECT sync_enabled FROM spaces WHERE id=?", {s}) ==
           "1";
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
      require(h[0]["kind"] ==
              (kind == 1 ? "ENTITY_KIND_REPOSITORY" : "ENTITY_KIND_TASK"));
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
  J taskBody(Db &d, J t) {
    fields(t, {"repositoryId", "title", "description", "prompt", "progress",
               "proposal"});
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
    return r;
  }
  J version(const std::string &s, const std::string &e, int k, J body,
            J parents = J::array(), bool deleted = false) {
    std::sort(parents.begin(), parents.end());
    J v = {{"formatVersion", 1},
           {"entityId", e},
           {"kind", k == 1 ? "ENTITY_KIND_REPOSITORY" : "ENTITY_KIND_TASK"},
           {"versionId", uuid()},
           {"parentVersionIds", parents},
           {"createdAt", timestamp()},
           {"origin", origin(s)},
           {"deleted", deleted}};
    if (!deleted)
      v[k == 1 ? "repository" : "task"] = body;
    return v;
  }
  J comparableVersion(J v) {
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
    int k = v["kind"] == "ENTITY_KIND_REPOSITORY" ? 1 : 2;
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
          {k == 2 && !deleted ? mapped(d, str(v["task"], "repositoryId")) : "",
           k == 1 && !deleted ? str(v["repository"], "normalizedUrl") : "",
           deleted ? "1" : "0", e});
  }
  void outbox(Db &d, const J &v, const std::string &kind) {
    auto rid = uuid();
    J m = {{"meta", {{"requestId", rid}, {"origin", v["origin"]}}},
           {"version", v},
           {"expectedHeadIds", v["parentVersionIds"]},
           {"kind", kind}};
    d.run("INSERT INTO outbox VALUES(?,?,?,1,NULL,?)",
          {rid, str(v, "entityId"), m.dump(), std::to_string(now())});
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
      for (auto &r : d.run("SELECT id FROM entities WHERE kind=2 AND deleted=0 "
                           "AND repository_id=?",
                           {e})) {
        auto old = heads(d, r[0]);
        J child = v;
        child["entityId"] = r[0];
        child["versionId"] = uuid();
        child["kind"] = "ENTITY_KIND_TASK";
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
           {"pendingItemCount", d.one("SELECT count(*) FROM outbox")},
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
                 "progress", "proposal"});
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
    if (m == "SetProgress") {
      fields(q, {"meta", "id", "expectedVersionId", "progress"});
      q["progress"] = enumValue(q, "progress", "PROGRESS_", progresses);
    }
    if (m == "SetProposal") {
      fields(q, {"meta", "id", "expectedVersionId", "proposal"});
      q["proposal"] = enumValue(q, "proposal", "PROPOSAL_", proposals);
    }
    if (m == "SaveTask") {
      fields(q, {"meta", "id", "expectedVersionId", "task"});
      auto &t = q["task"];
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
  J mutate(Db &d, const std::string &s, const std::string &m, J q) {
    q = normalizeRequest(m, q, s);
    require(q.contains("meta"));
    auto rid = str(q["meta"], "requestId"), e = str(q, "id");
    id(e);
    auto fingerprint = hash(q.dump());
    auto receipt = d.run("SELECT fingerprint,result_json,entity_id FROM "
                         "receipts WHERE method=? AND request_id=?",
                         {m, rid});
    if (!receipt.empty()) {
      if (receipt[0][0] != fingerprint)
        fail("REQUEST_ID_REUSED", "Request ID was used for different content.");
      auto r = parse(receipt[0][1]);
      if (r.contains("result") &&
          r["result"].value("result", "") != "RESULT_KIND_DELETED")
        r["result"]["result"] = "RESULT_KIND_REPLAYED";
      return r;
    }
    bool create = m == "CreateTask" || m == "CreateRepository",
         del = m == "DeleteTask" || m == "DeleteRepository",
         resolve = m == "ResolveConflict";
    int k = (m.find("Repository") != std::string::npos) ? 1 : 2;
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
      int actualKind = hs[0]["kind"] == "ENTITY_KIND_REPOSITORY" ? 1 : 2;
      require(resolve || actualKind == k, "Method does not match entity kind.");
      k = actualKind;
    }
    J body = J::object(), parents = J::array(), repoResult;
    bool stale = false, prompt = false;
    if (del) {
      for (auto &h : hs)
        parents.push_back(h["versionId"]);
      auto v = version(s, e, k, {}, parents, true);
      tombstone(d, v, true);
    } else {
      if (create) {
        if (k == 1) {
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
          body = taskBody(d, body);
          prompt = body.contains("prompt");
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
        if (k == 1) {
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
        if (k == 1) {
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
            body.erase("prompt");
            if (src.contains("prompt"))
              body["prompt"] = src["prompt"];
          } else
            fail("INVALID_INPUT", "Unknown mutation.");
          body = taskBody(d, body);
          prompt = m == "RestorePrompt" ||
                   body.contains("prompt") != old["task"].contains("prompt") ||
                   (body.contains("prompt") &&
                    body["prompt"] != old["task"].value("prompt", J()));
        }
      }
      auto v = version(s, e, k, body, parents);
      if (m == "RestorePrompt")
        v["promptRestoration"] = true;
      persist(d, v, prompt, !stale, !stale);
      outbox(d, v,
             create    ? "MUTATION_KIND_CREATE"
             : resolve ? "MUTATION_KIND_RESOLVE"
                       : "MUTATION_KIND_SAVE");
      changed(d, e);
      if (k == 2 && body["proposal"] != "PROPOSAL_PENDING")
        cancelNotification(d, e);
    }
    J r = {{"result", result(d, e)}};
    if (!repoResult.is_null())
      r["repository"] = repoResult;
    if (stale) {
      J h = J::array();
      for (auto &a : heads(d, e))
        h.push_back(a["versionId"]);
      r["_error"] = {{"reason", "ERROR_REASON_STALE_VERSION"},
                     {"currentHeadIds", h}};
    }
    d.run("INSERT INTO receipts VALUES(?,?,?,?,?)",
          {m, rid, fingerprint, e, r.dump()});
    return r;
  }
  std::pair<size_t, size_t> page(const J &q, const std::string &s,
                                 const std::string &m) {
    auto p = q.value("page", J::object());
    fields(p, {"size", "cursor"});
    int size = p.value("size", 100);
    if (size == 0)
      size = 100;
    require(size > 0 && size <= 1000);
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
  J read(Db &d, const std::string &s, const std::string &m, const J &q) {
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
    if (m == "ListTasks" || m == "ListRepositories") {
      fields(q, {"page", "repositoryId", "progress", "proposal", "search"});
      J all = J::array();
      auto wanted = str(q, "repositoryId"), search = fold(str(q, "search"));
      std::set<std::string> pg, pr;
      for (auto &v : q.value("progress", J::array()))
        pg.insert(enumValue(J{{"v", v}}, "v", "PROGRESS_", progresses));
      for (auto &v : q.value("proposal", J::array()))
        pr.insert(enumValue(J{{"v", v}}, "v", "PROPOSAL_", proposals));
      for (auto &r : d.run(
               "SELECT id FROM entities WHERE kind=? AND deleted=0 ORDER BY id",
               {m == "ListTasks" ? "2" : "1"})) {
        if (m == "ListRepositories" && mapped(d, r[0]) != r[0])
          continue;
        auto item = result(d, r[0]);
        if (m == "ListTasks") {
          auto t = item["version"]["task"];
          if (!wanted.empty() &&
              mapped(d, str(t, "repositoryId")) != mapped(d, wanted))
            continue;
          if (!pg.empty() && !pg.count(str(t, "progress")))
            continue;
          if (!pr.empty() && !pr.count(str(t, "proposal")))
            continue;
          if (!search.empty() &&
              fold(str(t, "title")).find(search) == std::string::npos &&
              fold(str(t, "description")).find(search) == std::string::npos)
            continue;
        }
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
  void validateVersion(Db &d, const J &v) {
    if (v.value("formatVersion", 0) != 1)
      fail("UNSUPPORTED_FORMAT", "Update Lane to read this entity version.");
    fields(v, {"formatVersion", "entityId", "kind", "versionId",
               "parentVersionIds", "createdAt", "origin", "deleted",
               "promptRestoration", "task", "repository"});
    parseTimestamp(str(v, "createdAt"));
    auto o = v.value("origin", J::object());
    fields(o, {"kind", "deviceId", "deviceName", "accountId"});
    enumValue(o, "kind", "ORIGIN_KIND_",
              {"ORIGIN_KIND_APP", "ORIGIN_KIND_CLI", "ORIGIN_KIND_MCP"});
    if (!str(o, "deviceId").empty())
      id(str(o, "deviceId"));
    id(str(v, "entityId"));
    id(str(v, "versionId"));
    require(v.value("kind", "") == "ENTITY_KIND_REPOSITORY" ||
            v.value("kind", "") == "ENTITY_KIND_TASK");
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
      require(v.value("kind", "") == "ENTITY_KIND_TASK" &&
                  !v.value("deleted", false),
              "Restoration marker requires a live task.");
    if (v.value("deleted", false)) {
      require(!v.contains("task") && !v.contains("repository"));
      return;
    }
    if (v["kind"] == "ENTITY_KIND_TASK") {
      require(!v.contains("repository"));
      taskBody(d, v.at("task"));
    } else {
      require(!v.contains("task"));
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
                              {"ENTITY_KIND_REPOSITORY", "ENTITY_KIND_TASK"});
      if (v.contains("origin"))
        v["origin"]["kind"] = enumValue(
            v["origin"], "kind", "ORIGIN_KIND_",
            {"ORIGIN_KIND_APP", "ORIGIN_KIND_CLI", "ORIGIN_KIND_MCP"});
      if (v.contains("task")) {
        v["task"]["progress"] =
            enumValue(v["task"], "progress", "PROGRESS_", progresses);
        v["task"]["proposal"] =
            enumValue(v["task"], "proposal", "PROPOSAL_", proposals);
      }
      if (v.value("formatVersion", 0) != 1)
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
      if (v.value("kind", "") == "ENTITY_KIND_TASK") {
        auto repo = mapped(d, str(v.at("task"), "repositoryId"));
        auto rh = heads(d, repo);
        if (!rh.empty() && rh[0].value("deleted", false)) {
          J dead = v;
          dead.erase("task");
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
                     v["task"].value("prompt", J());
        }
      }
      prompt = prompt || v.value("promptRestoration", false);
      if (v["kind"] == "ENTITY_KIND_REPOSITORY")
        uniqueURL(d, str(v["repository"], "normalizedUrl"), e);
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
        int k = only["kind"] == "ENTITY_KIND_REPOSITORY" ? 1 : 2;
        d.run("UPDATE entities SET repository_id=?,normalized_url=? WHERE id=?",
              {k == 2 ? mapped(d, str(only["task"], "repositoryId")) : "",
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
      d.run("UPDATE entities SET repository_id=? WHERE kind=2 AND "
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
    auto q = canonical(parse(text));
    fields(q, {"formatVersion", "spaceId"});
    if (q.value("formatVersion", 0) != 1)
      fail("UNSUPPORTED_FORMAT", "Update Lane to use this storage format.");
    auto s = str(q, "spaceId");
    if (s.empty())
      s = impl_->active();
    auto &d = impl_->db(s);
    ++impl_->spaces[s].handles;
    auto seq = sequence(d);
    return success({{"spaceId", s},
                    {"deviceId", impl_->device()},
                    {"schemaVersion", 1},
                    {"formatVersion", 1},
                    {"sequence", seq}},
                   seq);
  });
}
std::string Store::close(std::string s) {
  return protect([&] {
    impl_->registered(s);
    auto it = impl_->spaces.find(s);
    if (it != impl_->spaces.end() && it->second.handles > 0)
      --it->second.handles;
    return success({{"completed", true}});
  });
}
std::string Store::poll() {
  J events = J::array();
  try {
    for (auto &r :
         impl_->registry->run("SELECT id FROM spaces WHERE sync_enabled=1 AND "
                              "account_key NOT LIKE 'removed:%'"))
      impl_->db(r[0]);
    for (auto &[s, slot] : impl_->spaces) {
      if (slot.handles == 0 && !impl_->enabled(s))
        continue;
      impl_->registered(s);
      auto &d = *slot.observer;
      compatibility(d);
      auto seq = sequence(d);
      if (seq != slot.seen) {
        slot.seen = seq;
        events.push_back({{"spaceId", s}, {"sequence", seq}});
      }
    }
  } catch (...) { /* Retry on the next host tick; never emit an uncommitted
                     sequence. */
  }
  return events.dump();
}
std::string Store::invoke(std::string m, std::string text) {
  auto started = now();
  auto response = protect([&] {
    auto outer = canonical(parse(text));
    fields(outer, {"spaceId", "request"});
    auto s = str(outer, "spaceId");
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
        "ResolveConflict"};
    static const std::set<std::string> reads = {"GetRepository",
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
      if (retention == "LOCAL_RETENTION_REMOVE_LOCAL") {
        // Mark unavailable before clearing pages. Existing handles in other
        // processes check registry routing before their next operation. Do not
        // unlink a live DB.
        Tx rt(*impl_->registry);
        impl_->registry->run("UPDATE spaces SET "
                             "account_key=?,account_id=NULL,tenant_id=NULL,"
                             "issuer=NULL WHERE id=?",
                             {"removed:" + s, s});
        if (impl_->active() == s) {
          auto local = impl_->registry->one(
              "SELECT id FROM spaces WHERE account_key='local'");
          if (local.empty()) {
            local = uuid();
            impl_->registry->run(
                "INSERT INTO spaces(id,account_key) VALUES(?,'local')",
                {local});
          }
          impl_->registry->run(
              "UPDATE registry_meta SET value=? WHERE key='active_space_id'",
              {local});
        }
        rt.commit();
        Tx t(d);
        d.exec("DELETE FROM heads; DELETE FROM versions; DELETE FROM entities; "
               "DELETE FROM receipts; DELETE FROM outbox; DELETE FROM "
               "remote_objects; DELETE FROM repository_mappings; DELETE FROM "
               "notification_receipts; DELETE FROM store_meta WHERE key NOT IN "
               "('space_id','device_id','format_version');");
        changed(d, "", 3);
        t.commit();
        d.exec("PRAGMA wal_checkpoint(TRUNCATE)");
        impl_->spaces.erase(s);
      }
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
            d.exec("DELETE FROM heads;DELETE FROM versions;DELETE FROM "
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
  writeLog(impl_->root, "Invoke", parse(response), now() - started);
  return response;
}
std::string Store::approveAccount(std::string text, bool merge) {
  return protect([&] {
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
      compatibility(d); // Copy immutable standalone snapshots through the same
                        // conflict/deletion rules.
      for (auto &r : source.run("SELECT envelope_json FROM versions ORDER BY "
                                "created_at_us,version_id")) {
        auto v = parse(r[0]);
        if (v["kind"] == "ENTITY_KIND_REPOSITORY" &&
            !v.value("deleted", false)) {
          auto url = str(v["repository"], "normalizedUrl");
          auto other = d.one("SELECT id FROM entities WHERE kind=1 AND "
                             "normalized_url=? AND deleted=0",
                             {url});
          if (!other.empty() && other != str(v, "entityId")) {
            impl_->mappings(d, J::array({J{{"localId", str(v, "entityId")},
                                           {"canonicalId", other}}}));
          }
        }
        impl_->applyEntity(d, {{"version", v}});
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
      if (meta(d, "account_epoch").empty())
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
    auto &d = impl_->db(s);
    Tx t(d, false);
    impl_->lease(d, owner);
    J mutations = J::array();
    // Returning no uploads during an epoch mismatch makes stale reupload
    // impossible even if a host forgets its GetSyncInfo comparison.
    bool epochReady = meta(d, "approved_epoch", meta(d, "account_epoch")) ==
                          meta(d, "account_epoch") &&
                      meta(d, "reconnect_required", "0") != "1";
    for (auto &r : d.run("SELECT request_json FROM outbox ORDER BY "
                         "created_at_us,request_id"))
      if (epochReady)
        mutations.push_back(parse(r[0]));
    J r = {{"formatVersion", 1},
           {"epoch", meta(d, "account_epoch")},
           {"deviceId", impl_->device()},
           {"mutations", mutations},
           {"cursor", meta(d, "pull_cursor")}};
    t.commit();
    return success(r, sequence(d));
  });
}
std::string Store::applySync(std::string s, std::string text) {
  return protect([&] {
    auto q = canonical(parse(text));
    fields(q, {"formatVersion", "epoch", "owner", "push", "pull"});
    if (q.value("formatVersion", 0) != 1)
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
            str(r["error"], "reason") != "ERROR_REASON_UNSPECIFIED") {
          d.run("UPDATE outbox SET state=2,error_reason=? WHERE request_id=?",
                {str(r["error"], "reason"), rid});
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
            d.exec("DELETE FROM heads;DELETE FROM versions;DELETE FROM "
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
                s, e, kind == "ENTITY_KIND_OR_REMOTE_KIND_REPOSITORY" ? 1 : 2,
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
    auto &d = impl_->db(s);
    Tx t(d, false);
    auto items = notificationItems(*impl_, d);
    t.commit();
    return success({{"items", items}}, sequence(d));
  });
}
std::string Store::claimNotifications(std::string s, std::string claimant) {
  return protect([&] {
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
      d.exec("DELETE FROM outbox;");
      for (auto &r : d.run("SELECT envelope_json FROM versions ORDER BY "
                           "created_at_us,version_id")) {
        auto v = parse(r[0]);
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
