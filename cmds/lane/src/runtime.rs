// SPDX-License-Identifier: Apache-2.0
#![allow(
    clippy::print_stderr,
    reason = "CLI approval instructions and safe diagnostics must stay off stdout."
)]
use crate::{
    auth::{self, Auth},
    cli::{Dispatcher, Options},
    error::{Failure, Result, invalid},
    protocol::{self, strv},
    remote,
    store::{self, Store, meta, put_meta},
    syncclient,
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
};
pub struct Runtime {
    pub root: Option<PathBuf>,
    pub auth: Auth,
    pub origin: &'static str,
    store: Option<Store>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            root: None,
            auth: Auth::default(),
            origin: "ORIGIN_KIND_CLI",
            store: None,
        }
    }
}
impl Runtime {
    #[must_use]
    pub const fn new(root: Option<PathBuf>, auth: Auth, origin: &'static str) -> Self {
        Self {
            root,
            auth,
            origin,
            store: None,
        }
    }
    fn open(&mut self, opt: &Options, removal: bool) -> Result<()> {
        if self.store.is_none() {
            self.store = Some(Store::open(
                &self.root.clone().map_or_else(store::default_root, Ok)?,
                &opt.space,
                removal,
            )?);
        }
        Ok(())
    }
    fn invoke_inner(&mut self, opt: &Options, method: &str, mut q: Value) -> Result<Value> {
        if self
            .auth
            .cancelled
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(Failure::new("DEPENDENCY_UNAVAILABLE", "Request cancelled."));
        }
        self.open(opt, method == "RemoveDeviceData")?;
        let s = self.store.as_mut().unwrap();
        if !opt.expected_space.is_empty()
            && (s.active()? != opt.expected_space || s.space != opt.expected_space)
        {
            return Err(Failure::new(
                "FORBIDDEN",
                "The active space changed. Reopen the selected account and retry.",
            ));
        }
        if opt.remote
            && matches!(
                method,
                "GetLocalStatus"
                    | "GetNotificationPreferences"
                    | "SetNotificationPreferences"
                    | "ConnectSync"
                    | "DisconnectSync"
                    | "RemoveDeviceData"
                    | "SyncRun"
                    | "SyncCallback"
                    | "CancelReauthentication"
            )
        {
            return Err(invalid("This command requires the local target."));
        }
        match method {
            "GetLocalStatus" => {
                let mut status = s.invoke(method, &q)?;
                status["connected"] =
                    (status["syncEnabled"] == true && self.auth.load(s).is_ok()).into();
                return Ok(status);
            }
            "GetNotificationPreferences" | "SetNotificationPreferences" => {
                return s.invoke(method, &q);
            }
            "ConnectSync" => {
                let identity = s.identity()?;
                let mut account = strv(&q, "accountId").to_owned();
                if !identity.account.is_empty() {
                    if !account.is_empty() && account != identity.account {
                        return Err(Failure::new(
                            "FORBIDDEN",
                            "Reconnect must target the pinned account.",
                        ));
                    }
                    account = identity.account;
                }
                let confirm = if self.origin == "ORIGIN_KIND_MCP" {
                    s.with(false, |c| Ok(meta(c, "reconnect_required")? == "1"))?
                        && !strv(&q, "accountId").is_empty()
                        && q["accountId"] == account
                } else {
                    opt.confirm_epoch_change
                };
                let endpoint = if opt.endpoint.is_empty() {
                    "https://lane.api.delino.io"
                } else {
                    &opt.endpoint
                };
                if !opt.redirect_uri.is_empty() {
                    return self
                        .auth
                        .begin(s, endpoint, &opt.redirect_uri, &account, confirm);
                }
                let listener = self.auth.listen()?;
                let approval = self
                    .auth
                    .begin(s, endpoint, auth::LOOPBACK, &account, confirm)?;
                eprintln!(
                    "Open this URL in your browser to approve: {}",
                    strv(&approval, "authorizationUrl")
                );
                let credential = self
                    .auth
                    .wait_callback(listener, |callback| self.auth.complete(s, callback))?;
                return self.finalize(credential);
            }
            "SyncCallback" => {
                let callback = strv(&q, "callbackUri");
                if url::Url::parse(callback)
                    .is_ok_and(|u| u.query_pairs().any(|(k, _)| k == "reauth_challenge"))
                {
                    require_active(s)?;
                    let c = self.auth.load(s)?;
                    return Ok(json!({"challengeId":self.auth.complete_deletion(s,&c,callback)?}));
                }
                let c = self.auth.complete(s, callback)?;
                return self.finalize(c);
            }
            "CancelReauthentication" => {
                require_active(s)?;
                let c = self.auth.load(s)?;
                self.auth
                    .consume_deletion(s, &c, "", strv(&q, "challengeId"), true)?;
                return Ok(json!({"completed":true}));
            }
            "DisconnectSync" => {
                let retention = strv(&q, "retention");
                if !matches!(
                    retention,
                    "LOCAL_RETENTION_KEEP_LOCAL" | "LOCAL_RETENTION_REMOVE_LOCAL"
                ) {
                    return Err(invalid("Choose --retention keep-local or remove-local."));
                }
                s.set_enabled(false)?;
                if !s.identity()?.account.is_empty() {
                    let c = self.auth.load(s)?;
                    self.auth.revoke(&c)?;
                    self.auth.delete(s)?;
                }
                if retention == "LOCAL_RETENTION_REMOVE_LOCAL" {
                    s.remove()?;
                }
                return Ok(json!({"completed":true}));
            }
            "RemoveDeviceData" => {
                if s.identity().is_ok_and(|i| !i.account.is_empty()) {
                    self.auth.delete(s)?;
                }
                s.remove()?;
                return Ok(json!({"completed":true}));
            }
            "SyncRun" => {
                let result =
                    syncclient::run(&self.auth, s, &opt.endpoint, opt.once, opt.retry_blocked);
                if result
                    .as_ref()
                    .is_err_and(|e| e.reason == "ERROR_REASON_DELETED")
                {
                    self.auth.delete(s)?;
                    s.remove()?;
                }
                result?;
                return Ok(json!({"completed":true}));
            }
            "ConflictList" | "ConflictGet" => {
                let kind = strv(&q, "kind");
                if !matches!(kind, "" | "task" | "repository") {
                    return Err(invalid("--kind must be task or repository."));
                }
                if method == "ConflictGet" {
                    return self.invoke_inner(
                        opt,
                        if kind == "repository" {
                            "GetRepository"
                        } else {
                            "GetTask"
                        },
                        json!({"id":q["id"]}),
                    );
                }
                let methods = if kind == "task" {
                    vec!["ListTasks"]
                } else if kind == "repository" {
                    vec!["ListRepositories"]
                } else {
                    vec!["ListTasks", "ListRepositories"]
                };
                let mut items = vec![];
                for m in methods {
                    let mut cursor = String::new();
                    loop {
                        let page = self.invoke_inner(
                            opt,
                            m,
                            json!({"page":{"size":100,"cursor":cursor}}),
                        )?;
                        if let Some(a) = page["items"].as_array() {
                            items.extend(
                                a.iter()
                                    .filter(|v| {
                                        v["result"] == "RESULT_KIND_CONFLICT"
                                            || v["alternatives"]
                                                .as_array()
                                                .is_some_and(|a| a.len() > 1)
                                    })
                                    .cloned(),
                            );
                        }
                        cursor = strv(&page, "nextCursor").into();
                        if cursor.is_empty() {
                            break;
                        }
                    }
                }
                return Ok(json!({"items":items}));
            }
            "DeleteEverywhere" | "ClearServer" => return self.delete_account(opt, method, q),
            _ => {}
        }
        let descriptor =
            protocol::method(method).ok_or_else(|| invalid("Unknown typed method."))?;
        if descriptor.parent_service().name() == "SyncService" {
            return Err(invalid("Unknown typed method."));
        }
        q = protocol::request(method, &q)?;
        let identity = s.identity()?;
        let origin = json!({"kind":self.origin,"deviceId":s.device,"deviceName":"Lane","accountId":identity.account});
        let mut allocated = vec![];
        protocol::attribute(descriptor.input(), &mut q, &origin, &mut allocated)?;
        if self.origin == "ORIGIN_KIND_CLI" {
            for id in allocated {
                eprintln!("Request ID: {id}");
            }
        }
        if opt.remote {
            if matches!(method, "BeginDataDeletion" | "DeleteAccountData")
                && q["confirmedAccountId"] != identity.account
            {
                return Err(Failure::new(
                    "FORBIDDEN",
                    "Deletion must target the pinned account.",
                ));
            }
            if method == "DeleteAccountData" {
                if !strv(&q, "reauthProof").is_empty() || opt.reauth_challenge.is_empty() {
                    return Err(Failure::new(
                        "FORBIDDEN",
                        "Deletion requires the matching private browser challenge.",
                    ));
                }
                let c = self.auth.load(s)?;
                q["reauthProof"] = self
                    .auth
                    .consume_deletion(s, &c, strv(&q, "mode"), &opt.reauth_challenge, false)?
                    .into();
            }
            let response = remote::call(&self.auth, s, &opt.endpoint, method, &q)?;
            if method == "BeginDataDeletion" {
                let c = self.auth.load(s)?;
                self.auth.store_deletion(
                    s,
                    &c,
                    strv(&q, "mode"),
                    strv(&response, "challengeId"),
                )?;
            }
            if method == "DeleteAccountData" {
                s.set_enabled(false)?;
                self.auth.delete(s)?;
                if q["mode"] == "DATA_DELETION_MODE_DELETE_EVERYWHERE" {
                    s.remove()?;
                }
            }
            Ok(response)
        } else {
            if descriptor.parent_service().name() != "LaneService"
                && !matches!(method, "GetEvent" | "ListEvents")
            {
                return Err(Failure::new(
                    "ONLINE_REQUIRED",
                    "Use --remote after browser-approved login for this command.",
                ));
            }
            let value = s.invoke(method, &q)?;
            protocol::typed(descriptor.output(), &value, true)
        }
    }
    fn finalize(&mut self, c: Value) -> Result<Value> {
        let source = self.store.as_ref().unwrap();
        let identity = source.identity()?;
        if c["device_id"] != source.device {
            return Err(Failure::new(
                "FORBIDDEN",
                "Approved device does not match this installation.",
            ));
        }
        let mut target = source.select_account(
            strv(&c, "issuer"),
            strv(&c, "account_id"),
            strv(&c, "tenant_id"),
        )?;
        self.auth.save(&target, &c)?;
        let result = syncclient::reconnect(&self.auth, &target, &c);
        if result
            .as_ref()
            .is_err_and(|e| e.reason == "ERROR_REASON_DELETED")
        {
            if target.space == source.space {
                self.store.take();
            }
            self.auth.delete(&target)?;
            target.remove()?;
            return Err(syncclient::deleted());
        }
        result?;
        if identity.account.is_empty() && source.space != target.space {
            syncclient::import(source, &target)?;
        }
        target.set_enabled(true)?;
        Ok(json!({"spaceId":target.space,"deviceId":target.device,"connected":true}))
    }
    fn delete_account(&mut self, opt: &Options, method: &str, q: Value) -> Result<Value> {
        if !opt.remote {
            return Err(Failure::new(
                "ONLINE_REQUIRED",
                "Account deletion requires --remote and browser reauthentication.",
            ));
        }
        let account = strv(&q, "confirmedAccountId");
        if account.is_empty() {
            return Err(invalid("--confirmed-account-id is required."));
        }
        let mode = if method == "DeleteEverywhere" {
            "DATA_DELETION_MODE_DELETE_EVERYWHERE"
        } else {
            "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL"
        };
        let s = self.store.as_ref().unwrap();
        let c = self.auth.load(s)?;
        let challenge = strv(&q, "challengeId");
        if challenge.is_empty() {
            let listener = if c["redirect_uri"] == auth::LOOPBACK {
                Some(self.auth.listen()?)
            } else {
                None
            };
            let approval = self.invoke_inner(
                opt,
                "BeginDataDeletion",
                json!({"mode":mode,"confirmedAccountId":account,"meta":q["meta"]}),
            )?;
            if let Some(listener) = listener {
                eprintln!(
                    "Open this URL in your browser to approve: {}",
                    strv(&approval, "authorizationUrl")
                );
                let s = self.store.as_ref().unwrap();
                let id = self.auth.wait_callback(listener, |callback| {
                    self.auth.complete_deletion(s, &c, callback)
                })?;
                return Ok(json!({"challengeId":id}));
            }
            return Ok(approval);
        }
        let callback = strv(&q, "reauthCallback");
        if !callback.is_empty() && self.auth.complete_deletion(s, &c, callback)? != challenge {
            return Err(Failure::new(
                "FORBIDDEN",
                "Reauthentication challenge differs from the explicit selection.",
            ));
        }
        let mut selected = opt.clone();
        selected.reauth_challenge = challenge.into();
        self.invoke_inner(
            &selected,
            "DeleteAccountData",
            json!({"mode":mode,"confirmedAccountId":account,"meta":q["meta"]}),
        )
    }
    fn deletion_after_auth(&mut self, original: Failure) -> Failure {
        let Some(s) = self.store.as_mut() else {
            return original;
        };
        let result = (|| -> Result<()> {
            let c = self.auth.load(s)?;
            let epoch = s.with(false, |db| meta(db, "account_epoch"))?;
            if !protocol::valid_id(&epoch) {
                return Err(original.clone());
            }
            let response = self
                .auth
                .http
                .get(format!(
                    "{}/auth/deletion-status",
                    auth::endpoint(strv(&c, "endpoint"))?
                ))
                .bearer_auth(strv(&c, "access_token"))
                .send()
                .map_err(|_| original.clone())?;
            let directive = auth::read_response(response, 64 << 10)?;
            if directive["disconnected"] != true
                || directive["account_id"] != c["account_id"]
                || directive["device_id"] != c["device_id"]
                || directive["old_epoch"] != epoch
                || !protocol::valid_id(strv(&directive, "new_epoch"))
                || directive["new_epoch"] == epoch
            {
                return Err(original.clone());
            }
            match strv(&directive, "mode") {
                "DATA_DELETION_MODE_DELETE_EVERYWHERE" => {
                    s.with(true,|db|{if meta(db,"account_epoch")?!=epoch{return Err(original.clone());}store::sync::change(db,&json!({"deletion":{"kind":"ENTITY_KIND_OR_REMOTE_KIND_ACCOUNT","id":c["account_id"],"deletionVersionId":directive["new_epoch"]}}),strv(&c,"account_id"))})?;
                    self.auth.delete(s)?;
                    s.remove()?;
                    Err(syncclient::deleted())
                }
                "DATA_DELETION_MODE_CLEAR_SERVER_KEEP_LOCAL" => {
                    s.with(true, |db| {
                        if meta(db, "account_epoch")? != epoch {
                            return Err(original.clone());
                        }
                        put_meta(db, "reconnect_required", "1")?;
                        put_meta(db, "sync_error_reason", "ERROR_REASON_STALE_EPOCH")?;
                        db.execute("INSERT INTO changes(change_kind) VALUES(4)", [])?;
                        Ok(())
                    })?;
                    Err(auth::reconnect_error())
                }
                _ => Err(original.clone()),
            }
        })();
        result
            .err()
            .filter(|e| {
                matches!(
                    e.reason.as_str(),
                    "ERROR_REASON_DELETED" | "ERROR_REASON_STALE_EPOCH"
                )
            })
            .unwrap_or(original)
    }
}
fn require_active(s: &Store) -> Result<()> {
    if s.active()? != s.space {
        return Err(Failure::new(
            "FORBIDDEN",
            "The active account changed. Begin a new browser approval.",
        ));
    }
    Ok(())
}
impl Dispatcher for Runtime {
    fn invoke(&mut self, opt: &Options, method: &str, q: Value) -> Result<Value> {
        let mut result = self.invoke_inner(opt, method, q);
        if (opt.remote || method == "SyncRun")
            && result
                .as_ref()
                .is_err_and(|e| e.reason == "ERROR_REASON_UNAUTHENTICATED")
        {
            result = Err(self.deletion_after_auth(result.unwrap_err()));
        }
        if let Some(s) = &self.store {
            let reason = result
                .as_ref()
                .err()
                .map_or("APPLIED", |e| e.reason.as_str());
            let correlation = result
                .as_ref()
                .err()
                .map_or_else(protocol::id, |e| e.correlation_id.clone());
            if crate::locallog::record(&s.root, method, reason, &correlation).is_err() {
                eprintln!("Local diagnostic log could not be retained.");
            }
        }
        result
    }
    fn mcp(
        &mut self,
        opt: &Options,
        input: &mut dyn BufRead,
        out: &mut (dyn Write + Send),
    ) -> Result<()> {
        let selected = std::sync::Mutex::new(None::<String>);
        let root = self.root.clone();
        let auth = self.auth.clone();
        // Resolve on the first tool call: discovery must not create user state.
        // Each call then opens its own connection to the pinned session space.
        crate::mcp::serve(input, out, |method, q, cancelled| {
            let mut auth = auth.clone();
            auth.cancelled = cancelled;
            let mut runtime = Self::new(root.clone(), auth, "ORIGIN_KIND_MCP");
            let mut options = opt.clone();
            {
                let mut pinned = selected.lock().unwrap();
                if let Some(space) = pinned.as_ref() {
                    options.space.clone_from(space);
                } else {
                    runtime.open(&options, method == "RemoveDeviceData")?;
                    let space = runtime.store.as_ref().unwrap().space.clone();
                    options.space.clone_from(&space);
                    *pinned = Some(space);
                }
            }
            runtime.invoke(&options, method, q)
        })
    }
}
