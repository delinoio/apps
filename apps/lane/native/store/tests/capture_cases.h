// SPDX-License-Identifier: Apache-2.0
J captureImage() {
  return {{"capture",{{"attachmentId",uuid()},{"width",1},{"height",1},
          {"sha256","4ff6ab670a58c14270e034e2090d9a432caa263a14e0a25785386b0c12f880b5"},
          {"byteSize","70"},{"pageTitle","Synthetic page"},{"cssSelector","<script>inert</script>"}}},
          {"png","iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="}};
}
J captureRequest(Fixture &f,const J &repository) {
  return {{"meta",f.meta()},{"id",uuid()},{"repositoryId",repository},
          {"title","Capture task"},{"description","Problem description"},
          {"images",J::array({captureImage()})}};
}
void capture_lifecycle() {
  Fixture f;
  auto seed=f.task(), repo=seed["task"]["repositoryId"];
  auto request=captureRequest(f,repo), image=request["images"][0];
  auto aid=image["capture"]["attachmentId"];
  auto first=f.call("CreateCaptureTask",request)["result"]["version"];
  require(first["task"]["prompt"]=="Problem description","capture description becomes prompt");
  require(first["task"]["captures"][0]==image["capture"],"metadata retained as data");
  require(f.call("CreateCaptureTask",request)["result"]["version"]==first,"capture save replay");
  auto get=J{{"taskId",request["id"]},{"attachmentId",aid}};
  require(f.call("GetAttachment",get)["png"]==image["png"],"local PNG available");
  error(f.raw("GetAttachment",{{"taskId",seed["entityId"]},{"attachmentId",aid}}),"NOT_FOUND");
  auto body=first["task"];body["captures"]=J::array();
  auto removed=f.save(first,body);
  require(f.history(removed,true).size()==2,"capture removal enters prompt history");
  require(f.call("GetAttachment",get)["png"]==image["png"],"history retains bytes");
  auto restored=f.call("RestorePrompt",{{"meta",f.meta()},{"id",request["id"]},{"expectedVersionId",removed["versionId"]},{"sourceVersionId",first["versionId"]}})["result"]["version"];
  require(restored["task"]["captures"]==first["task"]["captures"],"restore captures");
  auto other=f.task(f.createRequest("Destination","https://example.test/destination"));
  body=restored["task"];body["repositoryId"]=other["task"]["repositoryId"];
  f.save(restored,body);
  f.call("DeleteRepository",{{"meta",f.meta()},{"id",repo}});
  require(f.call("GetAttachment",get)["png"]==image["png"],"move retains bytes after former repository deletion");
  f.call("DeleteTask",{{"meta",f.meta()},{"id",request["id"]}});
  require(f.call("GetAttachment",get)["state"]=="ATTACHMENT_STATE_DELETED","deleted state");
  require(f.call("CreateCaptureTask",request)["result"]["result"]=="RESULT_KIND_DELETED","deleted replay");
  Sql db(f.database()); require(db.scalar("SELECT count(*) FROM attachments")=="0","deleted bytes removed");
}
void capture_validation_and_cache() {
  Fixture f; auto seed=f.task(); auto q=captureRequest(f,seed["task"]["repositoryId"]);
  auto bad=q;bad["images"][0]["capture"]["sha256"]=std::string(64,'0');
  error(f.raw("CreateCaptureTask",bad),"INVALID_INPUT");
  auto corrupt=q;
  corrupt["images"][0]["png"]="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk8HQAAAABJRU5ErkJggg==";
  corrupt["images"][0]["capture"]["sha256"]="70d6a76472cebb144d0f4fb6b1787cb93c7b97fcd9c330f206989940546e4954";
  error(f.raw("CreateCaptureTask",corrupt),"INVALID_INPUT");
  auto partial=q;partial["images"].push_back(bad["images"][0]);
  partial["images"][1]["capture"]["attachmentId"]=uuid();
  error(f.raw("CreateCaptureTask",partial),"INVALID_INPUT");

  Sql db(f.database());require(db.scalar("SELECT count(*) FROM attachments")=="0","invalid capture rolls back");
  auto v=f.call("CreateCaptureTask",q)["result"]["version"];
  auto aid=q["images"][0]["capture"]["attachmentId"].get<std::string>(), task=q["id"].get<std::string>();
  auto png=q["images"][0]["png"].get<std::string>();
  auto body=seed["task"];body["captures"]=v["task"]["captures"];
  error(f.raw("SaveTask",{{"meta",f.meta()},{"id",seed["entityId"]},{"expectedVersionId",seed["versionId"]},{"task",body}}),"INVALID_INPUT");
  db.exec("UPDATE attachments SET png_base64=NULL");
  require(f.call("GetAttachment",{{"taskId",task},{"attachmentId",aid}})["state"]=="ATTACHMENT_STATE_WAITING_FOR_SYNC","pending bytes state");
  error(J::parse(f.store.cacheAttachment(f.space,task,aid,"AAAA")),"INVALID_INPUT");
  error(J::parse(f.store.cacheAttachment(f.space,seed["entityId"],aid,png)),"NOT_FOUND");
  require(success(f.store.cacheAttachment(f.space,task,aid,png))["png"]==png,"validated cache");
  require(db.scalar("SELECT synchronized FROM attachments")=="1","download marked synchronized");
  auto duplicate=captureRequest(f,seed["task"]["repositoryId"]);duplicate["images"]=q["images"];
  error(f.raw("CreateCaptureTask",duplicate),"INVALID_INPUT");
}
void url_connections_lifecycle() {
  Fixture f;auto task=f.task();auto repo=task["task"]["repositoryId"];
  auto q=J{{"meta",f.meta()},{"id",uuid()},{"connection",{{"prefix","HTTPS://EXAMPLE.COM:443/app///?private=x#part"},{"repositoryId",repo}}}};
  auto v=f.call("CreateURLConnection",q)["result"]["version"];
  require(v["urlConnection"]["prefix"]=="https://example.com/app","normalized URL prefix");
  auto dup=q;dup["meta"]=f.meta();dup["id"]=uuid();error(f.raw("CreateURLConnection",dup),"INVALID_INPUT");
  for (const auto &prefix : {"file:///tmp","https://user:pass@example.com","https://example.com\\app"," https://example.com"}) {
    dup["meta"]=f.meta();dup["connection"]["prefix"]=prefix;error(f.raw("CreateURLConnection",dup),"INVALID_INPUT");
  }
  auto edited=f.call("SaveURLConnection",{{"meta",f.meta()},{"id",v["entityId"]},{"expectedVersionId",v["versionId"]},{"connection",{{"prefix","https://example.com/App"},{"repositoryId",repo}}}})["result"]["version"];
  require(edited["urlConnection"]["prefix"]=="https://example.com/App","path case preserved");
  require(items(f.call("ListURLConnections")).size()==1,"rules listed");
  auto stale=f.raw("SaveURLConnection",{{"meta",f.meta()},{"id",v["entityId"]},{"expectedVersionId",v["versionId"]},{"connection",{{"prefix","https://example.com/other"},{"repositoryId",repo}}}});
  error(stale,"STALE_VERSION");
  auto conflict=items(f.call("ListURLConnections"))[0];
  require(conflict["alternatives"].size()==2,"URL conflict keeps alternatives");
  J heads=J::array();for(const auto &alternative:conflict["alternatives"]) heads.push_back(alternative["versionId"]);
  f.call("ResolveConflict",{{"meta",f.meta()},{"id",v["entityId"]},{"expectedHeadIds",heads},{"resolved",edited}});
  require(items(f.call("ListURLConnections"))[0]["result"]=="RESULT_KIND_APPLIED","URL conflict resolved explicitly");

  f.call("DeleteRepository",{{"meta",f.meta()},{"id",repo}});
  require(items(f.call("ListURLConnections")).empty(),"repository cascades rules");
}
void capture_sync_and_import() {
  Fixture f; RemoteProposals remote(f);
  auto request=captureRequest(f,remote.repository["entityId"]);
  auto image=request["images"][0];
  auto v=remote.create();
  v["entityId"]=uuid();v["versionId"]=uuid();
  v["task"]["captures"]=J::array({image["capture"]});
  remote.apply(v);
  auto get=J{{"taskId",v["entityId"]},{"attachmentId",image["capture"]["attachmentId"]}};
  require(f.call("GetAttachment",get)["state"]=="ATTACHMENT_STATE_WAITING_FOR_SYNC","sync inserts manifest");
  auto changed=v;changed["versionId"]=uuid();changed["parentVersionIds"]=J::array({v["versionId"]});changed["task"]["captures"][0]["width"]=2;
  auto raw=f.store.applySync(f.space,J{{"formatVersion",2},{"epoch",remote.epoch},{"owner",remote.owner},{"pull",{{"changes",J::array({J{{"sequence","2"},{"entity",{{"version",changed}}}}})}}}}.dump());
  error(J::parse(raw),"INVALID_INPUT");
  success(f.store.cacheAttachment(f.space,v["entityId"],image["capture"]["attachmentId"],image["png"]));
  require(f.call("GetAttachment",get)["png"]==image["png"],"synced cache available");
  Fixture local;auto seed=local.task();auto q=captureRequest(local,seed["task"]["repositoryId"]);
  local.call("CreateCaptureTask",q);
  auto account=uuid();success(local.store.approveAccount(J{{"issuer","https://identity.example.test"},{"accountId",account},{"tenantId",uuid()},{"epoch",uuid()}}.dump(),true));
  local.space=success(local.store.open(R"({"formatVersion":2,"spaceId":""})"))["spaceId"];
  require(local.call("GetAttachment",{{"taskId",q["id"]},{"attachmentId",q["images"][0]["capture"]["attachmentId"]}})["png"]==q["images"][0]["png"],"standalone import copies bytes");
}
void capture_legacy_empty_defaults() {
  Fixture f; auto task=f.task();
  auto request=J{{"meta",f.meta()},{"id",task["entityId"]},{"expectedVersionId",task["versionId"]},{"task",task["task"]}};
  request["task"]["title"]="Legacy save";
  auto saved=f.call("SaveTask",request)["result"]["version"];
  std::string receipts,versions,outbox;
  {
    Sql db(f.database());
    // Reconstruct the actual schema 1 table contract. Existing receipt hashes
    // were computed with the old SaveTask fields (no captures field).
    db.exec("PRAGMA foreign_keys=OFF; BEGIN EXCLUSIVE; DROP TABLE attachments; DROP TABLE attachment_identities;"
      "CREATE TABLE entities_old(id TEXT PRIMARY KEY,kind INTEGER NOT NULL CHECK(kind IN (1,2)),repository_id TEXT,normalized_url TEXT,deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1)));"
      "INSERT INTO entities_old SELECT * FROM entities; DROP TABLE entities; ALTER TABLE entities_old RENAME TO entities;"
      "CREATE UNIQUE INDEX repository_url ON entities(normalized_url) WHERE kind=1 AND deleted=0 AND normalized_url IS NOT NULL;"
      "UPDATE versions SET envelope_json=json_set(envelope_json,'$.formatVersion',1);"
      "UPDATE receipts SET result_json=json_set(result_json,'$.result.version.formatVersion',1);"
      "UPDATE outbox SET request_json=json_set(request_json,'$.version.formatVersion',1);"
      "UPDATE store_meta SET value='1' WHERE key='format_version'; PRAGMA user_version=1; COMMIT;");
    receipts=db.scalar("SELECT group_concat(fingerprint || result_json,'|') FROM receipts");
    versions=db.scalar("SELECT group_concat(envelope_json,'|') FROM versions");
    outbox=db.scalar("SELECT group_concat(request_json,'|') FROM outbox");
  }
  success(f.store.close(f.space));
  success(f.store.migrate(f.space));
  success(f.store.open(J{{"formatVersion",2},{"spaceId",f.space}}.dump()));
  {
    Sql db(f.database());
    require(receipts==db.scalar("SELECT group_concat(fingerprint || result_json,'|') FROM receipts"),"migration preserves receipt bytes");
    require(versions==db.scalar("SELECT group_concat(envelope_json,'|') FROM versions"),"migration preserves envelope bytes");
    require(outbox==db.scalar("SELECT group_concat(request_json,'|') FROM outbox"),"migration preserves outbox bytes");
  }
  request["task"]["captures"]=J::array();
  require(f.call("SaveTask",request)["result"]["result"]=="RESULT_KIND_REPLAYED","format 1 receipt replays with empty repeated defaults");
  auto epoch=uuid(),owner=uuid();
  success(f.store.approveAccount(J{{"issuer","https://identity.example.test"},{"accountId",uuid()},{"tenantId",uuid()},{"epoch",epoch}}.dump(),true));
  auto accountSpace=success(f.store.open(R"({"formatVersion":2,"spaceId":""})"))["spaceId"].get<std::string>();
  success(f.store.acquireSync(accountSpace,owner));
  saved["formatVersion"]=1;saved["task"]["captures"]=J::array();
  auto response=J{{"formatVersion",2},{"epoch",epoch},{"owner",owner},{"pull",{{"changes",J::array({J{{"sequence","1"},{"entity",{{"version",saved}}}}})}}}};
  success(f.store.applySync(accountSpace,response.dump()));
  require(f.call("ListHistory",{{"id",task["entityId"]}})["versions"].size()==2,"default-only import does not create a new version");
}

