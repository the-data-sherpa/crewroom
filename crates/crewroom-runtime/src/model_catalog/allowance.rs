//! FOR-74: remaining subscription allowance per harness login. Visibility
//! only: nothing here limits, pauses or admits work.
//!
//! Each read starts the harness with the login Crewroom launches it with and
//! exchanges only control frames: Codex's app-server `account/rateLimits/read`
//! and Claude Code's experimental stream-json `get_usage` control request.
//! No prompt or turn is ever sent, so no model inference can run. Responses
//! are reduced to window shares, reset times and the plan name; account
//! identifiers, credit balances and credentials never leave this module.
//! A source that cannot be read, or that the installed version does not
//! support, is `unavailable` with its reason, never zero.
use super::{
    billing::{self, BillingRoute},
    discovery::{self, Process, bounded, private_dir},
    *,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

/// Harnesses with a subscription allowance source, in display order.
pub const HARNESSES: [Harness; 2] = [Harness::Codex, Harness::ClaudeCode];
pub const NOTE: &str = "Remaining plan allowance as each harness reports it. Visibility only: Crewroom does not limit or pause work by it. Reading it starts no model.";
const READ_LIMIT: Duration = Duration::from_secs(30);
const CODEX_SOURCE: &str = "Codex app-server account/rateLimits/read";
const CLAUDE_SOURCE: &str = "Claude Code stream-json get_usage (experimental)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Every reported window has its share and reset time.
    Available,
    /// Some windows or values are missing; those carry a reason.
    Partial,
    /// Nothing could be read; `reason` says why.
    Unavailable,
}

/// One rate-limit window. A missing value stays `None` with a reason.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub id: String,
    pub label: String,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    /// Unix seconds.
    pub resets_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub harness: Harness,
    pub label: &'static str,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub source: Option<&'static str>,
    /// Unix seconds when this reading was taken; `None` before any read.
    pub observed_at: Option<u64>,
    /// Always false: a read sends no prompt.
    pub inference_requested: bool,
}
impl Reading {
    pub fn unavailable(
        harness: &Harness,
        reason: impl Into<String>,
        observed_at: Option<u64>,
    ) -> Self {
        Self {
            harness: harness.clone(),
            label: harness.label(),
            state: State::Unavailable,
            reason: Some(reason.into()),
            plan: None,
            windows: Vec::new(),
            notes: Vec::new(),
            source: None,
            observed_at,
            inference_requested: false,
        }
    }
    fn finish(
        harness: &Harness,
        source: &'static str,
        plan: Option<String>,
        windows: Vec<Window>,
        notes: Vec<String>,
        observed_at: u64,
        empty: &str,
    ) -> Self {
        if windows.iter().all(|w| w.remaining_percent.is_none()) {
            let mut reading = Self::unavailable(harness, empty, Some(observed_at));
            reading.source = Some(source);
            reading.plan = plan;
            reading.windows = windows;
            reading.notes = notes;
            return reading;
        }
        let complete = windows
            .iter()
            .all(|w| w.remaining_percent.is_some() && w.resets_at.is_some());
        Self {
            harness: harness.clone(),
            label: harness.label(),
            state: if complete {
                State::Available
            } else {
                State::Partial
            },
            reason: (!complete)
                .then(|| "Some windows were not reported in full; see each window.".into()),
            plan,
            windows,
            notes,
            source: Some(source),
            observed_at: Some(observed_at),
            inference_requested: false,
        }
    }
}

fn label_text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 80 && !s.chars().any(char::is_control))
        .map(String::from)
}

/// A window from a used share (0-100) and a reset time. Out-of-range or
/// missing values are unavailable with a reason, never zero.
fn window(
    id: String,
    label: String,
    used: &Value,
    resets_at: Option<i64>,
    missing: &str,
) -> Window {
    let used = used.as_f64().filter(|u| u.is_finite() && *u >= 0.0);
    let reason = match (used, resets_at) {
        (None, _) => Some(missing.to_owned()),
        (Some(_), None) => Some("The reset time was not reported.".to_owned()),
        _ => None,
    };
    Window {
        id,
        label,
        remaining_percent: used.map(|u| (100.0 - u).clamp(0.0, 100.0)),
        used_percent: used,
        resets_at,
        reason,
    }
}

fn duration_label(minutes: &Value) -> String {
    match minutes.as_u64() {
        Some(10080) => "Weekly".into(),
        Some(m) if m > 0 && m % 1440 == 0 => format!("{}-day", m / 1440),
        Some(m) if m > 0 && m % 60 == 0 => format!("{}-hour", m / 60),
        Some(m) if m > 0 => format!("{m}-minute"),
        _ => "Window of unreported length".into(),
    }
}

