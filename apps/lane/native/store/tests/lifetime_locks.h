// SPDX-License-Identifier: Apache-2.0
// These exercise native Store lifetime coordination. The independent Go
// worker test owns cross-runtime lock compatibility.
J remove_space(lane::Store& store, const std::string& space) {
  return J::parse(store.invoke("RemoveDeviceData", J{{"spaceId", space}, {"request", J::object()}}.dump()));
}
void lifetime_lock_removal_retry() {
  Fixture f; f.task();
  const auto directory = f.database().parent_path();
  const auto lock_path = f.root.path / "locks" / (f.space + ".handles.lock");
  require(fs::exists(lock_path), "opening space creates stable lifetime lock");
  lane::Store holder(f.root.path.string());
  success(holder.open(J{{"formatVersion", 2}, {"spaceId", f.space}}.dump()));
  holder.poll();
  const auto start = std::chrono::steady_clock::now();
  error(remove_space(f.store, f.space), "STORAGE_BUSY");
  const auto elapsed = std::chrono::steady_clock::now() - start;
  require(elapsed >= std::chrono::seconds(4) && elapsed < std::chrono::seconds(15), "exclusive lifetime acquisition has bounded five-second wait");
  require(fs::exists(f.database()), "busy removal never unlinks live database");
  Sql registry(f.root.path / "registry.sqlite");
  require(registry.scalar("SELECT value FROM registry_meta WHERE key='unavailable:" + f.space + "'") == "1", "busy removal permanently marks space unavailable");
  require(registry.scalar("SELECT account_key FROM spaces WHERE id='" + f.space + "'") == "removed:" + f.space,
          "busy removal removes normal account routing");
  lane::Store late(f.root.path.string());
  error(J::parse(late.open(J{{"formatVersion", 2}, {"spaceId", f.space}}.dump())), "NOT_FOUND");
  require(fs::exists(f.database()), "late rejected open leaves held database intact");
  require(J::parse(holder.poll()).empty(), "poll drains marked subscription without content invalidation");
  // No open call is legal now; cleanup is deliberately routed before ordinary
  // handle lookup. Reuse the original Store after its failed removal closed it.
  auto removed = remove_space(f.store, f.space);
  require(removed.value("ok", false) && removed.at("response").at("completed") == true, "marked cleanup retry succeeds after holder drains");
  require(!fs::exists(directory), "successful cleanup removes whole space directory");
  require(fs::exists(lock_path), "successful cleanup preserves stable lifetime lock file");
  require(registry.scalar("SELECT count(*) FROM spaces WHERE id='" + f.space + "'") == "0", "successful cleanup unregisters space");
  require(registry.scalar("SELECT value FROM registry_meta WHERE key='unavailable:" + f.space + "'") == "1", "unavailable marker remains permanent after cleanup");
  error(J::parse(holder.open(J{{"formatVersion", 2}, {"spaceId", f.space}}.dump())), "NOT_FOUND");
  require(!fs::exists(directory), "old holder cannot recreate removed database");
  auto repeated = remove_space(late, f.space);
  require(repeated.value("ok", false) && !fs::exists(directory), "cleanup idempotently retries without an open handle");
  require(fs::exists(lock_path), "cleanup replay does not delete lock identity");
}
void restart_marked_cleanup() {
  for (const bool files_already_removed : {false, true}) {
    TempRoot root; std::string space;
    {
      lane::Store initial(root.path.string());
      space = success(initial.open(R"({"formatVersion":2,"spaceId":""})")).at("spaceId");
    }
    const auto directory = root.path / "spaces" / space;
    const auto lock = root.path / "locks" / (space + ".handles.lock");
    {
      Sql registry(root.path / "registry.sqlite");
      // Model a crash after durable unavailability and before filesystem
      // cleanup, or after filesystem cleanup and before registry finalization.
      registry.exec("BEGIN IMMEDIATE; INSERT INTO registry_meta(key,value) VALUES('unavailable:" + space +
          "','1'); UPDATE spaces SET account_key='removed:" + space + "',sync_enabled=0 WHERE id='" + space + "'; COMMIT;");
    }
    if (files_already_removed) fs::remove_all(directory);
    lane::Store restarted(root.path.string());
    error(J::parse(restarted.open(J{{"formatVersion", 2}, {"spaceId", space}}.dump())), "NOT_FOUND");
    if (files_already_removed) require(!fs::exists(directory), "restart/open refusal cannot recreate marked database");
    require(J::parse(restarted.poll()).empty(), "restart poll does not reopen marked space");
    auto removed = remove_space(restarted, space);
    require(removed.value("ok", false) && removed.at("response").at("completed") == true, "restart cleans marked space without opening it");
    require(!fs::exists(directory) && fs::exists(lock), "restart removes data and retains external lock");
    Sql registry(root.path / "registry.sqlite");
    require(registry.scalar("SELECT count(*) FROM spaces WHERE id='" + space + "'") == "0" &&
        registry.scalar("SELECT value FROM registry_meta WHERE key='unavailable:" + space + "'") == "1", "restart finalizes registry but retains permanent marker");
    // Even a fresh process lifetime cannot treat an unavailable UUID as a new
    // empty space after cleanup completes.
    lane::Store next(root.path.string());
    error(J::parse(next.open(J{{"formatVersion", 2}, {"spaceId", space}}.dump())), "NOT_FOUND");
    require(!fs::exists(directory), "future Store cannot resurrect marked space");
  }
}
void deferred_registry_compatibility_failure() {
  for (const auto* pragma : {"application_id", "user_version"}) {
    TempRoot root;
    {
      lane::Store initial(root.path.string());
      success(initial.open(R"({"formatVersion":2,"spaceId":""})"));
    }
    Sql registry(root.path / "registry.sqlite");
    const auto active = registry.scalar("SELECT value FROM registry_meta WHERE key='active_space_id'");
    const auto count = registry.scalar("SELECT count(*) FROM spaces");
    registry.exec(std::string("PRAGMA ") + pragma + "=987654;");
    // Construction must not throw across the native host boundary. The normal
    // open response owns compatibility errors and update instructions.
    lane::Store unsupported(root.path.string());
    error(J::parse(unsupported.open(R"({"formatVersion":2,"spaceId":""})")), "UNSUPPORTED_FORMAT");
    error(J::parse(unsupported.open(J{{"formatVersion", 2}, {"spaceId", active}}.dump())), "UNSUPPORTED_FORMAT");
    require(registry.scalar(std::string("PRAGMA ") + pragma) == "987654", "unsupported registry header remains untouched");
    require(registry.scalar("SELECT value FROM registry_meta WHERE key='active_space_id'") == active &&
        registry.scalar("SELECT count(*) FROM spaces") == count, "deferred failure does not initialize replacement registry state");
    require(fs::exists(root.path / "spaces" / active / "lane.sqlite"), "compatibility failure preserves prior database");
  }
}
