use super::*;
use std::collections::{HashSet, VecDeque};

/// Only the selected run is retained. A new selection starts a fresh cursor;
/// transport reconnects preserve already delivered events and mark snapshots stale.
#[derive(Default)]
pub(super) struct Observation {
    pub id: Option<String>,
    pub generation: u64,
    pub runtime: Value,
    pub result: Value,
    pub runtime_fresh: bool,
    pub result_fresh: bool,
    pub errors: HashMap<String, String>,
    pub events: VecDeque<Value>,
    pub truncated: bool,
    pub archive: Option<Value>,
    pub detail: Option<super::results::Detail>,
    pub candidate: Option<String>,
    pub selected_attempt: Option<String>,
    pub feedback_for: Option<String>,
    pub feedback_pending: bool,
    pub feedback_error: String,
    pub archive_path: Option<String>,
    pub archive_error: String,
    pub archive_starts: Vec<u64>,
    pub archive_pending_start: Option<u64>,
    bytes: usize,
    cursors: HashSet<u64>,
}
impl Observation {
    pub fn disconnected(&mut self) {
        self.runtime_fresh = false;
        self.result_fresh = false;
    }
    pub fn accept(&mut self, generation: u64, message: Value) {
        if generation != self.generation {
            return;
        }
        let Some((kind, id)) = message["topic"].as_str().and_then(|s| s.split_once(':')) else {
            return;
        };
        if self.id.as_deref() != Some(id) {
            return;
        }
        if message["kind"] == "error" {
            self.errors.insert(kind.into(), string(&message, "error"));
            match kind {
                "run" => self.runtime_fresh = false,
                "result" => self.result_fresh = false,
                _ => (),
            }
        } else if message["kind"] == "state" {
            self.errors.remove(kind);
            match kind {
                "run" => {
                    self.runtime = message["data"].clone();
                    self.runtime_fresh = true;
                }
                "result" => {
                    self.result = message["data"].clone();
                    self.result_fresh = true;
                }
                _ => (),
            }
        } else if message["kind"] == "events" && kind == "events" {
            self.errors.remove(kind);
            for event in records(&message, "events") {
                let Some(cursor) = event["cursor"].as_u64() else {
                    continue;
                };
                if !self.cursors.insert(cursor) {
                    continue;
                }
                self.bytes += event.to_string().len();
                self.events.push_back(event);
                while self.events.len() > 2000 || self.bytes > 8 * 1024 * 1024 {
                    if let Some(old) = self.events.pop_front() {
                        self.bytes = self.bytes.saturating_sub(old.to_string().len());
                        if let Some(cursor) = old["cursor"].as_u64() {
                            self.cursors.remove(&cursor);
                        }
                        self.truncated = true;
                    }
                }
            }
        }
    }
}
impl Workbench {
    pub(super) fn observe_run(&mut self, id: Option<String>) {
        if self.observation.id == id {
            return;
        }
        let generation = self
            .client
            .as_ref()
            .map(|c| c.watch_run(id.clone()))
            .unwrap_or(0);
        self.observation = Observation {
            id,
            generation,
            ..Default::default()
        };
        if self.navigation.restore {
            let saved = self.navigation.saved();
            if saved.run == self.observation.id {
                self.observation.selected_attempt = saved.attempt.clone();
                self.observation.candidate = saved.candidate.clone();
            }
            self.navigation.restore = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_observation_rejects_stale_frames_and_bounds_replayed_history() {
        let mut state = Observation {
            id: Some("new".into()),
            generation: 7,
            ..Default::default()
        };
        state.accept(
            6,
            json!({"topic":"run:new","kind":"state","data":{"wrong":true}}),
        );
        state.accept(
            7,
            json!({"topic":"run:old","kind":"state","data":{"wrong":true}}),
        );
        assert!(!state.runtime_fresh);
        state.accept(
            7,
            json!({"topic":"run:new","kind":"state","data":{"run":{"id":"new"}}}),
        );
        state.accept(
            7,
            json!({"topic":"result:new","kind":"error","error":"Optional result unavailable"}),
        );
        assert!(state.runtime_fresh);
        assert!(!state.result_fresh);
        let events: Vec<_> = (1..=2200)
            .map(|cursor| json!({"cursor":cursor,"event":{"type":"recorded"}}))
            .collect();
        state.accept(
            7,
            json!({"topic":"events:new","kind":"events","events":events}),
        );
        assert_eq!(state.events.len(), 2000);
        assert!(state.truncated);
        assert_eq!(state.events.front().unwrap()["cursor"], 201);
        state.accept(7,json!({"topic":"events:new","kind":"events","events":[{"cursor":2200,"event":{"type":"recorded"}}]}));
        assert_eq!(state.events.len(), 2000);
        state.disconnected();
        assert!(!state.runtime_fresh);
        assert_eq!(state.runtime["run"]["id"], "new");
        assert_eq!(state.events.len(), 2000);
        state.accept(7,json!({"topic":"events:new","kind":"events","events":[{"cursor":2201,"event":{"body":"x".repeat(8*1024*1024)}}]}));
        assert!(state.bytes <= 8 * 1024 * 1024);
        assert!(state.events.is_empty());
    }
}
