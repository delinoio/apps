// SPDX-License-Identifier: Apache-2.0
void epoch_reapproval_lifecycle() {
  Fixture f;
  const auto account = uuid(), tenant = uuid(), old_epoch = uuid(), new_epoch = uuid(), owner = uuid();
  auto approved = identity(account, tenant, old_epoch);
  success(f.store.approveAccount(approved.dump(), false));
  f.space = success(f.store.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId");
  success(f.store.acquireSync(f.space, owner));
  auto task = f.task(); Sql db(f.database());
  // Acknowledge the initial versions, so confirming a new generation must
  // rebuild transport work from history rather than retaining an old outbox.
  while (db.scalar("SELECT count(*) FROM outbox") != "0") {
    auto mutation = J::parse(db.scalar("SELECT request_json FROM outbox ORDER BY created_at_us,request_id LIMIT 1"));
    success(f.store.applySync(f.space, J{{"formatVersion", 2}, {"epoch", old_epoch}, {"owner", owner},
        {"push", {{"results", J::array({J{{"requestId", mutation.at("meta").at("requestId")},
            {"result", {{"result", "RESULT_KIND_APPLIED"}, {"version", mutation.at("version")}}}}})}}}}.dump()));
  }
  success(f.store.applySync(f.space, J{{"formatVersion", 2}, {"epoch", old_epoch}, {"owner", owner},
      {"pull", {{"changes", J::array()}, {"nextCursor", "old-generation-cursor"}}}}.dump()));
  const auto retained = durable_snapshot(db);
  approved["epoch"] = new_epoch;
  success(f.store.approveAccount(approved.dump(), false));
  require(db.scalar("SELECT value FROM store_meta WHERE key='account_epoch'") == old_epoch &&
      db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'") == "old-generation-cursor", "reauthorization cannot reset epoch or cursor");
  require(durable_snapshot(db).at("outbox") == retained.at("outbox") && durable_snapshot(db).at("versions") == retained.at("versions"), "reauthorization cannot enqueue or mutate retained versions");
  require(success(f.store.pendingSync(f.space, owner)).at("mutations").empty(), "reauthorization cannot upload into unconfirmed generation");
  require(!J::parse(f.store.confirmSyncEpoch(f.space, new_epoch)).value("ok", true), "ordinary approval is not reconnect consent");
  success(f.store.recordSyncFailure(f.space, "ERROR_REASON_STALE_EPOCH"));
  auto status = f.call("GetLocalStatus");
  require(status.at("reconnectRequired") == true && status.at("accountId") == account &&
      status.at("syncErrorReason") == "ERROR_REASON_STALE_EPOCH", "stale epoch exposes pinned account and reconnect requirement");
  require(!J::parse(f.store.confirmSyncEpoch(f.space, new_epoch)).value("ok", true), "stale flag alone is not consent");
  int authorizations = 0;
  lane::HostHooks hooks;
  hooks.authorize = [&](std::string id) {
    require(id == account, "reconnect targets pinned account"); ++authorizations;
    return J{{"authorizationUrl", "https://identity.example.test/reconnect"}, {"challengeId", "fixture-reconnect"}}.dump();
  };
  f.store.setHostHooks(hooks);
  auto connect = [&] {
    f.call("ConnectSync", {{"accountId", account}});
    success(f.store.approveAccount(approved.dump(), false));
  };
  // Content changed after consent but before approval must invalidate the
  // captured generation/sequence, even though approval itself succeeds.
  f.call("ConnectSync", {{"accountId", account}});
  auto body = task.at("task"); body["title"] = "Edited during approval"; task = f.save(task, body);
  success(f.store.approveAccount(approved.dump(), false));
  require(!J::parse(f.store.confirmSyncEpoch(f.space, new_epoch)).value("ok", true), "intervening content before approval invalidates epoch consent");
  require(db.scalar("SELECT value FROM store_meta WHERE key='account_epoch'") == old_epoch, "failed confirmation retains original generation");
  connect();
  body = task.at("task"); body["title"] = "Edited after approval"; task = f.save(task, body);
  require(!J::parse(f.store.confirmSyncEpoch(f.space, new_epoch)).value("ok", true), "intervening content after approval invalidates epoch consent");
  connect();
  J event = {{"id", uuid()}, {"receiverId", uuid()}, {"repositoryId", task.at("task").at("repositoryId")},
      {"senderEventId", "epoch-probe-event"}, {"eventType", "test"}, {"occurredAt", "2026-01-01T00:00:00Z"},
      {"payload", J::object()}, {"state", "EVENT_STATE_PENDING"}, {"revision", "1"}};
  success(f.store.acquireSync(f.space, owner));
  success(f.store.applySync(f.space, J{{"formatVersion", 2}, {"epoch", old_epoch}, {"owner", owner},
      {"pull", {{"changes", J::array({J{{"sequence", "1"}, {"event", event}}})}, {"nextCursor", "old-generation-probe"}}}}.dump()));
  require(!J::parse(f.store.confirmSyncEpoch(f.space, new_epoch)).value("ok", true), "remote cache change invalidates captured content sequence");
  require(success(f.store.pendingSync(f.space, owner)).at("mutations").empty(), "unconfirmed retained edits remain withheld from upload");
  connect();
  require(!J::parse(f.store.confirmSyncEpoch(f.space, uuid())).value("ok", true), "unapproved epoch cannot be selected");
  const auto before_confirm = durable_snapshot(db);
  success(f.store.confirmSyncEpoch(f.space, new_epoch));
  require(authorizations == 4, "each retry uses explicit reconnect authorization");
  require(db.scalar("SELECT value FROM store_meta WHERE key='account_epoch'") == new_epoch &&
      db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'").empty(), "confirmed generation resets only its cursor");
  require(f.get(task.at("entityId")) == task && durable_snapshot(db).at("versions") == before_confirm.at("versions"), "confirmation preserves immutable history");
  require(db.scalar("SELECT count(*) FROM outbox") == db.scalar("SELECT count(*) FROM versions"), "new generation reconstructs retained version upload work");
  require(f.call("GetLocalStatus").at("reconnectRequired") == false, "confirmation clears reconnect requirement");
}
void exact_account_space_lookup() {
  Fixture f;
  const auto a = uuid(), b = uuid();
  auto approve = [&](const std::string& account) {
    success(f.store.approveAccount(identity(account, uuid(), uuid()).dump(), false));
    return success(f.store.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId").get<std::string>();
  };
  const auto first = approve(a), other = approve(b), second = approve(a);
  auto lookup = [&](const std::string& account) {
    return success(f.store.spacesForAccount(account)).at("items").get<std::set<std::string>>();
  };
  require(lookup(a) == std::set<std::string>{first, second}, "account lookup returns exactly its segregated tenant spaces");
  require(lookup(b) == std::set<std::string>{other} && lookup(a + "-suffix").empty(), "account lookup never uses prefix or active-space fallback");
  require(lookup(uuid()).empty(), "unknown account lookup does not create a space");
  f.space = second;
  f.call("DisconnectSync", {{"retention", "LOCAL_RETENTION_KEEP_LOCAL"}});
  require(lookup(a) == std::set<std::string>{first}, "disabled account space excluded from push wake routing");
  f.space = first; f.call("RemoveDeviceData");
  require(lookup(a).empty() && lookup(b) == std::set<std::string>{other}, "removed space excluded without affecting another account");
}
