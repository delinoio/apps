// SPDX-License-Identifier: Apache-2.0
use super::{
    Connection, Failure, OptionalExtension, Result, Store, Value, id, invalid, json, normalize,
    params, planning, rows, strv, valid_id,
};
use crate::protocol::{self, fingerprint};
use std::collections::{BTreeMap, HashMap, HashSet};
pub(super) fn heads(c: &Connection, id: &str) -> Result<Vec<Value>> {
    let raw = rows(
        c,
        "SELECT v.envelope_json FROM heads h JOIN versions v ON v.version_id=h.version_id AND v.entity_id=h.entity_id WHERE h.entity_id=? ORDER BY v.version_id",
        [id],
    )?;
    if raw.is_empty() {
        return Err(Failure::new("NOT_FOUND", "Entity does not exist."));
    }
    raw.into_iter()
        .map(|raw| {
            let v = protocol::message("EntityVersion", &serde_json::from_str(&raw)?)?;
            if !matches!(v["formatVersion"].as_u64(), Some(1..=3)) {
                return Err(Failure::new(
                    "UNSUPPORTED_FORMAT",
                    "Unsupported entity format.",
                ));
            }
            if strv(&v, "entityId") != id
                || !valid_id(strv(&v, "versionId"))
                || (v["deleted"] != true && v["createdAt"].is_null())
                || !matches!(
                    strv(&v, "kind"),
                    "ENTITY_KIND_REPOSITORY"
                        | "ENTITY_KIND_TASK"
                        | "ENTITY_KIND_URL_CONNECTION"
                        | "ENTITY_KIND_LABEL"
                        | "ENTITY_KIND_MILESTONE"
                        | "ENTITY_KIND_CYCLE"
                )
            {
                return Err(invalid("Invalid stored version."));
            }
            if v["deleted"] != true {
                if (v["kind"] == "ENTITY_KIND_TASK" && !v["task"].is_object())
                    || (v["kind"] == "ENTITY_KIND_REPOSITORY" && !v["repository"].is_object())
                    || (v["kind"] == "ENTITY_KIND_URL_CONNECTION"
                        && !v["urlConnection"].is_object())
                    || planning::body_field(strv(&v, "kind"))
                        .is_some_and(|field| !v[field].is_object())
                {
                    return Err(invalid("Invalid stored body."));
                }
                if let Some(task) = v.get("task")
                    && (!matches!(
                        strv(task, "progress"),
                        "PROGRESS_TODO"
                            | "PROGRESS_IN_PROGRESS"
                            | "PROGRESS_BLOCKED"
                            | "PROGRESS_DONE"
                            | "PROGRESS_CANCELED"
                    ) || !matches!(
                        strv(task, "proposal"),
                        "PROPOSAL_REGULAR"
                            | "PROPOSAL_PENDING"
                            | "PROPOSAL_ACCEPTED"
                            | "PROPOSAL_REJECTED"
                    ))
                {
                    return Err(invalid("Invalid stored task."));
                }
            }
            Ok(v)
        })
        .collect()
}
pub(super) fn result(h: &[Value]) -> Value {
    if h[0]["deleted"] == true {
        json!({
        "result":"RESULT_KIND_DELETED","version":h[0]}
        )
    } else if h.len() > 1 {
        json!({
        "result":"RESULT_KIND_CONFLICT","alternatives":h}
        )
    } else {
        json!({
        "result":"RESULT_KIND_APPLIED","version":h[0]}
        )
    }
}
pub(super) fn head_ids(h: &[Value]) -> Vec<String> {
    let mut ids: Vec<_> = h.iter().map(|v| strv(v, "versionId").to_owned()).collect();
    ids.sort();
    ids
}
pub(super) fn mapped(c: &Connection, id: &str) -> Result<String> {
    let mut id = id.to_owned();
    let mut seen = HashSet::new();
    while seen.insert(id.clone()) {
        let next: Option<String> = c
            .query_row(
                "SELECT canonical_id FROM repository_mappings WHERE local_id=?",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        match next {
            Some(v) => id = v,
            None => return Ok(id),
        }
    }
    Err(invalid("Invalid repository mapping."))
}
pub(super) fn new_version(id: &str, kind: &str, meta: &Value, parents: Vec<String>) -> Value {
    json!({
    "formatVersion":3,"entityId":id,"kind":kind,"versionId":protocol::id(),"parentVersionIds":parents,"createdAt":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros,true),"origin":meta["origin"]}
    )
}
fn save_receipt(
    c: &Connection,
    method: &str,
    q: &Value,
    fp: &str,
    id: &str,
    r: &Value,
) -> Result<()> {
    c.execute("INSERT INTO receipts(method,request_id,fingerprint,entity_id,result_json) VALUES(?,?,?,?,?)",params![method,strv(&q["meta"],"requestId"),fp,id,r.to_string()])?;
    Ok(())
}
pub(super) fn validate_task(c: &Connection, t: &Value) -> Result<()> {
    if strv(t, "title").trim().is_empty()
        || !valid_id(strv(t, "repositoryId"))
        || !matches!(
            strv(t, "progress"),
            "PROGRESS_TODO"
                | "PROGRESS_IN_PROGRESS"
                | "PROGRESS_BLOCKED"
                | "PROGRESS_DONE"
                | "PROGRESS_CANCELED"
        )
        || !matches!(
            strv(t, "proposal"),
            "PROPOSAL_REGULAR" | "PROPOSAL_PENDING" | "PROPOSAL_ACCEPTED" | "PROPOSAL_REJECTED"
        )
    {
        return Err(invalid("Invalid task snapshot."));
    }
    let h = heads(c, &mapped(c, strv(t, "repositoryId"))?)?;
    if h[0]["kind"] != "ENTITY_KIND_REPOSITORY" || h[0]["deleted"] == true {
        return Err(Failure::new("DELETED", "Repository is deleted or invalid."));
    }
    if h.len() > 1 {
        return Err(Failure::new(
            "CONFLICT_READ_ONLY",
            "Repository has unresolved conflicts.",
        ));
    }
    Ok(())
}
impl Store {
    pub(super) fn persist(
        &self,
        c: &Connection,
        v: &Value,
        meta: &Value,
        kind: &str,
        conflict: bool,
        prompt: bool,
    ) -> Result<()> {
        let id = strv(v, "entityId");
        let rid = if v["task"].is_object() {
            Some(mapped(c, strv(&v["task"], "repositoryId"))?)
        } else if v["urlConnection"].is_object() {
            Some(mapped(c, strv(&v["urlConnection"], "repositoryId"))?)
        } else if let Some(field) =
            planning::body_field(strv(v, "kind")).filter(|field| v[*field].is_object())
        {
            Some(mapped(c, strv(&v[field], "repositoryId"))?)
        } else {
            None
        };
        let url = v["repository"]["normalizedUrl"].as_str();
        if let Some(url) = url {
            for raw in rows(
                c,
                "SELECT v.envelope_json FROM heads h JOIN versions v ON h.version_id=v.version_id JOIN entities e ON e.id=h.entity_id WHERE e.kind=1 AND e.deleted=0 AND e.id<>?",
                [id],
            )? {
                let other: Value = serde_json::from_str(&raw)?;
                if other["repository"]["normalizedUrl"] == url {
                    return Err(invalid("Repository URL is already registered."));
                }
            }
        }
        c.execute(
            "INSERT OR IGNORE INTO entities(id,kind) VALUES(?,?)",
            params![id, planning::kind_number(strv(v, "kind"))],
        )?;
        if !conflict {
            c.execute(
                "UPDATE entities SET repository_id=?,normalized_url=?,deleted=? WHERE id=?",
                params![rid, url, v["deleted"] == true, id],
            )?;
            c.execute("DELETE FROM heads WHERE entity_id=?", [id])?;
        }
        let us = chrono::DateTime::parse_from_rfc3339(strv(v, "createdAt"))
            .map_err(|_| invalid("Invalid version time."))?
            .timestamp_micros();
        c.execute("INSERT INTO versions(version_id,entity_id,envelope_json,created_at_us,prompt_version) VALUES(?,?,?,?,?)",params![strv(v,"versionId"),id,v.to_string(),us,prompt])?;
        c.execute(
            "INSERT INTO heads(entity_id,version_id) VALUES(?,?)",
            params![id, strv(v, "versionId")],
        )?;
        let mut m = json!({
        "meta":meta,"version":v,"expectedHeadIds":v["parentVersionIds"],"kind":kind}
        );
        m["meta"]["requestId"] = v["versionId"].clone();
        c.execute(
            "INSERT INTO outbox(request_id,entity_id,request_json,created_at_us) VALUES(?,?,?,?)",
            params![strv(v, "versionId"), id, m.to_string(), us],
        )?;
        if kind == "MUTATION_KIND_CREATE"
            && v["task"]["proposal"] == "PROPOSAL_PENDING"
            && meta["origin"]["kind"] == "ORIGIN_KIND_MCP"
        {
            c.execute("INSERT OR IGNORE INTO notification_receipts(proposal_id,device_id,state) VALUES(?,?,1)",params![id,self.device])?;
        }
        if matches!(
            strv(&v["task"], "proposal"),
            "PROPOSAL_ACCEPTED" | "PROPOSAL_REJECTED"
        ) {
            c.execute(
                "UPDATE notification_receipts SET state=3 WHERE proposal_id=? AND state=1",
                [id],
            )?;
        }
        c.execute(
            "INSERT INTO changes(entity_id,change_kind) VALUES(?,1)",
            [id],
        )?;
        Ok(())
    }
    /// Write one new version for each changed task: relationship edits, symmetric links and the
    /// cleanup of moves and deletions. Callers have already rejected conflicted tasks.
    pub(super) fn write_others(
        &self,
        c: &Connection,
        others: &BTreeMap<String, Value>,
        meta: &Value,
    ) -> Result<Vec<Value>> {
        let mut out = vec![];
        for (id, body) in others {
            let h = heads(c, id)?;
            let mut v = new_version(
                id,
                "ENTITY_KIND_TASK",
                meta,
                vec![strv(&h[0], "versionId").to_owned()],
            );
            v["task"] = body.clone();
            self.persist(c, &v, meta, "MUTATION_KIND_SAVE", false, false)?;
            out.push(result(&heads(c, id)?));
        }
        Ok(out)
    }
    pub(super) fn erase(&self, c: &Connection, id: &str, meta: &Value) -> Result<Value> {
        self.erase_entity(c, id, meta, true)
    }
    /// Tombstone an entity. `cleanup` detaches other tasks from a deleted task or planning
    /// item in the same transaction; repository cascades delete everything and skip it.
    fn erase_entity(&self, c: &Connection, id: &str, meta: &Value, cleanup: bool) -> Result<Value> {
        let h = heads(c, id)?;
        if h[0]["deleted"] == true {
            return Ok(result(&h));
        }
        let kind = strv(&h[0], "kind");
        if kind == "ENTITY_KIND_REPOSITORY" {
            for child in rows(
                c,
                "SELECT id FROM entities WHERE repository_id=? AND deleted=0",
                [id],
            )? {
                self.erase_entity(c, &child, meta, false)?;
            }
        } else if cleanup && kind == "ENTITY_KIND_TASK" {
            let repository = mapped(c, strv(&h[0]["task"], "repositoryId"))?;
            let others = planning::delete_task_cleanup(c, id, &repository)?;
            self.write_others(c, &others, meta)?;
        } else if cleanup && let Some(field) = planning::body_field(kind) {
            let repository = mapped(c, strv(&h[0][field], "repositoryId"))?;
            let others = planning::delete_planning_cleanup(c, id, &repository)?;
            self.write_others(c, &others, meta)?;
        }
        let mut v = new_version(id, kind, meta, head_ids(&h));
        v["deleted"] = true.into();
        for sql in [
            "DELETE FROM attachments WHERE task_id=?",
            "DELETE FROM heads WHERE entity_id=?",
            "DELETE FROM versions WHERE entity_id=?",
            "DELETE FROM outbox WHERE entity_id=?",
            "UPDATE notification_receipts SET state=3 WHERE proposal_id=? AND state=1",
        ] {
            c.execute(sql, [id])?;
        }
        self.persist(c, &v, meta, "MUTATION_KIND_DELETE", false, false)?;
        let r = result(&[v]);
        c.execute(
            "UPDATE receipts SET result_json=? WHERE entity_id=?",
            params![r.to_string(), id],
        )?;
        redact(c, id, kind)?;
        Ok(r)
    }
    pub(super) fn mutate(&self, method: &str, input: &Value) -> Result<Value> {
        let mut q = protocol::request(method, input)?;
        match method {
            "CreateRepository" | "SaveRepository" => {
                q["url"] = normalize::url(strv(&q, "url"))?.into();
            }
            "CreateTask" | "CreateCaptureTask" => {
                if method == "CreateCaptureTask" {
                    q["repository"] = json!({"id":q["repositoryId"]});
                    q["progress"] = "PROGRESS_TODO".into();
                    q["proposal"] = "PROPOSAL_REGULAR".into();
                    q["prompt"] = q["description"].clone();
                }
                if q["progress"] == "PROGRESS_UNSPECIFIED" {
                    q["progress"] = "PROGRESS_TODO".into();
                }
                if q["proposal"] == "PROPOSAL_UNSPECIFIED" {
                    q["proposal"] = "PROPOSAL_REGULAR".into();
                }
                if !strv(&q["repository"], "url").is_empty() {
                    q["repository"]["url"] = normalize::url(strv(&q["repository"], "url"))?.into();
                }
                normalize_sets(&mut q);
            }
            "CreateURLConnection" | "SaveURLConnection" => {
                q["connection"]["prefix"] =
                    crate::capture::normalize_prefix(strv(&q["connection"], "prefix"))?.into();
            }
            "SaveTask" => {
                for key in ["task", "relationshipEdits"] {
                    if let Some(body) = q.get_mut(key) {
                        normalize_sets(body);
                    }
                }
            }
            "ResolveConflict" => {
                if let Some(a) = q["expectedHeadIds"].as_array_mut() {
                    a.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                }
            }
            _ => {}
        }
        let meta = &q["meta"];
        if !valid_id(strv(meta, "requestId"))
            || meta["origin"]["deviceId"] != self.device
            || !matches!(
                strv(&meta["origin"], "kind"),
                "ORIGIN_KIND_APP" | "ORIGIN_KIND_CLI" | "ORIGIN_KIND_MCP"
            )
        {
            return Err(invalid(
                "Valid request and local origin identities are required.",
            ));
        }
        // Capture creation uses derived task fields below, but its receipt must
        // fingerprint only the typed request shared with native and remote peers.
        let fp = if method == "CreateCaptureTask" {
            let mut wire = q.clone();
            for field in ["repository", "progress", "proposal", "prompt"] {
                wire.as_object_mut().unwrap().remove(field);
            }
            fingerprint(&wire)
        } else {
            fingerprint(&q)
        };
        let mut committed_error = None;
        let out=self.with(true,|c|{
            if strv(&meta["origin"],"accountId")!=self.identity()?.account{
return Err(invalid("Origin account does not match selected space."));
}
            let old:Option<(String,String)>=c.query_row("SELECT fingerprint,result_json FROM receipts WHERE method=? AND request_id=?",params![method,strv(meta,"requestId")],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((old,raw))=old{
if old!=fp{
return Err(Failure::new("REQUEST_ID_REUSED","Request ID was reused with different content."));
}
let mut r:Value=serde_json::from_str(&raw)?;
// Native receipts retain the response wrapper; earlier Rust receipts retain
// the inner EntityResult. Read both without rewriting durable replay records.
let mut affected_ids:Vec<String>=vec![];
if r["result"].is_object() {
affected_ids=planning::list(&r,"affectedVersionIds");
r = r["result"].clone();
}
                if r["result"]=="RESULT_KIND_CONFLICT"{
committed_error=Some(stale(&r,strv(&q,"id")));
}
else if r["result"]!="RESULT_KIND_DELETED"{
r["result"]="RESULT_KIND_REPLAYED".into();
}
                let repo=if method=="CreateTask" {
heads(c,strv(&r["version"]["task"],"repositoryId")).ok().map(|h|result(&h))}
else{
None}
;
// Affected tasks replay from their stored versions; versions erased by a later
// deletion are skipped so a receipt never retains deleted content.
let mut affected=vec![];
for version_id in &affected_ids{
let raw:Option<String>=c.query_row("SELECT envelope_json FROM versions WHERE version_id=?",[version_id],|row|row.get(0)).optional()?;
if let Some(raw)=raw{
affected.push(json!({"result":"RESULT_KIND_REPLAYED","version":serde_json::from_str::<Value>(&raw)?}));
}
}
return Ok(response(r,repo,affected));
}
            let create=matches!(method,"CreateTask"|"CreateCaptureTask"|"CreateRepository"|"CreateURLConnection"|"CreatePlanningItem");
let id=if create&&strv(&q,"id").is_empty(){
id()}
else{
strv(&q,"id").to_owned()}
;
if !valid_id(&id){
return Err(invalid("Invalid entity ID."));
}
            let h=if create{
let n:i64=c.query_row("SELECT count(*) FROM entities WHERE id=?",[&id],|r|r.get(0))?;
if n>0{
let h=heads(c,&id)?;
if h[0]["deleted"]==true{
let r=result(&h);
save_receipt(c,method,&q,&fp,&id,&r)?;
return Ok(response(r,None,vec![]));
}
return Err(invalid("Entity ID already exists."));
}
vec![]}
else{
heads(c,&id)?}
;
            if !create&&h[0]["deleted"]==true{
let r=result(&h);
save_receipt(c,method,&q,&fp,&id,&r)?;
return Ok(response(r,None,vec![]));
}
            if matches!(method,"DeleteTask"|"DeleteRepository"|"DeleteURLConnection"|"DeletePlanningItem"){
let want=if method=="DeleteTask"{
"ENTITY_KIND_TASK"}
else if method=="DeleteURLConnection" { "ENTITY_KIND_URL_CONNECTION" } else{
"ENTITY_KIND_REPOSITORY"}
;
if method=="DeletePlanningItem" {
if !planning::is_planning_kind(strv(&h[0],"kind")) {
return Err(invalid("Wrong entity kind."));
}
} else if h[0]["kind"]!=want{
return Err(invalid("Wrong entity kind."));
}
let r=self.erase(c,&id,meta)?;
save_receipt(c,method,&q,&fp,&id,&r)?;
return Ok(response(r,None,vec![]));
}
            if h.len()>1&&method!="ResolveConflict"{
return Err(Failure::new("CONFLICT_READ_ONLY","Resolve the conflict before editing."));
}
            let mut v=new_version(&id,if create{
"ENTITY_KIND_REPOSITORY"}
else{
strv(&h[0],"kind")}
,meta,if create{
vec![]}
else{
vec![strv(&q,"expectedVersionId").to_owned()]}
);
let mut repo=None;
let mut prompt=false;
let mut kind=if create{
"MUTATION_KIND_CREATE"}
else{
"MUTATION_KIND_SAVE"}
;
            match method{
                "CreateRepository"|"SaveRepository"=>{
if !create&&h[0]["kind"]!="ENTITY_KIND_REPOSITORY"{
return Err(invalid("Wrong entity kind."));
}
v["repository"]=json!({
"normalizedUrl":q["url"],"displayName":q["displayName"]}
);
}
,
                "CreateTask"|"CreateCaptureTask"=>{
let mut rid=strv(&q["repository"],"id").to_owned();
let url=strv(&q["repository"],"url");
if !url.is_empty(){
let existing:Option<String>=c.query_row("SELECT id FROM entities WHERE kind=1 AND normalized_url=? AND deleted=0",[url],|r|r.get(0)).optional()?;
if let Some(existing)=existing{
rid=existing;
repo=Some(result(&heads(c,&rid)?));
}
else{
rid=protocol::id();
let mut rv=new_version(&rid,"ENTITY_KIND_REPOSITORY",meta,vec![]);
rv["repository"]=json!({
"normalizedUrl":url,"displayName":""}
);
self.persist(c,&rv,meta,"MUTATION_KIND_CREATE",false,false)?;
repo=Some(result(&[rv]));
}
}
                    if !matches!(strv(&q,"proposal"),"PROPOSAL_REGULAR"|"PROPOSAL_PENDING"){
return Err(invalid("Creation requires regular or pending proposal."));
}
v["kind"]="ENTITY_KIND_TASK".into();
v["task"]=json!({
"repositoryId":rid,"title":q["title"],"description":q["description"],"progress":q["progress"],"proposal":q["proposal"]}
);
if let Some(p)=q.get("prompt"){
v["task"]["prompt"]=p.clone();
prompt=true;
}
for key in PLANNING_FIELDS{
if let Some(value)=q.get(key){
v["task"][key]=value.clone();
}
}
planning::compact(&mut v["task"]);
if method=="CreateCaptureTask" { v["task"]["captures"]=self.insert_captures(c,&id,&q["images"])?; }
}
,
                "CreatePlanningItem"|"SavePlanningItem"=>{
if !create && !planning::is_planning_kind(strv(&h[0],"kind")) { return Err(invalid("Wrong entity kind.")); }
let Some(field)=["label","milestone","cycle"].into_iter().find(|f|q["item"][*f].is_object()) else{
return Err(invalid("Exactly one of label, milestone or cycle is required."));
};
v["kind"]=match field{
"label"=>"ENTITY_KIND_LABEL","milestone"=>"ENTITY_KIND_MILESTONE",_=>"ENTITY_KIND_CYCLE"}.into();
v[field]=q["item"][field].clone();
}
,
                "CreateURLConnection"|"SaveURLConnection"=>{
if !create && h[0]["kind"]!="ENTITY_KIND_URL_CONNECTION" { return Err(invalid("Wrong entity kind.")); }
v["kind"]="ENTITY_KIND_URL_CONNECTION".into();
v["urlConnection"]=q["connection"].clone();
}
,
                "SaveTask"=>{
if !h[0]["task"].is_object()||!q["task"].is_object(){
return Err(invalid("Task is required."));
}
if q["task"]["proposal"]!=h[0]["task"]["proposal"]{
return Err(invalid("Use proposal disposition operation."));
}
v["task"]=q["task"].clone();
}
,
                "SetProgress"|"SetProposal"|"RestorePrompt"=>{
if !h[0]["task"].is_object(){
return Err(invalid("Wrong entity kind."));
}
v["task"]=h[0]["task"].clone();
match method{
"SetProgress"=>v["task"]["progress"]=q["progress"].clone(),"SetProposal"=>{
if !matches!(strv(&v["task"],"proposal"),"PROPOSAL_PENDING"|"PROPOSAL_REJECTED")||!matches!(strv(&q,"proposal"),"PROPOSAL_ACCEPTED"|"PROPOSAL_REJECTED"){
return Err(invalid("Invalid proposal disposition."));
}
v["task"]["proposal"]=q["proposal"].clone();
}
,_=>{
let raw:Option<String>=c.query_row("SELECT envelope_json FROM versions WHERE entity_id=? AND version_id=?",params![id,strv(&q,"sourceVersionId")],|r|r.get(0)).optional()?;
let source:Value=serde_json::from_str(&raw.ok_or_else(||Failure::new("NOT_FOUND","Prompt source does not exist."))?)?;
if !source["task"].is_object(){
return Err(invalid("Invalid prompt source."));
}
v["task"]["captures"]=source["task"].get("captures").cloned().unwrap_or_else(||json!([]));
v["task"].as_object_mut().unwrap().remove("prompt");
if let Some(p)=source["task"].get("prompt"){
v["task"]["prompt"]=p.clone();
}
v["promptRestoration"]=true.into();
prompt=true;
}
}
}
,
                "ResolveConflict"=>{
if h.len()<2||json!(head_ids(&h))!=q["expectedHeadIds"]{
return Err(Failure::new("STALE_RESOLUTION","Conflict heads changed."));
}
let resolved=&q["resolved"];
if resolved["kind"]!=h[0]["kind"]||resolved["deleted"]==true||resolved["entityId"]!=id||!matches!(resolved["formatVersion"].as_u64(),Some(1..=3)){
return Err(invalid("Invalid resolved snapshot."));
}
for field in ["task","repository","urlConnection","label","milestone","cycle"]{
if let Some(body)=resolved.get(field){
v[field]=body.clone();
}
}
v["parentVersionIds"]=json!(head_ids(&h));
kind="MUTATION_KIND_RESOLVE";
}
,
                _=>return Err(invalid("Unknown mutation.")),
            }
            let mut conflict=false;
            if !create&&method!="ResolveConflict"{
let expected=strv(&q,"expectedVersionId");
if !valid_id(expected){
return Err(invalid("Expected version is required."));
}
if h[0]["versionId"]!=expected{
let n:i64=c.query_row("SELECT count(*) FROM versions WHERE entity_id=? AND version_id=?",params![id,expected],|r|r.get(0))?;
if n==0{
return Err(invalid("Expected version does not belong to entity."));
}
conflict=true;
}
}
            let mut others=BTreeMap::new();
            if v["task"].is_object(){
validate_task(c,&v["task"])?;
self.validate_capture_references(c,&id,&v["task"]["captures"])?;
prompt|=h.iter().any(|old|old["task"].get("prompt")!=v["task"].get("prompt") || old["task"].get("captures")!=v["task"].get("captures"));
if matches!(method,"CreateTask"|"SaveTask"|"ResolveConflict"){
let previous=(method=="SaveTask"&&h.len()==1).then(||&h[0]["task"]);
let written=planning::write_task(c,&id,v["task"].clone(),&q["relationshipEdits"],previous)?;
v["task"]=written.primary;
// A stale save is preserved as an alternative and never edits other tasks.
if !conflict{
others=written.others;
}
}
}
else if let Some(field)=planning::body_field(strv(&v,"kind")).filter(|field|v[*field].is_object()){
let (_,body)=planning::write_planning(c,&id,&json!({field:v[field]}),(!create).then(||&h[0]))?;
v[field]=body;
}
else if v["repository"].is_object(){
let url=strv(&v["repository"],"normalizedUrl");
if normalize::url(&format!("https://{url}/lane-validation"))?!=format!("{url}/lane-validation"){
return Err(invalid("Invalid normalized repository URL."));
}
}
else if v["urlConnection"].is_object() { self.validate_connection(c,&id,&v["urlConnection"])?; }
else{
return Err(invalid("Snapshot body is missing."));
}
            let superseded=if !create&&!conflict&&method!="ResolveConflict"&&h.len()==1{
supersede_blocked(c,&h[0],&mut v,&mut kind)?
}else{
None
};
            self.persist(c,&v,meta,kind,conflict,prompt)?;
if let Some(position)=superseded{
c.execute("UPDATE outbox SET created_at_us=? WHERE request_id=?",params![position,strv(&v,"versionId")])?;
}
let r=result(&heads(c,&id)?);
if conflict{
committed_error=Some(stale(&r,&id));
}
let affected=self.write_others(c,&others,meta)?;
if affected.is_empty(){
save_receipt(c,method,&q,&fp,&id,&r)?;
}else{
let ids:Vec<&str>=affected.iter().map(|a|strv(&a["version"],"versionId")).collect();
save_receipt(c,method,&q,&fp,&id,&json!({"result":r,"affectedVersionIds":ids}))?;
}
Ok(response(r,repo,affected))
        }
)?;
        if let Some(e) = committed_error {
            return Err(e);
        }
        Ok(out)
    }
}
/// Task fields copied from the flat `CreateTask` request into the stored task body.
const PLANNING_FIELDS: [&str; 12] = [
    "priority",
    "type",
    "labelIds",
    "startDate",
    "dueDate",
    "storyPoints",
    "milestoneId",
    "cycleId",
    "parentTaskId",
    "dependsOnTaskIds",
    "relatedTaskIds",
    "duplicateOfTaskId",
];
/// Sort and deduplicate the ID sets of a request so retries and reordered input share one
/// fingerprint and produce identical stored sets.
fn normalize_sets(v: &mut Value) {
    let Some(object) = v.as_object_mut() else {
        return;
    };
    for key in [
        "labelIds",
        "dependsOnTaskIds",
        "relatedTaskIds",
        "addSubtaskIds",
        "removeSubtaskIds",
        "addBlockedTaskIds",
        "removeBlockedTaskIds",
    ] {
        if let Some(items) = object.get_mut(key).and_then(Value::as_array_mut) {
            items.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            items.dedup();
        }
    }
}
/// An ordinary edit that corrects a version the server rejected for a constraint violation
/// replaces the unsynchronized chain ending at the rejected head: the new version takes the
/// chain's parents (the last version the server accepted) and the superseded outbox rows are
/// dropped. The rejected immutable versions stay in local history; the server never sees them,
/// so the corrected version is not stranded behind a parent that will never be accepted.
fn supersede_blocked(
    c: &Connection,
    head: &Value,
    v: &mut Value,
    kind: &mut &str,
) -> Result<Option<i64>> {
    // Version ID -> (request ID, state, error reason) of every unsynchronized version.
    let mut pending: HashMap<String, (String, i64, Option<String>)> = HashMap::new();
    let mut stmt = c.prepare(
        "SELECT request_id,request_json,state,error_reason FROM outbox WHERE entity_id=?",
    )?;
    for row in stmt.query_map([strv(head, "entityId")], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })? {
        let (request, raw, state, reason) = row?;
        let mutation: Value = serde_json::from_str(&raw)?;
        pending.insert(
            strv(&mutation["version"], "versionId").to_owned(),
            (request, state, reason),
        );
    }
    // Walk from the head through the single-parent chain of unsynchronized versions.
    let mut chain: Vec<Value> = vec![];
    let mut current = head.clone();
    while pending.contains_key(strv(&current, "versionId")) {
        chain.push(current.clone());
        let parents = current["parentVersionIds"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let [parent] = &parents[..] else { break };
        let raw: Option<String> = c
            .query_row(
                "SELECT envelope_json FROM versions WHERE version_id=?",
                [parent.as_str().unwrap_or("")],
                |r| r.get(0),
            )
            .optional()?;
        let Some(raw) = raw else { break };
        current = serde_json::from_str(&raw)?;
    }
    let rejected = chain.iter().any(|version| {
        pending
            .get(strv(version, "versionId"))
            .is_some_and(|(_, state, reason)| {
                *state == 2
                    && reason
                        .as_deref()
                        .is_some_and(|r| r.starts_with("CONSTRAINT_VIOLATION:"))
            })
    });
    let Some(oldest) = chain.last().filter(|_| rejected) else {
        return Ok(None);
    };
    let parents = oldest["parentVersionIds"].clone();
    // The corrected version keeps the upload position of the oldest version it replaces, so
    // versions created in between that depend on this entity are still pushed after it.
    let mut position = None;
    for version in &chain {
        if let Some((request, _, _)) = pending.get(strv(version, "versionId")) {
            let created: Option<i64> = c
                .query_row(
                    "SELECT created_at_us FROM outbox WHERE request_id=?",
                    [request],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(created) = created {
                position = Some(position.map_or(created, |p: i64| p.min(created)));
            }
            c.execute("DELETE FROM outbox WHERE request_id=?", [request])?;
        }
    }
    *kind = match parents.as_array().map_or(0, Vec::len) {
        0 => "MUTATION_KIND_CREATE",
        1 => "MUTATION_KIND_SAVE",
        _ => "MUTATION_KIND_RESOLVE",
    };
    v["parentVersionIds"] = parents;
    Ok(position)
}
fn response(r: Value, repo: Option<Value>, affected: Vec<Value>) -> Value {
    let mut v = json!({
    "result":r}
    );
    if let Some(repo) = repo {
        v["repository"] = repo;
    }
    if !affected.is_empty() {
        v["affected"] = affected.into();
    }
    v
}
fn stale(r: &Value, id: &str) -> Failure {
    let mut e = Failure::new(
        "STALE_VERSION",
        "Version changed; submitted alternative was preserved.",
    );
    e.entity_id = id.into();
    e.current_head_ids = r["alternatives"].as_array().map_or_default(|h| head_ids(h));
    e.conflict = Some(Box::new(r.clone()));
    e
}
pub(super) fn redact(c: &Connection, id: &str, kind: &str) -> Result<()> {
    let mut stmt = c.prepare("SELECT kind,id,body_json FROM remote_objects WHERE deleted=0")?;
    let all = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut events = HashSet::new();
    for (k, key, raw) in &all {
        let v: Value = serde_json::from_str(raw)?;
        if (*k == 3
            && kind == "ENTITY_KIND_TASK"
            && v["taskIds"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(id))))
            || (*k == 2 && kind == "ENTITY_KIND_REPOSITORY" && v["repositoryId"] == id)
        {
            events.insert(if *k == 3 {
                strv(&v, "eventId").to_owned()
            } else {
                key.clone()
            });
        }
    }
    for (k, key, raw) in all {
        let v: Value = serde_json::from_str(&raw)?;
        if events.contains(&key) || (k == 3 && events.contains(strv(&v, "eventId"))) {
            c.execute(
                "UPDATE remote_objects SET body_json=NULL,deleted=1 WHERE kind=? AND id=?",
                params![k, key],
            )?;
        }
    }
    Ok(())
}
