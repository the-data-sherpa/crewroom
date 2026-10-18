//! Assignment-scoped logical calls survive process attempts. An incomplete
//! effect/result is a recovery hold, never permission to execute again.
use super::*;
use crate::host_journal::Journal;
use std::collections::BTreeMap;

pub struct RetainedToolReceipt {
    pub tool: String,
    pub arguments: Value,
    pub result: Option<Value>,
}

pub struct SessionJournal {
    journal: Journal,
    pub thread: Option<String>,
    calls: BTreeMap<String, Value>,
    pub usage: Value,
    unbound: std::collections::BTreeSet<String>,
    pub has_inputs: bool,
    inputs: BTreeMap<String, Value>,
}

impl SessionJournal {
    pub fn open(root: &Path, binding: &Value) -> Result<Self> {
        let mut saved_binding = None;
        let mut thread = None;
        let mut calls = BTreeMap::new();
        let mut usage = json!({});
        let mut unbound = std::collections::BTreeSet::new();
        let mut has_inputs = false;
        let mut inputs = BTreeMap::new();
        let mut journal = Journal::open(&root.join("crewroom.jsonl"), |record| {
            match record["kind"].as_str() {
                Some("binding") if saved_binding.is_none() => {
                    saved_binding = Some(record["value"].clone())
                }
                Some("thread") if thread.is_none() => {
                    thread = Some(text(&record, "id")?.to_owned())
                }
                Some("call") => {
                    let id = text(&record, "callId")?.to_owned();
                    if calls.insert(id, record).is_some() {
                        return Err(conflict("Duplicate logical call intent."));
                    }
                }
                Some("result") => {
                    let id = text(&record, "callId")?;
                    let call = calls
                        .get_mut(id)
                        .ok_or_else(|| conflict("Tool result has no intent."))?;
                    if call.get("result").is_some() {
                        return Err(conflict("Duplicate logical call result."));
                    }
                    call["result"] = record["result"].clone();
                }
                Some("input") => {
                    has_inputs = true;
                    unbound.insert(text(&record, "id")?.to_owned());
                    if inputs
                        .insert(text(&record, "id")?.to_owned(), record)
                        .is_some()
                    {
                        return Err(conflict("Duplicate native logical input intent."));
                    }
                }
                Some("unused_thread_replaced") if !has_inputs && calls.is_empty() => {
                    thread = Some(text(&record, "id")?.to_owned())
                }
                Some("turn") => {
                    unbound.remove(text(&record, "inputId")?);
                }
                Some("terminal") => (),
                Some("usage") => usage = record["value"].clone(),
                _ => return Err(conflict("Invalid Native runner session journal.")),
            }
            Ok(())
        })?;
        if let Some(saved) = saved_binding {
            if &saved != binding {
                return Err(conflict(
                    "Native runner session belongs to a different assignment or frozen profile.",
                ));
            }
        } else {
            if thread.is_some() || !calls.is_empty() {
                return Err(conflict("Native runner session binding is missing."));
            }
            journal.append(&json!({"kind":"binding","value":binding}))?;
        }
        Ok(Self {
            journal,
            thread,
            calls,
            usage,
            unbound,
            has_inputs,
            inputs,
        })
    }
    pub fn record(&mut self, record: Value) -> Result<()> {
        if record["kind"] == "input" && self.inputs.contains_key(text(&record, "id")?) {
            return Err(conflict(
                "Native logical input already has a dispatch intent.",
            ));
        }
        self.journal.append(&record)?;
        match record["kind"].as_str() {
            Some("input") => {
                let id = text(&record, "id")?.to_owned();
                self.has_inputs = true;
                self.unbound.insert(id.clone());
                self.inputs.insert(id, record);
            }
            Some("turn") => {
                self.unbound.remove(text(&record, "inputId")?);
            }
            _ => (),
        }
        Ok(())
    }
    pub fn reconcile_inputs(&self, messages: &BTreeMap<String, String>) -> Result<()> {
        for input in self.inputs.values() {
            if messages.get(text(input, "wireId")?).map(String::as_str) != input["hash"].as_str() {
                return Err(conflict(
                    "Claude input is absent or changed in retained history; dispatch requires reconciliation.",
                ));
            }
        }
        Ok(())
    }
    pub fn bind_thread(&mut self, id: &str) -> Result<()> {
        if self.thread.as_deref().is_some_and(|saved| saved != id) {
            if self.has_inputs || !self.calls.is_empty() {
                return Err(conflict("Native runner thread identity changed."));
            }
            self.record(json!({"kind":"unused_thread_replaced","id":id}))?;
            self.thread = Some(id.into());
        }
        if self.thread.is_none() {
            self.record(json!({"kind":"thread","id":id}))?;
            self.thread = Some(id.into());
        }
        Ok(())
    }
    /// Some is a retained result; None grants exactly one new local admission.
    pub fn admit(
        &mut self,
        call: &str,
        tool: &str,
        args: &Value,
        turn: &str,
    ) -> Result<Option<Value>> {
        self.journal.validate()?;
        if let Some(old) = self.calls.get(call) {
            if old["tool"] != tool || old["arguments"] != *args || old["turnId"] != turn {
                return Err(conflict(
                    "Native runner logical call identity was reused with different content.",
                ));
            }
            return old.get("result").cloned().map(Some).ok_or_else(||conflict("Native runner tool outcome is uncertain; reconcile the original receipt before resuming."));
        }
        let record =
            json!({"kind":"call","callId":call,"tool":tool,"arguments":args,"turnId":turn});
        self.record(record.clone())?;
        self.calls.insert(call.into(), record);
        Ok(None)
    }
    pub fn result(&mut self, call: &str, result: &Value) -> Result<()> {
        self.journal.validate()?;
        let old = self
            .calls
            .get(call)
            .ok_or_else(|| conflict("Unknown Native runner logical call."))?;
        if let Some(saved) = old.get("result") {
            return if saved == result {
                Ok(())
            } else {
                Err(conflict("Native runner result identity changed."))
            };
        }
        self.record(json!({"kind":"result","callId":call,"result":result}))?;
        self.calls.get_mut(call).unwrap()["result"] = result.clone();
        Ok(())
    }
    pub fn reconcile(&self, thread: &Value) -> Result<()> {
        let mut receipts = BTreeMap::new();
        for item in thread["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|turn| turn["items"].as_array().into_iter().flatten())
            .filter(|item| item["type"] == "dynamicToolCall")
        {
            let id = text(item, "id")?.to_owned();
            if receipts.insert(id, RetainedToolReceipt {
                tool: text(item, "tool")?.to_owned(),
                arguments: item["arguments"].clone(),
                result: (item["status"] == "completed").then(|| json!({"success":item["success"],"contentItems":item["contentItems"]})),
            }).is_some() { return Err(conflict("Native retained history repeated a call identity.")); }
        }
        self.reconcile_receipts(&receipts)
    }
    pub fn reconcile_receipts(
        &self,
        receipts: &BTreeMap<String, RetainedToolReceipt>,
    ) -> Result<()> {
        self.journal.validate()?;
        if !self.unbound.is_empty() {
            return Err(conflict(
                "Native runner input dispatch has no durable turn identity. Reconcile retained history before replacement; no input was replayed.",
            ));
        }
        for (id, call) in &self.calls {
            let result = call.get("result").ok_or_else(||conflict("Native runner tool effect is unresolved. Retained intent must be reconciled; no call was replayed."))?;
            let confirmed = receipts.get(id).is_some_and(|receipt| {
                receipt.tool == call["tool"]
                    && receipt.arguments == call["arguments"]
                    && receipt.result.as_ref() == Some(result)
            });
            if !confirmed {
                return Err(conflict(
                    "Native runner did not retain a completed tool receipt. The saved result is preserved; reconcile delivery before resuming. No effect was replayed.",
                ));
            }
        }
        Ok(())
    }
    pub fn usage(&mut self, value: &Value) -> Result<Value> {
        let mut delta = json!({});
        let mut retained = self.usage.clone();
        for (source, target) in [
            ("inputTokens", "input"),
            ("outputTokens", "output"),
            ("cachedInputTokens", "cacheRead"),
            ("cacheWriteInputTokens", "cacheWrite"),
            ("reasoningOutputTokens", "reasoning"),
            ("totalTokens", "total"),
        ] {
            let current = match value.get(source) {
                Some(v) => v
                    .as_u64()
                    .ok_or_else(|| conflict("Invalid native usage counter."))?,
                None => match retained[source].as_u64() {
                    Some(n) => n,
                    None => continue,
                },
            };
            let previous = self.usage[source].as_u64().unwrap_or(0);
            if current < previous {
                return Err(conflict(
                    "Native runner cumulative usage decreased; accounting requires reconciliation.",
                ));
            }
            delta[target] = json!(current);
            retained[source] = json!(current);
        }
        if let Some(cost) = value.get("estimatedCostUsd") {
            let cost = cost
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0.)
                .ok_or_else(|| conflict("Invalid native cost estimate."))?;
            if retained["estimatedCostUsd"]
                .as_f64()
                .is_some_and(|old| cost + 1e-12 < old)
            {
                return Err(conflict(
                    "Native cumulative cost decreased; reconcile accounting before resume.",
                ));
            }
            retained["estimatedCostUsd"] = json!(cost);
        }
        self.record(json!({"kind":"usage","value":retained}))?;
        self.usage = retained;
        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_reconciles_original_call_receipt_and_never_readmits_unknown_effect() {
        for phase in 0..3 {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let binding = json!({"assignment":"one"});
            let result =
                json!({"success":true,"contentItems":[{"type":"inputText","text":"saved"}]});
            let args = json!({"text":"effect"});
            {
                let mut journal = SessionJournal::open(&root, &binding).unwrap();
                journal.bind_thread("thread").unwrap();
                assert!(
                    journal
                        .admit("call", "write", &args, "turn")
                        .unwrap()
                        .is_none()
                );
                if phase > 0 {
                    journal.result("call", &result).unwrap();
                }
            }
            let mut journal = SessionJournal::open(&root, &binding).unwrap();
            let thread = if phase == 2 {
                json!({"turns":[{"items":[{"type":"dynamicToolCall","id":"call","tool":"write","arguments":args,"status":"completed","success":true,"contentItems":result["contentItems"]}]}]})
            } else {
                json!({"turns":[]})
            };
            assert_eq!(journal.reconcile(&thread).is_ok(), phase == 2);
            if phase == 0 {
                assert!(journal.admit("call", "write", &args, "turn").is_err());
            } else {
                assert_eq!(
                    journal.admit("call", "write", &args, "turn").unwrap(),
                    Some(result)
                );
            }
            assert!(
                journal
                    .admit("call", "write", &json!({"text":"different"}), "turn")
                    .is_err()
            );
            assert!(
                journal
                    .admit("call", "write", &args, "different-turn")
                    .is_err()
            );
        }
    }
    #[test]
    fn input_dispatch_binding_and_usage_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let binding = json!({"model":"one"});
        {
            let mut j = SessionJournal::open(&root, &binding).unwrap();
            j.record(json!({"kind":"input","id":"one"})).unwrap();
            assert_eq!(
                j.usage(&json!({"inputTokens":20,"totalTokens":20}))
                    .unwrap()["input"],
                20
            );
        }
        {
            let j = SessionJournal::open(&root, &binding).unwrap();
            assert!(j.reconcile(&json!({"turns":[]})).is_err());
        }
        {
            let mut j = SessionJournal::open(&root, &binding).unwrap();
            j.record(json!({"kind":"turn","inputId":"one","turnId":"turn"}))
                .unwrap();
            assert_eq!(
                j.usage(&json!({"inputTokens":25,"totalTokens":25}))
                    .unwrap()["input"],
                25
            );
        }
        {
            let j = SessionJournal::open(&root, &binding).unwrap();
            j.reconcile(&json!({"turns":[]})).unwrap();
        }
        assert!(SessionJournal::open(&root, &json!({"model":"changed"})).is_err());
    }
}
