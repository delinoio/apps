// SPDX-License-Identifier: Apache-2.0
// Included by store_test.cc inside its test namespace. Native planning
// behavior that the shared vectors cannot express: persistence across restart,
// idempotent replay, conflicts, redaction, rollback atomicity, rejected
// synchronization, format compatibility and the development reset.
J planning_item(Fixture& f, const std::string& kind, const J& body, const std::string& id = "") {
  const auto item_id = id.empty() ? uuid() : id;
  return f.call("CreatePlanningItem", {{"meta", f.meta()}, {"id", item_id}, {"item", {{kind, body}}}})
      .at("result").at("version");
}
J repository_for(Fixture& f, const std::string& url = "https://planning.example/repo") {
  return f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()}, {"url", url}}).at("result").at("version");
}
J planned_task(Fixture& f, const std::string& repository, J extra = J::object()) {
  J request = {{"meta", f.meta()}, {"id", uuid()}, {"repository", {{"id", repository}}}, {"title", "Planned"}};
  for (auto it = extra.begin(); it != extra.end(); ++it) request[it.key()] = it.value();
  return f.task(request);
}
void planning_lifecycle_and_restart() {
  Fixture f;
  auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto label = planning_item(f, "label", {{"repositoryId", repo_id}, {"name", "  Bug  "}, {"color", "#aabbcc"}, {"description", "Defects"}});
  require(label.at("label") == J({{"repositoryId", repo_id}, {"name", "Bug"}, {"color", "#AABBCC"}, {"description", "Defects"}}) &&
          label.at("kind") == "ENTITY_KIND_LABEL" && label.at("formatVersion") == 3, "label normalized and stored in format 3");
  auto milestone = planning_item(f, "milestone", {{"repositoryId", repo_id}, {"name", "Beta"}, {"targetDate", "2026-03-01"}});
  auto cycle = planning_item(f, "cycle", {{"repositoryId", repo_id}, {"name", "Sprint"}, {"startDate", "2026-01-05"}, {"endDate", "2026-01-18"}});
  const auto label_id = label.at("entityId").get<std::string>(), milestone_id = milestone.at("entityId").get<std::string>(),
             cycle_id = cycle.at("entityId").get<std::string>();
  auto task = planned_task(f, repo_id, {{"priority", "TASK_PRIORITY_URGENT"}, {"type", "TASK_TYPE_BUG"},
      {"labelIds", J::array({label_id, label_id})}, {"startDate", "2026-01-06"}, {"dueDate", "2026-01-07"},
      {"storyPoints", 0}, {"milestoneId", milestone_id}, {"cycleId", cycle_id}, {"prompt", "Original"}});
  const auto task_id = task.at("entityId").get<std::string>();
  require(task.at("task").at("labelIds") == J::array({label_id}) && task.at("task").at("storyPoints") == 0, "defaults and dedupe");
  // Specialized mutations keep every planning field; a prompt restore too.
  auto progressed = f.call("SetProgress", {{"meta", f.meta()}, {"id", task_id}, {"expectedVersionId", task.at("versionId")},
      {"progress", "PROGRESS_DONE"}}).at("result").at("version");
  auto body = progressed.at("task"); body["prompt"] = "Changed";
  auto edited = f.save(progressed, body);
  auto restored = f.call("RestorePrompt", {{"meta", f.meta()}, {"id", task_id}, {"expectedVersionId", edited.at("versionId")},
      {"sourceVersionId", task.at("versionId")}}).at("result").at("version");
  for (const auto* key : {"priority", "type", "labelIds", "startDate", "dueDate", "storyPoints", "milestoneId", "cycleId"})
    require(restored.at("task").at(key) == task.at("task").at(key), std::string("planning field survives mutations: ") + key);
  // Replay never duplicates a planning item or its history.
  J create = {{"meta", f.meta()}, {"id", uuid()}, {"item", {{"label", {{"repositoryId", repo_id}, {"name", "Replay"}, {"color", "#000000"}}}}}};
  auto first = f.call("CreatePlanningItem", create).at("result").at("version");
  require(f.call("CreatePlanningItem", create).at("result").at("result") == "RESULT_KIND_REPLAYED", "planning replay");
  require(f.call("ListHistory", {{"id", first.at("entityId")}}).at("versions").size() == 1, "replay appends no history");
  create["item"]["label"]["name"] = "Different";
  error(f.raw("CreatePlanningItem", create), "REQUEST_ID_REUSED");
  // Planning edits write versions in the item's own history.
  auto saved = f.call("SavePlanningItem", {{"meta", f.meta()}, {"id", label_id}, {"expectedVersionId", label.at("versionId")},
      {"item", {{"label", {{"repositoryId", repo_id}, {"name", "Bug"}, {"color", "#112233"}}}}}}).at("result").at("version");
  require(f.call("ListHistory", {{"id", label_id}}).at("versions").size() == 2 && saved.at("label").at("color") == "#112233", "planning history");
  // Listing filters by kind and repository and pages without repeating items.
  require(items(f.call("ListPlanningItems", {{"repositoryId", repo_id}})).size() == 4, "all planning kinds");
  require(items(f.call("ListPlanningItems", {{"kind", "ENTITY_KIND_LABEL"}})).size() == 2, "kind filter");
  error(f.raw("ListPlanningItems", {{"kind", "ENTITY_KIND_TASK"}}), "INVALID_INPUT");
  std::set<std::string> paged; std::string cursor;
  do {
    auto page = f.call("ListPlanningItems", {{"page", {{"size", 1}, {"cursor", cursor}}}});
    for (const auto& item : items(page)) require(paged.insert(item.at("version").at("entityId")).second, "page repeats an item");
    cursor = page.value("nextCursor", "");
  } while (!cursor.empty());
  require(paged.size() == 4, "paged planning items");
  // Restart: everything is durable in the schema 3 file.
  success(f.store.close(f.space));
  lane::Store reopened(f.root.path.string());
  success(reopened.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  auto reread = success(reopened.invoke("GetTask", J{{"spaceId", f.space}, {"request", {{"id", task_id}}}}.dump()));
  require(reread.at("result").at("version") == restored, "task planning fields persist across restart");
  auto item = success(reopened.invoke("GetPlanningItem", J{{"spaceId", f.space}, {"request", {{"id", label_id}}}}.dump()));
  require(item.at("result").at("version") == saved, "planning item persists across restart");
  Sql db(f.database());
  require(db.scalar("PRAGMA user_version") == "3" && db.scalar("SELECT value FROM store_meta WHERE key='format_version'") == "3", "schema 3 on disk");
}
void planning_conflicts_and_resolution() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto milestone = planning_item(f, "milestone", {{"repositoryId", repo_id}, {"name", "Base"}});
  auto save = [&](const J& base, const std::string& name) {
    return J{{"meta", f.meta()}, {"id", milestone.at("entityId")}, {"expectedVersionId", base.at("versionId")},
             {"item", {{"milestone", {{"repositoryId", repo_id}, {"name", name}}}}}};
  };
  auto left = f.call("SavePlanningItem", save(milestone, "Left")).at("result").at("version");
  error(f.raw("SavePlanningItem", save(milestone, "Right")), "STALE_VERSION");
  auto conflict = f.call("GetPlanningItem", {{"id", milestone.at("entityId")}}).at("result");
  require(conflict.at("result") == "RESULT_KIND_CONFLICT" && conflict.at("alternatives").size() == 2, "whole-item conflict");
  error(f.raw("SavePlanningItem", save(left, "Blocked")), "CONFLICT_READ_ONLY");
  error(f.raw("DeletePlanningItem", {{"meta", f.meta()}, {"id", uuid()}}), "NOT_FOUND");
  std::vector<std::string> heads;
  for (const auto& alternative : conflict.at("alternatives")) heads.push_back(alternative.at("versionId"));
  std::sort(heads.begin(), heads.end());
  J resolved = left; resolved["versionId"] = uuid(); resolved["parentVersionIds"] = heads; resolved["milestone"]["name"] = "Merged";
  auto winner = f.call("ResolveConflict", {{"meta", f.meta()}, {"id", milestone.at("entityId")}, {"expectedHeadIds", heads},
      {"resolved", resolved}}).at("result").at("version");
  require(winner.at("milestone").at("name") == "Merged" && f.call("ListHistory", {{"id", milestone.at("entityId")}}).at("versions").size() == 4,
          "resolution keeps alternatives in history");
}
void planning_deletion_redaction_and_cascade() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto label = planning_item(f, "label", {{"repositoryId", repo_id}, {"name", "SecretLabelName"}, {"color", "#010203"}, {"description", "SecretDescription"}});
  auto cycle = planning_item(f, "cycle", {{"repositoryId", repo_id}, {"name", "Cycle"}, {"startDate", "2026-01-01"}, {"endDate", "2026-01-02"}});
  auto task = planned_task(f, repo_id, {{"labelIds", J::array({label.at("entityId")})}, {"cycleId", cycle.at("entityId")}});
  auto deleted = f.call("DeletePlanningItem", {{"meta", f.meta()}, {"id", label.at("entityId")}}).at("result");
  require(deleted.at("result") == "RESULT_KIND_DELETED" && !deleted.at("version").contains("label"), "deleted without content");
  auto detached = f.get(task.at("entityId"));
  require(!detached.at("task").contains("labelIds") && detached.at("task").contains("cycleId") &&
          detached.at("parentVersionIds") == J::array({task.at("versionId")}), "cleanup version detaches only the deleted label");
  require(f.call("ListHistory", {{"id", label.at("entityId")}}).value("versions", J::array()).empty(), "deleted item history erased");
  {
    Sql db(f.database());
    for (const auto* query : {"SELECT count(*) FROM versions WHERE envelope_json LIKE '%SecretLabelName%' OR envelope_json LIKE '%SecretDescription%'",
                              "SELECT count(*) FROM receipts WHERE result_json LIKE '%SecretLabelName%' OR result_json LIKE '%SecretDescription%'",
                              "SELECT count(*) FROM outbox WHERE request_json LIKE '%SecretLabelName%' OR request_json LIKE '%SecretDescription%'"})
      require(db.scalar(query) == "0", "deleted planning content survives nowhere");
  }
  require(f.call("DeletePlanningItem", {{"meta", f.meta()}, {"id", label.at("entityId")}}).at("result").at("result") == "RESULT_KIND_DELETED", "idempotent deleted replay");
  // Repository deletion cascades planning items and tasks.
  f.call("DeleteRepository", {{"meta", f.meta()}, {"id", repo_id}});
  require(f.call("GetPlanningItem", {{"id", cycle.at("entityId")}}).at("result").at("result") == "RESULT_KIND_DELETED", "cascade deletes planning items");
  require(items(f.call("ListPlanningItems")).empty() && items(f.call("ListTasks")).empty(), "cascade leaves no planning items or tasks");
}
void planning_cleanup_rolls_back_atomically() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto label = planning_item(f, "label", {{"repositoryId", repo_id}, {"name", "L"}, {"color", "#010203"}});
  auto parent = planned_task(f, repo_id);
  auto child = planned_task(f, repo_id, {{"labelIds", J::array({label.at("entityId")})}, {"parentTaskId", parent.at("entityId")}});
  Sql db(f.database());
  const auto before = counts(db);
  db.exec("CREATE TRIGGER fail_cleanup BEFORE INSERT ON outbox WHEN NEW.entity_id='" + child.at("entityId").get<std::string>() +
          "' BEGIN SELECT RAISE(ABORT,'synthetic cleanup failure'); END;");
  require(!f.raw("DeletePlanningItem", {{"meta", f.meta()}, {"id", label.at("entityId")}}).at("ok").get<bool>(), "label cleanup failure reported");
  require(!f.raw("DeleteTask", {{"meta", f.meta()}, {"id", parent.at("entityId")}}).at("ok").get<bool>(), "task cleanup failure reported");
  require(counts(db) == before && f.get(child.at("entityId")) == child, "no partial cleanup, history, receipt or pending record");
  require(f.call("GetPlanningItem", {{"id", label.at("entityId")}}).at("result").at("result") == "RESULT_KIND_APPLIED", "item survives failed delete");
  db.exec("DROP TRIGGER fail_cleanup;");
  f.call("DeleteTask", {{"meta", f.meta()}, {"id", parent.at("entityId")}});
  require(!f.get(child.at("entityId")).at("task").contains("parentTaskId"), "retry cleans the incoming reference");
}
void planning_conflicted_tasks_block_cleanup() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto target = planned_task(f, repo_id);
  auto other = planned_task(f, repo_id, {{"parentTaskId", target.at("entityId")}});
  auto body = other.at("task"); body["title"] = "Left"; f.save(other, body);
  body["title"] = "Right";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", other.at("entityId")}, {"expectedVersionId", other.at("versionId")}, {"task", body}}), "STALE_VERSION");
  Sql db(f.database()); const auto before = counts(db);
  error(f.raw("DeleteTask", {{"meta", f.meta()}, {"id", target.at("entityId")}}), "CONFLICT_READ_ONLY");
  require(counts(db) == before, "a conflicted affected task fails the whole operation");
}
void planning_sync_rejection_and_retry() {
  Fixture f; const auto epoch = uuid();
  success(f.store.approveAccount(identity(uuid(), uuid(), epoch).dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
  const auto owner = uuid(); success(f.store.acquireSync(f.space, owner));
  auto repo = repository_for(f); auto task = planned_task(f, repo.at("entityId"));
  Sql db(f.database());
  auto request_for = [&](const std::string& entity) {
    return db.scalar("SELECT request_id FROM outbox WHERE entity_id='" + entity + "' ORDER BY created_at_us LIMIT 1");
  };
  const auto rid = request_for(task.at("entityId")), other = request_for(repo.at("entityId"));
  auto push = [&](J results) {
    return success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", epoch}, {"owner", owner}, {"push", {{"results", results}}}}.dump()));
  };
  push(J::array({J{{"requestId", rid}, {"error", {{"reason", "ERROR_REASON_CONSTRAINT_VIOLATION"},
      {"violation", "CONSTRAINT_VIOLATION_PARENT_CYCLE"}, {"entityId", task.at("entityId")}, {"retryable", false}}}}}));
  require(db.scalar("SELECT state FROM outbox WHERE request_id='" + rid + "'") == "2" &&
          db.scalar("SELECT error_reason FROM outbox WHERE request_id='" + rid + "'") == "CONSTRAINT_VIOLATION:PARENT_CYCLE", "rejected item stays blocked with its violation");
  require(f.get(task.at("entityId")) == task, "the rejected local version is kept");
  auto status = f.call("GetLocalStatus");
  bool listed = false;
  for (const auto& e : status.at("itemErrors"))
    listed |= e.at("requestId") == rid && e.at("entityId") == task.at("entityId") && e.at("reason") == "CONSTRAINT_VIOLATION:PARENT_CYCLE";
  require(listed && status.at("pendingItemCount") == "2", "status exposes the rejected item without marking it synchronized");
  auto pending = success(f.store.pendingSync(f.space, owner)).at("mutations");
  require(pending.size() == 1 && pending[0].at("meta").at("requestId") == other, "blocked items are not re-sent implicitly");
  // Numeric enum values and other violations are mapped as well.
  push(J::array({J{{"requestId", other}, {"error", {{"reason", 19}, {"violation", 2}}}}}));
  require(db.scalar("SELECT error_reason FROM outbox WHERE request_id='" + other + "'") == "CONSTRAINT_VIOLATION:DEPENDENCY_CYCLE", "numeric reason and violation");
  require(success(f.store.pendingSync(f.space, owner)).at("mutations").empty(), "nothing pending while all items are blocked");
  require(f.call("RetryBlockedItems").at("completed") == true, "explicit retry action");
  require(db.scalar("SELECT count(*) FROM outbox WHERE state=1 AND error_reason IS NULL") == "2" &&
          success(f.store.pendingSync(f.space, owner)).at("mutations").size() == 2, "retry restores the original pending requests");
  // An ordinary edit followed by retry pushes a new immutable version.
  auto edited = f.save(task, [&] { auto b = task.at("task"); b["title"] = "Corrected"; return b; }());
  require(edited.at("versionId") != task.at("versionId") && db.scalar("SELECT count(*) FROM outbox") == "3", "correction is a new pending version");
}
void planning_envelope_formats() {
  Fixture f; RemoteProposals remote(f);
  const auto repo_id = remote.repository.at("entityId").get<std::string>();
  auto label = [&](int format) {
    return J{{"formatVersion", format}, {"entityId", uuid()}, {"kind", "ENTITY_KIND_LABEL"}, {"versionId", uuid()},
        {"parentVersionIds", J::array()}, {"createdAt", "2026-01-01T00:00:00Z"},
        {"origin", {{"kind", "ORIGIN_KIND_CLI"}, {"deviceId", remote.remote_device}, {"deviceName", "Remote"}, {"accountId", ""}}},
        {"deleted", false}, {"label", {{"repositoryId", repo_id}, {"name", "Pulled"}, {"color", "#ABCDEF"}}}};
  };
  Sql db(f.database()); auto before = counts(db);
  for (int format : {1, 2}) {
    error(J::parse(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", remote.epoch}, {"owner", remote.owner},
        {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"entity", {{"version", label(format)}}}}})}}}}.dump())), "INVALID_INPUT");
    require(counts(db) == before, "planning data in an earlier envelope format is refused atomically");
  }
  auto pulled = label(3);
  remote.apply(pulled);
  require(f.call("GetPlanningItem", {{"id", pulled.at("entityId")}}).at("result").at("version") == pulled, "format 3 planning item pulled");
  // Earlier envelopes without planning fields stay readable as unassigned.
  for (int format : {1, 2}) {
    J version = {{"formatVersion", format}, {"entityId", uuid()}, {"kind", "ENTITY_KIND_TASK"}, {"versionId", uuid()},
        {"parentVersionIds", J::array()}, {"createdAt", "2026-01-01T00:00:00Z"},
        {"origin", {{"kind", "ORIGIN_KIND_CLI"}, {"deviceId", remote.remote_device}, {"deviceName", "Remote"}, {"accountId", ""}}},
        {"deleted", false}, {"task", {{"repositoryId", repo_id}, {"title", "Old"}, {"description", ""},
            {"progress", "PROGRESS_TODO"}, {"proposal", "PROPOSAL_REGULAR"}}}};
    remote.apply(version);
    auto task = f.get(version.at("entityId"));
    require(!task.at("task").contains("priority") && !task.at("task").contains("labelIds"), "earlier formats read as unassigned");
  }
  error(J::parse(f.store.applySync(f.space, J{{"formatVersion", 2}, {"epoch", remote.epoch}, {"owner", remote.owner}}.dump())), "UNSUPPORTED_FORMAT");
}
void unsupported_store_and_reset() {
  // Any non-3 store is refused with the reset instruction and never replaced.
  Fixture f; f.task(); success(f.store.close(f.space));
  {
    Sql db(f.database());
    db.exec("PRAGMA user_version=2; UPDATE store_meta SET value='2' WHERE key='format_version';");
  }
  lane::Store old(f.root.path.string());
  auto refused = J::parse(old.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  error(refused, "UNSUPPORTED_FORMAT");
  require(refused.at("error").at("message").get<std::string>().find("ResetDevelopmentData") != std::string::npos, "message names the reset");
  { Sql db(f.database()); require(db.scalar("PRAGMA user_version") == "2" && db.scalar("SELECT count(*) FROM versions") != "0", "open never resets or migrates"); }
  error(J::parse(old.open(R"({"formatVersion":2,"spaceId":""})")), "UNSUPPORTED_FORMAT");
  // Reset rejections change nothing.
  auto reset = [&](lane::Store& store, const J& request, const std::string& space) {
    return J::parse(store.invoke("ResetDevelopmentData", J{{"spaceId", space}, {"request", request}}.dump()));
  };
  error(reset(old, J::object(), f.space), "INVALID_INPUT");
  error(reset(old, {{"confirmed", false}}, f.space), "INVALID_INPUT");
  error(reset(old, {{"confirmed", "true"}}, f.space), "INVALID_INPUT");
  error(reset(old, {{"confirmed", true}, {"extra", 1}}, f.space), "INVALID_INPUT");
  error(reset(old, {{"confirmed", true}}, uuid()), "NOT_FOUND");
  { Sql db(f.database()); require(db.scalar("PRAGMA user_version") == "2", "rejected reset changes nothing"); }
  // The explicit reset works with no open handle and keeps the space ID.
  require(success(old.invoke("ResetDevelopmentData", J{{"spaceId", f.space}, {"request", {{"confirmed", true}}}}.dump())).at("completed") == true, "reset completes");
  auto opened = success(old.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  require(opened.at("spaceId") == f.space && opened.at("deviceId") == f.device && opened.at("sequence") == "0" &&
          opened.at("schemaVersion") == 3, "same space and device, fresh sequence");
  Sql db(f.database());
  for (const auto* table : {"entities", "versions", "heads", "receipts", "outbox", "changes", "attachments", "attachment_identities"})
    require(db.scalar(std::string("SELECT count(*) FROM ") + table) == "0", std::string("empty after reset: ") + table);
  require(db.scalar("PRAGMA user_version") == "3" && db.scalar("SELECT value FROM store_meta WHERE key='format_version'") == "3" &&
          db.scalar("SELECT value FROM store_meta WHERE key='space_id'") == f.space, "empty schema 3 store");
  Sql registry(f.root.path / "registry.sqlite");
  require(registry.scalar("SELECT count(*) FROM registry_meta WHERE key LIKE 'unavailable:%' OR key LIKE 'reset:%'") == "0", "no unavailable or pending marker");
  require(fs::exists(f.root.path / "locks" / (f.space + ".handles.lock")), "rendezvous lock retained");
}
void development_reset_coordination() {
  Fixture f; auto task = f.task(); const auto standalone = f.space;
  // A second space (an approved account) is isolated from the reset of the first.
  const auto epoch = uuid(), account = uuid(), tenant = uuid();
  success(f.store.approveAccount(identity(account, tenant, epoch).dump(), false));
  const auto account_space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId").get<std::string>();
  f.space = account_space; auto account_task = f.task();
  f.space = standalone;
  {
    // Another handle (any process) makes the exclusive wait fail with no change.
    lane::Store other(f.root.path.string());
    success(other.open(J{{"formatVersion", 3}, {"spaceId", standalone}}.dump()));
    const auto started = std::chrono::steady_clock::now();
    error(f.raw("ResetDevelopmentData", {{"confirmed", true}}), "STORAGE_BUSY");
    require(std::chrono::steady_clock::now() - started < std::chrono::seconds(15), "bounded wait");
    require(f.raw("ResetDevelopmentData", {{"confirmed", true}}).at("error").at("retryable") == true, "busy reset is retryable");
    require(f.get(task.at("entityId")) == task, "busy reset changes nothing");
  }
  success(f.store.close(f.space));
  require(f.call("ResetDevelopmentData", {{"confirmed", true}}).at("completed") == true, "reset after handles drain");
  success(f.store.open(J{{"formatVersion", 3}, {"spaceId", standalone}}.dump())); // Hosts reopen after a reset.
  error(f.raw("GetTask", {{"id", task.at("entityId")}}), "NOT_FOUND");
  require(items(f.call("ListTasks")).empty() && f.call("GetLocalStatus").at("spaceId") == standalone, "standalone space stays selected and empty");
  f.space = account_space;
  success(f.store.open(J{{"formatVersion", 3}, {"spaceId", account_space}}.dump()));
  require(f.get(account_task.at("entityId")) == account_task, "other spaces are untouched");
  // Standalone spaces keep their identity and still accept writes after reset.
  f.space = standalone; auto fresh = f.task();
  require(f.history(fresh).size() == 1, "new data after reset");
  // Account space: data is cleared, the binding is kept and reconnect is required.
  f.space = account_space;
  success(f.store.close(account_space));
  require(f.call("ResetDevelopmentData", {{"confirmed", true}}).at("completed") == true, "account space reset");
  success(f.store.open(J{{"formatVersion", 3}, {"spaceId", account_space}}.dump()));
  Sql registry(f.root.path / "registry.sqlite");
  require(registry.scalar("SELECT account_id FROM spaces WHERE id='" + account_space + "'") == account &&
          registry.scalar("SELECT sync_enabled FROM spaces WHERE id='" + account_space + "'") == "0", "binding kept and sync stopped");
  Sql db(f.database());
  require(db.scalar("SELECT value FROM store_meta WHERE key='reconnect_required'") == "1" &&
          db.scalar("SELECT value FROM store_meta WHERE key='account_epoch'") == epoch &&
          db.scalar("SELECT count(*) FROM versions") == "0", "reconnect confirmation required, epoch kept");
  error(f.raw("GetTask", {{"id", account_task.at("entityId")}}), "NOT_FOUND");
  success(f.store.close(account_space));
  // A restarted process sees the reset spaces and their new format.
  lane::Store restarted(f.root.path.string());
  auto reopened = success(restarted.open(J{{"formatVersion", 3}, {"spaceId", standalone}}.dump()));
  require(reopened.at("schemaVersion") == 3 && reopened.at("sequence") != "0", "restart reads the reset standalone space with its new data");
}
void interrupted_reset_is_repeatable() {
  Fixture f; f.task(); success(f.store.close(f.space));
  {
    // An interruption after removal but before recreation leaves the marker.
    Sql registry(f.root.path / "registry.sqlite");
    registry.exec("INSERT INTO registry_meta VALUES('reset:" + f.space + "','{\"account\":false,\"epoch\":\"\"}')");
  }
  std::error_code ec; fs::remove_all(f.root.path / "spaces" / f.space, ec);
  lane::Store restarted(f.root.path.string());
  error(J::parse(restarted.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump())), "STORAGE_BUSY");
  require(!fs::exists(f.database()) , "a partially removed space is never opened as valid data");
  require(success(restarted.reset(f.space)).at("completed") == true, "repeating the reset finishes it");
  require(success(restarted.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump())).at("sequence") == "0", "valid empty store afterwards");
}
void planning_conflict_union_cycle_and_cursor_scope() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto a = planned_task(f, repo_id); auto b = planned_task(f, repo_id);
  // A becomes conflicted: one head links to B, the other only changes the title.
  auto linked = a.at("task"); linked["parentTaskId"] = b.at("entityId");
  f.save(a, linked);
  auto titled = a.at("task"); titled["title"] = "Alternative";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", a.at("entityId")}, {"expectedVersionId", a.at("versionId")}, {"task", titled}}), "STALE_VERSION");
  // Cycle checks use the union of every head's references.
  auto parent = b.at("task"); parent["parentTaskId"] = a.at("entityId");
  auto rejected = f.raw("SaveTask", {{"meta", f.meta()}, {"id", b.at("entityId")}, {"expectedVersionId", b.at("versionId")}, {"task", parent}});
  error(rejected, "CONSTRAINT_VIOLATION");
  require(rejected.at("error").at("violation") == "CONSTRAINT_VIOLATION_PARENT_CYCLE", "union of conflicting heads closes the cycle");
  // A relationship edit of a conflicted task fails the operation without writes.
  auto edits = b.at("task");
  Sql db(f.database()); const auto before = counts(db);
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", b.at("entityId")}, {"expectedVersionId", b.at("versionId")}, {"task", edits},
      {"relationshipEdits", {{"addSubtaskIds", J::array({a.at("entityId")})}}}}), "CONFLICT_READ_ONLY");
  require(counts(db) == before, "conflicted affected task leaves no partial write");
  // Cursors are scoped to their query: another filter or sort rejects them.
  Fixture g; auto grepo = repository_for(g).at("entityId");
  for (int i = 0; i < 3; ++i) planned_task(g, grepo, {{"priority", i == 0 ? "TASK_PRIORITY_LOW" : "TASK_PRIORITY_HIGH"}});
  auto page = g.call("ListTasks", {{"page", {{"size", 1}}}, {"sortKey", "TASK_SORT_KEY_PRIORITY"}});
  const auto cursor = page.at("nextCursor").get<std::string>();
  require(!cursor.empty(), "keyset cursor issued");
  error(g.raw("ListTasks", {{"page", {{"size", 1}, {"cursor", cursor}}}, {"sortKey", "TASK_SORT_KEY_TYPE"}}), "INVALID_INPUT");
  error(g.raw("ListTasks", {{"page", {{"size", 1}, {"cursor", cursor}}}, {"sortKey", "TASK_SORT_KEY_PRIORITY"}, {"filter", {{"priorities", J::array({"TASK_PRIORITY_LOW"})}}}}), "INVALID_INPUT");
  require(g.call("ListTasks", {{"page", {{"size", 1}, {"cursor", cursor}}}, {"sortKey", "TASK_SORT_KEY_PRIORITY"}}).at("items").size() == 1, "same query continues");
}
void reset_account_space_reconnect_flow() {
  Fixture f; const auto epoch = uuid(), account = uuid(), tenant = uuid();
  lane::HostHooks hooks;
  hooks.authorize = [](std::string) { return J{{"authorizationUrl", "https://identity.example.test/approve"}, {"challengeId", "c"}}.dump(); };
  f.store.setHostHooks(hooks);
  success(f.store.approveAccount(identity(account, tenant, epoch).dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
  f.task(); success(f.store.close(f.space));
  f.call("ResetDevelopmentData", {{"confirmed", true}}).at("completed"); // Hosts reopen afterwards.
  success(f.store.open(J{{"formatVersion", 3}, {"spaceId", f.space}}.dump()));
  require(f.call("GetLocalStatus").at("reconnectRequired") == true && f.call("GetLocalStatus").at("syncEnabled") == false, "reset account space waits for reconnect");
  // Reconnect follows the epoch-change confirmation path and never uploads implicitly.
  f.call("ConnectSync", {{"accountId", account}});
  success(f.store.approveAccount(identity(account, tenant, epoch).dump(), false));
  require(f.call("GetLocalStatus").at("syncEnabled") == true && f.call("GetLocalStatus").at("reconnectRequired") == true, "approval alone does not resume synchronization");
  const auto owner = uuid(); success(f.store.acquireSync(f.space, owner));
  require(success(f.store.pendingSync(f.space, owner)).at("mutations").empty(), "no upload before confirmation");
  success(f.store.confirmSyncEpoch(f.space, epoch));
  require(f.call("GetLocalStatus").at("reconnectRequired") == false, "explicit confirmation resumes synchronization");
}
// An account-bound space with a synchronization lease, for upload-order and
// rejection scenarios. `accept` stands in for the server accepting a pushed
// version; `reject` for a constraint violation on it.
struct Synced {
  Fixture f; std::string epoch, owner;
  Synced() {
    epoch = uuid();
    success(f.store.approveAccount(identity(uuid(), uuid(), epoch).dump(), false));
    f.space = success(f.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
    owner = uuid(); success(f.store.acquireSync(f.space, owner));
  }
  J push(const J& results) {
    return success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", epoch}, {"owner", owner}, {"push", {{"results", results}}}}.dump()));
  }
  std::string request(const J& version) {
    Sql db(f.database());
    return db.scalar("SELECT request_id FROM outbox WHERE request_json LIKE '%\"versionId\":\"" + version.at("versionId").get<std::string>() + "\"%'");
  }
  void accept(const J& version) {
    push(J::array({J{{"requestId", request(version)}, {"result", {{"result", "RESULT_KIND_APPLIED"}, {"version", version}}}}}));
  }
  void reject(const J& version, const std::string& violation = "CONSTRAINT_VIOLATION_PARENT_CYCLE") {
    push(J::array({J{{"requestId", request(version)}, {"error", {{"reason", "ERROR_REASON_CONSTRAINT_VIOLATION"},
        {"violation", violation}, {"entityId", version.at("entityId")}, {"retryable", false}}}}}));
  }
  J pending() { return success(f.store.pendingSync(f.space, owner)).at("mutations"); }
  std::vector<std::string> pending_versions() {
    std::vector<std::string> ids;
    for (const auto& m : pending()) ids.push_back(m.at("version").at("versionId"));
    return ids;
  }
};
void planning_correction_supersedes_rejected_chain() {
  // Never accepted: the correction becomes the entity's creation.
  Synced s; auto& f = s.f;
  auto repo = repository_for(f); s.accept(repo);
  auto task = planned_task(f, repo.at("entityId"));
  auto body = task.at("task"); body["title"] = "Second";
  auto second = f.save(task, body);
  Sql db(f.database());
  const auto first_at = db.scalar("SELECT min(created_at_us) FROM outbox WHERE entity_id='" + task.at("entityId").get<std::string>() + "'");
  s.reject(task);
  require(s.pending().empty(), "a version whose parent was rejected is held back");
  body["title"] = "Corrected";
  auto fixed = f.save(second, body);
  require(fixed.at("parentVersionIds").empty() && fixed.at("versionId") != second.at("versionId"), "correction takes the parents of the rejected chain");
  const auto entity = task.at("entityId").get<std::string>();
  require(db.scalar("SELECT count(*) FROM outbox WHERE entity_id='" + entity + "'") == "1" &&
          db.scalar("SELECT created_at_us FROM outbox WHERE entity_id='" + entity + "'") == first_at, "the chain is replaced at its upload position");
  auto mutations = s.pending();
  require(mutations.size() == 1 && mutations[0].at("kind") == "MUTATION_KIND_CREATE" &&
          mutations[0].at("version").at("versionId") == fixed.at("versionId"), "the corrected creation is uploaded alone");
  require(f.history(fixed).size() == 3 && f.get(entity) == fixed, "rejected versions stay in local history");
  require(f.call("RetryBlockedItems").at("completed") == true && s.pending_versions() == std::vector<std::string>{fixed.at("versionId")},
          "superseded versions are not retried");
  require(f.call("GetLocalStatus").at("itemErrors").empty(), "no blocked item remains");
  // Accepted before: the correction is a save on the last accepted head.
  auto accepted = planned_task(f, repo.at("entityId")); s.accept(accepted);
  body = accepted.at("task"); body["title"] = "Rejected"; auto bad = f.save(accepted, body);
  s.reject(bad);
  body["title"] = "Better";
  auto better = f.save(bad, body);
  require(better.at("parentVersionIds") == J::array({accepted.at("versionId")}), "correction parents are the last accepted heads");
  mutations = s.pending();
  const auto better_id = better.at("versionId").get<std::string>();
  const auto sent = std::find_if(mutations.begin(), mutations.end(), [&](const J& m) { return m.at("version").at("versionId") == better_id; });
  require(sent != mutations.end() && sent->at("kind") == "MUTATION_KIND_SAVE" &&
          sent->at("expectedHeadIds") == J::array({accepted.at("versionId")}), "the correction is a save over the accepted head");
  require(std::none_of(mutations.begin(), mutations.end(), [&](const J& m) { return m.at("version").at("versionId") == bad.at("versionId"); }), "rejected version is never uploaded");
  // A stale edit is only an alternative and does not supersede anything.
  body["title"] = "Alternative";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", accepted.at("entityId")}, {"expectedVersionId", accepted.at("versionId")}, {"task", body}}), "STALE_VERSION");
}
void planning_stale_alternative_ignores_discarded_edits() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto a = planned_task(f, repo_id); auto b = planned_task(f, repo_id);
  const auto a_id = a.at("entityId").get<std::string>(), b_id = b.at("entityId").get<std::string>();
  auto blocked = b.at("task"); blocked["dependsOnTaskIds"] = J::array({a_id});
  auto b2 = f.save(b, blocked);
  auto retitled = a.at("task"); retitled["title"] = "Newer";
  f.save(a, retitled); // A's base version is now stale.
  // A stale save is stored only as an alternative: its removal edit is discarded, so B
  // still depends on A and A -> B would close a cycle across the committed heads.
  auto cyclic = a.at("task"); cyclic["dependsOnTaskIds"] = J::array({b_id});
  Sql db(f.database()); const auto before = counts(db);
  auto rejected = f.raw("SaveTask", {{"meta", f.meta()}, {"id", a_id}, {"expectedVersionId", a.at("versionId")}, {"task", cyclic},
      {"relationshipEdits", {{"removeBlockedTaskIds", J::array({b_id})}}}});
  error(rejected, "CONSTRAINT_VIOLATION");
  require(rejected.at("error").at("violation") == "CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE", "discarded removal does not hide the cycle");
  require(counts(db) == before && f.get(b_id) == b2, "rejected stale save writes nothing");
  // Without the cycle the alternative is kept, and the discarded edit still leaves B alone.
  auto plain = a.at("task"); plain["title"] = "Alternative";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", a_id}, {"expectedVersionId", a.at("versionId")}, {"task", plain},
      {"relationshipEdits", {{"removeBlockedTaskIds", J::array({b_id})}}}}), "STALE_VERSION");
  require(f.call("GetTask", {{"id", a_id}}).at("result").at("alternatives").size() == 2 && f.get(b_id) == b2, "alternative stored, other tasks untouched");
}
void planning_stale_specialized_mutations_are_validated() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto label = planning_item(f, "label", {{"repositoryId", repo_id}, {"name", "Doomed"}, {"color", "#010203"}});
  auto task = planned_task(f, repo_id, {{"labelIds", J::array({label.at("entityId")})}, {"proposal", "PROPOSAL_PENDING"}, {"prompt", "Original"}});
  const auto task_id = task.at("entityId").get<std::string>();
  // Deleting the label writes a cleanup version without the assignment.
  f.call("DeletePlanningItem", {{"meta", f.meta()}, {"id", label.at("entityId")}});
  require(!f.get(task_id).at("task").contains("labelIds"), "cleanup removed the assignment");
  Sql db(f.database()); const auto before = counts(db);
  const J stale_requests[] = {
      {{"SetProgress"}, {{"progress", "PROGRESS_DONE"}}},
      {{"SetProposal"}, {{"proposal", "PROPOSAL_ACCEPTED"}}},
      {{"RestorePrompt"}, {{"sourceVersionId", task.at("versionId")}}}};
  for (const auto& request : stale_requests) {
    J body = request.at(1); body["meta"] = f.meta(); body["id"] = task_id; body["expectedVersionId"] = task.at("versionId");
    auto rejected = f.raw(request.at(0).at(0), body);
    error(rejected, "CONSTRAINT_VIOLATION");
    require(rejected.at("error").at("violation") == "CONSTRAINT_VIOLATION_REFERENCE_NOT_FOUND", request.at(0).at(0).get<std::string>() + " reports the deleted label");
  }
  require(counts(db) == before && f.call("GetTask", {{"id", task_id}}).at("result").at("result") == "RESULT_KIND_APPLIED",
          "a rejected stale edit leaves no conflict alternative or write");
  // A stale base whose restored dependency would close a cycle across the heads is refused too.
  auto a = planned_task(f, repo_id); auto b = planned_task(f, repo_id);
  const auto a_id = a.at("entityId").get<std::string>(), b_id = b.at("entityId").get<std::string>();
  auto linked = a.at("task"); linked["dependsOnTaskIds"] = J::array({b_id});
  auto a2 = f.save(a, linked);
  auto cleared = linked; cleared.erase("dependsOnTaskIds");
  f.save(a2, cleared);
  auto reverse = b.at("task"); reverse["dependsOnTaskIds"] = J::array({a_id});
  f.save(b, reverse);
  auto cyclic = f.raw("SetProgress", {{"meta", f.meta()}, {"id", a_id}, {"expectedVersionId", a2.at("versionId")}, {"progress", "PROGRESS_DONE"}});
  error(cyclic, "CONSTRAINT_VIOLATION");
  require(cyclic.at("error").at("violation") == "CONSTRAINT_VIOLATION_DEPENDENCY_CYCLE", "union of heads closes the dependency cycle");
  // A valid stale edit is still preserved as an alternative.
  auto other = planned_task(f, repo_id); auto newer = other.at("task"); newer["title"] = "Newer"; f.save(other, newer);
  error(f.raw("SetProgress", {{"meta", f.meta()}, {"id", other.at("entityId")}, {"expectedVersionId", other.at("versionId")}, {"progress", "PROGRESS_DONE"}}), "STALE_VERSION");
  require(f.call("GetTask", {{"id", other.at("entityId")}}).at("result").at("alternatives").size() == 2, "a valid stale edit is kept as an alternative");
}
void planning_deleted_affected_task_is_redacted_from_receipts() {
  Fixture f; auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto a = planned_task(f, repo_id);
  auto b = planned_task(f, repo_id, {{"description", "SecretDescription"}, {"prompt", "SecretPrompt"}});
  const auto b_id = b.at("entityId").get<std::string>();
  // Saving A changes B through a relationship edit, so B is an affected task of A's call.
  auto body = a.at("task"); body["title"] = "Parent";
  const J request = {{"meta", f.meta()}, {"id", a.at("entityId")}, {"expectedVersionId", a.at("versionId")}, {"task", body},
      {"relationshipEdits", {{"addSubtaskIds", J::array({b_id})}}}};
  auto first = f.call("SaveTask", request);
  require(first.at("affected").size() == 1 && first.at("affected")[0].at("version").at("task").at("prompt") == "SecretPrompt", "affected task is returned");
  auto replay = f.call("SaveTask", request);
  require(replay.at("result").at("result") == "RESULT_KIND_REPLAYED" && replay.at("affected").size() == 1 &&
          replay.at("affected")[0].at("version").at("entityId") == b_id, "replay returns the affected task while it exists");
  f.call("DeleteTask", {{"meta", f.meta()}, {"id", b_id}});
  {
    Sql db(f.database());
    for (const auto* query : {"SELECT count(*) FROM receipts WHERE result_json LIKE '%SecretPrompt%' OR result_json LIKE '%SecretDescription%'",
                              "SELECT count(*) FROM versions WHERE envelope_json LIKE '%SecretPrompt%' OR envelope_json LIKE '%SecretDescription%'"})
      require(db.scalar(query) == "0", "deleted affected content survives nowhere");
  }
  auto after = f.raw("SaveTask", request);
  require(after.at("ok").get<bool>() && after.dump().find("Secret") == std::string::npos, "replay never returns deleted content");
  require(!after.at("response").contains("affected") && after.at("response").at("result").at("result") == "RESULT_KIND_REPLAYED", "replay keeps the primary result");
}
void planning_list_ignores_dangling_references() {
  Synced s; auto& f = s.f;
  auto repo = repository_for(f); const auto repo_id = repo.at("entityId").get<std::string>();
  auto label = planning_item(f, "label", {{"repositoryId", repo_id}, {"name", "Doomed"}, {"color", "#010203"}});
  auto milestone = planning_item(f, "milestone", {{"repositoryId", repo_id}, {"name", "Doomed"}});
  auto cycle = planning_item(f, "cycle", {{"repositoryId", repo_id}, {"name", "Doomed"}, {"startDate", "2026-01-01"}, {"endDate", "2026-01-02"}});
  auto peer = planned_task(f, repo_id);
  auto task = planned_task(f, repo_id, {{"labelIds", J::array({label.at("entityId")})}, {"milestoneId", milestone.at("entityId")},
      {"cycleId", cycle.at("entityId")}, {"parentTaskId", peer.at("entityId")}, {"dependsOnTaskIds", J::array({peer.at("entityId")})},
      {"relatedTaskIds", J::array({peer.at("entityId")})}, {"duplicateOfTaskId", peer.at("entityId")}});
  const auto task_id = task.at("entityId").get<std::string>();
  // A conflicted task: deletions that arrive from the server never clean it up.
  auto body = task.at("task"); body["title"] = "Left"; f.save(task, body);
  body["title"] = "Right";
  error(f.raw("SaveTask", {{"meta", f.meta()}, {"id", task_id}, {"expectedVersionId", task.at("versionId")}, {"task", body}}), "STALE_VERSION");
  auto plain = planned_task(f, repo_id);
  auto tombstone = [&](const J& head) {
    J v = {{"formatVersion", 3}, {"entityId", head.at("entityId")}, {"kind", head.at("kind")}, {"versionId", uuid()},
        {"parentVersionIds", J::array({head.at("versionId")})}, {"createdAt", "2026-01-01T00:00:00Z"},
        {"origin", {{"kind", "ORIGIN_KIND_CLI"}, {"deviceId", uuid()}, {"deviceName", "Remote"}, {"accountId", ""}}}, {"deleted", true}};
    success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", s.epoch}, {"owner", s.owner},
        {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"entity", {{"result", "RESULT_KIND_DELETED"}, {"version", v}}}}})},
                  {"nextCursor", "c"}, {"hasMore", false}}}}.dump()));
  };
  for (const auto* head : {&label, &milestone, &cycle, &peer}) tombstone(*head);
  auto raw = f.call("GetTask", {{"id", task_id}}).at("result");
  require(raw.at("result") == "RESULT_KIND_CONFLICT" && raw.at("version").at("task").contains("labelIds") &&
          raw.at("version").at("task").contains("parentTaskId"), "the conflicted task retains its dangling references");
  auto listed = [&](const J& filter) {
    std::set<std::string> ids;
    for (const auto& item : items(f.call("ListTasks", {{"filter", filter}}))) ids.insert(item.at("version").at("entityId"));
    return ids;
  };
  const std::set<std::string> only_plain{plain.at("entityId")}, both{plain.at("entityId"), task_id};
  require(listed({{"labelIds", J::array({label.at("entityId")})}}).empty(), "a deleted label selects nothing");
  require(listed({{"includeUnlabeled", true}}) == both, "unlabeled includes the task whose label was deleted");
  require(listed({{"milestoneIds", J::array({milestone.at("entityId")})}}).empty() && listed({{"includeNoMilestone", true}}) == both, "deleted milestone");
  require(listed({{"cycleIds", J::array({cycle.at("entityId")})}}).empty() && listed({{"includeNoCycle", true}}) == both, "deleted cycle");
  require(listed({{"relationships", J::array({"RELATIONSHIP_KIND_PARENT", "RELATIONSHIP_KIND_DEPENDS_ON", "RELATIONSHIP_KIND_RELATED",
                                               "RELATIONSHIP_KIND_DUPLICATE_OF"})}}).empty() && listed({{"includeNoRelationships", true}}) == both,
          "deleted related tasks are not relationships");
  // Returned snapshots are not rewritten.
  auto page = items(f.call("ListTasks", {{"filter", {{"includeUnlabeled", true}}}}));
  const auto found = std::find_if(page.begin(), page.end(), [&](const J& i) { return i.at("version").at("entityId") == task_id; });
  require(found != page.end() && found->at("version").at("task").contains("labelIds"), "listed snapshots keep their stored references");
  // Live references of the same repository still count.
  auto live = planning_item(f, "label", {{"repositoryId", repo_id}, {"name", "Live"}, {"color", "#010203"}});
  auto labeled = planned_task(f, repo_id, {{"labelIds", J::array({live.at("entityId")})}});
  require(listed({{"labelIds", J::array({live.at("entityId")})}}) == std::set<std::string>{labeled.at("entityId")}, "live assignments still match");
}