void capture_upload_handoff() {
  Fixture f;RemoteProposals remote(f);
  auto q=captureRequest(f,remote.repository["entityId"]);
  f.call("CreateCaptureTask",q);
  auto pending=success(f.store.pendingSync(f.space,remote.owner));
  require(pending["attachments"].size()==1,"native upload enumerates pending bytes");
  auto task=q["id"].get<std::string>(),aid=q["images"][0]["capture"]["attachmentId"].get<std::string>();
  error(J::parse(f.store.acknowledgeAttachment(f.space,remote.owner,uuid(),task,aid)),"STALE_EPOCH");
  auto before=std::stoll(f.call("GetLocalStatus")["pendingItemCount"].get<std::string>());
  success(f.store.acknowledgeAttachment(f.space,remote.owner,remote.epoch,task,aid));
  require(success(f.store.pendingSync(f.space,remote.owner))["attachments"].empty(),"server acceptance clears pending bytes");
  auto after=std::stoll(f.call("GetLocalStatus")["pendingItemCount"].get<std::string>());
  require(after+1==before,"pending status includes image work");
}

void moved_capture_survives_deleted_repository_replay() {
  Fixture f; RemoteProposals remote(f);
  auto source = remote.repository["entityId"];
  auto destination = f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()},
      {"url", "https://example.test/capture-destination"}})["result"]["version"]["entityId"];
  auto request = captureRequest(f, source);
  auto original = f.call("CreateCaptureTask", request)["result"]["version"];
  auto body = original["task"];
  body["repositoryId"] = destination;
  auto moved = f.save(original, body);
  f.call("DeleteRepository", {{"meta", f.meta()}, {"id", source}});
  auto history = f.history(moved);
  auto attachment = J{{"taskId", request["id"]},
      {"attachmentId", request["images"][0]["capture"]["attachmentId"]}};
  remote.apply(original);
  require(f.get(request["id"]) == moved, "old repository replay preserves moved task head");
  require(f.history(moved) == history, "old repository replay preserves capture history");
  require(f.call("GetAttachment", attachment)["png"] == request["images"][0]["png"],
          "old repository replay preserves moved capture bytes");
}

