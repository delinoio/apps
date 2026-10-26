// SPDX-License-Identifier: Apache-2.0
// Included inside the test namespace after sync_hard_cases.h.
//
// A stored normalized repository identifier carries no transport. Raw-URL normalization
// removes only the default port of the URL's own transport (HTTPS 443, SSH 22), so an
// identifier such as `example.com:443/a/b` created from an SSH URL must stay valid and
// stable wherever a stored identifier is validated or re-submitted.
struct IdentifierCase {
  std::string raw, normalized;
  // The URL the repository editor pre-fills for an unchanged name-only save.
  std::string edit;
};
const std::vector<IdentifierCase>& identifier_cases() {
  static const std::vector<IdentifierCase> cases = {
    {"ssh://git@example.com:443/a/b.git", "example.com:443/a/b", "ssh://git@example.com:443/a/b"},
    {"ssh://git@example.com:2222/a/b.git", "example.com:2222/a/b", "https://example.com:2222/a/b"},
    {"https://example.com:443/a/b.git", "example.com/a/b", "https://example.com/a/b"},
    {"ssh://git@example.com:22/a/b.git", "example.com/a/b", "https://example.com/a/b"},
    {"https://example.com:22/a/b.git", "example.com:22/a/b", "https://example.com:22/a/b"},
  };
  return cases;
}
std::string replace_path(std::string value) {
  const auto at = value.find("/a/b");
  require(at != std::string::npos, "fixture path");
  return value.replace(at, 4, "/c/d");
}
J port_repository(Fixture& f, const std::string& raw, const std::string& expected, const std::string& name = "") {
  J request = {{"meta", f.meta()}, {"id", uuid()}, {"url", raw}};
  if (!name.empty()) request["displayName"] = name;
  auto version = f.call("CreateRepository", request).at("result").at("version");
  require(version.at("repository").at("normalizedUrl") == expected, "created identifier " + raw);
  return version;
}
J pull_of(const RemoteProposals& remote, const std::vector<J>& versions, const std::string& cursor) {
  J changes = J::array(); unsigned sequence = 0;
  for (const auto& version : versions)
    changes.push_back(J{{"sequence", std::to_string(++sequence)}, {"entity", {
        {"result", "RESULT_KIND_APPLIED"}, {"version", version}}}});
  return {{"formatVersion", 3}, {"epoch", remote.epoch}, {"owner", remote.owner},
          {"pull", {{"changes", changes}, {"nextCursor", cursor}, {"hasMore", false}}}};
}
void repository_identifier_resolution_and_editing() {
  for (const auto& c : identifier_cases()) {
    Fixture f; Sql db(f.database());
    auto base = port_repository(f, c.raw, c.normalized, "original");
    const auto id = base.at("entityId");
    // Two heads: both saves submit the unchanged editor URL and differ in display name only.
    auto save = [&](const std::string& name, const std::string& expected_version) {
      return f.raw("SaveRepository", {{"meta", f.meta()}, {"id", id},
          {"expectedVersionId", expected_version}, {"url", c.edit}, {"displayName", name}});
    };
    auto first = save("left", base.at("versionId"));
    require(first.value("ok", false), "name-only save succeeds: " + c.raw);
    auto left = first.at("response").at("result").at("version");
    require(left.at("repository").at("normalizedUrl") == c.normalized && left.at("entityId") == id,
            "name-only save retains identifier: " + c.raw);
    error(save("right", base.at("versionId")), "STALE_VERSION");
    auto conflict = f.call("GetRepository", {{"id", id}}).at("result");
    require(conflict.at("alternatives").size() == 2, "two repository heads: " + c.raw);
    std::set<std::string> heads;
    for (const auto& alternative : conflict.at("alternatives")) {
      heads.insert(alternative.at("versionId"));
      require(alternative.at("repository").at("normalizedUrl") == c.normalized, "head identifier: " + c.raw);
    }
    J resolved = left; resolved["versionId"] = uuid(); resolved["parentVersionIds"] = heads;
    resolved["repository"] = {{"normalizedUrl", c.normalized}, {"displayName", "chosen"}};
    // Stale head set: rejected without any write.
    const auto before = durable_snapshot(db);
    error(f.raw("ResolveConflict", {{"meta", f.meta()}, {"id", id},
        {"expectedHeadIds", J::array({left.at("versionId")})}, {"resolved", resolved}}), "STALE_RESOLUTION");
    require(durable_snapshot(db) == before, "stale resolution writes nothing: " + c.raw);
    // Current sorted head set and the original normalized identifier.
    auto winner = f.call("ResolveConflict", {{"meta", f.meta()}, {"id", id},
        {"expectedHeadIds", heads}, {"resolved", resolved}}).at("result");
    require(winner.at("result") != "RESULT_KIND_CONFLICT", "one resolving head: " + c.raw);
    require(winner.at("version").at("entityId") == id &&
            winner.at("version").at("repository").at("normalizedUrl") == c.normalized &&
            winner.at("version").at("repository").at("displayName") == "chosen", "resolved identity: " + c.raw);
    require(f.history(winner.at("version")).size() == 4, "resolution retains history: " + c.raw);
    require(items(f.call("ListRepositories")).size() == 1, "one repository: " + c.raw);
  }
}
void repository_identifier_name_only_save_with_counterpart() {
  for (const auto& c : identifier_cases()) {
    if (c.normalized == "example.com/a/b") continue;
    Fixture f;
    auto counterpart = port_repository(f, "https://example.com/a/b", "example.com/a/b", "portless");
    auto base = port_repository(f, c.raw, c.normalized, "original");
    auto saved = f.call("SaveRepository", {{"meta", f.meta()}, {"id", base.at("entityId")},
        {"expectedVersionId", base.at("versionId")}, {"url", c.edit}, {"displayName", "renamed"}})
        .at("result").at("version");
    require(saved.at("repository").at("normalizedUrl") == c.normalized, "identifier kept next to counterpart: " + c.raw);
    require(f.call("GetRepository", {{"id", counterpart.at("entityId")}}).at("result").at("version") == counterpart,
            "counterpart untouched: " + c.raw);
    // Only the explicit :443 identifier is changed by an HTTPS reconstruction. An explicit
    // URL edit keeps the normal uniqueness rule and cannot take the portless identifier.
    if (c.normalized == "example.com:443/a/b") {
      auto attempt = f.raw("SaveRepository", {{"meta", f.meta()}, {"id", base.at("entityId")},
          {"expectedVersionId", saved.at("versionId")}, {"url", "https://example.com:443/a/b"},
          {"displayName", "renamed"}});
      require(!attempt.value("ok", true), "explicit URL edit cannot take the portless identifier");
    }
    // An explicit edit to an unused URL renames and releases the old identifier.
    auto moved = f.call("SaveRepository", {{"meta", f.meta()}, {"id", base.at("entityId")},
        {"expectedVersionId", saved.at("versionId")}, {"url", "https://example.com/new/repo"}, {"displayName", "moved"}})
        .at("result").at("version");
    require(moved.at("repository").at("normalizedUrl") == "example.com/new/repo" &&
            moved.at("entityId") == base.at("entityId"), "explicit edit renames: " + c.raw);
  }
}
void repository_identifier_sync_import_and_merge() {
  for (const auto& c : identifier_cases()) {
    // A donor store creates the canonical immutable versions that the account would deliver.
    Fixture donor;
    auto pulled = port_repository(donor, c.raw, c.normalized, "pulled");
    const auto other_raw = replace_path(c.raw), other_normalized = replace_path(c.normalized);
    auto pushed_donor = port_repository(donor, other_raw, other_normalized, "pushed");

    Fixture f; RemoteProposals remote(f); Sql db(f.database());
    // Pull: an accepted version retains the exact identifier and the cursor follows durable apply.
    const auto cursor_before = db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'");
    success(f.store.applySync(f.space, pull_of(remote, {pulled}, "identifier-cursor").dump()));
    require(db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'") == "identifier-cursor" &&
            cursor_before != "identifier-cursor", "cursor follows durable apply: " + c.raw);
    require(f.call("GetRepository", {{"id", pulled.at("entityId")}}).at("result").at("version") == pulled,
            "pulled identifier retained: " + c.raw);

    // Push result: acknowledge only the matching outbox request.
    auto local = port_repository(f, other_raw, other_normalized, "local");
    const auto local_id = local.at("entityId").get<std::string>();
    const auto pending = std::stoul(db.scalar("SELECT count(*) FROM outbox"));
    auto mutation = J::parse(db.scalar("SELECT request_json FROM outbox WHERE entity_id='" + local_id + "'"));
    const auto request_id = mutation.at("meta").at("requestId").get<std::string>();
    J push = {{"results", J::array({J{{"requestId", request_id}, {"result", {
        {"result", "RESULT_KIND_APPLIED"}, {"version", mutation.at("version")}}}}})}};
    success(f.store.applySync(f.space, J{{"formatVersion", 3}, {"epoch", remote.epoch}, {"owner", remote.owner},
        {"push", push}}.dump()));
    require(db.scalar("SELECT count(*) FROM outbox WHERE request_id='" + request_id + "'") == "0" &&
            std::stoul(db.scalar("SELECT count(*) FROM outbox")) == pending - 1,
            "push result acknowledges only its request: " + c.raw);
    require(f.call("GetRepository", {{"id", local_id}}).at("result").at("version").at("repository").at("normalizedUrl") ==
            other_normalized, "pushed identifier retained: " + c.raw);

    // Standalone merge keeps identity and history.
    Fixture standalone;
    auto standalone_repository = port_repository(standalone, c.raw, c.normalized, "standalone");
    success(standalone.store.approveAccount(identity(uuid(), uuid(), uuid()).dump(), true));
    standalone.space = success(standalone.store.open(R"({"formatVersion":3,"spaceId":""})")).at("spaceId");
    auto merged = standalone.call("GetRepository", {{"id", standalone_repository.at("entityId")}}).at("result");
    require(merged.at("version") == standalone_repository && standalone.history(standalone_repository).size() == 1,
            "standalone merge retains identifier and history: " + c.raw);
  }
}
void repository_identifier_malformed_pull_rolls_back() {
  Fixture donor;
  auto valid = port_repository(donor, "ssh://git@example.com:443/valid/one", "example.com:443/valid/one");
  auto template_version = port_repository(donor, "ssh://git@example.com:443/a/b", "example.com:443/a/b");
  Fixture f; RemoteProposals remote(f); Sql db(f.database());
  const auto before = durable_snapshot(db);
  for (const auto* identifier : {"example.com:443/a/../b", "example.com:443/a/./b", "example.com:443/a?x=1",
       "example.com:443/a#frag", "example.com:443/a/%2Fb", "example.com:443/a/%5Cb", "example.com:443/a/%5cb",
       "example.com:443\\a", "example.com:443/a/%zz", "example.com:abc/a/b", "example.com", "example.com/",
       "/a/b", "", "https://example.com/a/b", "Example.com:443/a/b", "example.com:443/a/b/", "example.com:443/a/b "}) {
    J malformed = template_version;
    malformed["entityId"] = uuid(); malformed["versionId"] = uuid();
    malformed["repository"]["normalizedUrl"] = identifier;
    auto rejected = J::parse(f.store.applySync(f.space, pull_of(remote, {valid, malformed}, "must-not-advance").dump()));
    require(!rejected.value("ok", true), std::string("malformed identifier rejected: ") + identifier);
    error(rejected, "INVALID_INPUT");
    require(durable_snapshot(db) == before, std::string("malformed pull rolls back: ") + identifier);
  }
  // The valid entity applies when it is alone, proving the rollback was caused by the malformed one.
  success(f.store.applySync(f.space, pull_of(remote, {valid}, "valid-only").dump()));
  require(db.scalar("SELECT value FROM store_meta WHERE key='pull_cursor'") == "valid-only", "valid pull advances cursor");
}
