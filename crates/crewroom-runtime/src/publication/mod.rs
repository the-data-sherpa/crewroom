//! Revision-bound publication intents and observations. Remote effects retain
//! separate push/PR receipts; no operation merges a pull request.
mod dispatch;
mod policy;
mod prepare;
use crate::{
    conflict,
    delivery_flight::DeliveryFlight,
    evidence::Evidence,
    messaging::StoreAccess,
    providers::remote::UnavailablePublication,
    publication_remote::{PublicationRemote, Ticket, unmatched},
    repository::{Repository, array, text},
};
use crewroom_core::{Error, Result, store::Store};
pub use policy::assess;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;
fn time() -> String {
    crewroom_core::timestamp(crewroom_core::now_ms())
}
fn present(v: &Value) -> bool {
    !v.is_null() && v != false
}
/// The provider name ("Jira", "GitHub") of a run whose ticket requirements changed under it.
fn source_conflict(run: &Value) -> Option<&'static str> {
    present(&run["sourceConflict"])
        .then(|| crewroom_core::ticket_source::source_name(&run["taskSnapshot"]))
}
fn remove(v: &mut Value, key: &str) {
    v.as_object_mut().unwrap().remove(key);
}
fn digest(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}
fn marker(v: &Value) -> String {
    format!("Crewroom publication {}", v["id"].as_str().unwrap_or(""))
}
fn identity(record: &Value, pr: &Value) -> bool {
    pr["description"]
        .as_str()
        .is_some_and(|s| s.contains(&marker(record)))
        && pr["source"]["branch"]["name"] == record["branch"]
        && pr["destination"]["branch"]["name"] == record["target"]
        && pr["source"]["repository"]["uuid"] == record["repository"]["uuid"]
        && pr["destination"]["repository"]["uuid"] == record["repository"]["uuid"]
        && (record["pr"]["id"].is_null() || pr["id"] == record["pr"]["id"])
}
#[derive(Clone)]
pub struct Publication<S: StoreAccess> {
    remote: PublicationRemote<S>,
    work: Repository<S>,
    evidence: Evidence<S>,
    flight: DeliveryFlight,
    mutation: Arc<Mutex<()>>,
}
impl<S: StoreAccess> Publication<S> {
    pub fn new(remote: PublicationRemote<S>, work: Repository<S>, evidence: Evidence<S>) -> Self {
        Self {
            remote,
            work,
            evidence,
            flight: DeliveryFlight::default(),
            mutation: Arc::new(Mutex::new(())),
        }
    }
    pub fn available(&self, run: &str) -> Result<bool> {
        Ok(self
            .remote
            .store(|s| s.repository_work(run))?
            .is_some_and(|w| {
                self.remote
                    .connection(w["repository"]["path"].as_str().unwrap_or(""))
                    .is_some()
            }))
    }
    fn unavailable_provider(&self, run: &str) -> Result<Option<UnavailablePublication>> {
        Ok(self
            .remote
            .store(|s| s.repository_work(run))?
            .and_then(|w| {
                self.remote
                    .unavailable(w["repository"]["path"].as_str().unwrap_or(""))
            }))
    }
    async fn local(&self, run: &str) -> Option<Value> {
        self.evidence.assess(run).await.ok()
    }
    pub async fn read(&self, run: &str) -> Result<Option<Value>> {
        let Some(mut record) = self.remote.store(|s| s.publication(run))? else {
            return Ok(None);
        };
        let local = self.local(run).await;
        let source = self
            .remote
            .store(|s| Ok(source_conflict(&s.get_run(run)?)))?;
        let assessed = assess(&record, local.as_ref(), source);
        record
            .as_object_mut()
            .unwrap()
            .extend(assessed.as_object().unwrap().clone());
        Ok(Some(record))
    }
    pub async fn view(&self, run: &str) -> Result<Value> {
        let record = self.read(run).await?;
        let available = self.available(run)?;
        let p = record.as_ref().unwrap_or(&Value::Null);
        let reason = if present(&p["merged"]) {
            format!(
                "Merged into {} as {}. Delivery is complete.",
                p["target"].as_str().unwrap_or(""),
                p["merged"]["commit"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(12)
                    .collect::<String>()
            )
        } else if p["ready"] == true {
            "PR ready for human review. Merge remains your decision.".into()
        } else if p["pr"]["state"] == "confirmed" {
            "PR created. Validation is pending or blocked; inspect the recorded evidence.".into()
        } else if record.is_some() {
            "Publication requested. Push and PR receipts are tracked separately.".into()
        } else if self
            .remote
            .store(|s| Ok(present(&s.get_run(run)?["continuedFrom"]["publicationId"])))?
        {
            "An existing PR is retained in predecessor history. This run requires fresh validation before updating it.".into()
        } else if available {
            "Publication is available after local validation. The coordinator can request it."
                .into()
        } else if let Some(unavailable) = self.unavailable_provider(run)? {
            unavailable.reason
        } else {
            "PR publication is unavailable. Local validation can complete; no PR has been created."
                .into()
        };
        let mut view = json!({"available":available,"reason":reason});
        if !available && let Some(unavailable) = self.unavailable_provider(run)? {
            view["provider"] = json!({"state":"unavailable","host":unavailable.host});
        }
        if let Some(record) = record {
            view["record"] = record;
        }
        Ok(view)
    }
    pub async fn observe_feedback(&self, run: &str) -> Result<Option<Value>> {
        let this = self.clone();
        let id = run.to_owned();
        self.remote
            .operation(run, move |ticket| async move {
                let Some(record) = this.remote.store(|s| s.publication(&id))? else {
                    return Ok(None);
                };
                if record["pr"]["state"] != "confirmed" || record["pr"]["id"].as_i64().is_none() {
                    return Ok(record.get("feedback").cloned());
                }
                let started = time();
                let pr = record["pr"]["id"].as_i64().unwrap();
                ticket.check()?;
                let observation = match this.remote.matched(&record) {
                    Some(connection) => connection
                        .host
                        .comments(pr)
                        .await
                        .map_err(|e| conflict(&e.to_string()))?,
                    None => {
                        json!({"state":"unavailable","comments":[],"reason":unmatched(&record)})
                    }
                };
                // An already observed response is useful after stop; it authorizes no
                // new effect. The store verifies PR identity and merges newer feedback.
                this.remote
                    .store(|s| s.save_publication_feedback(&id, pr as u64, &observation, &started))
                    .map(Some)
            })
            .await
    }
    pub async fn flush(&self) -> Result<()> {
        let this = self.clone();
        self.flight
            .run(async move { this.perform_flush().await })
            .await
    }
    pub async fn stop_run(&self, run: &str) -> Result<()> {
        self.remote.stop_run(run).await
    }
    pub async fn close(&self) -> Result<()> {
        let (flight, remote) = tokio::join!(self.flight.close(), self.remote.close());
        remote?;
        flight
    }
    fn save(&self, record: &Value) -> Result<()> {
        self.remote
            .store(|s| s.save_publication(record))
            .map(|_| ())
    }
    fn writable(s: &Store, record: &Value) -> Result<()> {
        let run = s.get_run(text(record, "runId")?)?;
        if run["status"] != "running"
            || run["desiredState"] != "running"
            || present(&run["sourceConflict"])
        {
            return Err(conflict(
                "Publication is waiting for an active run with accepted requirements.",
            ));
        }
        let current = s
            .publication(text(record, "runId")?)?
            .unwrap_or(Value::Null);
        if selected(&current) != selected(record) {
            return Err(conflict("Publication changed before dispatch."));
        }
        Ok(())
    }
    async fn may_write(&self, record: &Value, ticket: &Ticket) -> Result<Proof> {
        let run = text(record, "runId")?;
        let proof = ticket.with_current(|| {
            self.flight.with_open(|| {
                self.remote.store(|s| {
                    Self::writable(s, record)?;
                    Proof::capture(s, run)
                })
            })
        })?;
        let local = self.evidence.assess(run).await?;
        ticket.with_current(|| {
            self.flight.with_open(|| {
                self.remote.store(|s| {
                    Self::writable(s, record)?;
                    proof.check(s)?;
                    if local["ready"] != true
                        || local["candidate"]["id"] != record["candidateId"]
                        || local["candidate"]["revision"] != record["revision"]
                    {
                        return Err(conflict(
                            "Publication evidence is stale. Revalidate before writing.",
                        ));
                    }
                    Ok(())
                })
            })
        })?;
        Ok(proof)
    }
}
fn selected(p: &Value) -> Value {
    json!({"id":p["id"],"candidateId":p["candidateId"],"revision":p["revision"],"description":p["description"],"repository":p["repository"],"repositoryPath":p["repositoryPath"],"branch":p["branch"],"target":p["target"],"policy":p["policy"]})
}
struct Proof {
    run: String,
    facts: Value,
}
impl Proof {
    fn facts(s: &Store, id: &str) -> Result<Value> {
        let run = s.get_run(id)?;
        let runtime = s.runtime(id)?;
        Ok(
            json!({"controlRevision":run["controlRevision"],"status":run["status"],"desiredState":run["desiredState"],"taskRevision":run["taskRevision"],"taskSnapshot":run["taskSnapshot"],"sourceConflict":run["sourceConflict"],"coding":s.coding_record(id)?,"work":s.repository_work(id)?,"insertion":s.insertion_blockers(id)?,"assignments":array(&runtime["assignments"]).iter().map(|a|json!({"id":a["id"],"currentAttemptId":a["currentAttemptId"],"retiredAt":a["retiredAt"],"rebriefRequired":a["rebriefRequired"]})).collect::<Vec<_>>(),"publication":selected(&s.continuation_publication(id)?.unwrap_or(Value::Null))}),
        )
    }
    fn capture(s: &Store, run: &str) -> Result<Self> {
        Ok(Self {
            run: run.into(),
            facts: Self::facts(s, run)?,
        })
    }
    fn check(&self, s: &Store) -> Result<()> {
        if Self::facts(s, &self.run)? != self.facts {
            return Err(conflict(
                "Evidence or control changed before publication authorization.",
            ));
        }
        Ok(())
    }
}