/// Codex `GetAccountRateLimitsResponse` → reading. Uses the multi-bucket
/// view when present, else the single-bucket view.
pub fn codex_reading(result: &Value, observed_at: u64) -> Reading {
    let mut snapshots: Vec<(String, &Value)> = match result["rateLimitsByLimitId"].as_object() {
        Some(map) if !map.is_empty() => map.iter().map(|(k, v)| (k.clone(), v)).collect(),
        _ => vec![(
            result["rateLimits"]["limitId"]
                .as_str()
                .unwrap_or("codex")
                .to_owned(),
            &result["rateLimits"],
        )],
    };
    snapshots.sort_by_key(|(id, _)| (id != "codex", id.clone()));
    let mut windows = Vec::new();
    let mut notes = Vec::new();
    for (id, snapshot) in &snapshots {
        if !snapshot.is_object() {
            continue;
        }
        let prefix = if id == "codex" {
            String::new()
        } else {
            format!(
                "{} · ",
                label_text(&snapshot["limitName"])
                    .or_else(|| label_text(&json!(id)))
                    .unwrap_or_else(|| "Other limit".into())
            )
        };
        for slot in ["primary", "secondary"] {
            let w = &snapshot[slot];
            if !w.is_object() {
                continue;
            }
            windows.push(window(
                format!("{id}:{slot}"),
                format!("{prefix}{}", duration_label(&w["windowDurationMins"])),
                &w["usedPercent"],
                w["resetsAt"].as_i64().filter(|t| *t > 0),
                "Codex did not report the used share of this window.",
            ));
        }
        if let Some(reached) = label_text(&snapshot["rateLimitReachedType"]) {
            notes.push(format!("Codex reports {}.", reached.replace('_', " ")));
        }
    }
    if result["ordinaryUsageAllowed"] == false {
        notes.push("Codex reports that ordinary included usage is not allowed right now.".into());
    }
    let plan = label_text(&result["rateLimits"]["planType"]).or_else(|| {
        snapshots
            .iter()
            .find_map(|(_, s)| label_text(&s["planType"]))
    });
    Reading::finish(
        &Harness::Codex,
        CODEX_SOURCE,
        plan,
        windows,
        notes,
        observed_at,
        "Codex reported no rate-limit windows for this login.",
    )
}

fn iso_seconds(value: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|t| t.timestamp())
}

/// Claude Code `get_usage` response → reading. The 5-hour and weekly
/// windows are expected for every subscription; model-specific weekly
/// windows appear when the account has them.
pub fn claude_reading(response: &Value, observed_at: u64) -> Reading {
    let harness = Harness::ClaudeCode;
    let plan = label_text(&response["subscription_type"]);
    if response["rate_limits_available"] == false {
        return Reading::unavailable(
            &harness,
            "Claude Code reports that plan rate limits do not apply to this login (API key or third-party provider).",
            Some(observed_at),
        );
    }
    let limits = &response["rate_limits"];
    if !limits.is_object() {
        let mut reading = Reading::unavailable(
            &harness,
            "Claude Code could not read plan rate limits from claude.ai for this login.",
            Some(observed_at),
        );
        reading.plan = plan;
        reading.source = Some(CLAUDE_SOURCE);
        return reading;
    }
    let mut windows = Vec::new();
    for (key, label, expected) in [
        ("five_hour", "5-hour", true),
        ("seven_day", "Weekly", true),
        ("seven_day_opus", "Weekly · Opus", false),
        ("seven_day_sonnet", "Weekly · Sonnet", false),
        ("seven_day_oauth_apps", "Weekly · OAuth apps", false),
    ] {
        let w = &limits[key];
        if w.is_object() {
            windows.push(window(
                key.into(),
                label.into(),
                &w["utilization"],
                iso_seconds(&w["resets_at"]),
                "Claude Code reported no utilization for this window.",
            ));
        } else if expected {
            windows.push(Window {
                id: key.into(),
                label: label.into(),
                used_percent: None,
                remaining_percent: None,
                resets_at: None,
                reason: Some("Claude Code did not report this window.".into()),
            });
        }
    }
    for scoped in limits["model_scoped"]
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
    {
        let Some(name) = label_text(&scoped["display_name"]) else {
            continue;
        };
        windows.push(window(
            format!("model:{name}"),
            format!("Weekly · {name}"),
            &scoped["utilization"],
            iso_seconds(&scoped["resets_at"]),
            "Claude Code reported no utilization for this window.",
        ));
    }
    Reading::finish(
        &harness,
        CLAUDE_SOURCE,
        plan,
        windows,
        Vec::new(),
        observed_at,
        "Claude Code reported no plan rate-limit windows for this login.",
    )
}

