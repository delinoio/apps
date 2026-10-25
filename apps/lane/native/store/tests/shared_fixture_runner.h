// SPDX-License-Identifier: Apache-2.0
// Included by store_test.cc inside its test namespace. No production internals
// are exposed: vectors execute public Store operations and inspect durable SQL.
J load_vectors(const fs::path& directory, const std::string& name) {
  std::ifstream input(directory / (name + ".json"));
  require(input.good(), "missing shared fixture file: " + name);
  J document; input >> document;
  require(document.at("formatVersion") == 1, "unsupported fixture format");
  return document.at("cases");
}
J fixture_task(Fixture& f, const J& body, const std::string& entity_id = "") {
  f.call("CreateRepository", {{"meta", f.meta()}, {"id", body.at("repositoryId")},
      {"url", "https://fixtures.example/test/repository"}});
  J request = body;
  request.erase("repositoryId");
  request["meta"] = f.meta(); request["id"] = entity_id.empty() ? uuid() : entity_id;
  request["repository"] = {{"id", body.at("repositoryId")}};
  return f.task(request);
}
void fixture_error(Fixture& f, const std::string& method, const J& request, const J& expected) {
  Sql db(f.database()); auto before = counts(db);
  error(f.raw(method, request), expected.at("reason"));
  if (expected.value("writes", -1) == 0) require(counts(db) == before, "fixture invalid input must not write");
}
void normalization_vector(const J& c) {
  Fixture f;
  J request = {{"meta", f.meta()}, {"id", uuid()}, {"url", c.at("input").at("url")}};
  const auto expected = c.at("expected");
  if (expected.contains("reason")) fixture_error(f, "CreateRepository", request, expected);
  else require(f.call("CreateRepository", request).at("result").at("version").at("repository").at("normalizedUrl") ==
      expected.at("normalizedUrl"), "fixture normalized URL mismatch");
}
void task_vector(const J& c) {
  Fixture f; const auto input = c.at("input"), expected = c.at("expected");
  if (input.value("operation", "") == "search") {
    auto request = f.createRequest(input.at("title")); request["description"] = input.at("description");
    // The search-only vector includes an empty title, which creation correctly
    // rejects. Seed that exact snapshot through the external writer schema to
    // exercise search independently of CreateTask title validation.
    const bool empty_title = input.at("title") == "";
    if (empty_title) request["title"] = "Temporary fixture title";
    auto v = f.task(request);
    if (empty_title) {
      v["task"]["title"] = "";
      Sql db(f.database()); sqlite3_stmt* statement = nullptr;
      require(sqlite3_prepare_v2(db.db, "UPDATE versions SET envelope_json=? WHERE version_id=?", -1,
          &statement, nullptr) == SQLITE_OK, "prepare search snapshot");
      const auto body = v.dump(), version = v.at("versionId").get<std::string>();
      sqlite3_bind_text(statement, 1, body.c_str(), -1, SQLITE_TRANSIENT);
      sqlite3_bind_text(statement, 2, version.c_str(), -1, SQLITE_TRANSIENT);
      const auto rc = sqlite3_step(statement); sqlite3_finalize(statement);
      require(rc == SQLITE_DONE, "seed exact search snapshot");
    }
    require(!items(f.call("ListTasks", {{"search", input.at("query")}})).empty() == expected.at("matches").get<bool>(),
        "fixture literal Unicode search mismatch");
  } else if (input.value("operation", "") == "validateStoredTask") {
    J valid = input.at("task"); valid["progress"] = "PROGRESS_TODO"; valid["proposal"] = "PROPOSAL_REGULAR";
    auto v = fixture_task(f, valid);
    fixture_error(f, "SaveTask", {{"meta", f.meta()}, {"id", v.at("entityId")},
        {"expectedVersionId", v.at("versionId")}, {"task", input.at("task")}}, expected);
  } else if (input.contains("snapshots")) {
    const auto snapshots = input.at("snapshots");
    std::vector<J> versions{fixture_task(f, snapshots.at(0))};
    for (size_t i = 1; i < snapshots.size(); ++i) versions.push_back(f.save(versions.back(), snapshots.at(i)));
    auto restored = f.call("RestorePrompt", {{"meta", f.meta()}, {"id", versions.back().at("entityId")},
        {"expectedVersionId", versions.back().at("versionId")},
        {"sourceVersionId", versions.at(input.at("restoreSourceIndex").get<size_t>()).at("versionId")}}).at("result").at("version");
    require(restored.at("task").at("prompt") == expected.at("prompt"), "fixture restored prompt");
    if (expected.contains("promptPresent")) require(restored.at("task").contains("prompt") == expected.at("promptPresent"), "fixture prompt presence");
    require(f.history(restored).size() == expected.at("newVersionsIncludingCreation"), "fixture restore version count");
    if (expected.contains("promptHistoryCount")) require(f.history(restored, true).size() == expected.at("promptHistoryCount"), "fixture prompt history count");
  } else if (input.contains("decisions")) {
    auto v = fixture_task(f, input.at("task")); size_t index = 0;
    for (const auto& decision : input.at("decisions")) {
      v = f.call("SetProposal", {{"meta", f.meta()}, {"id", v.at("entityId")},
          {"expectedVersionId", v.at("versionId")}, {"proposal", decision}}).at("result").at("version");
      require(v.at("task").at("progress") == expected.at("progresses").at(index) &&
          v.at("task").at("proposal") == expected.at("proposals").at(index), "fixture proposal decision");
      ++index;
    }
    require(f.history(v).size() == expected.at("newVersions").get<size_t>() + 1, "fixture decision history");
  } else {
    auto request = input.at("request"); const auto method = input.at("method").get<std::string>();
    J before;
    if (input.contains("before")) {
      before = fixture_task(f, input.at("before"), request.at("id"));
      // The fixture uses symbolic initial version IDs; bind the CAS to the
      // actual generated version without altering the task or requested edit.
      request["expectedVersionId"] = before.at("versionId");
    }
    if (expected.contains("reason")) { fixture_error(f, method, request, expected); return; }
    auto v = f.call(method, request).at("result").at("version");
    if (expected.contains("task")) require(v.at("task") == expected.at("task"), "fixture full task mismatch");
    if (expected.contains("newVersions")) require(f.history(v).size() == expected.at("newVersions").get<size_t>() + 1 &&
        v.at("versionId") != before.at("versionId"), "fixture transition version");
    for (const auto* key : {"progress", "proposal"})
      if (expected.contains(key)) require(v.at("task").at(key) == expected.at(key), "fixture default enum");
    if (expected.contains("promptPresent")) require(v.at("task").contains("prompt") == expected.at("promptPresent"), "fixture optional prompt");
    if (expected.contains("repositoryCount")) require(items(f.call("ListRepositories")).size() == expected.at("repositoryCount"), "fixture repository count");
    if (expected.contains("taskCount")) require(items(f.call("ListTasks")).size() == expected.at("taskCount"), "fixture task count");
    if (expected.contains("historyCount")) require(f.history(v).size() == expected.at("historyCount"), "fixture history count");
  }
}
void fingerprint_vector(const J& c) {
  Fixture f;
  auto normalized = c.at("input").at("normalizedRequest");
  const auto canonical = c.at("expected").at("canonicalJson").get<std::string>();
  require(J::parse(canonical) == normalized && picosha2::hash256_hex_string(canonical) == c.at("expected").at("sha256"),
          "shared canonical fingerprint oracle");
  auto request = normalized;
  request["repository"]["url"] = "https://" + normalized.at("repository").at("url").get<std::string>() + ".git";
  // Exercise typed default expansion as well as optional prompt presence.
  request.erase("description"); request.erase("progress"); request.erase("proposal");
  auto v = f.task(request);
  require(v.at("origin").at("kind") == "ORIGIN_KIND_APP", "fixture UI origin boundary");
  auto expected_canonical = J::parse(canonical);
  expected_canonical["meta"]["origin"] = v.at("origin");
  Sql db(f.database()); const auto rid = request.at("meta").at("requestId").get<std::string>();
  const auto actual = db.scalar("SELECT fingerprint FROM receipts WHERE method='CreateTask' AND request_id='" + rid + "'");
  require(actual == picosha2::hash256_hex_string(expected_canonical.dump()), "engine canonical fingerprint with authoritative APP attribution");
  auto replay = normalized; replay["repository"]["url"] = "git@github.com:Acme/Repo";
  require(f.task(replay).at("versionId") == v.at("versionId") && f.history(v).size() == 1, "fixture normalized replay");
  if (replay.contains("prompt")) replay.erase("prompt"); else replay["prompt"] = "";
  error(f.raw("CreateTask", replay), "REQUEST_ID_REUSED");
  require(f.history(v).size() == 1, "prompt presence fingerprint rejection is atomic");
}
unsigned run_shared_fixtures(const fs::path& directory) {
  unsigned failed = 0, total = 0;
  for (const auto& name : {"normalization", "tasks", "fingerprints"}) {
    J cases;
    try { cases = load_vectors(directory, name); }
    catch (const std::exception& e) { ++failed; std::cerr << "FAIL fixtures " << name << ": " << e.what() << '\n'; continue; }
    for (const auto& c : cases) {
      ++total;
      try {
        if (std::string(name) == "normalization") normalization_vector(c);
        else if (std::string(name) == "tasks") task_vector(c);
        else fingerprint_vector(c);
        std::cout << "PASS fixture " << name << '/' << c.at("id").get<std::string>() << '\n';
      } catch (const std::exception& e) {
        ++failed; std::cerr << "FAIL fixture " << name << '/' << c.value("id", "unknown") << ": " << e.what() << '\n';
      }
    }
  }
  std::cout << total << " shared vectors executed; " << failed << " failures\n";
  return failed;
}
