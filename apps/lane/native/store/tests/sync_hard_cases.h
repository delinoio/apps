// SPDX-License-Identifier: Apache-2.0
// Included inside the test namespace after shared fixture helpers.
J durable_snapshot(Sql& db) {
  J state = J::object();
  for (const auto* table : {"entities", "versions", "heads", "receipts", "outbox", "changes",
                            "remote_objects", "notification_receipts", "repository_mappings", "store_meta"}) {
    sqlite3_stmt* statement = nullptr;
    const std::string query = std::string("SELECT * FROM ") + table;
    require(sqlite3_prepare_v2(db.db, query.c_str(), -1, &statement, nullptr) == SQLITE_OK, "prepare durable snapshot");
    std::vector<std::string> rows;
    int rc;
    while ((rc = sqlite3_step(statement)) == SQLITE_ROW) {
      J row = J::array();
      for (int column = 0; column < sqlite3_column_count(statement); ++column) {
        if (sqlite3_column_type(statement, column) == SQLITE_NULL) row.push_back(nullptr);
        else row.push_back(std::string(reinterpret_cast<const char*>(sqlite3_column_text(statement, column)),
                                       sqlite3_column_bytes(statement, column)));
      }
      rows.push_back(row.dump());
    }
    sqlite3_finalize(statement);
    require(rc == SQLITE_DONE, "read durable snapshot");
    std::sort(rows.begin(), rows.end()); state[table] = rows;
  }
  return state;
}
J named_repository(Fixture& f, const std::string& name, const std::string& url) {
  return f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()},
      {"url", url}, {"displayName", name}}).at("result").at("version");
}
void merge_repository_histories() {
  Fixture f;
  auto local = named_repository(f, "Standalone name", "https://merge.example/repo");
  auto original = local;
  local = f.call("SaveRepository", {{"meta", f.meta()}, {"id", local.at("entityId")},
      {"expectedVersionId", local.at("versionId")}, {"url", "https://merge.example/repo"},
      {"displayName", "Standalone renamed"}}).at("result").at("version");
  auto request = f.createRequest("Standalone task"); request["repository"] = {{"id", local.at("entityId")}};
  auto local_task = f.task(request);
  const auto account = identity(uuid(), uuid(), uuid());
  success(f.store.approveAccount(account.dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId");
  auto online = named_repository(f, "Account name", "https://merge.example/repo");
  request = f.createRequest("Account task"); request["repository"] = {{"id", online.at("entityId")}};
  auto account_task = f.task(request);
  success(f.store.approveAccount(account.dump(), true));
  auto listed = items(f.call("ListRepositories"));
  require(listed.size() == 1, "same URL merge lists one repository");
  require(listed[0].at("result") == "RESULT_KIND_CONFLICT", "different display names preserve conflict");
  std::set<std::string> names;
  for (const auto& head : listed[0].at("alternatives")) names.insert(head.at("repository").at("displayName"));
  require(names == std::set<std::string>{"Standalone renamed", "Account name"}, "both display-name heads retained");
  auto history = f.call("ListHistory", {{"id", online.at("entityId")}}).at("versions");
  std::set<std::string> versions;
  for (const auto& v : history) versions.insert(v.at("versionId"));
  for (const auto& v : {original, local, online}) require(versions.count(v.at("versionId")) == 1, "union immutable repository histories");
  require(items(f.call("ListTasks")).size() == 2, "both distinct tasks survive merge");
  require(f.get(local_task.at("entityId")).at("entityId") == local_task.at("entityId") &&
      f.get(account_task.at("entityId")).at("entityId") == account_task.at("entityId"), "task IDs preserved");
  success(f.store.approveAccount(account.dump(), true));
  require(items(f.call("ListRepositories")).size() == 1 &&
      f.call("ListHistory", {{"id", online.at("entityId")}}).at("versions").size() == history.size(), "merge replay dedupe");
}
void mapping_alias_reads_and_writes() {
  Fixture f; RemoteProposals remote(f);
  auto local = named_repository(f, "Alias", "https://alias.example/repo");
  J canonical = local;
  canonical["entityId"] = uuid(); canonical["versionId"] = uuid();
  J push = {{"repositoryMappings", J::array({J{{"localId", local.at("entityId")}, {"canonicalId", canonical.at("entityId")}}})}};
  J sync = {{"formatVersion", 2}, {"epoch", remote.epoch}, {"owner", remote.owner}, {"push", push},
      {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"entity", {
          {"result", "RESULT_KIND_APPLIED"}, {"version", canonical}}}}})}, {"nextCursor", "alias-cursor"}}}};
  success(f.store.applySync(f.space, sync.dump()));
  unsigned matching = 0;
  for (const auto& result : items(f.call("ListRepositories")))
    if (result.at("version").at("repository").at("normalizedUrl") == "alias.example/repo") ++matching;
  require(matching == 1, "mapped aliases must not duplicate repository list");
  auto request = f.createRequest("Task using alias"); request["repository"] = {{"id", local.at("entityId")}};
  auto task = f.task(request);
  require(items(f.call("ListTasks", {{"repositoryId", canonical.at("entityId")}})).size() == 1, "alias task visible under canonical repository");
  require(items(f.call("ListTasks", {{"repositoryId", local.at("entityId")}})).size() == 1, "alias filter resolves canonical identity");
  auto body = task.at("task"); body["repositoryId"] = local.at("entityId"); body["title"] = "Alias accepted on save";
  f.save(task, body);
  success(f.store.applySync(f.space, sync.dump()));
  Sql db(f.database());
  require(db.scalar("SELECT envelope_json FROM versions WHERE version_id='" + local.at("versionId").get<std::string>() + "'") == local.dump(),
      "mapping preserves original immutable version");
}
void server_resolution_supersedes_provisional() {
  Fixture f; RemoteProposals remote(f); auto base = f.task();
  auto body = base.at("task"); body["title"] = "Left"; auto left = f.save(base, body);
  body["title"] = "Right";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", base.at("entityId")},
      {"expectedVersionId", base.at("versionId")}, {"task", body}}), "STALE_VERSION");
  auto conflict = f.call("GetTask", {{"id", base.at("entityId")}}).at("result");
  std::set<std::string> heads;
  for (const auto& v : conflict.at("alternatives")) heads.insert(v.at("versionId"));
  J provisional = left; provisional["versionId"] = uuid(); provisional["parentVersionIds"] = heads;
  provisional["task"]["title"] = "Local provisional";
  provisional = f.call("ResolveConflict", {{"meta", f.meta()}, {"id", base.at("entityId")},
      {"expectedHeadIds", heads}, {"resolved", provisional}}).at("result").at("version");
  J accepted = provisional; accepted["versionId"] = uuid(); accepted["task"]["title"] = "Server accepted";
  accepted["origin"]["deviceId"] = remote.remote_device;
  Sql db(f.database());
  auto mutation = J::parse(db.scalar("SELECT request_json FROM outbox WHERE entity_id='" +
      base.at("entityId").get<std::string>() + "' AND request_json LIKE '%" + provisional.at("versionId").get<std::string>() + "%' ORDER BY created_at_us DESC LIMIT 1"));
  J sync = {{"formatVersion", 2}, {"epoch", remote.epoch}, {"owner", remote.owner},
      {"push", {{"results", J::array({J{{"requestId", mutation.at("meta").at("requestId")},
          {"error", {{"reason", "ERROR_REASON_STALE_RESOLUTION"}, {"currentHeadIds", J::array({accepted.at("versionId")})}}}}})}}},
      {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"entity", {
          {"result", "RESULT_KIND_APPLIED"}, {"version", accepted}}}}})}, {"nextCursor", "resolution-cursor"}}}};
  success(f.store.applySync(f.space, sync.dump()));
  auto result = f.call("GetTask", {{"id", base.at("entityId")}}).at("result");
  require(result.at("result") != "RESULT_KIND_CONFLICT" && result.at("version").at("versionId") == accepted.at("versionId"),
      "server accepted resolution replaces provisional head");
  auto history = f.history(accepted); bool retained = false;
  for (const auto& v : history) retained |= v == provisional;
  require(retained && history.size() == 5, "rejected provisional resolution remains inspectable history");
  remote.apply(provisional);
  require(f.get(base.at("entityId")).at("versionId") == accepted.at("versionId"), "replayed rejected resolution cannot become head");
}
struct CachedEvent {
  J event, processing;
  CachedEvent(Fixture& f, RemoteProposals& remote, const J& repository_id, J payload = J{{"synthetic", true}}) {
    event = {{"id", uuid()}, {"receiverId", uuid()}, {"repositoryId", repository_id},
        {"senderEventId", "fixture-event"}, {"eventType", "test.completed"}, {"occurredAt", "2026-01-01T00:00:00Z"},
        {"payload", payload}, {"state", "EVENT_STATE_COMPLETED"}, {"revision", "1"}};
    processing = {{"id", uuid()}, {"eventId", event.at("id")}, {"taskIds", J::array()},
        {"completedAt", "2026-01-01T00:01:00Z"}, {"origin", f.meta().at("origin")}};
    success(f.store.applySync(f.space, J{{"formatVersion", 2}, {"epoch", remote.epoch}, {"owner", remote.owner},
        {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"event", event}},
            J{{"sequence", "2"}, {"processing", processing}}})}, {"nextCursor", "cache-cursor"}}}}.dump()));
  }
};
void repository_deletion_redacts_related_content() {
  Fixture f; RemoteProposals remote(f);
  auto repository = named_repository(f, "Private repository display fixture", "https://private-repo.example/source");
  auto request = f.createRequest("Moved task survives"); request["repository"] = {{"id", repository.at("entityId")}};
  auto task = f.task(request);
  CachedEvent cached(f, remote, repository.at("entityId"));
  auto body = task.at("task"); body["repositoryId"] = remote.repository.at("entityId");
  auto moved = f.save(task, body);
  f.call("DeleteRepository", {{"meta", f.meta()}, {"id", repository.at("entityId")}});
  require(f.get(task.at("entityId")) == moved, "moved task survives source deletion");
  Sql db(f.database());
  require(db.scalar("SELECT count(*) FROM remote_objects WHERE kind=3 AND body_json IS NOT NULL AND id='" +
      cached.processing.at("id").get<std::string>() + "'") == "0", "repository deletion removes associated processing body");
  require(db.scalar("SELECT count(*) FROM remote_objects WHERE kind=2 AND body_json IS NOT NULL AND id='" +
      cached.event.at("id").get<std::string>() + "'") == "0", "repository deletion removes event body");
  const auto receipt = db.scalar("SELECT result_json FROM receipts WHERE method='CreateTask' AND request_id='" +
      request.at("meta").at("requestId").get<std::string>() + "'");
  require(receipt.find("Private repository display fixture") == std::string::npos && receipt.find("private-repo.example") == std::string::npos,
      "moved task receipt redacts nested deleted repository content");
  auto replay = f.call("CreateTask", request);
  require(replay.dump().find("Private repository display fixture") == std::string::npos, "creation replay cannot disclose deleted repository");
}
void deletion_transaction_failure() {
  Fixture f; RemoteProposals remote(f); auto task = f.task();
  auto repo = task.at("task").at("repositoryId"); CachedEvent cached(f, remote, repo);
  Sql db(f.database()); auto before = counts(db); const auto snapshot = durable_snapshot(db); f.store.poll();
  db.exec("CREATE TRIGGER test_delete_failure BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'synthetic delete failure'); END;");
  J request = {{"meta", f.meta()}, {"id", repo}};
  require(!f.raw("DeleteRepository", request).value("ok", true), "deletion failure reported");
  require(counts(db) == before && durable_snapshot(db) == snapshot && f.get(task.at("entityId")) == task, "cascade deletion failure rolls back all durable state");
  require(f.call("GetEvent", {{"id", cached.event.at("id")}}).at("processing").size() == 1,
      "cascade deletion failure preserves event and processing");
  require(J::parse(f.store.poll()).empty(), "failed deletion emits no invalidation");
  db.exec("DROP TRIGGER test_delete_failure;"); f.call("DeleteRepository", request);
}
void deletion_disk_full() {
  DiskFullVfs fault;
  Fixture f; auto task = f.task(); Sql db(f.database());
  const auto before = counts(db); const auto snapshot = durable_snapshot(db); f.store.poll();
  fault.failing_root = fs::canonical(f.root.path).string(); fault.armed = true;
  auto request = J{{"meta", f.meta()}, {"id", task.at("task").at("repositoryId")}};
  auto result = f.raw("DeleteRepository", request);
  fault.armed = false;
  require(fault.failures > 0, "exercise actual SQLite write fault");
  error(result, "STORAGE_FULL");
  require(counts(db) == before && durable_snapshot(db) == snapshot && f.get(task.at("entityId")) == task, "SQLITE_FULL deletion rolls back all durable state");
  require(J::parse(f.store.poll()).empty(), "disk-full deletion emits no commit");
  f.call("DeleteRepository", request);
}
void typed_payload_preservation() {
  Fixture f; RemoteProposals remote(f);
  J payload = {{"snake_case", nullptr}, {"snakeCase", "distinct"},
      {"nested_object", {{"inner_key", nullptr}, {"null_array", J::array({nullptr, J{{"another_key", 3}}})}}}};
  CachedEvent cached(f, remote, remote.repository.at("entityId"), payload);
  auto read = f.call("GetEvent", {{"id", cached.event.at("id")}});
  require(read.at("event").at("payload") == payload, "Struct keys and null values survive applySync");
  require(items(f.call("ListEvents"))[0].at("payload") == payload, "Struct payload survives list query");
  Sql db(f.database());
  require(J::parse(db.scalar("SELECT body_json FROM remote_objects WHERE kind=2")).at("payload") == payload,
      "stored payload preserves exact object keys and nulls");
}
void duplicate_input_keys() {
  Fixture f; Sql db(f.database()); auto before = counts(db);
  auto request = f.createRequest();
  auto raw_request = request.dump(); raw_request.pop_back(); raw_request += ",\"title\":\"Duplicate title\"}";
  error(J::parse(f.store.invoke("CreateTask", "{\"spaceId\":\"" + f.space + "\",\"request\":" + raw_request + "}")), "INVALID_INPUT");
  error(J::parse(f.store.open("{\"formatVersion\":1,\"formatVersion\":1,\"spaceId\":\"" + f.space + "\"}")), "INVALID_INPUT");
  require(counts(db) == before, "duplicate input keys rejected without writes");
  RemoteProposals remote(f);
  auto sync = J{{"formatVersion", 2}, {"epoch", remote.epoch}, {"owner", remote.owner},
      {"pull", {{"changes", J::array()}, {"nextCursor", ""}}}}.dump();
  sync.pop_back(); sync += ",\"pull\":{\"changes\":[],\"nextCursor\":\"forged\"}}";
  error(J::parse(f.store.applySync(f.space, sync)), "INVALID_INPUT");
}
void crossprocess_busy() {
#if defined(__unix__) || defined(__APPLE__)
  Fixture f; auto task = f.task();
  int ready[2], release[2];
  require(pipe(ready) == 0 && pipe(release) == 0, "create child synchronization pipes");
  const auto path = f.database().string();
  const pid_t child = fork();
  require(child >= 0, "fork external SQL writer");
  if (child == 0) {
    ::close(ready[0]); ::close(release[1]);
    sqlite3* db = nullptr;
    int rc = sqlite3_open(path.c_str(), &db);
    if (rc == SQLITE_OK) rc = sqlite3_exec(db, "BEGIN IMMEDIATE", nullptr, nullptr, nullptr);
    const char status = rc == SQLITE_OK ? '1' : '0';
    (void)::write(ready[1], &status, 1);
    char token;
    (void)::read(release[0], &token, 1);
    if (rc == SQLITE_OK) sqlite3_exec(db, "ROLLBACK", nullptr, nullptr, nullptr);
    sqlite3_close(db); _exit(rc == SQLITE_OK ? 0 : 1);
  }
  ::close(ready[1]); ::close(release[0]);
  struct ChildLock {
    pid_t child; int release, ready;
    ~ChildLock() {
      const char token = '1'; (void)::write(release, &token, 1);
      ::close(release); ::close(ready); int status; (void)waitpid(child, &status, 0);
    }
  };
  J request = {{"meta", f.meta()}, {"id", task.at("entityId")},
      {"expectedVersionId", task.at("versionId")}, {"progress", "PROGRESS_DONE"}};
  {
    ChildLock lock{child, release[1], ready[0]}; char token = 0;
    require(::read(ready[0], &token, 1) == 1 && token == '1', "child acquired independent SQLite write lock");
    const auto start = std::chrono::steady_clock::now();
    error(f.raw("SetProgress", request), "STORAGE_BUSY");
    const auto elapsed = std::chrono::steady_clock::now() - start;
    require(elapsed >= std::chrono::seconds(4) && elapsed < std::chrono::seconds(15), "bounded SQLite busy timeout");
    require(f.get(task.at("entityId")) == task, "busy writer leaves task unchanged");
  }
  f.call("SetProgress", request);
  require(f.history(task).size() == 2, "same request retries after cross-process lock release");
#else
  std::cout << "SKIP cross-process busy: POSIX process test requires Unix host\n";
#endif
}
void account_deletion_binding() {
  Fixture f; RemoteProposals remote(f); auto task = f.task();
  CachedEvent event(f, remote, task.at("task").at("repositoryId"));
  Sql db(f.database()), registry(f.root.path / "registry.sqlite");
  const auto account = registry.scalar("SELECT account_id FROM spaces WHERE id='" + f.space + "'");
  int disconnects = 0;
  lane::HostHooks hooks;
  hooks.disconnect = [&](std::string space) { require(space == f.space, "account deletion disconnect routing"); ++disconnects; };
  f.store.setHostHooks(hooks);
  auto change = [&](const std::string& target) {
    return J{{"formatVersion", 2}, {"epoch", remote.epoch}, {"owner", remote.owner},
        {"pull", {{"changes", J::array({J{{"sequence", "3"}, {"deletion", {
            {"kind", "ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT"}, {"id", target}, {"deletionVersionId", uuid()}}}}})},
            {"nextCursor", "account-delete-cursor"}}}};
  };
  auto before = counts(db);
  const auto snapshot = durable_snapshot(db);
  const auto cache_before = db.scalar("SELECT count(*) FROM remote_objects");
  const auto cursor_before = db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'");
  auto wrong = change(uuid());
  // Apply a preceding cache update in the same pull to prove full rollback,
  // not just rejection before reaching the account marker.
  auto altered = event.event; altered["payload"] = {{"must_rollback", true}};
  wrong["pull"]["changes"].insert(wrong["pull"]["changes"].begin(), J{{"sequence", "2"}, {"event", altered}});
  error(J::parse(f.store.applySync(f.space, wrong.dump())), "FORBIDDEN");
  require(counts(db) == before && durable_snapshot(db) == snapshot && db.scalar("SELECT count(*) FROM remote_objects") == cache_before &&
      db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'") == cursor_before && disconnects == 0,
      "cross-account marker rolls back state and cursor without disconnect");
  require(f.call("GetEvent", {{"id", event.event.at("id")}}).at("event") == event.event, "cross-account marker rolls back preceding cache write");
  auto stale = change(account); stale["epoch"] = uuid();
  error(J::parse(f.store.applySync(f.space, stale.dump())), "STALE_EPOCH");
  require(counts(db) == before && f.get(task.at("entityId")) == task && disconnects == 0, "stale epoch never erases local content");
  // CLEAR_SERVER_KEEP_LOCAL is a stale-epoch/disconnection outcome, not an
  // ACCOUNT deletion marker. Native failure recording must preserve bodies.
  success(f.store.recordSyncFailure(f.space, "ERROR_REASON_STALE_EPOCH"));
  require(f.get(task.at("entityId")) == task &&
      f.call("GetEvent", {{"id", event.event.at("id")}}).at("event") == event.event, "clear-server keep-local retains task and cached content");
  // Explicitly reconnect after stale epoch before accepting a valid marker.
  const auto tenant = registry.scalar("SELECT tenant_id FROM spaces WHERE id='" + f.space + "'");
  success(f.store.approveAccount(identity(account, tenant, remote.epoch).dump(), false));
  success(f.store.acquireSync(f.space, remote.owner));
  disconnects = 0;
  success(f.store.applySync(f.space, change(account).dump()));
  for (const auto* table : {"entities", "versions", "heads", "receipts", "outbox", "remote_objects", "notification_receipts"})
    require(db.scalar(std::string("SELECT count(*) FROM ") + table) == "0", "valid account marker erases content tables");
  require(registry.scalar("SELECT sync_enabled FROM spaces WHERE id='" + f.space + "'") == "0", "account deletion disables registry synchronization");
  require(db.scalar("SELECT value FROM store_meta WHERE key='sync_owner'").empty(), "account deletion releases sync owner");
  require(disconnects == 1, "account deletion invokes host disconnect once");
}
