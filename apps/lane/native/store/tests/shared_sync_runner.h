// SPDX-License-Identifier: Apache-2.0
// SyncMutation fixture graphs are imported as accepted pull EntityResults.
// This does not emulate the server PushChanges receipt/limit/serialization API.
struct SyncVectors {
  Fixture f;
  std::string epoch = uuid(), owner = uuid();
  unsigned sequence = 0;
  SyncVectors() {
    success(f.store.approveAccount(identity(uuid(), uuid(), epoch).dump(), false));
    f.space = success(f.store.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId");
    success(f.store.acquireSync(f.space, owner));
  }
  J packet(const J& versions) {
    J changes = J::array();
    for (const auto& v : versions) changes.push_back({{"sequence", std::to_string(++sequence)},
        {"entity", {{"result", "RESULT_KIND_APPLIED"}, {"version", v}}}});
    return {{"formatVersion", 2}, {"epoch", epoch}, {"owner", owner},
        {"pull", {{"changes", changes}, {"nextCursor", "fixture-" + std::to_string(sequence)}}}};
  }
  void apply(const J& versions) { success(f.store.applySync(f.space, packet(versions).dump())); }
  J repo(const std::string& id, const std::string& url = "fixture.example/repo") {
    return f.call("CreateRepository", {{"meta", f.meta()}, {"id", id}, {"url", "https://" + url}}).at("result").at("version");
  }
  void dependencies(const J& versions) {
    std::set<std::string> repositories;
    for (const auto& v : versions) if (v.contains("task")) repositories.insert(v.at("task").at("repositoryId"));
    for (const auto& id : repositories) repo(id, "fixture.example/" + id);
  }
};
std::set<std::string> version_ids(const J& versions) {
  std::set<std::string> result;
  for (const auto& v : versions) result.insert(v.at("versionId"));
  return result;
}
void assert_sync_result(Fixture& f, const std::string& id, const J& expected, bool repository = false) {
  auto result = f.call(repository ? "GetRepository" : "GetTask", {{"id", id}}).at("result");
  auto versions = result.value("alternatives", J::array());
  if (result.contains("version")) versions.push_back(result.at("version"));
  if (expected.contains("headIds")) require(version_ids(versions) == expected.at("headIds").get<std::set<std::string>>(), "fixture head set");
  // REPLAYED is a server mutation receipt result; pull imports assert no new
  // versions and unchanged heads below rather than inventing a server receipt.
  if (expected.contains("result") && expected.at("result") != "RESULT_KIND_REPLAYED")
    require(result.at("result") == expected.at("result"), "fixture result kind");
  if (expected.contains("alternatives")) {
    auto actual = result.value("alternatives", J::array());
    require(actual.size() == expected.at("alternatives").size(), "fixture alternative count");
    for (const auto& alternative : expected.at("alternatives"))
      require(std::find(actual.begin(), actual.end(), alternative) != actual.end(), "fixture complete alternative preserved");
  }
  if (expected.contains("bodyPresent")) require(result.at("version").contains("task") == expected.at("bodyPresent"), "fixture body presence");
  if (expected.contains("historyBodies")) {
    Sql db(f.database());
    require(db.scalar("SELECT count(*) FROM versions WHERE entity_id='" + id + "' AND envelope_json LIKE '%\"task\":%'") ==
        std::to_string(expected.at("historyBodies").get<int>()), "fixture retained history bodies");
  }
}
void graph_sync_vector(const J& c) {
  SyncVectors s; const auto input = c.at("input"), expected = c.at("expected");
  auto initial = input.value("initial", J::array()), mutations = input.value("mutations", J::array());
  J all = initial;
  for (const auto& m : mutations) all.push_back(m.at("version"));
  s.dependencies(all);
  if (!initial.empty()) s.apply(initial);
  Sql db(s.f.database()); const auto before = durable_snapshot(db);
  if (input.contains("interactiveRequest")) {
    auto r = input.at("interactiveRequest");
    error(s.f.raw(r.at("method"), r.at("request")), expected.at("reason"));
    require(durable_snapshot(db) == before, "read-only fixture writes nothing");
  } else {
    for (const auto& m : mutations) {
      auto packet = s.packet(J::array({m.at("version")}));
      if (c.at("id") == "missing-parent") {
        auto response = J::parse(s.f.store.applySync(s.f.space, packet.dump()));
        error(response, "DEPENDENCY_UNAVAILABLE");
        require(response.at("error").at("retryable") == expected.at("retryable"), "missing-parent retryability");
        require(durable_snapshot(db) == before, "missing-parent rejects atomically including cursor");
        return;
      }
      success(s.f.store.applySync(s.f.space, packet.dump()));
    }
  }
  const auto id = all.at(0).at("entityId").get<std::string>();
  assert_sync_result(s.f, id, expected);
  if (expected.contains("newVersions")) {
    auto history = s.f.history(all.at(0));
    require(history.size() == initial.size() + expected.at("newVersions").get<size_t>(), "fixture added versions");
  }
  if (expected.contains("newPromptHistoryVersionIds")) {
    auto prompts = s.f.history(all.at(0), true);
    auto ids = version_ids(prompts);
    for (const auto& v : initial) ids.erase(v.at("versionId"));
    require(ids == expected.at("newPromptHistoryVersionIds").get<std::set<std::string>>(), "import preserves restoration prompt history");
    std::set<std::string> restorations;
    for (const auto& v : s.f.history(all.at(0))) if (v.value("promptRestoration", false)) restorations.insert(v.at("versionId"));
    require(restorations == expected.at("restorationVersionIds").get<std::set<std::string>>(), "restoration marker preserved");
    require(s.f.get(id).at("task").contains("prompt") == expected.at("promptPresent"), "restoration prompt presence");
  }
  const auto count = s.f.history(all.at(0)).size();
  for (const auto& index : input.value("replayMutationIndices", J::array())) s.apply(J::array({mutations.at(index.get<size_t>()).at("version")}));
  require(s.f.history(all.at(0)).size() == count, "import replay cannot append history");
}
void local_sync_vector(const J& c, const J& cases) {
  const auto key = c.at("id").get<std::string>();
  const auto input = c.at("input"), expected = c.at("expected");
  if (input.contains("initial")) { graph_sync_vector(c); return; }
  auto find_case = [&](const std::string& id) -> J {
    for (const auto& entry : cases) if (entry.at("id") == id) return entry;
    throw std::runtime_error("missing dependent vector " + id);
  };
  if (key == "retry-equivalent-url" || key == "retry-changed-title") {
    Fixture f; size_t index = 0;
    for (const auto& request : input.at("requests")) {
      const auto result = expected.at("results").at(index++).get<std::string>();
      if (result.rfind("ERROR_REASON_", 0) == 0) error(f.raw(input.at("method"), request), result);
      else require(f.call(input.at("method"), request).at("result").at("result") == result, "fixture replay result");
    }
    require(items(f.call("ListTasks")).size() == expected.at("taskCount"), "fixture replay task count");
    require(f.call("ListHistory", {{"id", input.at("requests")[0].at("id")}}).at("versions").size() == expected.at("historyCount"), "fixture replay history");
  } else if (key == "creation-replay-after-delete") {
    Fixture f; auto request = input.at("request"); f.task(request);
    f.call("DeleteTask", {{"meta", f.meta()}, {"id", request.at("id")}});
    require(f.call(input.at("method"), request).at("result").at("result") == expected.at("result"), "fixture deleted replay");
    assert_sync_result(f, request.at("id"), expected);
    require(items(f.call("ListTasks")).size() == expected.at("taskCount"), "fixture deleted task list");
  } else if (key == "atomic-local-write") {
    Fixture f; Sql db(f.database()); const auto before = durable_snapshot(db);
    db.exec("CREATE TRIGGER fixture_failure BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'fixture atomic failure'); END;");
    require(!f.raw("CreateTask", input.at("request")).value("ok", true), "fixture failure injected");
    require(durable_snapshot(db) == before, "fixture atomic failure preserves every durable table");
    require(db.scalar("SELECT count(*) FROM versions") == std::to_string(expected.at("historyCount").get<int>()) &&
        db.scalar("SELECT count(*) FROM outbox") == std::to_string(expected.at("outboxCount").get<int>()), "fixture atomic counts");
  } else if (key == "unsupported-schema") {
    Fixture f; Sql db(f.database()); const auto before = durable_snapshot(db);
    // Older shared vectors used schema 2 as the unknown future schema.
    // Keep that scenario unknown after adding the explicit schema 2 upgrade.
    db.exec("PRAGMA user_version=" + std::to_string(std::max(99,input.at("schemaVersion").get<int>())));
    lane::Store other(f.root.path.string());
    error(J::parse(other.open(J{{"formatVersion", 2}, {"spaceId", f.space}}.dump())), "UNSUPPORTED_FORMAT");
    require(durable_snapshot(db) == before, "fixture unsupported schema untouched");
  } else if (key == "rename-releases-url") {
    SyncVectors s; auto v = s.repo(input.at("repositoryId"), input.at("oldUrl"));
    s.f.call("SaveRepository", {{"meta", s.f.meta()}, {"id", v.at("entityId")}, {"expectedVersionId", v.at("versionId")},
        {"url", "https://" + input.at("newUrl").get<std::string>()}});
    s.repo(input.at("createId"), input.at("createUrl"));
    for (auto it = expected.at("repositoryUrls").begin(); it != expected.at("repositoryUrls").end(); ++it)
      require(s.f.call("GetRepository", {{"id", it.key()}}).at("result").at("version").at("repository").at("normalizedUrl") == it.value(), "fixture URL rename lookup");
  } else if (key == "account-isolation") {
    Fixture f; size_t index = 0;
    for (const auto& account : input.at("accounts")) {
      success(f.store.approveAccount(identity(account, uuid(), uuid()).dump(), false));
      f.space = success(f.store.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId");
      require(items(f.call("ListTasks")).empty(), "fixture account starts isolated");
      require(f.call("CreateTask", input.at("requests").at(index)).at("result").at("result") == "RESULT_KIND_APPLIED", "no cross-account dedupe");
      require(items(f.call("ListTasks")).size() == expected.at("tasksPerAccount").at(index++), "fixture account task count");
    }
  } else if (key == "unsupported-format" || input.value("operation", "") == "validateEnvelope") {
    SyncVectors s;
    J version = key == "unsupported-format" ? input.at("version") : find_case("whole-task-conflict").at("input").at("initial")[0];
    s.dependencies(J::array({version}));
    if (input.contains("kind")) version["kind"] = input.at("kind");
    Sql db(s.f.database()); const auto before = durable_snapshot(db);
    error(J::parse(s.f.store.applySync(s.f.space, s.packet(J::array({version})).dump())), expected.at("reason"));
    require(durable_snapshot(db) == before, "invalid envelope leaves compatible state intact");
  } else if (key == "stale-account-epoch") {
    SyncVectors s;
    // The fixture generation identities are real inputs, not an arbitrary stale UUID.
    Sql registry(s.f.root.path / "registry.sqlite");
    const auto account = registry.scalar("SELECT account_id FROM spaces WHERE id='" + s.f.space + "'");
    const auto tenant = registry.scalar("SELECT tenant_id FROM spaces WHERE id='" + s.f.space + "'");
    // A new isolated fixture account can establish the requested initial epoch.
    success(s.f.store.approveAccount(identity(uuid(), tenant, input.at("currentEpoch")).dump(), false));
    s.f.space = success(s.f.store.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId");
    success(s.f.store.acquireSync(s.f.space, s.owner)); auto task = s.f.task();
    Sql db(s.f.database()); const auto before = durable_snapshot(db);
    error(J::parse(s.f.store.applySync(s.f.space, J{{"formatVersion", 2}, {"epoch", input.at("pushEpoch")}, {"owner", s.owner}}.dump())), expected.at("reason"));
    require(durable_snapshot(db) == before && s.f.get(task.at("entityId")) == task, "fixture stale epoch retains local content");
    (void)account;
  } else if (key == "repository-display-conflict" || key == "repository-concurrent-edits" || key == "parent-set-order" || key == "resolution-url-collision") {
    SyncVectors s; J versions;
    if (key == "repository-display-conflict") versions = input.at("versions");
    else if (key == "repository-concurrent-edits") {
      auto base = find_case("repository-display-conflict").at("input").at("versions")[0];
      base["versionId"] = input.at("baseVersionId"); s.apply(J::array({base})); versions = J::array();
      for (const auto& edit : input.at("versions")) { auto v = base; v["versionId"] = edit.at("versionId"); v["parentVersionIds"] = edit.at("parentVersionIds"); v["repository"]["displayName"] = edit.at("displayName"); versions.push_back(v); }
    } else if (key == "parent-set-order") {
      auto graph = find_case("whole-task-conflict").at("input"); versions = graph.at("initial");
      s.dependencies(versions);
      for (const auto& m : graph.at("mutations")) versions.push_back(m.at("version"));
    } else versions = find_case("repository-display-conflict").at("input").at("versions");
    s.apply(versions);
    if (key == "parent-set-order" || key == "resolution-url-collision") {
      auto resolved = key == "parent-set-order" ? versions.back() : input.at("resolved");
      if (key == "parent-set-order") { resolved["versionId"] = uuid(); resolved["parentVersionIds"] = input.at("parentVersionIds"); }
      else s.repo(input.at("otherRepository").at("id"), input.at("otherRepository").at("normalizedUrl"));
      auto heads = key == "parent-set-order" ? input.at("parentVersionIds") : resolved.at("parentVersionIds");
      J request = {{"meta", s.f.meta()}, {"id", resolved.at("entityId")}, {"expectedHeadIds", heads}, {"resolved", resolved}};
      if (key == "resolution-url-collision") fixture_error(s.f, "ResolveConflict", request, expected);
      else require(s.f.call("ResolveConflict", request).at("result").at("version").at("parentVersionIds") == expected.at("parentVersionIds"), "fixture normalized parent set");
    } else assert_sync_result(s.f, versions.at(0).at("entityId"), expected, true);
  } else if (key == "move-preserves-event" || key == "repository-delete-cascade" || key == "deleted-repository-delayed-task" || key == "initial-repository-merge") {
    SyncVectors s;
    auto make_task = [&](const J& task_id, const J& repo_id, J prompt = nullptr) {
      auto request = s.f.createRequest("Fixture task"); request["id"] = task_id;
      request["repository"] = {{"id", repo_id}}; if (!prompt.is_null()) request["prompt"] = prompt;
      return s.f.task(request);
    };
    auto cache_event = [&](const J& event_id, const J& repo_id, const J& task_ids) {
      J event = {{"id", event_id}, {"repositoryId", repo_id}, {"receiverId", uuid()}, {"senderEventId", "fixture"},
          {"eventType", "fixture"}, {"occurredAt", "2026-01-01T00:00:00Z"}, {"payload", J::object()}, {"state", "EVENT_STATE_COMPLETED"}, {"revision", "1"}};
      J processing = {{"id", uuid()}, {"eventId", event_id}, {"taskIds", task_ids},
          {"completedAt", "2026-01-01T00:01:00Z"}, {"origin", s.f.meta().at("origin")}};
      auto packet = s.packet(J::array()); packet["pull"]["changes"] = J::array({J{{"sequence", "1"}, {"event", event}}, J{{"sequence", "2"}, {"processing", processing}}});
      success(s.f.store.applySync(s.f.space, packet.dump()));
    };
    if (key == "move-preserves-event") {
      s.repo(input.at("fromRepositoryId"), "move.example/source"); s.repo(input.at("toRepositoryId"), "move.example/target");
      auto task = make_task(input.at("taskId"), input.at("fromRepositoryId"), input.at("prompt"));
      auto event_id = uuid(); cache_event(event_id, input.at("eventRepositoryId"), J::array({input.at("taskId")}));
      auto body = task.at("task"); body["repositoryId"] = input.at("toRepositoryId"); auto moved = s.f.save(task, body);
      require(moved.at("entityId") == expected.at("taskId") && moved.at("task").at("repositoryId") == expected.at("repositoryId") && moved.at("task").at("prompt") == expected.at("prompt"), "fixture move preserves identity and prompt");
      require(s.f.call("GetEvent", {{"id", event_id}}).at("event").at("repositoryId") == expected.at("eventRepositoryId") && s.f.history(moved).size() == 2, "fixture event stays with original repository and history retained");
    } else if (key == "deleted-repository-delayed-task") {
      s.repo(input.at("deletedRepositoryId"));
      s.f.call("DeleteRepository", {{"meta", s.f.meta()}, {"id", input.at("deletedRepositoryId")}});
      auto version = find_case("whole-task-conflict").at("input").at("initial")[0];
      version["entityId"] = input.at("taskId"); version["task"]["repositoryId"] = input.at("taskRepositoryId");
      s.apply(J::array({version}));
      auto result = s.f.call("GetTask", {{"id", input.at("taskId")}}).at("result");
      require(result.at("result") == expected.at("result") && result.at("version").contains("task") == expected.at("taskBodyPresent"), "fixture deleted parent dominates delayed task");
    } else if (key == "repository-delete-cascade") {
      std::set<std::string> repos{input.at("repositoryId").get<std::string>()};
      for (const auto& t : input.at("tasks")) repos.insert(t.at("repositoryId"));
      for (const auto& r : repos) s.repo(r, "cascade.example/" + r);
      for (const auto& t : input.at("tasks")) {
        auto v = make_task(t.at("id"), t.value("previousRepositoryId", t.at("repositoryId")));
        if (t.contains("previousRepositoryId")) { auto body = v.at("task"); body["repositoryId"] = t.at("repositoryId"); s.f.save(v, body); }
      }
      for (const auto& e : input.at("events")) cache_event(e.at("id"), e.at("repositoryId"), e.at("linkedTaskIds"));
      s.f.call("DeleteRepository", {{"meta", s.f.meta()}, {"id", input.at("repositoryId")}});
      for (const auto& id : expected.at("deletedTaskIds")) require(s.f.call("GetTask", {{"id", id}}).at("result").at("result") == "RESULT_KIND_DELETED", "fixture cascade task");
      for (const auto& id : expected.at("retainedTaskIds")) require(s.f.call("ListHistory", {{"id", id}}).at("versions").size() == 2, "fixture moved history retained");
      require(items(s.f.call("ListEvents")).empty(), "fixture deleted events removed");
      auto r = s.f.call("GetRepository", {{"id", input.at("repositoryId")}}).at("result");
      require(!r.at("version").contains("repository"), "fixture deleted repository body removed");
    } else {
      auto local = s.repo(input.at("localRepositoryId"), input.at("normalizedUrl"));
      auto local_task = make_task(input.at("taskIds")[0], input.at("localRepositoryId"));
      auto remote = local; remote["entityId"] = input.at("remoteRepositoryId"); remote["versionId"] = uuid();
      auto remote_task = local_task; remote_task["entityId"] = input.at("taskIds")[1]; remote_task["versionId"] = uuid(); remote_task["task"]["repositoryId"] = input.at("remoteRepositoryId");
      auto packet = s.packet(J::array({remote, remote_task})); packet["push"] = {{"repositoryMappings", expected.at("repositoryMappings")}};
      success(s.f.store.applySync(s.f.space, packet.dump()));
      if (input.at("replay")) success(s.f.store.applySync(s.f.space, packet.dump()));
      std::set<std::string> ids;
      for (const auto& task : items(s.f.call("ListTasks"))) ids.insert(task.at("version").at("entityId"));
      require(ids == expected.at("taskIds").get<std::set<std::string>>() && items(s.f.call("ListRepositories")).size() == 1, "fixture repository merge identity dedupe");
      Sql db(s.f.database());
      require(J::parse(db.scalar("SELECT envelope_json FROM versions WHERE version_id='" + local_task.at("versionId").get<std::string>() + "'")) == local_task, "fixture immutable task snapshot not rewritten");
      require(db.scalar("SELECT canonical_id FROM repository_mappings WHERE local_id='" + input.at("localRepositoryId").get<std::string>() + "'") == input.at("remoteRepositoryId"), "fixture mapping persisted");
    }
  } else {
    throw std::runtime_error("sync vector has no runtime adapter: " + key);
  }
}
unsigned run_sync_fixtures(const fs::path& directory) {
  J cases;
  try { cases = load_vectors(directory, "sync"); }
  catch (const std::exception& e) { std::cerr << "FAIL sync fixtures: " << e.what() << '\n'; return 1; }
  const std::map<std::string, std::string> server_only = {
      {"first-resolution-wins", "server PushChanges serialization; native accepted-resolution import has independent coverage"},
      {"immutable-version-id", "server PushChanges existing transport receipt and REQUEST_ID_REUSED mapping"},
      {"per-item-limit", "server input-byte enforcement, not native result application"},
      {"lowered-limit-read-delete", "server configured input limit and read/delete exemption"}};
  unsigned failures = 0, executed = 0, skipped = 0;
  for (const auto& c : cases) {
    const auto id = c.at("id").get<std::string>();
    const auto skip = server_only.find(id);
    if (skip != server_only.end()) { ++skipped; std::cout << "SERVER-ONLY sync/" << id << ": " << skip->second << '\n'; continue; }
    ++executed;
    try { local_sync_vector(c, cases); std::cout << "PASS fixture sync/" << id << '\n'; }
    catch (const std::exception& e) { ++failures; std::cerr << "FAIL fixture sync/" << id << ": " << e.what() << '\n'; }
  }
  std::cout << executed << " native sync vectors executed; " << failures << " failures; " << skipped << " server-only vectors excluded\n";
  return failures;
}
