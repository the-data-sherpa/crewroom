//! External delivery around the existing durable message transactions. Storage,
//! acceptance, runner receipt and processing remain distinct states.
use crate::{conflict, mail::MailClient};
use crewroom_core::{Result, source_time, store::Store};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};

/// Short synchronous access lets the supervisor retain its authority-owned store.
/// Implementations must release the guard before returning; no network operation
/// runs inside this callback. The daemon can also check its authority here.
pub trait StoreAccess: Clone + Send + Sync + 'static {
    fn with_store<T>(&self, operation: impl FnOnce(&Store) -> Result<T>) -> Result<T>;
}
impl StoreAccess for Arc<Mutex<Store>> {
    fn with_store<T>(&self, operation: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        let store = self
            .lock()
            .map_err(|_| conflict("Workflow store access failed."))?;
        operation(&store)
    }
}
struct Inner {
    mail: MailClient,
    project: String,
    retention_ms: i64,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    next_observation: Mutex<i64>,
    observing: Semaphore,
    next_mailbox: Mutex<usize>,
}
#[derive(Clone)]
pub struct Messaging<S: StoreAccess> {
    store: S,
    inner: Arc<Inner>,
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| conflict("Invalid durable message identity."))
}
fn assignments(runtime: &Value) -> Result<&Vec<Value>> {
    runtime["assignments"]
        .as_array()
        .ok_or_else(|| conflict("Invalid assignment state."))
}
fn safe_integer(v: &Value) -> Option<u64> {
    v.as_f64()
        .filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= 9_007_199_254_740_991.0)
        .map(|n| n as u64)
}
/// Retired entries still consume their original index. Names are never recycled
/// within the same run, including across registration retries and restarts.
pub fn mailbox_names() -> Vec<String> {
    let mut names: Vec<String> = [
        "BlueLake",
        "GreenCastle",
        "RedPine",
        "SilverHill",
        "AmberCreek",
    ]
    .iter()
    .map(|s| (*s).into())
    .collect();
    for adjective in [
        "Blue", "Green", "Red", "Silver", "Amber", "White", "Brown", "Orange", "Purple", "Golden",
    ] {
        for noun in [
            "Lake", "Castle", "Pine", "Hill", "Creek", "Mountain", "Dog", "Stone", "River",
            "Forest",
        ] {
            let name = format!("{adjective}{noun}");
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}
impl<S: StoreAccess> Messaging<S> {
    pub fn new(store: S, mail: MailClient, project: String, retention: Duration) -> Result<Self> {
        Self::with_clock(store, mail, project, retention, crewroom_core::now_ms)
    }
    pub fn with_clock(
        store: S,
        mail: MailClient,
        project: String,
        retention: Duration,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Result<Self> {
        let retention_ms = i64::try_from(retention.as_millis())
            .map_err(|_| conflict("Invalid Mail retry retention."))?;
        if project.is_empty() || retention_ms == 0 {
            return Err(conflict("Mail project and retry retention are required."));
        }
        Ok(Self {
            store,
            inner: Arc::new(Inner {
                mail,
                project,
                retention_ms,
                now: Arc::new(now),
                next_observation: Mutex::new(0),
                observing: Semaphore::new(1),
                next_mailbox: Mutex::new(0),
            }),
        })
    }
    async fn call(&self, name: &str, args: Value) -> Result<Value> {
        self.inner
            .mail
            .call(name, args)
            .await
            .map_err(|error| std::io::Error::other(error).into())
    }
    pub async fn register(&self, run_id: &str) -> Result<()> {
        self.call("ensure_project", json!({"human_key":self.inner.project}))
            .await?;
        let runtime = self.store.with_store(|s| s.runtime(run_id))?;
        let names = mailbox_names();
        for (index, assignment) in assignments(&runtime)?.iter().enumerate() {
            if assignment.get("retiredAt").is_some_and(|v| !v.is_null()) {
                continue;
            }
            let name=assignment["mailboxName"].as_str().or_else(||names.get(index).map(String::as_str)).ok_or_else(||conflict("This run has exhausted the 100 verified Rust Mail identities; start a separate run."))?;
            let model = self.store.with_store(|s| {
                s.bind_mailbox(field(assignment, "id")?, name)?;
                let attempt = s.get_attempt(field(assignment, "currentAttemptId")?)?;
                let model = attempt
                    .get("model")
                    .filter(|v| !v.is_null())
                    .unwrap_or(&assignment["model"]);
                Ok(field(&model["resolved"], "selector")?.to_owned())
            })?;
            let registered=self.call("register_agent",json!({"project_key":self.inner.project,"name":name,"program":"omp","model":model})).await?;
            if registered["name"] != name {
                return Err(conflict("Mailbox registration identity mismatch."));
            }
            self.call(
                "set_contact_policy",
                json!({"project_key":self.inner.project,"agent_name":name,"policy":"open"}),
            )
            .await?;
        }
        Ok(())
    }
    pub async fn send(&self, attempt_id: &str, tool_call_id: &str, input: &Value) -> Result<Value> {
        let message = self
            .store
            .with_store(|s| s.prepare_message(attempt_id, tool_call_id, input))?;
        self.reconcile(field(&message, "id")?).await
    }
    pub async fn reconcile(&self, id: &str) -> Result<Value> {
        let message = self.store.with_store(|s| s.get_peer_message(id))?;
        if message.get("transportId").is_some() {
            return Ok(message);
        }
        let runtime = self
            .store
            .with_store(|s| s.runtime(field(&message, "runId")?))?;
        let assignments = assignments(&runtime)?;
        let sender = assignments
            .iter()
            .find(|a| a["id"] == message["from"])
            .and_then(|a| a["mailboxName"].as_str());
        let recipient = assignments
            .iter()
            .find(|a| a["id"] == message["to"])
            .and_then(|a| a["mailboxName"].as_str());
        let (Some(sender), Some(recipient)) = (sender, recipient) else {
            return Err(conflict("Mailboxes are not registered."));
        };
        let created = source_time::parse(field(&message, "createdAt")?).ok_or_else(|| {
            conflict("Message retry timestamp is invalid; storage outcome is uncertain.")
        })?;
        if (self.inner.now)().saturating_sub(created) >= self.inner.retention_ms {
            let inbox=self.call("fetch_inbox",json!({"project_key":self.inner.project,"agent_name":recipient,"include_bodies":true,"mark_read":false,"limit":100})).await?;
            let inbox = inbox
                .as_array()
                .ok_or_else(|| conflict("Invalid Mail inbox response."))?;
            let mut matches = Vec::new();
            for item in inbox {
                let transport = safe_integer(&item["id"])
                    .filter(|id| *id > 0)
                    .ok_or_else(|| conflict("Invalid Mail inbox identity."))?;
                for key in ["subject", "from", "body_md"] {
                    if !item[key].is_string() {
                        return Err(conflict("Invalid Mail inbox response."));
                    }
                }
                if item["subject"] == id
                    && item["from"] == sender
                    && item["body_md"] == message["body"]
                {
                    matches.push(transport);
                }
            }
            if matches.len() != 1 {
                return self.store.with_store(|s| {
                    let current = s.get_peer_message(id)?;
                    if current.get("transportId").is_some() { return Ok(current); }
                    s.mark_message_uncertain(id)?;
                    Err(conflict("Message retry key expired; storage outcome remains uncertain. Do not resend."))
                });
            }
            return self
                .store
                .with_store(|s| s.record_message_stored(id, matches[0]));
        }
        let result=self.call("send_message",json!({"project_key":self.inner.project,"sender_name":sender,"to":[recipient],"subject":id,"body_md":message["body"],"ack_required":true,"thread_id":message["runId"],"idempotency_key":id})).await?;
        if safe_integer(&result["count"]) != Some(1)
            || result["deliveries"].as_array().is_none_or(|v| v.len() != 1)
        {
            return Err(conflict("Invalid Mail storage receipt."));
        }
        let transport = safe_integer(&result["deliveries"][0]["payload"]["id"])
            .filter(|id| *id > 0)
            .ok_or_else(|| conflict("Invalid Mail storage receipt."))?;
        self.store
            .with_store(|s| s.record_message_stored(id, transport))
    }
    pub async fn accept(&self, id: &str) -> Result<Value> {
        let message = self.store.with_store(|s| s.accept_message(id))?;
        let runtime = self
            .store
            .with_store(|s| s.runtime(field(&message, "runId")?))?;
        let name = assignments(&runtime)?
            .iter()
            .find(|a| a["id"] == message["to"])
            .and_then(|a| a["mailboxName"].as_str())
            .ok_or_else(|| conflict("Recipient mailbox is not registered."))?;
        self.call("acknowledge_message",json!({"project_key":self.inner.project,"agent_name":name,"message_id":message["transportId"]})).await?;
        Ok(message)
    }
    pub async fn observe(&self, run_id: &str) -> Result<Vec<Value>> {
        let Ok(_observer) = self.inner.observing.try_acquire() else {
            return Ok(vec![]);
        };
        {
            let mut next = self.inner.next_observation.lock().unwrap();
            let now = (self.inner.now)();
            if now < *next {
                return Ok(vec![]);
            }
            *next = now.saturating_add(1500);
        }
        let runtime = self.store.with_store(|s| s.runtime(run_id))?;
        let mut collected = Vec::new();
        let mut retained_bytes = 0;
        let mut pages = 0;
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut mailboxes: VecDeque<_> = assignments(&runtime)?
            .iter()
            .enumerate()
            .filter(|(_, a)| a["mailboxName"].is_string())
            .collect();
        let next = *self.inner.next_mailbox.lock().unwrap();
        if let Some(offset) = mailboxes.iter().position(|(index, _)| *index >= next) {
            mailboxes.rotate_left(offset);
        }
        while let Some((index, assignment)) = mailboxes.pop_front() {
            let name = assignment["mailboxName"].as_str().unwrap();
            let id = field(assignment, "id")?;
            let after = self.store.with_store(|s| s.observer_cursor(id))?;
            let page=self.inner.mail.call_before("fetch_inbox_events",json!({"project_key":self.inner.project,"agent_name":name,"after":after,"limit":100}),deadline).await.map_err(std::io::Error::other)?;
            let cursor = safe_integer(&page["next_cursor"])
                .ok_or_else(|| conflict("Invalid mailbox observer cursor."))?
                as i64;
            let more = page["has_more"]
                .as_bool()
                .ok_or_else(|| conflict("Invalid mailbox observer page."))?;
            let events = page["events"]
                .as_array()
                .ok_or_else(|| conflict("Invalid mailbox observer page."))?;
            if events.len() > 100
                || events.iter().any(|e| {
                    safe_integer(&e["cursor"]).is_none()
                        || safe_integer(&e["message_id"]).is_none_or(|n| n == 0)
                })
            {
                return Err(conflict("Invalid mailbox observer events."));
            }
            if more && cursor <= after {
                return Err(conflict("Mailbox observer made no progress."));
            }
            // JSON numbers such as 1.0 denote the same safe integer on the
            // wire. Canonicalize before the store's integer transaction.
            let events: Vec<Value> = events
                .iter()
                .cloned()
                .map(|mut event| {
                    event["cursor"] = json!(safe_integer(&event["cursor"]).unwrap());
                    event["message_id"] = json!(safe_integer(&event["message_id"]).unwrap());
                    event
                })
                .collect();
            self.store
                .with_store(|s| s.record_observer_page(id, &events, cursor))?;
            *self.inner.next_mailbox.lock().unwrap() = index + 1;
            pages += 1;
            retained_bytes += crewroom_core::serialized::stringify(&page["events"]).len();
            collected.extend(events.iter().cloned());
            // Each saved page is atomic. A large backlog continues from its
            // durable cursor on the next poll rather than holding observation
            // indefinitely or retaining unbounded event payloads in memory.
            if collected.len() >= 1000 || retained_bytes >= 8 * 1024 * 1024 || pages >= 100 {
                return Ok(collected);
            }
            if more {
                mailboxes.push_back((index, assignment));
            }
        }
        Ok(collected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_retain_the_verified_order_without_reuse() {
        let names = mailbox_names();
        assert_eq!(names.len(), 100);
        assert_eq!(
            names.iter().collect::<std::collections::HashSet<_>>().len(),
            100
        );
        assert_eq!(
            &names[..6],
            &[
                "BlueLake",
                "GreenCastle",
                "RedPine",
                "SilverHill",
                "AmberCreek",
                "BlueCastle"
            ]
        );
        assert_eq!(names.last().unwrap(), "GoldenForest");
        assert!(names.get(100).is_none());
    }
    #[test]
    fn transport_numbers_match_safe_integer_semantics() {
        for raw in ["0", "1", "1.0", "9007199254740991"] {
            assert!(safe_integer(&serde_json::from_str(raw).unwrap()).is_some());
        }
        for raw in ["-1", "1.1", "9007199254740992", "\"1\"", "null", "true"] {
            assert!(safe_integer(&serde_json::from_str(raw).unwrap()).is_none());
        }
    }
}