/// What one read needs: the harness executable and the login sources the
/// runner launches it with. Private: never serialized.
#[derive(Clone)]
pub struct Target {
    pub config: discovery::Config,
    pub sources: billing::Sources,
}

/// A read only for a login that reaches the subscription route.
fn subscription_gate(target: &Target) -> std::result::Result<(), String> {
    let harness = &target.config.harness;
    let detection = target.sources.detect(harness, harness.as_str());
    if detection.routes.contains(&BillingRoute::Subscription) {
        return Ok(());
    }
    Err(if detection.routes.contains(&BillingRoute::Api) {
        format!(
            "{} uses API billing, which has no plan allowance. {}",
            harness.label(),
            detection.basis
        )
    } else {
        format!(
            "{} has no subscription login Crewroom can read. {}",
            harness.label(),
            detection.basis
        )
    })
}

/// One bounded read. Never fails: every failure is an unavailable reading.
pub async fn read(
    target: &Target,
    root: &Path,
    observed_at: u64,
    cancel: &CancellationToken,
    limit: Duration,
) -> Reading {
    let harness = target.config.harness.clone();
    if let Err(reason) = subscription_gate(target) {
        return Reading::unavailable(&harness, reason, Some(observed_at));
    }
    if let Err(reason) = prepare(root).await {
        return Reading::unavailable(&harness, reason, Some(observed_at));
    }
    let result = match harness {
        Harness::Codex => codex(target, root, cancel, limit)
            .await
            .map(|v| codex_reading(&v, observed_at)),
        Harness::ClaudeCode => claude(target, root, cancel, limit)
            .await
            .map(|v| claude_reading(&v, observed_at)),
        Harness::Omp => Err("OMP reports no subscription allowance.".into()),
    };
    // Credentials and process state never outlive the read.
    let _ = std::fs::remove_dir_all(root.join("home"));
    result.unwrap_or_else(|reason| Reading::unavailable(&harness, reason, Some(observed_at)))
}

/// A process left by an interrupted read is stopped before a new one starts.
async fn prepare(root: &Path) -> std::result::Result<(), String> {
    let owned = root.join("allowance");
    if owned.exists() && crate::stop_owned(&owned).await.state != "stopped" {
        return Err("A previous allowance read process needs inspection.".into());
    }
    if root.exists() {
        std::fs::remove_dir_all(root)
            .map_err(|_| "Allowance read state could not be reset.".to_owned())?;
    }
    private_dir(root).map_err(|_| "Allowance read state could not be created.".to_owned())
}

async fn exchange<T>(
    config: &discovery::Config,
    root: &Path,
    args: Vec<String>,
    env: &BTreeMap<String, String>,
    cancel: &CancellationToken,
    limit: Duration,
    f: impl for<'a> FnOnce(
        &'a mut Process,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T>> + Send + 'a>,
    >,
) -> std::result::Result<T, String> {
    let mut process = Process::start(config, root, "allowance", args, env)
        .await
        .map_err(|_| {
            format!(
                "{} could not be started to read its allowance.",
                config.harness.label()
            )
        })?;
    let result = bounded(&mut process, cancel, limit, f).await;
    let closed = process.close().await;
    let value = result.map_err(|e| match e {
        crewroom_core::Error::Conflict(message) => message,
        _ => format!("{} allowance read failed.", config.harness.label()),
    })?;
    closed
        .map_err(|_| "The allowance read process could not be verified as stopped.".to_owned())?;
    Ok(value)
}

