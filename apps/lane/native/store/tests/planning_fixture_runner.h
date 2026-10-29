// SPDX-License-Identifier: Apache-2.0
// Included by store_test.cc inside its test namespace. Replays the shared Lane
// planning vectors (planning-validation, planning-graph, planning-cleanup and
// planning-list) against the native store. States are seeded through the
// external-writer SQL boundary so cyclic, deleted or cross-repository fixtures
// need no workaround; every operation then runs through the public Store API.
struct PlanningSeed {
  Fixture f;
  std::map<std::string, std::string> versions; // entity id -> seeded version id
  std::map<std::string, J> envelopes;
  explicit PlanningSeed(const J& entities) {
    Sql db(f.database());
    int64_t created = 1767225600000000;
    for (const auto& entity : entities) {
      J v = entity;
      const auto id = v.at("entityId").get<std::string>();
      const auto kind = v.at("kind").get<std::string>();
      v["versionId"] = uuid(); v["parentVersionIds"] = J::array();
      v["createdAt"] = "2026-01-01T00:00:00.000000Z";
      v["origin"] = {{"kind", "ORIGIN_KIND_APP"}, {"deviceId", f.device}, {"deviceName", "Fixture"}, {"accountId", ""}};
      const bool deleted = v.value("deleted", false);
      static const std::map<std::string, std::pair<int, std::string>> kinds = {
          {"ENTITY_KIND_REPOSITORY", {1, "repository"}}, {"ENTITY_KIND_TASK", {2, "task"}},
          {"ENTITY_KIND_LABEL", {4, "label"}}, {"ENTITY_KIND_MILESTONE", {5, "milestone"}},
          {"ENTITY_KIND_CYCLE", {6, "cycle"}}};
      const auto info = kinds.at(kind);
      std::string repository = "NULL", url = "NULL";
      if (!deleted && info.first == 1) url = "'" + v.at("repository").at("normalizedUrl").get<std::string>() + "'";
      else if (!deleted) repository = "'" + v.at(info.second).at("repositoryId").get<std::string>() + "'";
      db.exec("INSERT INTO entities(id,kind,repository_id,normalized_url,deleted) VALUES('" + id + "'," +
          std::to_string(info.first) + "," + repository + "," + url + "," + (deleted ? "1" : "0") + ")");
      Sql& sql = db;
      sqlite3_stmt* statement = nullptr;
      require(sqlite3_prepare_v2(sql.db, "INSERT INTO versions VALUES(?,?,?,?,0)", -1, &statement, nullptr) == SQLITE_OK, "prepare seed");
      const auto version = v.at("versionId").get<std::string>(), body = v.dump();
      sqlite3_bind_text(statement, 1, version.c_str(), -1, SQLITE_TRANSIENT);
      sqlite3_bind_text(statement, 2, id.c_str(), -1, SQLITE_TRANSIENT);
      sqlite3_bind_text(statement, 3, body.c_str(), -1, SQLITE_TRANSIENT);
      sqlite3_bind_int64(statement, 4, created++);
      require(sqlite3_step(statement) == SQLITE_DONE, "seed version"); sqlite3_finalize(statement);
      db.exec("INSERT INTO heads VALUES('" + id + "','" + version + "')");
      versions[id] = version; envelopes[id] = v;
    }
  }
  // Fixture version IDs are symbolic; bind the compare-and-set to the actual head.
  J request(const J& input) {
    J r = input;
    if (r.contains("expectedVersionId")) r["expectedVersionId"] = versions.at(r.at("id").get<std::string>());
    return r;
  }
  J head(const std::string& id) {
    Sql db(f.database());
    return J::parse(db.scalar("SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id WHERE h.entity_id='" + id + "'"));
  }
};
J planning_cases(const fs::path& directory, const std::string& name, J* state) {
  std::ifstream input(directory / (name + ".json"));
  require(input.good(), "missing shared fixture file: " + name);
  J document; input >> document;
  require(document.at("formatVersion") == 1, "unsupported fixture format");
  if (state) *state = document.contains("state") ? document.at("state").at("entities") : document.at("dataset").at("entities");
  return document.at("cases");
}
J version_body(const J& result, const char* key) { return result.at("result").at("version").at(key); }
void check_outcome(PlanningSeed& s, const J& c, const J& response, const std::vector<std::string>& before) {
  const auto expected = c.at("expected");
  Sql db(s.f.database());
  if (expected.at("outcome") == "error") {
    require(!response.value("ok", true), "fixture must reject: " + response.dump());
    const auto& error = response.at("error");
    require(error.at("reason") == expected.at("reason"), "reason " + error.at("reason").dump() + " != " + expected.at("reason").dump());
    require(error.value("violation", "") == expected.value("violation", ""), "violation " + error.dump());
    require(counts(db) == before, "a rejected planning write leaves no partial rows");
    return;
  }
  require(response.value("ok", false), "fixture must succeed: " + response.dump());
  const auto& result = response.at("response");
  const auto normalized = expected.at("normalized");
  const std::string key = normalized.begin().key();
  require(version_body(result, key.c_str()) == normalized.begin().value(),
      "normalized body " + version_body(result, key.c_str()).dump() + " != " + normalized.dump());
  if (expected.contains("affected")) {
    const auto actual = result.value("affected", J::array());
    require(actual.size() == expected.at("affected").size(), "affected count " + J(actual).dump());
    for (size_t i = 0; i < actual.size(); ++i) {
      const auto& want = expected.at("affected").at(i);
      require(actual[i].at("version").at("entityId") == want.at("entityId") &&
              actual[i].at("version").at("task") == want.at("task"), "affected task " + actual[i].dump());
      require(s.head(want.at("entityId")).at("task") == want.at("task"), "affected task persisted");
    }
  }
}
void planning_validation_or_graph_vector(const J& state, const J& c) {
  PlanningSeed s(state);
  Sql db(s.f.database()); const auto before = counts(db);
  const auto method = c.at("input").at("method").get<std::string>();
  check_outcome(s, c, s.f.raw(method, s.request(c.at("input").at("request"))), before);
}
void planning_cleanup_vector(const J& state, const J& c) {
  PlanningSeed s(state);
  const auto method = c.at("input").at("method").get<std::string>();
  auto response = s.f.raw(method, s.request(c.at("input").at("request")));
  require(response.value("ok", false), "cleanup must succeed: " + response.dump());
  std::vector<std::string> deleted;
  std::map<std::string, J> changed;
  for (const auto& entry : s.envelopes) {
    if (entry.second.value("deleted", false)) continue;
    const auto head = s.head(entry.first);
    if (head.value("deleted", false)) deleted.push_back(entry.first);
    else if (head.at("versionId") != s.versions.at(entry.first)) {
      const std::string key = head.at("kind") == "ENTITY_KIND_TASK" ? "task" : "label";
      changed[entry.first] = head.at(key);
      require(head.at("parentVersionIds") == J::array({s.versions.at(entry.first)}), "cleanup version parents the previous head");
      require(head.at("origin").at("kind") == "ORIGIN_KIND_APP" && head.at("origin").at("deviceId") == s.f.device, "cleanup uses the acting origin");
    }
  }
  require(deleted == c.at("expected").at("deleted").get<std::vector<std::string>>(), "deleted ids " + J(deleted).dump());
  const auto want = c.at("expected").at("changed");
  require(changed.size() == want.size(), "changed ids");
  for (const auto& entry : want) {
    const auto id = entry.at("entityId").get<std::string>();
    require(changed.count(id) && changed.at(id) == entry.at("task"), "changed task " + id);
  }
  // Cleanup shares the transaction: each rewritten task has exactly one outbox row.
  Sql db(s.f.database());
  require(db.scalar("SELECT count(*) FROM outbox") == std::to_string(want.size() + deleted.size()),
      "one pending record per written version");
}
void planning_list_vector(PlanningSeed& s, const J& c) {
  auto request = c.at("input").at("request");
  const auto expected = c.at("expected");
  auto unpaged = request; unpaged["page"] = {{"size", 1000}};
  std::vector<std::string> order;
  for (const auto& item : items(s.f.call("ListTasks", unpaged))) order.push_back(item.at("version").at("entityId"));
  require(order == expected.at("taskIds").get<std::vector<std::string>>(), "unpaged order " + J(order).dump());
  std::vector<std::vector<std::string>> pages;
  std::string cursor;
  for (int guard = 0; guard < 100; ++guard) {
    auto paged = request;
    paged["page"]["cursor"] = cursor;
    auto response = s.f.call("ListTasks", paged);
    pages.emplace_back();
    for (const auto& item : items(response)) pages.back().push_back(item.at("version").at("entityId"));
    cursor = response.value("nextCursor", "");
    if (cursor.empty()) break;
    require(guard < 99, "pagination must terminate");
  }
  require(J(pages) == expected.at("pages"), "pages " + J(pages).dump() + " != " + expected.at("pages").dump());
}
unsigned run_planning_fixtures(const fs::path& directory) {
  unsigned failed = 0, total = 0;
  auto run = [&](const std::string& suite, const J& c, const std::function<void()>& body) {
    ++total;
    try { body(); std::cout << "PASS fixture " << suite << '/' << c.at("id").get<std::string>() << '\n'; }
    catch (const std::exception& e) { ++failed; std::cerr << "FAIL fixture " << suite << '/' << c.value("id", "unknown") << ": " << e.what() << '\n'; }
  };
  for (const std::string suite : {"planning-validation", "planning-graph", "planning-cleanup"}) {
    J state, cases;
    try { cases = planning_cases(directory, suite, &state); }
    catch (const std::exception& e) { ++failed; std::cerr << "FAIL fixtures " << suite << ": " << e.what() << '\n'; continue; }
    for (const auto& c : cases)
      run(suite, c, [&] { suite == "planning-cleanup" ? planning_cleanup_vector(state, c) : planning_validation_or_graph_vector(state, c); });
  }
  J dataset, cases;
  try {
    cases = planning_cases(directory, "planning-list", &dataset);
    PlanningSeed seeded(dataset);
    for (const auto& c : cases) run("planning-list", c, [&] { planning_list_vector(seeded, c); });
  } catch (const std::exception& e) { ++failed; std::cerr << "FAIL fixtures planning-list: " << e.what() << '\n'; }
  std::cout << total << " planning vectors executed; " << failed << " failures\n";
  return failed;
}
