//! Compatible SQLite records; imported fields and local decisions have separate storage.
use crate::lifecycle::Transition;
use crate::{Error, Result, now_ms, timestamp};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};
use uuid::Uuid;
/// Current workflow store schema. Version 31 moves ticket source status to the
/// provider-neutral `sourceStatus`/`sourceStatusId` fields (FOR-40).
pub const SCHEMA_VERSION: u32 = 34;

pub struct Store {
    pub(crate) db: Connection,
    clock: std::sync::Arc<dyn Fn() -> i64 + Send + Sync>,
}
pub(crate) fn conflict(message: &str) -> Error {
    Error::Conflict(message.into())
}
pub(crate) fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
pub(crate) fn operator_task(task: &Value) -> Result<()> {
    if task["internal"] == true {
        return Err(conflict(
            "Internal comparison tasks cannot enter ordinary ticket workflows.",
        ));
    }
    Ok(())
}
pub(crate) fn id() -> String {
    Uuid::new_v4().to_string()
}
/// Match the original JavaScript validator's trim set (BOM is whitespace; NEL is not).
pub fn trim_text(value: &str) -> &str {
    value.trim_matches(|c: char| matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'))
}
pub(crate) fn text<'a>(v: &'a Value, key: &str, min: usize, max: usize) -> Result<&'a str> {
    let s = v[key]
        .as_str()
        .ok_or_else(|| invalid(&format!("{key} must be text.")))?;
    if !(min..=max).contains(&s.chars().count()) {
        return Err(invalid(&format!(
            "{key} must contain {min}–{max} characters."
        )));
    }
    Ok(s)
}
/// Operator prose is trimmed before applying the retained Unicode code-point limits.
pub(crate) fn trimmed_text<'a>(v: &'a Value, key: &str, min: usize, max: usize) -> Result<&'a str> {
    let s = trim_text(text(v, key, 0, usize::MAX)?);
    if !(min..=max).contains(&s.chars().count()) {
        return Err(invalid(&format!(
            "{key} must contain {min}–{max} characters."
        )));
    }
    Ok(s)
}
pub(crate) fn integer(v: &Value, key: &str, min: u64, max: u64) -> Result<u64> {
    // JSON has one numeric type: 1, 1.0 and 1e0 have the same integer value.
    v[key]
        .as_f64()
        .filter(|n| {
            n.is_finite()
                && n.fract() == 0.0
                && *n >= min as f64
                && *n <= max.min(9_007_199_254_740_991) as f64
        })
        .map(|n| n as u64)
        .ok_or_else(|| {
            invalid(&format!(
                "{key} must be an integer between {min} and {max}."
            ))
        })
}
pub(crate) fn strict(v: &Value, keys: &[&str]) -> Result<()> {
    let object = v
        .as_object()
        .ok_or_else(|| invalid("Expected a JSON object."))?;
    if let Some(key) = object.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(invalid(&format!("Unrecognized key: {key}")));
    }
    Ok(())
}
/// The accepted UUID spelling and version/variant contract of the retained validator.
pub fn valid_uuid(value: &str) -> bool {
    if value.len() != 36 || Uuid::parse_str(value).is_err() {
        return false;
    }
    let bytes = value.as_bytes();
    if [8, 13, 18, 23].iter().any(|i| bytes[*i] != b'-') {
        return false;
    }
    value == "00000000-0000-0000-0000-000000000000"
        || value == "ffffffff-ffff-ffff-ffff-ffffffffffff"
        || ((b'1'..=b'8').contains(&bytes[14])
            && matches!(bytes[19], b'8' | b'9' | b'a' | b'A' | b'b' | b'B'))
}
pub(crate) fn request_id(v: &Value) -> Result<&str> {
    let key = text(v, "requestId", 1, 200)?;
    if !valid_uuid(key) {
        return Err(invalid("requestId must be a UUID."));
    }
    Ok(key)
}
pub(crate) fn remove(v: &mut Value, key: &str) {
    if let Some(v) = v.as_object_mut() {
        v.remove(key);
    }
}
pub(crate) fn parse(s: String) -> Result<Value> {
    Ok(serde_json::from_str(&s)?)
}
fn merged(mut imported: Value, refinement: Value) -> Value {
    imported
        .as_object_mut()
        .unwrap()
        .extend(refinement.as_object().unwrap().clone());
    imported
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_clock(path, now_ms)
    }
    /// Supply the controller clock before migration and subsequent transactions.
    pub fn open_with_clock(
        path: &Path,
        clock: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // macOS /var and /tmp are symlinks. Resolve the parent, while retaining
        // NOFOLLOW on the database itself (including a raced final symlink).
        let parent = std::fs::canonicalize(path.parent().unwrap_or(Path::new(".")))?;
        let resolved = parent.join(
            path.file_name()
                .ok_or_else(|| invalid("Database needs a filename."))?,
        );
        let path = resolved.as_path();
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(conflict("Workflow database cannot be a symlink."));
        }
        let db = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: u32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(conflict(
                "Workflow database is newer than this application.",
            ));
        }
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        if version == 0 {
            let count: u32 = db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |r| r.get(0))?;
            if count != 0 {
                return Err(conflict(
                    "Unversioned nonempty database is not a fresh Crewroom store.",
                ));
            }
            db.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(e) = db.execute_batch(include_str!("schema.sql")) {
                db.execute_batch("ROLLBACK")?;
                return Err(e.into());
            }
            db.execute_batch("COMMIT")?;
        }
        let store = Self {
            db,
            clock: std::sync::Arc::new(clock),
        };
        if version > 0 && version < SCHEMA_VERSION {
            store.migrate(version)?;
        }
        let check: String = store.db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(conflict("Workflow database integrity check failed."));
        }
        Ok(store)
    }
    /// Supply a deterministic controller clock for transaction/recovery tests.
    pub fn with_clock(mut self, clock: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.clock = std::sync::Arc::new(clock);
        self
    }
    pub(crate) fn clock_ms(&self) -> i64 {
        (self.clock)()
    }
    pub(crate) fn time(&self) -> String {
        timestamp(self.clock_ms())
    }
    pub fn transaction<T>(&self, work: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        // Controller evidence sometimes composes several public transactions.
        // Savepoints let a failed inner operation roll back without committing
        // or discarding its caller's still-pending receipts.
        let nested = !self.db.is_autocommit();
        let savepoint = format!("crewroom_{}", Uuid::new_v4().simple());
        let begin = if nested {
            format!("SAVEPOINT {savepoint}")
        } else {
            "BEGIN IMMEDIATE".into()
        };
        let commit = if nested {
            format!("RELEASE SAVEPOINT {savepoint}")
        } else {
            "COMMIT".into()
        };
        let rollback = if nested {
            format!("ROLLBACK TO SAVEPOINT {savepoint}; RELEASE SAVEPOINT {savepoint}")
        } else {
            "ROLLBACK".into()
        };
        self.db.execute_batch(&begin)?;
        match work(self) {
            Ok(result) => {
                if let Err(error) = self.db.execute_batch(&commit) {
                    self.db.execute_batch(&rollback)?;
                    return Err(error.into());
                }
                Ok(result)
            }
            Err(error) => {
                self.db.execute_batch(&rollback)?;
                Err(error)
            }
        }
    }
    pub(crate) fn all(&self, table: &str) -> Result<Vec<Value>> {
        let mut statement = self
            .db
            .prepare(&format!("SELECT data FROM {table} ORDER BY rowid"))?;
        statement
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| parse(r?))
            .collect()
    }
    pub fn get_task(&self, task_id: &str) -> Result<Value> {
        let row: Option<(String, String)> = self
            .db
            .query_row(
                "SELECT imported,refinement FROM tickets WHERE id=?",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (source, refinement) =
            row.ok_or_else(|| Error::NotFound("Ticket not found.".into()))?;
        Ok(merged(parse(source)?, parse(refinement)?))
    }
    pub fn get_run(&self, run_id: &str) -> Result<Value> {
        self.record("runs", "id", run_id)?
            .ok_or_else(|| Error::NotFound("Run not found.".into()))
    }
    pub(crate) fn record(&self, table: &str, column: &str, key: &str) -> Result<Option<Value>> {
        self.db
            .query_row(
                &format!("SELECT data FROM {table} WHERE {column}=?"),
                [key],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(parse)
            .transpose()
    }
    pub(crate) fn refinement(&self, task_id: &str) -> Result<Value> {
        parse(self.db.query_row(
            "SELECT refinement FROM tickets WHERE id=?",
            [task_id],
            |r| r.get(0),
        )?)
    }
    pub(crate) fn save_refinement(&self, task_id: &str, value: &Value) -> Result<()> {
        self.db.execute(
            "UPDATE tickets SET refinement=? WHERE id=?",
            params![value.to_string(), task_id],
        )?;
        Ok(())
    }
    pub(crate) fn tasks(&self) -> Result<Vec<Value>> {
        let mut statement = self
            .db
            .prepare("SELECT imported,refinement FROM tickets WHERE COALESCE(json_extract(imported,'$.internal'),0)=0 ORDER BY rowid")?;
        statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .map(|r| {
                let (a, b) = r?;
                Ok(merged(parse(a)?, parse(b)?))
            })
            .collect()
    }
    pub(crate) fn open_run(&self, task_id: &str) -> Result<Option<Value>> {
        self.db.query_row("SELECT data FROM runs WHERE task_id=? AND status NOT IN ('cancelled','closed','completed')", [task_id], |r| r.get::<_,String>(0)).optional()?.map(parse).transpose()
    }
    pub(crate) fn repository(&self, repository_id: &str) -> Result<Value> {
        self.record("repositories", "id", repository_id)?
            .ok_or_else(|| Error::NotFound("Repository not found.".into()))
    }
    pub(crate) fn active_repository(&self, repository_id: &str) -> Result<Value> {
        let repo = self.repository(repository_id)?;
        if repo.get("retiredAt").is_some() {
            return Err(conflict(
                "The selected repository is retired. Select an active repository.",
            ));
        }
        Ok(repo)
    }
    pub fn create_task(&self, body: &Value) -> Result<Value> {
        let title = trimmed_text(body, "title", 1, 300)?;
        let description = text(body, "description", 0, 40000)?;
        let repository = body
            .get("repositoryId")
            .map(|_| text(body, "repositoryId", 0, usize::MAX))
            .transpose()?;
        self.transaction(|s| {
            if let Some(repo) = repository.filter(|s| !s.is_empty()) { s.active_repository(repo)?; }
            let task_id = id(); let source = json!({"id":task_id,"key":format!("LOCAL-{}", &task_id[..8]), "title":title,"description":description,
                "priority":"Unspecified","source":"local","project":"","rank":0,"sourceRevision":1,"updatedAt":s.time()});
            let mut refinement = json!({"revision":1,"brief":"","criteria":[],"needsReacceptance":false});
            if let Some(repo) = body.get("repositoryId") { refinement["repositoryId"] = repo.clone(); }
            s.db.execute("INSERT INTO tickets VALUES(?,NULL,NULL,?,?)", params![task_id,source.to_string(),refinement.to_string()])?;
            s.get_task(&task_id)
        })
    }
    pub fn refine(&self, task_id: &str, body: &Value) -> Result<Value> {
        self.accept_requirements(task_id, body, &json!({"origin":"operator"}))
    }
    pub fn requirement_revisions(&self, task_id: &str) -> Result<Vec<Value>> {
        self.get_task(task_id)?;
        let mut statement = self
            .db
            .prepare("SELECT data FROM requirement_revisions WHERE task_id=? ORDER BY revision")?;
        statement
            .query_map([task_id], |r| r.get::<_, String>(0))?
            .map(|r| parse(r?))
            .collect()
    }
    pub fn assign_repository(&self, task_id: &str, body: &Value) -> Result<Value> {
        let repo = text(body, "repositoryId", 0, usize::MAX)?;
        let revision = integer(body, "revision", 1, u64::MAX)?;
        self.transaction(|s| {
            let task = s.get_task(task_id)?;
            operator_task(&task)?;
            if task["revision"] != revision {
                return Err(conflict("The ticket changed. Reload before saving."));
            }
            s.active_repository(repo)?;
            if s.open_run(task_id)?.is_some() {
                return Err(conflict("The repository is fixed for the admitted run."));
            }
            let mut local = s.refinement(task_id)?;
            local["repositoryId"] = json!(repo);
            local["revision"] = json!(revision + 1);
            s.save_refinement(task_id, &local)?;
            s.get_task(task_id)
        })
    }
    pub fn add_note(&self, task_id: &str, body: &Value) -> Result<Value> {
        self.get_task(task_id)?;
        strict(body, &["mode", "body"])?;
        let mode = text(body, "mode", 1, 20)?;
        if !["explore", "grill", "decision"].contains(&mode) {
            return Err(invalid("Invalid note mode."));
        }
        let content = trimmed_text(body, "body", 1, 40000)?;
        let note =
            json!({"id":id(),"taskId":task_id,"mode":mode,"body":content,"createdAt":self.time()});
        self.db.execute(
            "INSERT INTO notes VALUES(?,?,?)",
            params![note["id"].as_str(), task_id, note.to_string()],
        )?;
        Ok(note)
    }
    pub fn queue(&self, task_id: &str, body: &Value) -> Result<Value> {
        self.queue_with_roles(task_id, body, &[])
    }
    pub fn queue_with_roles(&self, task_id: &str, body: &Value, roles: &[Value]) -> Result<Value> {
        self.queue_with_playbooks(task_id, body, roles, &[])
    }
    pub fn queue_with_playbooks(
        &self,
        task_id: &str,
        body: &Value,
        roles: &[Value],
        playbooks: &[Value],
    ) -> Result<Value> {
        strict(
            body,
            &[
                "revision",
                "activeLimitMs",
                "requestId",
                "launch",
                "playbookId",
            ],
        )?;
        let revision = integer(body, "revision", 1, u64::MAX)?;
        let limit = if body.get("activeLimitMs").is_some() {
            integer(body, "activeLimitMs", 1000, 9_007_199_254_740_991)?
        } else {
            3_600_000
        };
        let launch = body
            .get("launch")
            .map(|v| crate::launch::parse_launch(v, None))
            .transpose()?;
        let key = body
            .get("requestId")
            .map(|_| request_id(body))
            .transpose()?;
        let playbook_id = body
            .get("playbookId")
            .map(|v| v.as_str().ok_or_else(|| invalid("Invalid playbook ID.")))
            .transpose()?;
        let mut payload = json!({"taskId":task_id,"revision":revision,"activeLimitMs":limit});
        if let Some(id) = playbook_id {
            payload["playbookId"] = json!(id);
        }
        if let Some(launch) = &launch {
            payload["launch"] = launch.clone();
        }
        self.transaction(|s| {
            if let Some(key) = key && let Some(prior) = s.replay(key,"queue",&payload)? { return Ok(prior); }
            let task = s.get_task(task_id)?;
            operator_task(&task)?;
            if let Some(key) = key {
                let prior = s.db.query_row("SELECT data FROM runs WHERE json_extract(data,'$.requestId')=?", [key], |r| r.get::<_, String>(0)).optional()?.map(parse).transpose()?;
                if let Some(prior) = prior {
                    if prior["taskId"] != task_id || !crate::dependencies::coding(&prior) || prior["taskRevision"] != revision || prior["activeLimitMs"] != limit || prior.get("queuedPlaybookId") != body.get("playbookId") || !crate::repository_work::same(prior.get("launch"), launch.as_ref()) {
                        return Err(conflict("Run request identity has different content."));
                    }
                    return Ok(prior);
                }
            }
            if task["revision"] != revision { return Err(conflict("The brief changed. Reload before queuing.")); }
            let repository = task["repositoryId"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| conflict("Select a target repository before queuing."))?;
            s.active_repository(repository)?;
            if task["needsReacceptance"] == true { return Err(conflict("Source requirements changed. Review and save the brief before queuing.")); }
            if let Some(prior) = s.open_run(task_id)? {
                if prior.get("purpose").is_some_and(|p| p != "coding") { return Err(conflict("Close the open refinement before queuing coding work.")); }
                if prior["taskRevision"] != revision || prior["taskSnapshot"]["sourceRevision"] != task["sourceRevision"] { return Err(conflict("This ticket is already queued with an earlier brief.")); }
                if key.is_some() || prior.get("queuedPlaybookId") != body.get("playbookId") || !crate::repository_work::same(prior.get("launch"), launch.as_ref()) { return Err(conflict("This ticket already has a different queued run request.")); } return Ok(prior);
            }
            if let Some(launch) = &launch { crate::launch::parse_launch(&body["launch"],Some(roles))?; s.validate_launch_briefs(&task,launch)?; }
            let implementation_plan = s.current_implementation_plan(task_id)?;
            if implementation_plan.is_none() {
                let run = s.queue_intake(&task, &payload)?;
                if let Some(key) = key { s.receipt(key,"queue",&payload,&run)?; }
                return Ok(run);
            }
            let position = s.queue_view()?["plans"].as_array().unwrap().len();
            let mut run = json!({"id":id(),"taskId":task_id,"purpose":"coding","status":"awaiting_runner","position":position,"createdAt":s.time(),"queueOrdinal":s.queue_view()?["revision"],
                "taskRevision":revision,"taskSnapshot":task,"roles":[],"stages":[],"activeMs":0,"activeLimitMs":limit});
            run["implementationPlan"] = implementation_plan.unwrap();
            if let Some(key) = key { run["requestId"] = json!(key); }
            if let Some(id) = playbook_id { run["queuedPlaybookId"] = json!(id); }
            if let Some(id)=playbook_id.filter(|id|!id.is_empty()) {crate::playbooks::initial(&mut run,crate::playbooks::selected(playbooks,id)?);}
            s.db.execute("INSERT INTO runs(id,task_id,data) VALUES(?,?,?)",params![run["id"].as_str(),task_id,run.to_string()])?;
            s.bump_queue()?;
            if let Some(launch) = &launch {run["launch"]=launch.clone();s.save_run(&run)?;run=s.get_run(run["id"].as_str().unwrap())?;}
            if let Some(key) = key { s.receipt(key,"queue",&payload,&run)?; } Ok(run)
        })
    }
    pub(crate) fn replay(&self, key: &str, kind: &str, payload: &Value) -> Result<Option<Value>> {
        self.exact_queue_replay(key, kind, payload)
    }
    pub(crate) fn receipt(
        &self,
        key: &str,
        kind: &str,
        payload: &Value,
        result: &Value,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO queue_requests VALUES(?,?,?,?)",
            params![
                key,
                kind,
                crate::serialized::stringify(payload),
                result.to_string()
            ],
        )?;
        Ok(())
    }
    pub(crate) fn bump_queue(&self) -> Result<()> {
        self.db
            .execute("UPDATE queue_state SET revision=revision+1 WHERE id=1", [])?;
        Ok(())
    }
    pub fn queue_view(&self) -> Result<Value> {
        self.coding_queue()
    }
    pub fn reorder_queue(&self, body: &Value) -> Result<Value> {
        strict(body, &["requestId", "expectedRevision", "runIds"])?;
        let key = request_id(body)?;
        let revision = integer(body, "expectedRevision", 0, u64::MAX)?;
        let ids = body["runIds"]
            .as_array()
            .ok_or_else(|| invalid("runIds must be a list."))?
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| valid_uuid(s))
                    .map(str::to_owned)
                    .ok_or_else(|| invalid("Run IDs must be UUIDs."))
            })
            .collect::<Result<Vec<_>>>()?;
        let payload = json!({"expectedRevision":revision,"runIds":ids});
        self.transaction(|s| {
            if let Some(prior) = s.replay(key, "order", &payload)? {
                return Ok(prior);
            }
            let queue = s.queue_view()?;
            if queue["revision"] != revision {
                return Err(conflict("Queue changed. Reload before reordering."));
            }
            let current: HashSet<_> = queue["plans"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap())
                .collect();
            let selected: HashSet<_> = ids.iter().map(String::as_str).collect();
            if selected != current || selected.len() != ids.len() {
                return Err(conflict(
                    "Queue order must be a permutation of all queued coding plans.",
                ));
            }
            for plan in queue["plans"].as_array().unwrap() {
                let mut plan = plan.clone();
                plan["position"] = json!(
                    ids.iter()
                        .position(|id| Some(id.as_str()) == plan["id"].as_str())
                        .unwrap()
                );
                s.db.execute(
                    "UPDATE runs SET data=? WHERE id=?",
                    params![plan.to_string(), plan["id"].as_str()],
                )?;
            }
            s.bump_queue()?;
            let result = s.queue_view()?;
            s.receipt(key, "order", &payload, &result)?;
            Ok(result)
        })
    }
    pub fn cancel_queued(&self, run_id: &str, body: &Value) -> Result<Value> {
        strict(body, &["requestId"])?;
        let key = request_id(body)?;
        let payload = json!({"runId":run_id});
        self.transaction(|s| {
            if let Some(prior) = s.replay(key, "cancel", &payload)? {
                return Ok(prior);
            }
            let run = s.get_run(run_id)?;
            if !crate::dependencies::coding(&run) {
                return Err(conflict("Only coding plans use the queue."));
            }
            if run["status"] == "cancelled" {
                s.receipt(key, "cancel", &payload, &run)?;
                return Ok(run);
            }
            if run.get("position").is_none()
                || run["status"] != "awaiting_runner"
                || !s.runtime(run_id)?["assignments"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            {
                return Err(conflict(
                    "Only an unadmitted queued plan can be removed here.",
                ));
            }
            s.change_run(
                run_id,
                Transition::Desire {
                    desired_state: "cancelled".into(),
                },
            )?;
            s.change_run(run_id, Transition::BeginPause { reason: None })?;
            let run = s.change_run(run_id, Transition::FinishPause { unresolved: false })?;
            s.receipt(key, "cancel", &payload, &run)?;
            Ok(run)
        })
    }
    pub fn runtime(&self, run_id: &str) -> Result<Value> {
        self.get_run(run_id)?;
        let assignments: Vec<_> = self
            .all("assignments")?
            .into_iter()
            .filter(|a| a["runId"] == run_id)
            .collect();
        let assignment_ids: HashSet<_> = assignments
            .iter()
            .filter_map(|a| a["id"].as_str())
            .collect();
        let attempts: Vec<_> = self
            .all("attempts")?
            .into_iter()
            .filter(|a| assignment_ids.contains(a["assignmentId"].as_str().unwrap_or("")))
            .collect();
        let mut runtime = json!({"assignments":assignments,"attempts":attempts,
            "messages":self.all("peer_messages")?.into_iter().filter(|m|m["runId"] == run_id).collect::<Vec<_>>(),
            "deliveryNotices":self.run_records("delivery_notices",run_id)?});
        if self.get_run(run_id)?["executionPolicy"] == 1 || self.choreography(run_id)?.is_some() {
            runtime["choreography"] = self.choreography(run_id)?.unwrap_or(Value::Null);
            runtime["choreographyBlockers"] = json!(self.choreography_blockers(run_id)?);
            let coding = self.coding_record(run_id)?.unwrap_or(Value::Null);
            runtime["packageCompletions"] = coding["completions"].clone();
            runtime["packageClaims"] = coding["claims"].clone();
            runtime["packageWriter"] =
                self.repository_work(run_id)?.unwrap_or(Value::Null)["writer"].clone();
        }
        Ok(runtime)
    }
    pub fn snapshot(&self) -> Result<Value> {
        let mut tasks = self.tasks()?;
        let mut runs = self.all("runs")?;
        let holder = self.coding_slot()?;
        let holder_id = holder.as_ref().and_then(|r| r["id"].as_str());
        for run in &mut runs {
            if let Some(hold) = self.readiness_hold(run, holder_id)? {
                run["readinessHold"] = hold;
            }
        }
        let mut repository_snapshots = json!({});
        for work in self.all("repository_work")? {
            repository_snapshots[work["runId"].as_str().unwrap()] = work["repository"].clone();
        }
        let mut continuations = json!({});
        for run in &runs {
            if run["desiredState"] == "cancelled" && crate::dependencies::coding(run) {
                continuations[run["id"].as_str().unwrap()] =
                    self.continuation_view(run["id"].as_str().unwrap())?;
            }
        }
        if let Some(jira) = self.record("jira_sync", "id", "1")?
            && let Some(order) = jira["taskIds"].as_array()
        {
            tasks.sort_by_key(|t| {
                order
                    .iter()
                    .position(|id| id == &t["id"])
                    .unwrap_or(order.len())
            });
        }
        let mut repositories = self.all("repositories")?;
        for repo in &mut repositories {
            repo["publicationAvailable"] = json!(false);
        }
        let activity: Vec<_> = self.all("assignments")?.iter().filter_map(|assignment| {
            let attempt = self.record("attempts","id",assignment["currentAttemptId"].as_str()?).ok()??;
            Some(json!({"runId":assignment["runId"],"assignmentName":assignment["name"],"state":if attempt["state"] == "exited" || attempt["state"] == "admitted" {attempt["state"].clone()} else {json!("ownership_unconfirmed")}}))
        }).collect();
        Ok(
            json!({"tasks":tasks,"runs":runs,"repositories":repositories,"requirementRevisions":self.all("requirement_revisions")?,
            "notes":self.all("notes")?,"messages":self.all("steering")?,"refinementProposals":self.all("refinement_proposals")?,"learning":self.all("learning_proposals")?,
            "queue":self.queue_view()?,"activity":activity,"connections":{"jira":false,"runner":false},"dependencyStates":self.dependency_states()?,"repositorySnapshots":repository_snapshots,"continuations":continuations,
            "capabilities":{"execution":false,"publication":false,"liveJira":false,"nativePersistence":true}}),
        )
    }
}
