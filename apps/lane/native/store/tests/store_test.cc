// SPDX-License-Identifier: Apache-2.0
#include "../lane_store.h"
#include "../vendor/json/json.hpp"
#include "../vendor/sqlite/sqlite3.h"

#include <algorithm>
#include <chrono>
#include <filesystem>
#include <functional>
#include <future>
#include <atomic>
#include <thread>
#include <fstream>
#include <map>
#if defined(__unix__) || defined(__APPLE__)
#include <unistd.h>
#include <sys/wait.h>
#endif
#include "../vendor/picosha2/picosha2.h"
#include <iomanip>
#include <iostream>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
using J = nlohmann::json;
namespace fs = std::filesystem;
void require(bool condition, const std::string& message) {
  if (!condition) throw std::runtime_error(message);
}
std::string uuid() {
  static unsigned long long next = 1;
  std::ostringstream out;
  out << "01900000-0000-7000-8000-" << std::hex << std::setfill('0')
      << std::setw(12) << next++;
  return out.str();
}
J success(const std::string& raw) {
  auto value = J::parse(raw);
  require(value.value("ok", false), "expected success: " + value.dump());
  return value.at("response");
}
void error(const J& value, const std::string& reason) {
  require(!value.value("ok", true), "expected " + reason + ": " + value.dump());
  const auto actual = value.at("error").at("reason").get<std::string>();
  require(actual == reason || actual == "ERROR_REASON_" + reason,
          "expected " + reason + ": " + value.dump());
}
struct TempRoot {
  fs::path path;
  TempRoot() {
    const auto tick = std::chrono::steady_clock::now().time_since_epoch().count();
    for (unsigned i = 0; i < 100; ++i) {
      path = fs::temp_directory_path() /
          ("lane-engine-test-" + std::to_string(tick) + "-" + std::to_string(i));
      if (fs::create_directory(path)) return;
    }
    throw std::runtime_error("cannot allocate explicit test root");
  }
  ~TempRoot() { std::error_code ec; fs::remove_all(path, ec); }
};
struct Sql {
  sqlite3* db = nullptr;
  explicit Sql(const fs::path& path) {
    require(sqlite3_open(path.string().c_str(), &db) == SQLITE_OK, "open fixture SQL");
    exec("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;");
  }
  ~Sql() { sqlite3_close(db); }
  void exec(const std::string& sql) {
    char* message = nullptr;
    const int rc = sqlite3_exec(db, sql.c_str(), nullptr, nullptr, &message);
    std::string detail = message ? message : "";
    sqlite3_free(message);
    require(rc == SQLITE_OK, "fixture SQL: " + detail);
  }
  std::string scalar(const std::string& sql) {
    sqlite3_stmt* stmt = nullptr;
    require(sqlite3_prepare_v2(db, sql.c_str(), -1, &stmt, nullptr) == SQLITE_OK,
            "prepare fixture query");
    const int rc = sqlite3_step(stmt);
    std::string result;
    if (rc == SQLITE_ROW && sqlite3_column_text(stmt, 0))
      result = reinterpret_cast<const char*>(sqlite3_column_text(stmt, 0));
    sqlite3_finalize(stmt);
    require(rc == SQLITE_ROW || rc == SQLITE_DONE, "read fixture query");
    return result;
  }
};
struct Fixture {
  TempRoot root;
  lane::Store store{root.path.string()};
  std::string space, device;
  Fixture() {
    const auto opened = success(store.open(R"({"formatVersion":3,"spaceId":""})"));
    space = opened.at("spaceId"); device = opened.at("deviceId");
    require(opened.at("schemaVersion") == 3 && opened.at("formatVersion") == 3,
            "open version metadata");
  }
  fs::path database() const { return root.path / "spaces" / space / "lane.sqlite"; }
  J meta(const std::string& kind = "APP") {
    return {{"requestId", uuid()}, {"origin", {{"kind", "ORIGIN_KIND_" + kind},
             {"deviceId", device}, {"deviceName", "Synthetic test device"}}}};
  }
  J raw(const std::string& method, const J& request = J::object()) {
    return J::parse(store.invoke(method, J{{"spaceId", space}, {"request", request}}.dump()));
  }
  J call(const std::string& method, const J& request = J::object()) {
    auto result = raw(method, request);
    require(result.value("ok", false), method + ": " + result.dump());
    require(result.at("sequence").is_string(), "sequence must be decimal string");
    return result.at("response");
  }
  J createRequest(const std::string& title = "Synthetic task",
                  const std::string& url = "https://example.com/test/repo") {
    return {{"meta", meta()}, {"id", uuid()}, {"repository", {{"url", url}}},
            {"title", title}};
  }
  J task(const J& request) { return call("CreateTask", request).at("result").at("version"); }
  J task() { return task(createRequest()); }
  J get(const std::string& id) { return call("GetTask", {{"id", id}}).at("result").at("version"); }
  J save(const J& version, J body) {
    return call("SaveTask", {{"meta", meta()}, {"id", version.at("entityId")},
        {"expectedVersionId", version.at("versionId")}, {"task", body}}).at("result").at("version");
  }
  J history(const J& version, bool prompts = false) {
    return call("ListHistory", {{"id", version.at("entityId")}, {"promptsOnly", prompts}})
        .value("versions", J::array());
  }
};
J items(const J& response) { return response.value("items", J::array()); }
std::vector<std::string> counts(Sql& db) {
  std::vector<std::string> result;
  for (const auto* table : {"entities", "versions", "heads", "receipts", "outbox", "changes", "notification_receipts"})
    result.push_back(db.scalar(std::string("SELECT count(*) FROM ") + table));
  return result;
}
void crud_replay_persistence() {
  Fixture f;
  auto request = f.createRequest("  Exact title  ");
  auto v = f.task(request);
  require(v.at("task").at("title") == "  Exact title  ", "preserve title");
  require(v.at("task").at("progress") == "PROGRESS_TODO", "default progress");
  require(v.at("task").at("proposal") == "PROPOSAL_REGULAR", "default proposal");
  require(!v.at("task").contains("prompt"), "absent prompt");
  require(f.history(v).size() == 1, "creation history");
  request["repository"]["url"] = "ssh://git@example.com:22/test/repo.git/";
  require(f.task(request).at("versionId") == v.at("versionId"), "normalized replay identity");
  require(f.history(v).size() == 1, "replay cannot append history");
  request["title"] = "Changed";
  error(f.raw("CreateTask", request), "REQUEST_ID_REUSED");
  auto body = v.at("task"); body["description"] = "# Exact Markdown\n\n**source**\n";
  v = f.save(v, body);
  require(v.at("task") == body, "full snapshot save");
  auto same = f.save(v, body);
  require(same.at("versionId") != v.at("versionId"), "explicit equal save adds version");
  require(f.history(same).size() == 3, "save history");
  require(items(f.call("ListTasks")).size() == 1, "list task");
  require(items(f.call("ListRepositories")).size() == 1, "atomic repository creation");
  success(f.store.close(f.space));
  lane::Store reopened(f.root.path.string());
  auto opened = success(reopened.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  require(opened.at("deviceId") == f.device, "persistent device");
  auto result = success(reopened.invoke("GetTask", J{{"spaceId", f.space}, {"request", {{"id", v.at("entityId")}}}}.dump()));
  require(result.at("result").at("version") == same, "durable reopen");
  error(J::parse(reopened.open(J{{"formatVersion", 3}, {"spaceId", uuid()}}.dump())), "NOT_FOUND");
}
void transitions_and_proposals() {
  Fixture f;
  const std::vector<std::string> states = {"TODO", "IN_PROGRESS", "BLOCKED", "DONE", "CANCELED"};
  for (const auto& from : states) for (const auto& to : states) {
    auto request = f.createRequest(); request["progress"] = "PROGRESS_" + from;
    auto v = f.task(request);
    auto next = f.call("SetProgress", {{"meta", f.meta()}, {"id", v.at("entityId")},
        {"expectedVersionId", v.at("versionId")}, {"progress", "PROGRESS_" + to}}).at("result").at("version");
    require(next.at("task").at("progress") == "PROGRESS_" + to, "progress transition");
    require(next.at("task").at("proposal") == "PROPOSAL_REGULAR", "proposal preserved");
    require(next.at("versionId") != v.at("versionId") && f.history(next).size() == 2,
            "every transition including identity creates history");
  }
  auto request = f.createRequest(); request["proposal"] = "PROPOSAL_PENDING";
  request["progress"] = "PROGRESS_BLOCKED";
  auto v = f.task(request);
  for (const auto* proposal : {"PROPOSAL_REJECTED", "PROPOSAL_ACCEPTED"}) {
    v = f.call("SetProposal", {{"meta", f.meta()}, {"id", v.at("entityId")},
        {"expectedVersionId", v.at("versionId")}, {"proposal", proposal}}).at("result").at("version");
    require(v.at("task").at("progress") == "PROGRESS_BLOCKED", "disposition preserves progress");
    require(v.at("task").at("proposal") == proposal, "disposition stored");
  }
  require(f.history(v).size() == 3, "proposal decisions retained");
  auto regular = f.task(); auto body = regular.at("task"); body["proposal"] = "PROPOSAL_PENDING";
  require(!f.raw("SaveTask", {{"meta", f.meta()}, {"id", regular.at("entityId")},
      {"expectedVersionId", regular.at("versionId")}, {"task", body}}).at("ok").get<bool>(), "save cannot create proposal");
  for (const auto* proposal : {"PROPOSAL_ACCEPTED", "PROPOSAL_REJECTED", "PROPOSAL_UNKNOWN"}) {
    request = f.createRequest(); request["proposal"] = proposal;
    error(f.raw("CreateTask", request), "INVALID_INPUT");
  }
}
void prompt_history_restore() {
  Fixture f;
  auto absent = f.task();
  auto body = absent.at("task"); body["prompt"] = "";
  auto empty = f.save(absent, body);
  body["prompt"] = "one\n"; auto one = f.save(empty, body);
  body["title"] = "Only title changes"; auto title = f.save(one, body);
  require(!absent.value("promptRestoration", false) && !empty.value("promptRestoration", false) &&
      !one.value("promptRestoration", false) && !title.value("promptRestoration", false), "ordinary saves do not claim restoration");
  const auto prompt_count = f.history(title, true).size();
  require(prompt_count == f.history(one, true).size(), "title save not prompt history");
  auto restore = [&](const J& current, const J& source) {
    return f.call("RestorePrompt", {{"meta", f.meta()}, {"id", current.at("entityId")},
        {"expectedVersionId", current.at("versionId")}, {"sourceVersionId", source.at("versionId")}}).at("result").at("version");
  };
  auto restored = restore(title, empty);
  require(restored.at("task").at("prompt") == "", "restore empty presence");
  require(restored.at("task").at("title") == "Only title changes", "restore only prompt");
  require(f.history(restored, true).size() == prompt_count + 1, "restore prompt history");
  auto equal = restore(restored, empty);
  require(f.history(equal, true).size() == prompt_count + 2, "equal restore creates prompt version");
  auto removed = restore(equal, absent);
  require(!removed.at("task").contains("prompt"), "restore absence");
  require(restored.value("promptRestoration", false) && equal.value("promptRestoration", false) && removed.value("promptRestoration", false), "all explicit restores carry immutable restoration marker");
  auto other = f.task();
  error(f.raw("RestorePrompt", {{"meta", f.meta()}, {"id", removed.at("entityId")},
      {"expectedVersionId", removed.at("versionId")}, {"sourceVersionId", other.at("versionId")}}), "NOT_FOUND");
}
void conflicts_and_resolution() {
  Fixture f; auto base = f.task(); auto body = base.at("task");
  body["title"] = "Title alternative"; auto left = f.save(base, body);
  body = base.at("task"); body["prompt"] = "Prompt alternative";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", base.at("entityId")},
      {"expectedVersionId", base.at("versionId")}, {"task", body}}), "STALE_VERSION");
  auto result = f.call("GetTask", {{"id", base.at("entityId")}}).at("result");
  require(result.at("result") == "RESULT_KIND_CONFLICT", "whole-task conflict");
  auto alternatives = result.at("alternatives");
  require(alternatives.size() == 2, "both complete alternatives");
  std::set<std::string> heads; bool found_left = false, found_right = false;
  for (const auto& alternative : alternatives) {
    heads.insert(alternative.at("versionId").get<std::string>());
    found_left |= alternative.at("task") == left.at("task");
    found_right |= alternative.at("task") == body;
  }
  require(found_left && found_right, "do not merge distinct fields");
  error(f.raw("SetProgress", {{"meta", f.meta()}, {"id", base.at("entityId")},
      {"expectedVersionId", left.at("versionId")}, {"progress", "PROGRESS_DONE"}}), "CONFLICT_READ_ONLY");
  J resolved = left; resolved["versionId"] = uuid();
  resolved["parentVersionIds"] = heads; resolved["task"]["prompt"] = "Resolved prompt";
  J request = {{"meta", f.meta()}, {"id", base.at("entityId")},
               {"expectedHeadIds", heads}, {"resolved", resolved}};
  auto winner = f.call("ResolveConflict", request).at("result").at("version");
  require(winner.at("task") == resolved.at("task"), "resolved whole task");
  require(f.history(winner).size() == 4, "retain alternatives and resolution");
  const auto before = f.history(winner);
  request["meta"] = f.meta(); request["resolved"]["versionId"] = uuid();
  error(f.raw("ResolveConflict", request), "STALE_RESOLUTION");
  require(f.history(winner) == before && f.get(base.at("entityId")) == winner,
          "stale resolution does not overwrite or append");
  auto prompts = f.history(winner, true); bool has_resolution = false;
  for (const auto& p : prompts) has_resolution |= p.at("versionId") == winner.at("versionId");
  require(has_resolution, "prompt change in resolution appears in prompt history");
}
void deletion_move_and_replay() {
  Fixture f; auto request = f.createRequest(); request["prompt"] = "Deleted secret fixture";
  auto v = f.task(request); const auto id = v.at("entityId");
  auto deleted = f.call("DeleteTask", {{"meta", f.meta()}, {"id", id}}).at("result");
  require(deleted.at("result") == "RESULT_KIND_DELETED", "delete result");
  auto replay = f.call("CreateTask", request).at("result");
  require(replay.at("result") == "RESULT_KIND_DELETED", "deleted creation replay");
  require(replay.dump().find("Deleted secret fixture") == std::string::npos, "no replay body");
  auto delayed = f.raw("SaveTask", {{"meta", f.meta()}, {"id", id},
      {"expectedVersionId", v.at("versionId")}, {"task", v.at("task")}});
  require((delayed.value("ok", false) && delayed.at("response").at("result").at("result") == "RESULT_KIND_DELETED") ||
          (!delayed.value("ok", true) && delayed.at("error").at("reason").get<std::string>().find("DELETED") != std::string::npos),
          "late save must not resurrect");
  Sql db(f.database());
  for (const auto* column : {"SELECT envelope_json FROM versions", "SELECT result_json FROM receipts", "SELECT request_json FROM outbox"}) {
    const std::string query = "SELECT count(*) FROM (" + std::string(column) +
        ") WHERE " + (std::string(column).find("envelope") != std::string::npos ? "envelope_json" :
          std::string(column).find("result") != std::string::npos ? "result_json" : "request_json") +
        " LIKE '%Deleted secret fixture%'";
    require(db.scalar(query) == "0", "deletion redacts durable content");
  }
  auto moving = f.task(); auto staying = f.task();
  auto old_repo = moving.at("task").at("repositoryId");
  auto new_repo = f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()},
      {"url", "https://example.com/other/repo"}}).at("result").at("version");
  auto body = moving.at("task"); body["repositoryId"] = new_repo.at("entityId");
  auto moved = f.save(moving, body);
  f.call("DeleteRepository", {{"meta", f.meta()}, {"id", old_repo}});
  require(f.get(moving.at("entityId")) == moved && f.history(moved).size() == 2,
          "moved task survives repository cascade with history");
  auto cascade = f.call("GetTask", {{"id", staying.at("entityId")}}).at("result");
  require(cascade.at("result") == "RESULT_KIND_DELETED", "cascade remaining task");
}
void urls_unicode_and_validation() {
  Fixture f;
  const std::vector<std::pair<std::string, std::string>> urls = {
    {"https://GitHub.com/Acme/Repo.git", "github.com/Acme/Repo"},
    {"https://example.com:8443/a/b.git", "example.com:8443/a/b"},
    {"https://example.com/a/%62.git", "example.com/a/b"},
    {"https://bücher.example/Path/Repo", "xn--bcher-kva.example/Path/Repo"},
    {"https://example.com/한글/Repo", "example.com/한글/Repo"}};
  for (const auto& pair : urls) {
    auto r = f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()}, {"url", pair.first}});
    require(r.at("result").at("version").at("repository").at("normalizedUrl") == pair.second,
            "URL normalization: " + pair.first);
  }
  Sql db(f.database()); const auto before = counts(db);
  for (const auto* url : {"https://example.com/a/../b", "https://example.com/a%2Fb", "https://example.com/a%5cb",
       "https://example.com/a%zz", "https://example.com/a?x=1", "https://example.com/a#f", "https://example.com/", "https://example.com/a\\b"})
    error(f.raw("CreateRepository", {{"meta", f.meta()}, {"id", uuid()}, {"url", url}}), "INVALID_INPUT");
  auto invalid = f.createRequest(" \t\n"); error(f.raw("CreateTask", invalid), "INVALID_INPUT");
  invalid = f.createRequest(); invalid["progress"] = "PROGRESS_UNKNOWN";
  error(f.raw("CreateTask", invalid), "INVALID_INPUT");
  require(counts(db) == before, "invalid input leaves no partial writes");
  auto v = f.task(f.createRequest("Straße 한글 [literal]"));
  require(items(f.call("ListTasks", {{"search", "STRASSE"}})).size() == 1, "Unicode case folding");
  require(items(f.call("ListTasks", {{"search", "[literal]"}})).size() == 1, "literal search");
  require(items(f.call("ListTasks", {{"search", ".*"}})).empty(), "no regex search");
  auto repo_id = v.at("task").at("repositoryId");
  auto repo = f.call("GetRepository", {{"id", repo_id}}).at("result").at("version");
  auto renamed = f.call("SaveRepository", {{"meta", f.meta()}, {"id", repo_id},
      {"expectedVersionId", repo.at("versionId")}, {"url", "https://example.com/new/repo"},
      {"displayName", "New display"}}).at("result").at("version");
  require(renamed.at("entityId") == repo_id, "rename retains identity");
  auto new_task = f.task();
  require(new_task.at("task").at("repositoryId") != repo_id, "old URL released immediately");
  require(f.get(v.at("entityId")).at("task").at("repositoryId") == repo_id, "rename retains tasks");
  auto upper = f.task(f.createRequest("Upper", "https://case.example/Repo"));
  auto lower = f.task(f.createRequest("Lower", "https://case.example/repo"));
  require(upper.at("task").at("repositoryId") != lower.at("task").at("repositoryId"), "path case sensitive");
}
void observation_and_rollback() {
  Fixture f; auto v = f.task();
  f.store.poll();
  Sql db(f.database());
  const auto before = counts(db);
  db.exec("BEGIN IMMEDIATE; INSERT INTO changes(entity_id,change_kind) VALUES(NULL,3);");
  require(J::parse(f.store.poll()).empty(), "uncommitted external change invisible");
  db.exec("ROLLBACK;");
  require(J::parse(f.store.poll()).empty(), "rollback emits no event");
  require(counts(db) == before, "external rollback preserves tables");
  db.exec("BEGIN IMMEDIATE; INSERT INTO changes(entity_id,change_kind) VALUES(NULL,3); COMMIT;");
  auto events = J::parse(f.store.poll());
  require(events.is_array() && events.size() == 1, "observe external commit");
  require(events[0].at("spaceId") == f.space && events[0].at("sequence").is_string(), "event identity");
  require(events[0].size() == 2, "event carries no content");
  require(events[0].at("sequence") == db.scalar("SELECT max(sequence) FROM changes"), "committed sequence");
  require(J::parse(f.store.poll()).empty(), "no repeated unchanged invalidation");
  lane::Store other(f.root.path.string());
  success(other.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  auto request = f.createRequest("Second writer");
  success(other.invoke("CreateTask", J{{"spaceId", f.space}, {"request", request}}.dump()));
  require(!J::parse(f.store.poll()).empty(), "second engine writer observed");
  require(f.get(request.at("id")).at("task").at("title") == "Second writer", "second writer content visible");
  // Abort after snapshot work, at the durable outbox boundary. All affected
  // tables must roll back together; the same request remains safe to retry.
  db.exec("CREATE TRIGGER test_outbox_failure BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;");
  const auto failed_before = counts(db);
  f.store.poll();
  auto body = v.at("task"); body["title"] = "Must roll back";
  J save_request = {{"meta", f.meta()}, {"id", v.at("entityId")},
      {"expectedVersionId", v.at("versionId")}, {"task", body}};
  require(!f.raw("SaveTask", save_request).at("ok").get<bool>(), "injected failure reported");
  require(counts(db) == failed_before && f.get(v.at("entityId")) == v, "snapshot/outbox/receipt/sequence atomicity");
  require(J::parse(f.store.poll()).empty(), "failed write emits no invalidation");
  db.exec("DROP TRIGGER test_outbox_failure;");
  f.call("SaveTask", save_request);
  require(f.get(v.at("entityId")).at("task").at("title") == "Must roll back", "retry after rollback");
}
// Simulate the authenticated sync adapter, never the untrusted UI origin field.
struct RemoteProposals {
  Fixture& f;
  std::string epoch = uuid(), owner = uuid(), remote_device = uuid();
  J repository;
  explicit RemoteProposals(Fixture& fixture) : f(fixture) {
    success(f.store.approveAccount(J{{"issuer", "https://identity.example.test"},
        {"accountId", uuid()}, {"tenantId", uuid()}, {"epoch", epoch}}.dump(), false));
    f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
    success(f.store.acquireSync(f.space, owner));
    repository = f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()},
        {"url", "https://example.test/remote/proposals"}}).at("result").at("version");
  }
  void apply(const J& version) {
    success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", epoch}, {"owner", owner},
        {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"entity", {
            {"result", "RESULT_KIND_APPLIED"}, {"version", version}}}}})},
            {"nextCursor", "synthetic-proposal-cursor"}, {"hasMore", false}}}}.dump()));
  }
  J create() {
    J version = {{"formatVersion", 3}, {"entityId", uuid()}, {"kind", "ENTITY_KIND_TASK"},
        {"versionId", uuid()}, {"parentVersionIds", J::array()}, {"createdAt", "2026-01-01T00:00:00Z"},
        {"origin", {{"kind", "ORIGIN_KIND_MCP"}, {"deviceId", remote_device},
            {"deviceName", "Synthetic remote MCP"}, {"accountId", ""}}}, {"deleted", false},
        {"task", {{"repositoryId", repository.at("entityId")}, {"title", "Remote proposal"},
            {"description", ""}, {"progress", "PROGRESS_TODO"}, {"proposal", "PROPOSAL_PENDING"}}}};
    apply(version);
    return version;
  }
};
void preferences_and_notifications() {
  Fixture f; RemoteProposals remote(f); int prompts = 0;
  lane::HostHooks hooks;
  hooks.requestNotificationPermission = [&] { ++prompts; return "NOTIFICATION_PERMISSION_GRANTED"; };
  f.store.setHostHooks(hooks);
  f.call("GetNotificationPreferences"); f.task();
  require(prompts == 0, "no implicit permission request");
  f.call("SetNotificationPreferences", {{"enabled", true}, {"content", "NOTIFICATION_CONTENT_GENERIC"}});
  require(prompts == 1, "explicit enablement permission request");
  auto preferences = f.call("GetNotificationPreferences");
  require(preferences.at("enabled") == true && preferences.at("content") == "NOTIFICATION_CONTENT_GENERIC" &&
          preferences.at("permission") == "NOTIFICATION_PERMISSION_GRANTED", "preferences read back");
  auto proposal = remote.create();
  remote.apply(proposal);
  Sql db(f.database());
  const auto id = proposal.at("entityId").get<std::string>();
  require(db.scalar("SELECT count(*) FROM notification_receipts WHERE proposal_id='" + id + "'") == "1", "notification replay dedupe");
  // Native-only notification return fields are intentionally not assumed here.
  f.store.pendingNotifications(f.space);
  success(f.store.acknowledgeNotification(f.space, id));
  require(db.scalar("SELECT state FROM notification_receipts WHERE proposal_id='" + id + "'") == "2", "delivery acknowledged durably");
  auto cancel = remote.create();
  f.call("SetProposal", {{"meta", f.meta()}, {"id", cancel.at("entityId")},
      {"expectedVersionId", cancel.at("versionId")}, {"proposal", "PROPOSAL_REJECTED"}});
  f.store.pendingNotifications(f.space);
  require(db.scalar("SELECT count(*) FROM notification_receipts WHERE state=1 AND proposal_id='" +
      cancel.at("entityId").get<std::string>() + "'") == "0", "rejected notification canceled");
  const auto count = db.scalar("SELECT count(*) FROM notification_receipts");
  for (const auto* origin : {"APP", "CLI", "MCP"}) {
    auto request = f.createRequest(); request["meta"] = f.meta(origin); request["proposal"] = "PROPOSAL_PENDING";
    request["meta"]["origin"]["deviceId"] = uuid();
    request["meta"]["origin"]["accountId"] = "forged-account";
    auto created = f.task(request);
    require(created.at("origin").at("kind") == "ORIGIN_KIND_APP" &&
            created.at("origin").at("deviceId") == f.device &&
            created.at("origin").at("accountId") != "forged-account", "UI cannot forge adapter origin");
  }
  require(db.scalar("SELECT count(*) FROM notification_receipts") == count, "only MCP proposals notify");
  hooks.requestNotificationPermission = [&] { ++prompts; return "NOTIFICATION_PERMISSION_DENIED"; };
  f.store.setHostHooks(hooks);
  f.call("SetNotificationPreferences", {{"enabled", false}, {"content", "NOTIFICATION_CONTENT_DETAIL"}});
  require(prompts == 1, "disable never requests permission");
  f.call("SetNotificationPreferences", {{"enabled", true}, {"content", "NOTIFICATION_CONTENT_DETAIL"}});
  require(f.call("GetNotificationPreferences").at("permission") == "NOTIFICATION_PERMISSION_DENIED", "denial visible");
}
void concurrent_notification_claims() {
  Fixture f; RemoteProposals remote(f);
  lane::HostHooks hooks;
  hooks.requestNotificationPermission = [] { return "NOTIFICATION_PERMISSION_GRANTED"; };
  f.store.setHostHooks(hooks);
  f.call("SetNotificationPreferences", {{"enabled", true}, {"content", "NOTIFICATION_CONTENT_DETAIL"}});
  auto proposal = remote.create();
  lane::Store other(f.root.path.string());
  success(other.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  std::atomic<int> ready{0};
  auto claim = [&](lane::Store& store) {
    const auto claimant = uuid();
    return std::async(std::launch::async, [&store, &ready, &f, claimant] {
      ++ready;
      while (ready.load() != 2) std::this_thread::yield();
      return success(store.claimNotifications(f.space, claimant)).at("items");
    });
  };
  auto first = claim(f.store); auto second = claim(other);
  auto a = first.get(), b = second.get();
  require(a.size() + b.size() == 1, "two concurrent stores claim once");
  const auto claimed = a.empty() ? b.at(0) : a.at(0);
  require(claimed.at("proposalId") == proposal.at("entityId") && claimed.at("deviceId") == f.device,
          "OS dedupe identity preserved in claim");
  require(success(other.claimNotifications(f.space, uuid())).at("items").empty(), "active claim excluded");
  success(f.store.acknowledgeNotification(f.space, proposal.at("entityId")));
  remote.apply(proposal);
  require(success(other.claimNotifications(f.space, uuid())).at("items").empty(), "remote replay after delivery cannot notify again");
  auto pending = remote.create();
  success(f.store.recordNotificationPermission(f.space, "NOTIFICATION_PERMISSION_DENIED"));
  require(success(other.claimNotifications(f.space, uuid())).at("items").empty(), "other store rechecks revoked permission");
  success(f.store.recordNotificationPermission(f.space, "NOTIFICATION_PERMISSION_GRANTED"));
  f.call("SetNotificationPreferences", {{"enabled", false}, {"content", "NOTIFICATION_CONTENT_DETAIL"}});
  require(success(other.claimNotifications(f.space, uuid())).at("items").empty(), "other store rechecks disabled preference");
  f.call("SetNotificationPreferences", {{"enabled", true}, {"content", "NOTIFICATION_CONTENT_DETAIL"}});
  f.call("SetProposal", {{"meta", f.meta()}, {"id", pending.at("entityId")},
      {"expectedVersionId", pending.at("versionId")}, {"proposal", "PROPOSAL_ACCEPTED"}});
  require(success(other.claimNotifications(f.space, uuid())).at("items").empty(), "claim rechecks proposal disposition");
  Sql db(f.database());
  require(db.scalar("SELECT count(*) FROM notification_receipts") == "2", "remote receipt dedupe persists across stores");
}
J identity(const std::string& account, const std::string& tenant, const std::string& epoch) {
  return {{"issuer", "https://identity.example.test"}, {"accountId", account},
          {"tenantId", tenant}, {"epoch", epoch}};
}
void accounts_and_host_hooks() {
  Fixture f; auto local = f.task(); const auto standalone = f.space;
  int authorizations = 0, disconnects = 0, remote_calls = 0;
  lane::HostHooks hooks;
  hooks.authorize = [&](std::string account) {
    require(account.empty(), "first approval account input"); ++authorizations;
    return J{{"authorizationUrl", "https://identity.example.test/approve"}, {"challengeId", "synthetic-challenge"}}.dump();
  };
  hooks.disconnect = [&](std::string space) { require(space == f.space, "disconnect pinned space"); ++disconnects; };
  f.store.setHostHooks(hooks);
  auto connection = f.call("ConnectSync", {{"accountId", ""}});
  require(authorizations == 1 && connection.at("challengeId") == "synthetic-challenge", "authorization hook");
  const auto account_a = identity(uuid(), uuid(), uuid());
  success(f.store.approveAccount(account_a.dump(), false));
  auto opened = success(f.store.open(R"({"formatVersion":3,"spaceId":""})"));
  f.space = opened.at("spaceId"); const auto first = f.space;
  require(first != standalone && items(f.call("ListTasks")).empty(), "approval without merge isolates standalone");
  auto same_request = f.createRequest("Account-scoped ID"); auto a = f.task(same_request);
  hooks.remote = [&](std::string space, std::string method, std::string request) {
    require(space == first && method == "ListDevices", "typed remote hook routing");
    require(J::parse(request).is_object(), "typed remote request"); ++remote_calls;
    return R"({"items":[]})";
  };
  f.store.setHostHooks(hooks);
  require(items(f.call("ListDevices")).empty() && remote_calls == 1, "online dispatch");
  f.call("DisconnectSync", {{"retention", "LOCAL_RETENTION_KEEP_LOCAL"}});
  require(disconnects == 1 && f.get(a.at("entityId")) == a, "keep local disconnect");
  const auto account_b = identity(uuid(), uuid(), uuid());
  success(f.store.approveAccount(account_b.dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
  require(f.space != first && items(f.call("ListTasks")).empty(), "account B isolated");
  error(f.raw("GetTask", {{"id", a.at("entityId")}}), "NOT_FOUND");
  same_request["meta"]["origin"]["accountId"] = account_b.at("accountId");
  auto b = f.task(same_request);
  require(b.at("entityId") == a.at("entityId"), "same identity permitted in separate account");
  success(f.store.approveAccount(account_a.dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
  require(f.space == first && f.get(a.at("entityId")) == a, "account reselection preserves original data");
  success(f.store.open(J{{"formatVersion", 3}, {"spaceId", standalone}}.dump()));
  f.space = standalone;
  require(f.get(local.at("entityId")) == local, "standalone retained");
}
void explicit_account_merge() {
  Fixture f; auto local = f.task();
  success(f.store.approveAccount(identity(uuid(), uuid(), uuid()).dump(), true));
  f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
  require(f.get(local.at("entityId")).at("task") == local.at("task"), "explicit standalone merge preserves task");
  require(f.history(local).size() == 1, "merge preserves history");
}
void sync_and_remote_cache() {
  Fixture f; const auto epoch = uuid();
  success(f.store.approveAccount(identity(uuid(), uuid(), epoch).dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
  auto v = f.task(); Sql db(f.database());
  const auto owner = uuid(), rival = uuid();
  success(f.store.acquireSync(f.space, owner));
  f.store.acquireSync(f.space, rival);
  require(db.scalar("SELECT value FROM store_meta WHERE key='sync_owner'") == owner, "single sync owner");
  f.store.pendingSync(f.space, owner);
  const auto before = counts(db);
  error(J::parse(f.store.applySync(f.space, J{{"formatVersion", 99}, {"epoch", epoch}, {"owner", owner}}.dump())), "UNSUPPORTED_FORMAT");
  error(J::parse(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", uuid()}, {"owner", owner}}.dump())), "STALE_EPOCH");
  require(counts(db) == before, "invalid sync preserves pending mutations");
  const auto event_id = uuid(), receiver_id = uuid(), processing_id = uuid();
  auto repository_id = v.at("task").at("repositoryId");
  J event = {{"id", event_id}, {"receiverId", receiver_id}, {"repositoryId", repository_id},
      {"senderEventId", "synthetic-event"}, {"eventType", "test.created"},
      {"occurredAt", "2026-01-01T00:00:00Z"}, {"payload", {{"sample", true}}},
      {"state", "EVENT_STATE_COMPLETED"}, {"revision", "1"}};
  J receiver = {{"id", receiver_id}, {"repositoryId", repository_id}, {"name", "Synthetic receiver"}, {"revision", "1"}};
  J processing = {{"id", processing_id}, {"eventId", event_id}, {"taskIds", J::array()},
      {"completedAt", "2026-01-01T00:01:00Z"}, {"origin", f.meta().at("origin")}};
  J changes = J::array({J{{"sequence", "1"}, {"receiver", receiver}},
      J{{"sequence", "2"}, {"event", event}}, J{{"sequence", "3"}, {"processing", processing}}});
  J sync = {{"formatVersion", 3}, {"epoch", epoch}, {"owner", owner},
      {"pull", {{"changes", changes}, {"nextCursor", "synthetic-cursor-3"}, {"hasMore", false}}}};
  const auto cursor_before = db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'");
  db.exec("CREATE TRIGGER test_cache_failure BEFORE INSERT ON remote_objects BEGIN SELECT RAISE(ABORT,'synthetic cache failure'); END;");
  require(!J::parse(f.store.applySync(f.space, sync.dump())).value("ok", true), "pull failure reported");
  require(db.scalar("SELECT count(*) FROM remote_objects") == "0" &&
          db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'") == cursor_before,
          "failed pull cannot advance cursor or partially populate cache");
  db.exec("DROP TRIGGER test_cache_failure;");
  success(f.store.applySync(f.space, sync.dump()));
  require(db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'") == "synthetic-cursor-3", "durable pull cursor");
  require(db.scalar("SELECT count(*) FROM remote_objects WHERE deleted=0") == "3", "all remote cache kinds");
  f.store.setHostHooks({});
  auto cached = f.call("GetEvent", {{"id", event_id}});
  require(cached.at("event") == event && cached.at("processing").size() == 1, "offline event and processing read");
  require(items(f.call("ListEvents", {{"repositoryId", repository_id}})).size() == 1, "offline event list");
  const auto pending = db.scalar("SELECT count(*) FROM outbox");
  error(f.raw("ClaimEvent", {{"meta", f.meta()}, {"id", event_id}}), "ONLINE_REQUIRED");
  require(db.scalar("SELECT count(*) FROM outbox") == pending, "offline lease never queued");
  sync["pull"]["changes"] = J::array({J{{"sequence", "4"}, {"deletion", {
      {"kind", "ENTITY_KIND_OR_REMOTE_KIND_EVENT"}, {"id", event_id}, {"deletionVersionId", uuid()}}}}});
  sync["pull"]["nextCursor"] = "synthetic-cursor-4";
  success(f.store.applySync(f.space, sync.dump()));
  require(items(f.call("ListEvents")).empty(), "remote deletion removes cached event");
  // Acknowledge an actual immutable outbox item rather than inventing a
  // transport mutation ID or relying on the native pendingSync result shape.
  auto mutation = J::parse(db.scalar("SELECT request_json FROM outbox ORDER BY created_at_us,request_id LIMIT 1"));
  const auto request_id = mutation.at("meta").at("requestId").get<std::string>();
  J blocked = {{"results", J::array({J{{"requestId", request_id}, {"error", {
      {"reason", "ERROR_REASON_ITEM_TOO_LARGE"}, {"retryable", false}}}}})}};
  success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", epoch}, {"owner", owner}, {"push", blocked}}.dump()));
  require(db.scalar("SELECT state FROM outbox WHERE request_id='" + request_id + "'") == "2", "oversized item remains blocked");
  require(db.scalar("SELECT count(*) FROM outbox") == pending, "one blocked item preserves other mutations");
  J push = {{"results", J::array({J{{"requestId", request_id}, {"result", {
      {"result", "RESULT_KIND_APPLIED"}, {"version", mutation.at("version")}}}}})}};
  success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", epoch}, {"owner", owner}, {"push", push}}.dump()));
  require(db.scalar("SELECT count(*) FROM outbox WHERE request_id='" + request_id + "'") == "0", "ack removes only durable pending item");
}
void pagination_and_removal() {
  Fixture f;
  std::set<std::string> expected;
  for (int i = 0; i < 5; ++i) expected.insert(f.task().at("entityId").get<std::string>());
  std::vector<std::string> observed;
  std::string cursor;
  unsigned pages = 0;
  do {
    auto page = f.call("ListTasks", {{"page", {{"size", 2}, {"cursor", cursor}}}});
    require(items(page).size() <= 2, "requested page bound");
    for (const auto& item : items(page)) observed.push_back(item.at("version").at("entityId"));
    cursor = page.value("nextCursor", "");
    require(++pages <= 5, "pagination must terminate");
  } while (!cursor.empty());
  require(std::set<std::string>(observed.begin(), observed.end()) == expected && observed.size() == expected.size(),
          "pagination returns every identity exactly once");
  require(std::is_sorted(observed.begin(), observed.end()), "stable entity ID order");
  require(items(f.call("ListTasks", {{"progress", J::array({"PROGRESS_DONE"})}})).empty(), "progress filtering");
  require(items(f.call("ListTasks", {{"proposal", J::array({"PROPOSAL_PENDING"})}})).empty(), "proposal filtering");
  auto status = f.call("GetLocalStatus");
  require(status.at("spaceId") == f.space && status.at("deviceId") == f.device, "local status identity");
  const auto old_space = f.space;
  require(f.call("RemoveDeviceData").at("completed") == true, "remove selected device data");
  auto stale = f.raw("CreateTask", f.createRequest());
  require(!stale.value("ok", true), "removed space refuses new operations");
  error(J::parse(f.store.open(J{{"formatVersion", 3}, {"spaceId", old_space}}.dump())), "NOT_FOUND");
}
void repository_conflicts() {
  Fixture f;
  auto create = [&](const std::string& url) {
    return f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()}, {"url", url}}).at("result").at("version");
  };
  auto base = create("https://conflict.example/base");
  auto occupied = create("https://conflict.example/occupied");
  auto rename_request = [&](const J& version, const std::string& url) {
    return J{{"meta", f.meta()}, {"id", version.at("entityId")},
        {"expectedVersionId", version.at("versionId")}, {"url", url}};
  };
  auto collision = f.raw("SaveRepository", rename_request(base, "https://conflict.example/occupied"));
  require(!collision.value("ok", true), "repository rename cannot steal URL");
  auto left = f.call("SaveRepository", rename_request(base, "https://conflict.example/left")).at("result").at("version");
  error(f.raw("SaveRepository", rename_request(base, "https://conflict.example/right")), "STALE_VERSION");
  auto result = f.call("GetRepository", {{"id", base.at("entityId")}}).at("result");
  require(result.at("alternatives").size() == 2, "repository conflict preserves both heads");
  auto reserve = f.raw("SaveRepository", rename_request(occupied, "https://conflict.example/right"));
  require(!reserve.value("ok", true), "conflict candidate reserves URL");
  std::set<std::string> heads;
  for (const auto& alternative : result.at("alternatives")) heads.insert(alternative.at("versionId"));
  J resolved = left; resolved["versionId"] = uuid(); resolved["parentVersionIds"] = heads;
  resolved["repository"]["normalizedUrl"] = "conflict.example/occupied";
  require(!f.raw("ResolveConflict", {{"meta", f.meta()}, {"id", base.at("entityId")},
      {"expectedHeadIds", heads}, {"resolved", resolved}}).value("ok", true), "resolution cannot steal URL");
  auto still = f.call("GetRepository", {{"id", base.at("entityId")}}).at("result");
  require(still.at("alternatives").size() == 2, "failed resolution preserves conflict");
}
void unsupported_schema() {
  Fixture f; auto v = f.task(); Sql db(f.database());
  const auto before = counts(db);
  db.exec("PRAGMA user_version=99;");
  error(f.raw("CreateTask", f.createRequest()), "UNSUPPORTED_FORMAT");
  require(counts(db) == before, "long-lived writer refuses upgraded schema");
  lane::Store second(f.root.path.string());
  error(J::parse(second.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump())), "UNSUPPORTED_FORMAT");
  require(db.scalar("PRAGMA user_version") == "99", "unsupported schema not replaced");
  db.exec("PRAGMA user_version=3;");
  db.exec("UPDATE store_meta SET value='99' WHERE key='format_version';");
  error(f.raw("DeleteTask", {{"meta", f.meta()}, {"id", v.at("entityId")}}), "UNSUPPORTED_FORMAT");
  require(counts(db) == before, "unsupported format cannot delete data");
  db.exec("UPDATE store_meta SET value='3' WHERE key='format_version'; PRAGMA application_id=123;");
  error(f.raw("CreateTask", f.createRequest()), "UNSUPPORTED_FORMAT");
  require(counts(db) == before, "foreign application database preserved");
}
#include "capture_cases.h"
#include "shared_fixture_runner.h"
#include "planning_fixture_runner.h"
#include "planning_cases.h"
#include "disk_full_vfs.h"
#include "sync_hard_cases.h"
#include "shared_sync_runner.h"
#include "epoch_lifecycle.h"
#include "lifetime_locks.h"
}  // namespace

int main(int argc, char** argv) {
  const std::vector<std::pair<const char*, std::function<void()>>> tests = {
    {"CRUD, replay and persistence", crud_replay_persistence},
    {"25 progress transitions and proposals", transitions_and_proposals},
    {"legacy capture defaults",capture_legacy_empty_defaults},
    {"capture lifecycle",capture_lifecycle},
    {"capture upload handoff",capture_upload_handoff},
    {"moved capture retained history import", moved_capture_and_rule_import_retained_history},
    {"attachment identity deletion and import", attachment_identity_survives_deletion_and_import},
    {"moved capture deleted repository replay", moved_capture_survives_deleted_repository_replay},
    {"moved URL rule deleted repository replay", moved_url_rule_survives_deleted_repository_replay},
    {"capture validation and cache",capture_validation_and_cache},
    {"URL connection lifecycle",url_connections_lifecycle},
    {"capture sync and import",capture_sync_and_import},
    {"prompt history and restore", prompt_history_restore},
    {"whole-task conflicts and resolution", conflicts_and_resolution},
    {"deletion, moves and replay", deletion_move_and_replay},
    {"URLs, Unicode and validation", urls_unicode_and_validation},
    {"external writers and rollback", observation_and_rollback},
    {"preferences and notifications", preferences_and_notifications},
    {"concurrent notification claims", concurrent_notification_claims},
    {"accounts and host hooks", accounts_and_host_hooks},
    {"explicit account merge", explicit_account_merge},
    {"sync and remote cache", sync_and_remote_cache},
    {"pagination and removal", pagination_and_removal},
    {"repository conflicts", repository_conflicts},
    {"unsupported schema", unsupported_schema},
    {"merge repository histories", merge_repository_histories},
    {"mapping alias reads and writes", mapping_alias_reads_and_writes},
    {"server accepted resolution", server_resolution_supersedes_provisional},
    {"repository deletion related content", repository_deletion_redacts_related_content},
    {"deletion transaction failure", deletion_transaction_failure},
    {"deletion disk full", deletion_disk_full},
    {"typed payload preservation", typed_payload_preservation},
    {"duplicate input keys", duplicate_input_keys},
    {"cross-process busy", crossprocess_busy},
    {"account deletion binding", account_deletion_binding},
    {"epoch reapproval lifecycle", epoch_reapproval_lifecycle},
    {"exact account space lookup", exact_account_space_lookup},
    {"lifetime lock removal retry", lifetime_lock_removal_retry},
    {"restart marked cleanup", restart_marked_cleanup},
    {"deferred registry compatibility failure", deferred_registry_compatibility_failure},
    {"planning lifecycle and restart", planning_lifecycle_and_restart},
    {"planning conflicts and resolution", planning_conflicts_and_resolution},
    {"planning deletion redaction and cascade", planning_deletion_redaction_and_cascade},
    {"planning cleanup rollback", planning_cleanup_rolls_back_atomically},
    {"conflicted tasks block cleanup", planning_conflicted_tasks_block_cleanup},
    {"planning sync rejection and retry", planning_sync_rejection_and_retry},
    {"planning envelope formats", planning_envelope_formats},
    {"correction supersedes rejected chain", planning_correction_supersedes_rejected_chain},
    {"stale alternative ignores discarded edits", planning_stale_alternative_ignores_discarded_edits},
    {"stale specialized mutations are validated", planning_stale_specialized_mutations_are_validated},
    {"deleted affected task is redacted from receipts", planning_deleted_affected_task_is_redacted_from_receipts},
    {"list ignores dangling references", planning_list_ignores_dangling_references},
    {"uploads edge removals before additions", planning_uploads_edge_removals_before_dependent_additions},
    {"unsupported store and reset", unsupported_store_and_reset},
    {"development reset coordination", development_reset_coordination},
    {"interrupted reset is repeatable", interrupted_reset_is_repeatable},
    {"conflicted heads, union cycles and cursor scope", planning_conflict_union_cycle_and_cursor_scope},
    {"reset account reconnect flow", reset_account_space_reconnect_flow},
  };
  unsigned failures = 0;
  for (const auto& test : tests) {
    try { test.second(); std::cout << "PASS " << test.first << '\n'; }
    catch (const std::exception& e) { ++failures; std::cerr << "FAIL " << test.first << ": " << e.what() << '\n'; }
  }
  std::cout << tests.size() - failures << '/' << tests.size() << " test groups passed\n";
  std::string fixture_directory;
#ifdef LANE_STORE_FIXTURE_DIR
  fixture_directory = LANE_STORE_FIXTURE_DIR;
#endif
  if (argc > 2) { std::cerr << "Usage: lane_store_tests [fixture-directory]\n"; return 2; }
  if (argc == 2) fixture_directory = argv[1];
  if (!fixture_directory.empty()) { failures += run_shared_fixtures(fixture_directory); failures += run_sync_fixtures(fixture_directory); failures += run_planning_fixtures(fixture_directory); }
  else std::cout << "Shared fixtures not run: provide fixture-directory or LANE_STORE_FIXTURE_DIR\n";
  return failures ? 1 : 0;
}