/// Codex app-server in a private home whose only state is a link to the
/// launch login, under the same lock as Crewroom's token refresh.
async fn codex(
    target: &Target,
    root: &Path,
    cancel: &CancellationToken,
    limit: Duration,
) -> std::result::Result<Value, String> {
    let source: PathBuf = target
        .sources
        .codex_auth
        .clone()
        .ok_or("No Codex login file is configured.")?;
    let lock_path = crate::runner::codex_login_lock_path(&source);
    let _lease = tokio::select! {
        _ = cancel.cancelled() => return Err("Allowance read was cancelled.".into()),
        lease = crate::runner::codex_login_lock(&lock_path) => lease.map_err(|e| match e {
            crewroom_core::Error::Conflict(message) => message,
            _ => "The Codex login lock is unavailable.".into(),
        })?,
    };
    let home = root.join("home");
    private_dir(&home).map_err(|_| "Allowance read state could not be created.".to_owned())?;
    std::os::unix::fs::symlink(&source, home.join("auth.json"))
        .map_err(|_| "The Codex login could not be linked for reading.".to_owned())?;
    crate::durable_new(
        &home.join("config.toml"),
        b"cli_auth_credentials_store = \"file\"\nproject_doc_max_bytes = 0\n",
    )
    .map_err(|_| "Allowance read state could not be created.".to_owned())?;
    let env = crate::runner::codex_environment(&home, &home);
    let args = ["app-server", "--listen", "stdio://", "--strict-config"]
        .map(String::from)
        .to_vec();
    let result = exchange(&target.config, root, args, &env, cancel, limit, |p| {
        Box::pin(async move {
            discovery::request(p, 1, "initialize", json!({"clientInfo":{"name":"crewroom_allowance","title":"Crewroom allowance","version":"0.1.0"}})).await?;
            p.send(&json!({"method":"initialized"})).await?;
            p.send(&json!({"id":2,"method":"account/rateLimits/read","params":{"excludeResetCreditDetails":true}})).await?;
            let frame = p.receive(|v| v["id"] == 2).await?;
            if let Some(error) = frame.get("error") {
                let code = error["code"].as_i64().unwrap_or(0);
                let message = error["message"].as_str().unwrap_or("");
                return Err(conflict(&if code == -32601 || message.contains("unknown variant") {
                    "The installed Codex does not support account/rateLimits/read.".to_owned()
                } else {
                    format!("Codex could not read rate limits from its account usage service (error {code}).")
                }));
            }
            if !frame["result"].is_object() {
                return Err(conflict("Codex returned no rate-limit result."));
            }
            Ok(frame["result"].clone())
        })
    })
    .await;
    // The link, never a copy, is removed while the lock is still held.
    let _ = std::fs::remove_file(home.join("auth.json"));
    result
}

/// Claude Code with its native home, every setting source, tool and MCP
/// server disabled, and session persistence off. Only `initialize` and
/// `get_usage` control requests are sent.
async fn claude(
    target: &Target,
    root: &Path,
    cancel: &CancellationToken,
    limit: Duration,
) -> std::result::Result<Value, String> {
    let home = target
        .sources
        .claude_home
        .clone()
        .ok_or("No Claude Code login home is configured.")?;
    let env = crate::runner::claude_environment(&home);
    let args = [
        "--safe-mode",
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-chrome",
        "--permission-mode",
        "dontAsk",
    ]
    .map(String::from)
    .to_vec();
    exchange(&target.config, root, args, &env, cancel, limit, |p| {
        Box::pin(async move {
            p.send(&json!({"type":"control_request","request_id":"crewroom-allowance-init","request":{"subtype":"initialize","hooks":null}})).await?;
            let init = p.receive(|v| v["type"] == "control_response" && v["response"]["request_id"] == "crewroom-allowance-init").await?;
            if init["response"]["subtype"] != "success" {
                return Err(conflict("Claude Code rejected the control initialization."));
            }
            p.send(&json!({"type":"control_request","request_id":"crewroom-allowance","request":{"subtype":"get_usage","skip_behaviors":true}})).await?;
            let frame = p.receive(|v| v["type"] == "control_response" && v["response"]["request_id"] == "crewroom-allowance").await?;
            if frame["response"]["subtype"] != "success" || !frame["response"]["response"].is_object() {
                return Err(conflict("The installed Claude Code does not answer the experimental get_usage request."));
            }
            Ok(frame["response"]["response"].clone())
        })
    })
    .await
}

struct Inner {
    root: PathBuf,
    state: Mutex<CacheState>,
    cancel: CancellationToken,
    changed: Arc<dyn Fn() + Send + Sync>,
}
#[derive(Default)]
struct CacheState {
    readings: BTreeMap<&'static str, Reading>,
    worker: Option<tokio::task::JoinHandle<()>>,
    closing: bool,
}

