use super::*;
use crate::supervisor::tasks::Task;
use serde_json::json;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Inventory {
    known: HashSet<PathBuf>,
    selected: HashMap<PathBuf, CheckedProcess>,
    roots: Vec<(Option<String>, PathBuf)>,
    reasons: Vec<String>,
}
fn relevant(selected: Option<&str>, owner: Option<&str>) -> bool {
    selected.is_none() || owner.is_none() || selected == owner
}
impl Inventory {
    fn add(&mut self, records: Records, run: Option<&str>) -> Vec<CheckedProcess> {
        self.roots.extend(records.roots);
        self.reasons.extend(
            records
                .errors
                .into_iter()
                .filter(|(owner, _)| relevant(run, owner.as_deref()))
                .map(|(_, error)| error),
        );
        let mut added = Vec::new();
        for (owner, descriptor) in records.saved {
            self.known.insert(descriptor.root.clone());
            if relevant(run, Some(&owner)) && self.select(descriptor.clone()) {
                added.push(descriptor);
            }
        }
        added
    }
    fn select(&mut self, descriptor: CheckedProcess) -> bool {
        self.known.insert(descriptor.root.clone());
        if let Some(saved) = self.selected.get(&descriptor.root) {
            if saved != &descriptor {
                self.reasons
                    .push("Conflicting checked Git ownership records.".into());
            }
            false
        } else {
            self.selected.insert(descriptor.root.clone(), descriptor);
            true
        }
    }
    fn check_roots(&mut self, run: Option<&str>) {
        for (owner, root) in &self.roots {
            if relevant(run, owner.as_deref()) && !self.known.contains(root) {
                self.reasons.push(format!(
                    "Git process directory has no verified ownership record: {}.",
                    root.display()
                ));
            }
        }
        self.reasons.sort();
        self.reasons.dedup();
    }
}
impl Git {
    pub fn busy(&self, run: &str) -> bool {
        self.operations.busy(run) || self.cleanup.busy(run)
    }
    /// Fence synchronously; registered cleanup survives cancellation of its waiter.
    /// Passive reads and ordinary recovery never call this signaling boundary.
    pub fn stop(
        &self,
        run: Option<&str>,
        additional: impl Fn() -> Records + Send + 'static,
    ) -> Result<Task> {
        self.authority.assert_current()?;
        if let Some(run) = run {
            uuid::Uuid::parse_str(run).map_err(|_| conflict("Invalid repository run identity."))?;
        }
        let (epoch, pending) = self.operations.fence(run);
        let this = self.clone();
        let run_id = run.map(String::from);
        self.cleanup.enqueue(run.unwrap_or("__close__"), None, move || async move {
            let run = run_id.as_deref();
            let mut records = this.saved_records();
            records.extend(additional());
            let mut inventory = Inventory::default();
            inventory.add(records, run);
            for descriptor in pending.iter().filter_map(|p| p.descriptor.clone()) {
                inventory.select(descriptor);
            }
            // Each known owned child is attempted even if another record is corrupt.
            for descriptor in inventory.selected.values() {
                if let Err(e) = this.stop_descriptor(descriptor).await {
                    inventory.reasons.push(e.to_string());
                }
            }
            for mut job in pending {
                while !*job.done.borrow_and_update() {
                    if job.done.changed().await.is_err() {
                        inventory.reasons.push("Registered Git worker did not confirm completion.".into());
                        break;
                    }
                }
            }
            // A description may have reached SQLite after the first snapshot.
            // Its fenced ticket cannot dispatch; seal it after preparation drains.
            let mut records = this.saved_records();
            records.extend(additional());
            for descriptor in inventory.add(records, run) {
                if let Err(e) = this.stop_descriptor(&descriptor).await {
                    inventory.reasons.push(e.to_string());
                }
            }
            inventory.check_roots(run);
            let mut results = Vec::new();
            for descriptor in inventory.selected.values() {
                if let Err(e) = this.authority.assert_current() {
                    inventory.reasons.push(e.to_string());
                    break;
                }
                let owned = crate::seal_checked(descriptor).await;
                if owned.state != "stopped" {
                    inventory.reasons.push(format!("Git ownership remains unresolved at {}.", descriptor.root.display()));
                }
                results.push(json!({"root":descriptor.root,"state":owned.state,"reason":owned.reason}));
            }
            results.sort_by(|a,b| a["root"].as_str().cmp(&b["root"].as_str()));
            let stopped = inventory.reasons.is_empty();
            if stopped {
                this.operations.stopped(run, epoch);
            }
            Ok(json!({"stopped":stopped,"operations":results,"ownershipErrors":inventory.reasons}))
        })
    }
    async fn stop_descriptor(&self, descriptor: &CheckedProcess) -> Result<()> {
        self.authority.assert_current()?;
        let owned = crate::seal_checked(descriptor).await;
        if owned.state == "live" {
            self.authority.assert_current()?;
            let _ = crate::stop_owned(&descriptor.root).await;
        }
        Ok(())
    }
}
