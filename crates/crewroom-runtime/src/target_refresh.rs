//! Operator target requests replay pinned receipts before any provider lookup.
use crate::{
    conflict,
    evidence::Evidence,
    messaging::StoreAccess,
    publication_remote::{PublicationRemote, Ticket},
    repository::{Repository, array, request, text},
};
use crewroom_core::{Error, Result, serialized::stringify};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct TargetRefresh<S: StoreAccess> {
    work: Repository<S>,
    evidence: Evidence<S>,
    remote: PublicationRemote<S>,
}
impl<S: StoreAccess> TargetRefresh<S> {
    pub fn new(work: Repository<S>, evidence: Evidence<S>, remote: PublicationRemote<S>) -> Self {
        Self {
            work,
            evidence,
            remote,
        }
    }
    pub async fn refresh(&self, run: &str, input: &Value) -> Result<Value> {
        request(input, &["requestId", "operatorDecision"])?;
        let key = text(input, "requestId")?.to_owned();
        if !crewroom_core::store::valid_uuid(run) || !crewroom_core::store::valid_uuid(&key) {
            return Err(Error::Invalid(
                "Target refresh requires valid UUIDs.".into(),
            ));
        }
        let decision = if let Some(d) = input.get("operatorDecision") {
            request(d, &["conflictRequestId", "reason"])?;
            let id = text(d, "conflictRequestId")?;
            let reason = crewroom_core::store::trim_text(text(d, "reason")?);
            if !crewroom_core::store::valid_uuid(id)
                || reason.is_empty()
                || reason.chars().count() > 4000
            {
                return Err(Error::Invalid("Invalid target retry decision.".into()));
            }
            Some(json!({"requestId":id,"reason":reason}))
        } else {
            None
        };
        let id = run.to_owned();
        let this = self.clone();
        self.remote
            .ordered(run, &format!("refresh:{run}"), move |ticket| async move {
                let lane = this.remote.lane(&format!("refresh:{id}"));
                let _guard = lane.lock().await;
                ticket.check()?;
                this.serialized(&ticket, &id, &key, decision.as_ref()).await
            })
            .await
    }
    fn owned(&self, run: &str) -> Result<Value> {
        self.remote
            .store(|s| s.repository_work(run))?
            .ok_or_else(|| Error::NotFound("Repository work has not been prepared.".into()))
    }
    fn coding(&self, run: &str) -> Result<Value> {
        self.remote
            .store(|s| s.coding_record(run))?
            .filter(|r| !array(&r["candidates"]).is_empty())
            .ok_or_else(|| conflict("Refreshing from the target needs a recorded candidate."))
    }
    async fn serialized(
        &self,
        ticket: &Ticket,
        run: &str,
        key: &str,
        decision: Option<&Value>,
    ) -> Result<Value> {
        let owned = self.owned(run)?;
        let coding = self.coding(run)?;
        if array(&owned["refreshes"]).iter().any(unverified) {
            return Err(conflict(
                "Retained Git mutation ownership is unverified; inspect every workspace before retrying the target.",
            ));
        }
        let mut replay = array(&owned["refreshes"])
            .iter()
            .find(|r| r["requestId"] == key)
            .cloned();
        if replay.as_ref().is_some_and(|r| {
            r["state"] == "rebasing"
                || r["resolutionWorkspace"]["state"] == "promoting"
                || r["resolutionWorkspace"]["state"] == "ready" && pending(&coding, key)
        }) {
            self.work.reconcile_refresh(run).await?;
            ticket.check()?;
            replay = array(&self.owned(run)?["refreshes"])
                .iter()
                .find(|r| r["requestId"] == key)
                .cloned();
        }
        if let Some(replay) = replay {
            if replay.get("supersedes").map(stringify) != decision.map(stringify) {
                return Err(conflict(
                    "This refresh request has a different operator decision.",
                ));
            }
            if unverified(&replay) {
                return Err(conflict(
                    "Git mutation ownership is unverified; retain the target workspace for operator inspection.",
                ));
            }
            if replay["state"] == "conflicted" {
                return Ok(json!({"changed":false,"refresh":replay}));
            }
            if replay["state"] != "completed" {
                return Err(conflict(
                    "This refresh request was interrupted. Start a new refresh.",
                ));
            }
            return self.complete(run, &replay).await;
        }
        self.work.reconcile_refresh(run).await?;
        ticket.check()?;
        let work = self.owned(run)?;
        let refreshes = array(&work["refreshes"]);
        if refreshes.iter().any(|r| r["state"] == "rebasing") {
            return Err(conflict(
                "Reconcile the existing target refresh before starting another.",
            ));
        }
        if let Some(held) = refreshes
            .iter()
            .rev()
            .find(|r| r["state"] == "conflicted" && r.get("supersededBy").is_none())
        {
            if held
                .get("resolutionWorkspace")
                .is_some_and(|w| w["state"] != "operator_required")
                || decision.is_none_or(|d| d["requestId"] != held["requestId"])
            {
                return Err(conflict(
                    "An operator reason is required to retry this held target conflict.",
                ));
            }
            if pending(&self.coding(run)?, text(held, "requestId")?) {
                return Err(conflict(
                    "Reconcile the held conflict completion before retrying the target.",
                ));
            }
        } else if decision.is_some() {
            return Err(conflict(
                "The operator retry no longer matches an active target hold.",
            ));
        }
        if let Some(old) = refreshes
            .iter()
            .find(|r| r["state"] == "completed" && r.get("candidateId").is_none())
        {
            self.complete(run, old).await?;
        }
        ticket.check()?;
        let coding = self.coding(run)?;
        let source = text(array(&coding["candidates"]).last().unwrap(), "id")?.to_owned();
        let current = self
            .remote
            .store(|s| s.preflight_target_refresh(run, decision))?;
        let target = self
            .remote
            .fetch_current(ticket.clone(), run.into(), key.into(), decision.cloned())
            .await?
            .unwrap_or_else(|| {
                current
                    .get("continuationTargetRef")
                    .unwrap_or(&current["targetRef"])
                    .as_str()
                    .unwrap_or("")
                    .into()
            });
        ticket.check()?;
        let result = self
            .work
            .refresh_base(run, key, &target, &source, decision)
            .await?;
        ticket.check()?;
        if result["refresh"]["state"] == "conflicted" {
            return Ok(json!({"changed":false,"refresh":result["refresh"]}));
        }
        self.complete(run, &result["refresh"]).await
    }
    async fn complete(&self, run: &str, refresh: &Value) -> Result<Value> {
        let changed =
            refresh["fromBase"] != refresh["toBase"] || refresh["fromHead"] != refresh["toHead"];
        if let Some(id) = refresh["candidateId"].as_str() {
            return Ok(json!({"changed":changed,"refresh":refresh,"candidateId":id}));
        }
        // Conflict completion has its own transaction and must never be replaced
        // by ordinary candidate association, including during startup recovery.
        let candidate = self
            .evidence
            .associate_refresh(run, text(refresh, "requestId")?)
            .await?;
        let work = self.owned(run)?;
        let saved = array(&work["refreshes"])
            .iter()
            .find(|r| r["requestId"] == refresh["requestId"])
            .ok_or_else(|| conflict("Target receipt disappeared."))?;
        Ok(json!({"changed":changed,"refresh":saved,"candidateId":candidate["id"]}))
    }
    /// Startup may settle recorded ownership; it never starts a new target fetch.
    pub async fn reconcile(&self) -> Result<()> {
        let runs = self.remote.store(|s| s.snapshot())?["runs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for run in runs {
            let id = text(&run, "id")?.to_owned();
            let this = self.clone();
            self.remote.ordered(&id.clone(),&format!("refresh:{id}"),move |ticket|async move {
                let lane=this.remote.lane(&format!("refresh:{id}"));let _guard=lane.lock().await;
                ticket.check()?;
                let Some(work)=this.remote.store(|s|s.repository_work(&id))? else{return Ok(())};
                let coding=this.remote.store(|s|s.coding_record(&id))?.unwrap_or(Value::Null);
                if !array(&work["refreshes"]).iter().any(|r|r["state"]=="rebasing" || unverified(r)
                    || r["state"]=="conflicted" && r.get("supersededBy").is_none() && (r["resolutionWorkspace"]["state"]=="promoting" || r["resolutionWorkspace"]["state"]=="ready" && pending(&coding,r["requestId"].as_str().unwrap_or("")))
                    || r["state"]=="completed" && (r["resolutionWorkspace"]["state"]=="promoted" || r.get("candidateId").is_none())) {return Ok(())}
                let result:Result<()>=async {
                    this.work.reconcile_refresh(&id).await?;ticket.check()?;
                    let current=this.owned(&id)?;
                    for r in array(&current["refreshes"]).iter().filter(|r|r["state"]=="completed" && r.get("candidateId").is_none()) {this.complete(&id,r).await?;}
                    Ok(())
                }.await;
                if result.is_err() {
                    ticket.with_current(||this.remote.store(|s| {
                        let run=s.get_run(&id)?;
                        if !["cancelled","completed","closed"].contains(&run["status"].as_str().unwrap_or("")) && run["desiredState"]!="cancelled" {s.require_attention(&id,"Git mutation ownership or target recovery is unverified; retain source and workspace for operator inspection before any retry.")?;}
                        Ok(())
                    }))?;
                }
                Ok(())
            }).await?;
        }
        Ok(())
    }
}
fn unverified(refresh: &Value) -> bool {
    refresh["state"] == "conflicted"
        && ["preparing", "continuing"].contains(
            &refresh["resolutionWorkspace"]["state"]
                .as_str()
                .unwrap_or(""),
        )
}
fn pending(coding: &Value, key: &str) -> bool {
    array(&coding["completions"])
        .iter()
        .any(|c| c["state"] == "requested" && c["conflictRefreshId"] == key)
}
