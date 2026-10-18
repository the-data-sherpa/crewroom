use super::*;
use std::time::Duration;
use tokio::time::{Instant, sleep};

impl<S: StoreAccess> HostManager<S> {
    pub(super) fn register_observer(&self, slot: &Arc<Slot>, verified: bool) -> Result<()> {
        let registry = self.0.registry.lock().unwrap();
        if registry.closing || registry.stopping.contains(&slot.run) || slot.cancel.is_cancelled() {
            return Err(conflict("Agent observation was stopped."));
        }
        if slot.observer.lock().unwrap().is_some() {
            return Err(conflict("Agent already has an observer."));
        }
        drop(registry);
        self.status(slot, if verified { "active" } else { "degraded" }, None);
        let this = self.clone();
        let observed = slot.clone();
        let task=self.0.observers.enqueue(&slot.attempt,Some("observe"),move ||async move {
            if let Err(error)=this.observe(&observed,verified).await {
                this.status(&observed,"blocked",Some("Agent observations could not be reconciled; inspect saved ownership and events."));
                return Err(Arc::new(error));
            }
            Ok(Value::Null)
        })?;
        *slot.observer.lock().unwrap() = Some(task);
        Ok(())
    }
    async fn observe(&self, slot: &Slot, mut verified: bool) -> Result<()> {
        let client = slot
            .client
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| conflict("Observed host has no endpoint."))?;
        let mut cursor = 0_u64;
        let mut backoff = 500_u64;
        while !slot.cancel.is_cancelled() {
            let started = Instant::now();
            let attempt = self.store(|s| s.get_attempt(&slot.attempt))?;
            let request = async {
                if !verified {
                    let identity = client
                        .request(&json!({"op":"identity"}), &slot.cancel)
                        .await?;
                    // A mismatching HTTP identity is a hard hold even if the
                    // saved OS processes are still running.
                    if Self::check_identity(&attempt, &identity).is_err() {
                        return Ok::<_, crewroom_core::Error>(None);
                    }
                    verified = true;
                }
                Ok(Some(
                    client
                        .request(
                            &json!({"op":"events","after":cursor,"wait":5000}),
                            &slot.cancel,
                        )
                        .await?,
                ))
            };
            let reply = tokio::select! {
                _=slot.cancel.cancelled()=>break,
                _=slot.wake.notified()=>Err(conflict("Pane observation woke ownership reconciliation.")),
                result=request=>result,
            };
            if slot.cancel.is_cancelled() {
                break;
            }
            let mut events = 0;
            let unavailable = match reply {
                Ok(None) => {
                    self.status(
                        slot,
                        "blocked",
                        Some("Live process identity differs from the saved attempt."),
                    );
                    return Ok(());
                }
                Ok(Some(page)) => {
                    let page = page
                        .as_array()
                        .ok_or_else(|| conflict("Host event page is not an array."))?;
                    if page.len() > crate::host_journal::EVENT_PAGE_RECORDS {
                        return Err(conflict("Host event page exceeds its record limit."));
                    }
                    for item in page {
                        if item["sequence"].as_u64() != Some(cursor + 1)
                            || !item["event"].is_object()
                        {
                            return Err(conflict(
                                "Host event page has an invalid sequence or payload.",
                            ));
                        }
                        self.store(|s| {
                            s.record_agent_event(&slot.attempt, cursor + 1, &item["event"])
                        })?;
                        cursor += 1;
                        events += 1;
                    }
                    false
                }
                Err(_) => true,
            };
            let ownership = if unavailable {
                Some(self.owned_state(&attempt).await?)
            } else {
                None
            };
            if slot.cancel.is_cancelled() {
                break;
            }
            if ownership.as_ref().is_some_and(|v| v["state"] != "live") {
                self.collect_events(slot).await?;
            }
            let (observed, policy) = self.store(|s| {
                Ok((
                    s.get_attempt(&slot.attempt)?,
                    s.has_agent_event(&slot.attempt, "model_policy_rejected")?,
                ))
            })?;
            if policy {
                self.status(slot,"blocked",Some("Model policy or input budget was rejected. Inspect the retained event before resuming."));
                return Ok(());
            }
            if matches!(
                observed["state"].as_str(),
                Some("failed" | "attention_required")
            ) {
                self.status(
                    slot,
                    "recovery_required",
                    Some("The attempt failed; reconcile ownership before any replacement."),
                );
                return Ok(());
            }
            if let Some(ownership) = ownership {
                match ownership["state"].as_str() {
                    Some("live")=>self.status(slot,"degraded",Some("Host observations are unavailable; its saved OS processes remain live.")),
                    Some("uncertain")=>{self.status(slot,"blocked",Some("Process ownership is uncertain after an observation failure."));return Ok(());}
                    _=>{self.status(slot,"recovery_required",Some("The saved host/runner pair is no longer fully live; stop and reconcile before replacement."));return Ok(());}
                }
            } else {
                self.status(slot, "active", None);
            }
            let pause = if unavailable {
                backoff
            } else if events > 0 {
                0
            } else {
                200_u64.saturating_sub(started.elapsed().as_millis() as u64)
            };
            backoff = if unavailable {
                (backoff * 2).min(2000)
            } else {
                500
            };
            if pause > 0 {
                tokio::select! {_=slot.cancel.cancelled()=>break,_=slot.wake.notified()=>{},_=sleep(Duration::from_millis(pause))=>{}}
            }
        }
        Ok(())
    }
}