/// The service's latest readings. In memory only: after a restart each
/// harness reads as not yet read until the next refresh.
#[derive(Clone)]
pub struct Allowance(Arc<Inner>);
impl Allowance {
    pub fn open(root: &Path, changed: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        private_dir(root)?;
        Ok(Self(Arc::new(Inner {
            root: root.into(),
            state: Mutex::default(),
            cancel: CancellationToken::new(),
            changed: Arc::new(changed),
        })))
    }
    pub fn view(&self) -> Value {
        let state = self.0.state.lock().unwrap();
        let refreshing = state.worker.as_ref().is_some_and(|w| !w.is_finished());
        let harnesses: Vec<_> = HARNESSES
            .iter()
            .map(|h| {
                state.readings.get(h.as_str()).cloned().unwrap_or_else(|| {
                    Reading::unavailable(h, "Not read yet. Refresh allowance to read it.", None)
                })
            })
            .collect();
        json!({"state": if refreshing {"refreshing"} else {"idle"}, "note": NOTE, "harnesses": harnesses})
    }
    /// Starts one background read per harness; a refresh already running is
    /// joined. A harness without a target reads as unavailable with `missing`.
    pub fn refresh(&self, targets: Vec<Target>, missing: &str) -> Result<Value> {
        self.refresh_with(targets, missing, READ_LIMIT)
    }
    pub fn refresh_with(
        &self,
        targets: Vec<Target>,
        missing: &str,
        limit: Duration,
    ) -> Result<Value> {
        {
            let mut state = self.0.state.lock().unwrap();
            if state.closing {
                return Err(conflict("Allowance reading is stopping."));
            }
            if !state.worker.as_ref().is_some_and(|w| !w.is_finished()) {
                let this = self.clone();
                let missing = missing.to_owned();
                state.worker = Some(tokio::spawn(async move {
                    this.run(targets, missing, limit).await
                }));
            }
        }
        (self.0.changed)();
        Ok(self.view())
    }
    async fn run(&self, targets: Vec<Target>, missing: String, limit: Duration) {
        let reads = HARNESSES.iter().map(|harness| {
            let target = targets
                .iter()
                .find(|t| &t.config.harness == harness)
                .cloned();
            let root = self.0.root.join(harness.as_str());
            let cancel = self.0.cancel.clone();
            let missing = missing.clone();
            async move {
                let now = store::now();
                match target {
                    Some(target) => read(&target, &root, now, &cancel, limit).await,
                    None => Reading::unavailable(
                        harness,
                        missing.replace("{harness}", harness.label()),
                        Some(now),
                    ),
                }
            }
        });
        let readings = futures_util::future::join_all(reads).await;
        let mut state = self.0.state.lock().unwrap();
        for reading in readings {
            state.readings.insert(reading.harness.as_str(), reading);
        }
        drop(state);
        (self.0.changed)();
    }
    /// Waits for the current refresh, if any. Used by tests and shutdown.
    pub async fn settle(&self) {
        let worker = self.0.state.lock().unwrap().worker.take();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
    }
    pub async fn close(&self) {
        {
            let mut state = self.0.state.lock().unwrap();
            state.closing = true;
        }
        self.0.cancel.cancel();
        self.settle().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_windows_report_remaining_share_and_reset() {
        let result = json!({"ordinaryUsageAllowed":true,"accountId":"account-canary",
            "rateLimits":{"limitId":"codex","planType":"pro","primary":{"usedPercent":8,"windowDurationMins":10080,"resetsAt":1792876956},"secondary":null,
                "credits":{"hasCredits":true,"unlimited":false,"balance":"balance-canary"}},
            "rateLimitsByLimitId":{"codex":{"limitId":"codex","planType":"pro",
                "primary":{"usedPercent":36.5,"windowDurationMins":300,"resetsAt":1792800000},
                "secondary":{"usedPercent":8,"windowDurationMins":10080,"resetsAt":1792876956}}}});
        let reading = codex_reading(&result, 100);
        assert_eq!(reading.state, State::Available);
        assert_eq!(reading.plan.as_deref(), Some("pro"));
        assert_eq!(reading.observed_at, Some(100));
        let labels: Vec<_> = reading.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5-hour", "Weekly"]);
        assert_eq!(reading.windows[0].remaining_percent, Some(63.5));
        assert_eq!(reading.windows[1].resets_at, Some(1792876956));
        let text = serde_json::to_string(&reading).unwrap();
        assert!(!text.contains("canary"));
        assert!(!reading.inference_requested);
    }

    #[test]
    fn codex_partial_and_empty_sources_are_never_zero() {
        let partial = codex_reading(
            &json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":null},
                "secondary":{"usedPercent":null,"windowDurationMins":10080,"resetsAt":5}}}),
            7,
        );
        assert_eq!(partial.state, State::Partial);
        assert_eq!(partial.windows[0].remaining_percent, Some(80.0));
        assert!(partial.windows[0].reason.is_some());
        assert_eq!(partial.windows[1].remaining_percent, None);
        assert_eq!(partial.windows[1].used_percent, None);
        let empty = codex_reading(
            &json!({"rateLimits":{"limitId":"codex","primary":null,"secondary":null},"ordinaryUsageAllowed":false}),
            7,
        );
        assert_eq!(empty.state, State::Unavailable);
        assert!(empty.reason.unwrap().contains("no rate-limit windows"));
        assert!(empty.notes.iter().any(|n| n.contains("not allowed")));
        // Over 100 % used is no allowance left, not a negative share.
        let over = codex_reading(
            &json!({"rateLimits":{"primary":{"usedPercent":130,"windowDurationMins":60,"resetsAt":9}}}),
            7,
        );
        assert_eq!(over.windows[0].remaining_percent, Some(0.0));
        assert_eq!(over.windows[0].label, "1-hour");
    }

    #[test]
    fn claude_windows_cover_available_partial_and_unavailable() {
        let full = claude_reading(
            &json!({"subscription_type":"max","rate_limits_available":true,"session":{"total_cost_usd":0},
                "rate_limits":{"five_hour":{"utilization":36,"resets_at":"2026-10-19T09:19:59.540647+00:00"},
                    "seven_day":{"utilization":41,"resets_at":"2026-10-23T20:59:59+00:00"},"seven_day_opus":null,
                    "model_scoped":[{"display_name":"Fable","utilization":0,"resets_at":"2026-10-23T21:00:00+00:00"}],
                    "spend":{"used":{"amount_minor":1234}}}}),
            50,
        );
        assert_eq!(full.state, State::Available);
        assert_eq!(full.plan.as_deref(), Some("max"));
        let labels: Vec<_> = full.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5-hour", "Weekly", "Weekly · Fable"]);
        assert_eq!(full.windows[0].remaining_percent, Some(64.0));
        assert_eq!(full.windows[0].resets_at, Some(1792401599));
        assert_eq!(full.windows[2].remaining_percent, Some(100.0));
        assert!(!serde_json::to_string(&full).unwrap().contains("1234"));

        let partial = claude_reading(
            &json!({"rate_limits_available":true,"rate_limits":{"five_hour":null,"seven_day":{"utilization":41,"resets_at":null}}}),
            50,
        );
        assert_eq!(partial.state, State::Partial);
        assert_eq!(partial.windows[0].remaining_percent, None);
        assert_eq!(
            partial.windows[0].reason.as_deref(),
            Some("Claude Code did not report this window.")
        );
        assert_eq!(partial.windows[1].remaining_percent, Some(59.0));

        for (response, reason) in [
            (
                json!({"rate_limits_available":false,"rate_limits":null}),
                "do not apply",
            ),
            (
                json!({"rate_limits_available":true,"rate_limits":null}),
                "could not read",
            ),
            (
                json!({"rate_limits_available":true,"rate_limits":{"five_hour":{"utilization":null,"resets_at":null}}}),
                "no plan rate-limit windows",
            ),
        ] {
            let reading = claude_reading(&response, 50);
            assert_eq!(reading.state, State::Unavailable, "{response}");
            assert!(
                reading.reason.as_deref().unwrap().contains(reason),
                "{response}"
            );
            assert!(
                reading
                    .windows
                    .iter()
                    .all(|w| w.remaining_percent.is_none())
            );
        }
    }

    #[test]
    fn api_and_unknown_logins_are_unavailable_without_a_read() {
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth.json");
        crewroom_core::authority::write_private(
            &auth,
            br#"{"auth_mode":"apikey","OPENAI_API_KEY":"key-canary"}"#,
        )
        .unwrap();
        let target = |harness: Harness| Target {
            config: discovery::Config {
                harness,
                binary: "/nonexistent/harness".into(),
                helper: "/nonexistent/helper".into(),
                path: "/usr/bin:/bin".into(),
                omp_models: json!({}),
            },
            sources: billing::Sources {
                codex_auth: Some(auth.clone()),
                claude_home: Some(root.path().join("empty-home")),
                omp_providers: json!({}),
            },
        };
        let api = subscription_gate(&target(Harness::Codex)).unwrap_err();
        assert!(api.contains("API billing") && !api.contains("key-canary"));
        let unknown = subscription_gate(&target(Harness::ClaudeCode)).unwrap_err();
        assert!(unknown.contains("no subscription login"));
    }
}
