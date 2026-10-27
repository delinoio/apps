// SPDX-License-Identifier: Apache-2.0
// Included by store_test.cc inside its test namespace. Execute the shared
// completed-list vectors through the native store's public ListTasks method.
void completed_insert(Sql& db, const std::string& statement,
                      const std::vector<std::string>& values) {
  sqlite3_stmt* prepared = nullptr;
  require(sqlite3_prepare_v2(db.db, statement.c_str(), -1, &prepared, nullptr) == SQLITE_OK,
          "prepare completed fixture insert");
  for (size_t i = 0; i < values.size(); ++i)
    sqlite3_bind_text(prepared, int(i + 1), values[i].c_str(), -1, SQLITE_TRANSIENT);
  const int rc = sqlite3_step(prepared);
  sqlite3_finalize(prepared);
  require(rc == SQLITE_DONE, "insert completed fixture row");
}
struct CompletedSeed {
  Fixture f;
  explicit CompletedSeed(const J& dataset) {
    Sql db(f.database());
    std::map<std::string, J> versions;
    for (const auto& version : dataset.at("versions"))
      versions.emplace(version.at("versionId").get<std::string>(), version);
    for (const auto& row : dataset.at("rows")) {
      const auto& selected = versions.at(row.at("selectedVersionId").get<std::string>());
      completed_insert(db, "INSERT INTO entities(id,kind,repository_id,deleted) VALUES(?,2,?,0)",
                       {selected.at("entityId"), selected.at("task").at("repositoryId")});
    }
    for (const auto& [id, version] : versions)
      completed_insert(db, "INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us,prompt_version) "
                           "VALUES(?,?,?,0,0)",
                       {id, version.at("entityId"), version.dump()});
    for (const auto& row : dataset.at("rows")) {
      const auto& selected = versions.at(row.at("selectedVersionId").get<std::string>());
      for (const auto& head : row.at("headVersionIds"))
        completed_insert(db, "INSERT INTO heads(entity_id,version_id) VALUES(?,?)",
                         {selected.at("entityId"), head});
    }
  }
  void account(const std::string& id) {
    Sql registry(f.root.path / "registry.sqlite");
    completed_insert(registry, "UPDATE spaces SET account_id=? WHERE id=?", {id, f.space});
  }
};
std::vector<std::string> completed_ids(const J& response) {
  std::vector<std::string> ids;
  for (const auto& row : response.at("items"))
    ids.push_back(row.at("version").at("entityId"));
  return ids;
}
std::string completed_next_microsecond(const std::string& value) {
  std::tm time{};
  std::istringstream input(value.substr(0, 19));
  input >> std::get_time(&time, "%Y-%m-%dT%H:%M:%S");
  require(!input.fail(), "parse fixture timestamp");
#ifdef _WIN32
  const auto seconds = _mkgmtime(&time);
#else
  const auto seconds = timegm(&time);
#endif
  auto micros = std::stoi(value.substr(20, 6)) + 1;
  const auto next = seconds + micros / 1000000;
  micros %= 1000000;
  std::tm normalized{};
#ifdef _WIN32
  gmtime_s(&normalized, &next);
#else
  gmtime_r(&next, &normalized);
#endif
  std::ostringstream output;
  output << std::put_time(&normalized, "%Y-%m-%dT%H:%M:%S") << '.'
         << std::setfill('0') << std::setw(6) << micros << 'Z';
  return output.str();
}
void completed_fixture(const J& document) {
  require(document.at("formatVersion") == 1, "completed fixture format");
  CompletedSeed seed(document.at("dataset"));
  for (const auto& c : document.at("entryCases")) {
    const auto selected = c.at("input").at("selectedVersionId").get<std::string>();
    std::string entity;
    for (const auto& version : document.at("dataset").at("versions"))
      if (version.at("versionId") == selected)
        entity = version.at("entityId").get<std::string>();
    require(!entity.empty(), "entry fixture selected version");
    const auto expected = c.at("expected");
    auto matches = [&](const std::string& cutoff) {
      J request = {{"completed", {{"selection", "COMPLETED_SELECTION_LAST_24_HOURS"},
                                  {"cutoff", cutoff}}}};
      const auto ids = completed_ids(seed.f.call("ListTasks", request));
      return std::find(ids.begin(), ids.end(), entity) != ids.end();
    };
    if (expected.at("kind") == "TERMINAL_ENTRY_KIND_KNOWN") {
      const auto at = expected.at("enteredAt").get<std::string>();
      require(matches(at), "inclusive entry " + c.at("id").get<std::string>());
      require(!matches(completed_next_microsecond(at)),
              "entry precision " + c.at("id").get<std::string>());
    } else {
      require(expected.at("kind") == "TERMINAL_ENTRY_KIND_UNKNOWN", "entry kind");
      require(matches("2100-01-01T00:00:00.000000Z"),
              "unknown entry " + c.at("id").get<std::string>());
    }
  }
  for (const auto& c : document.at("listCases")) {
    const auto request = c.at("input").at("request");
    auto first = seed.f.call("ListTasks", request);
    auto ids = completed_ids(first);
    const auto size = request.at("page").at("size").get<size_t>();
    auto unpaged = request;
    unpaged["page"] = {{"size", 1000}};
    require(completed_ids(seed.f.call("ListTasks", unpaged)) ==
                c.at("expected").at("taskIds").get<std::vector<std::string>>(),
            "unpaged completed list " + c.at("id").get<std::string>());
    std::vector<std::vector<std::string>> pages{ids};
    auto cursor = first.value("nextCursor", "");
    while (!cursor.empty()) {
      auto continuation = request;
      continuation["page"] = {{"size", size}, {"cursor", cursor}};
      auto next = seed.f.call("ListTasks", continuation);
      auto page = completed_ids(next);
      ids.insert(ids.end(), page.begin(), page.end());
      pages.push_back(page);
      cursor = next.value("nextCursor", "");
      require(pages.size() <= 100, "completed fixture pagination terminates");
    }
    require(ids == c.at("expected").at("taskIds").get<std::vector<std::string>>(),
            "paged completed list " + c.at("id").get<std::string>());
    require(pages == c.at("expected").at("pages").get<std::vector<std::vector<std::string>>>(),
            "completed page boundaries " + c.at("id").get<std::string>());
  }
  for (const auto& c : document.at("invalidCases"))
    error(seed.f.raw("ListTasks", c.at("request")), "INVALID_INPUT");
  for (const auto& c : document.at("cursorCases")) {
    seed.account(c.at("firstAccountId"));
    auto first = seed.f.call("ListTasks", c.at("first"));
    const auto cursor = first.value("nextCursor", "");
    require(!cursor.empty(), "cursor fixture needs a continuation");
    auto resume = c.at("resume");
    resume["page"]["cursor"] = cursor;
    if (c.at("resumeSpaceId") != c.at("firstSpaceId")) {
      CompletedSeed other(document.at("dataset"));
      other.account(c.at("resumeAccountId"));
      error(other.f.raw("ListTasks", resume), "INVALID_INPUT");
    } else {
      seed.account(c.at("resumeAccountId"));
      if (c.at("valid").get<bool>())
        seed.f.call("ListTasks", resume);
      else
        error(seed.f.raw("ListTasks", resume), "INVALID_INPUT");
    }
  }
}
unsigned run_completed_fixtures(const fs::path& directory) {
  std::ifstream input(directory / "completed-list.json");
  require(input.good(), "missing completed-list fixture");
  J document; input >> document;
  completed_fixture(document);
  std::cout << "PASS completed-list shared vectors\n";
  return 0;
}