void moved_url_rule_survives_deleted_repository_replay() {
  Fixture f; RemoteProposals remote(f);
  auto source = remote.repository["entityId"];
  auto destination = f.call("CreateRepository", {{"meta", f.meta()}, {"id", uuid()},
      {"url", "https://example.test/rule-destination"}})["result"]["version"]["entityId"];
  auto original = f.call("CreateURLConnection", {{"meta", f.meta()}, {"id", uuid()},
      {"connection", {{"prefix", "https://example.test/app"}, {"repositoryId", source}}}})["result"]["version"];
  auto body = original["urlConnection"];
  body["repositoryId"] = destination;
  auto moved = f.call("SaveURLConnection", {{"meta", f.meta()}, {"id", original["entityId"]},
      {"expectedVersionId", original["versionId"]}, {"connection", body}})["result"]["version"];
  f.call("DeleteRepository", {{"meta", f.meta()}, {"id", source}});
  auto history = f.history(moved);
  remote.apply(original);
  auto rules = items(f.call("ListURLConnections"));
  require(rules.size() == 1 && rules[0]["version"] == moved,
          "old repository replay preserves moved URL rule head");
  require(f.history(moved) == history, "old repository replay preserves URL rule history");
}

void moved_capture_and_rule_import_retained_history() {
  Fixture f;
  auto source = f.call("CreateRepository", {{"meta",f.meta()},{"id",uuid()},
      {"url","https://example.test/import-source"}})["result"]["version"]["entityId"];
  auto destination = f.call("CreateRepository", {{"meta",f.meta()},{"id",uuid()},
      {"url","https://example.test/import-destination"}})["result"]["version"]["entityId"];
  auto request = captureRequest(f,source);
  auto original = f.call("CreateCaptureTask",request)["result"]["version"];
  auto body = original["task"];
  body["repositoryId"] = destination;
  body["captures"] = J::array();
  auto moved = f.save(original,body);
  auto rule = f.call("CreateURLConnection",{{"meta",f.meta()},{"id",uuid()},
      {"connection",{{"prefix","https://example.test/app"},{"repositoryId",source}}}})["result"]["version"];
  auto connection = rule["urlConnection"];
  connection["repositoryId"] = destination;
  auto movedRule = f.call("SaveURLConnection",{{"meta",f.meta()},{"id",rule["entityId"]},
      {"expectedVersionId",rule["versionId"]},{"connection",connection}})["result"]["version"];
  f.call("DeleteRepository",{{"meta",f.meta()},{"id",source}});
  auto taskHistory = f.history(moved), ruleHistory = f.history(movedRule);
  success(f.store.approveAccount(J{{"issuer","https://identity.example.test"},{"accountId",uuid()},
      {"tenantId",uuid()},{"epoch",uuid()}}.dump(),true));
  f.space = success(f.store.open(R"({"formatVersion":2,"spaceId":""})"))["spaceId"];
  require(f.get(request["id"]) == moved,"import preserves moved task head");
  require(f.history(moved) == taskHistory,"import preserves immutable history under deleted former repository");
  require(f.history(movedRule) == ruleHistory,"import preserves moved URL rule history");
  require(items(f.call("ListURLConnections"))[0]["version"] == movedRule,"import preserves current URL rule ownership");
  auto attachment = J{{"taskId",request["id"]},{"attachmentId",request["images"][0]["capture"]["attachmentId"]}};
  require(f.call("GetAttachment",attachment)["png"] == request["images"][0]["png"],"import copies bytes referenced only by history");
  auto restored = f.call("RestorePrompt",{{"meta",f.meta()},{"id",request["id"]},
      {"expectedVersionId",moved["versionId"]},{"sourceVersionId",original["versionId"]}})["result"]["version"];
  require(restored["task"]["captures"] == original["task"]["captures"] &&
          restored["task"]["repositoryId"] == destination,"imported historical captures restore in current repository");
}

void attachment_identity_survives_deletion_and_import() {
  Fixture f; auto seed = f.task();
  auto request = captureRequest(f,seed["task"]["repositoryId"]);
  f.call("CreateCaptureTask",request);
  f.call("DeleteTask",{{"meta",f.meta()},{"id",request["id"]}});
  auto reuse = request; reuse["id"] = uuid(); reuse["meta"] = f.meta();
  error(f.raw("CreateCaptureTask",reuse),"INVALID_INPUT");
  {
    Sql db(f.database());
    require(db.scalar("SELECT count(*) FROM attachments") == "0","deleted content removed");
    require(db.scalar("SELECT count(*) FROM attachment_identities") == "1","minimal immutable identity retained");
  }
  success(f.store.approveAccount(J{{"issuer","https://identity.example.test"},{"accountId",uuid()},
      {"tenantId",uuid()},{"epoch",uuid()}}.dump(),true));
  f.space = success(f.store.open(R"({"formatVersion":2,"spaceId":""})"))["spaceId"];
  reuse["meta"] = f.meta();
  error(f.raw("CreateCaptureTask",reuse),"INVALID_INPUT");
  Sql db(f.database());
  require(db.scalar("SELECT count(*) FROM attachment_identities") == "1","import copies deleted identity reservations");
  require(db.scalar("SELECT count(*) FROM attachments") == "0","identity import does not restore deleted bytes");
  auto owner = uuid();
  success(f.store.acquireSync(f.space,owner));
  auto epoch = success(f.store.pendingSync(f.space,owner))["epoch"];
  auto incoming = seed;
  incoming["entityId"] = uuid(); incoming["versionId"] = uuid();
  incoming["parentVersionIds"] = J::array();
  incoming["task"]["captures"] = J::array({request["images"][0]["capture"]});
  auto response = J{{"formatVersion",2},{"epoch",epoch},{"owner",owner},
      {"pull",{{"changes",J::array({J{{"sequence","1"},{"entity",{{"version",incoming}}}}})}}}};
  error(J::parse(f.store.applySync(f.space,response.dump())),"INVALID_INPUT");
  require(db.scalar("SELECT count(*) FROM attachments") == "0","pending sync cannot reassign a deleted attachment identity");

}
