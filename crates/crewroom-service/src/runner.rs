//! Settings → Runner (FOR-83): what the agent runner needs, what is present,
//! and a private runner configuration the operator edits from the app.
//!
//! The configuration comes from one source. `CREWROOM_RUNTIME_CONFIG` wins for
//! headless use and is read-only here; otherwise `<data>/runtime.json`, which
//! this module writes owner-only (0600). Secrets (the Rust Mail token and OMP
//! provider keys) never enter that file or a response: the file holds only a
//! `{"$crewroomSecret": id}` reference and the value is encrypted in the
//! install credential store. Configured access is never reported as proven
//! inference.
use crate::{ApiError, AppState, array, operation};
use crewroom_core::{Error, Result, authority::write_private};
use crewroom_runtime::{
    mail::{MailClient, MailError},
    providers::secrets::{SecretStore, StoredToken},
    supervisor::workflow::configuration::RuntimeConfig,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::watch;

pub const SETTINGS_FILE: &str = "runtime.json";
pub const ENVIRONMENT: &str = "CREWROOM_RUNTIME_CONFIG";
const MARKER: &str = "$crewroomSecret";
const MAIL_SECRET: &str = "runner-mail";
/// Top-level keys the Runner form owns; Advanced holds every other key.
pub const FORM_KEYS: [&str; 8] = [
    "root",
    "socket",
    "mail",
    "commandEngine",
    "evaluationImage",
    "ompBinary",
    "codex",
    "claudeCode",
];
const PROBE: Duration = Duration::from_secs(2);

/// Where this process's runner configuration came from, and why it is not
/// running when it failed to load.
#[derive(Clone, Debug)]
pub struct RunnerLoad {
    /// `environment`, `settings`, `none`, or `embedded` (supplied directly by
    /// the program that started the service).
    pub source: &'static str,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}
impl Default for RunnerLoad {
    fn default() -> Self {
        Self {
            source: "embedded",
            path: None,
            error: None,
        }
    }
}

/// Runner facts captured when the service started.
pub(crate) struct RunnerState {
    pub data: PathBuf,
    pub load: RunnerLoad,
    /// The resolved configuration the running runner was built from.
    pub applied: Option<Value>,
    pub tmux: Option<PathBuf>,
    pub helpers: Vec<PathBuf>,
    pub reload: watch::Sender<u64>,
    /// True once the owning process restarts the service on request.
    pub supervised: AtomicBool,
}
impl RunnerState {
    pub fn new(data: &Path, options: Option<&crate::ServiceOptions>) -> Self {
        Self {
            data: data.to_owned(),
            load: options.map(|o| o.runner.clone()).unwrap_or(RunnerLoad {
                source: "none",
                path: None,
                error: None,
            }),
            applied: options.and_then(|o| o.runtime.clone()),
            tmux: options.map(|o| o.tmux.clone()),
            helpers: options
                .map(|o| vec![o.helper.clone(), o.host.clone()])
                .unwrap_or_default(),
            reload: watch::channel(0).0,
            supervised: AtomicBool::new(false),
        }
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
fn conflict(message: impl Into<String>) -> Error {
    Error::Conflict(message.into())
}

/// The active source: the environment variable, else the settings file.
pub fn locate(data: &Path) -> (&'static str, Option<PathBuf>) {
    if let Some(path) = std::env::var_os(ENVIRONMENT).filter(|v| !v.is_empty()) {
        return ("environment", Some(PathBuf::from(path)));
    }
    let path = data.join(SETTINGS_FILE);
    if std::fs::symlink_metadata(&path).is_ok() {
        ("settings", Some(path))
    } else {
        ("none", None)
    }
}

/// Reads a private regular JSON file owned by this user (mode 0600 or tighter).
pub fn read_private(path: &Path) -> std::result::Result<Value, String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        format!(
            "Runtime configuration {} is unreadable: {e}",
            path.display()
        )
    })?;
    // SAFETY: geteuid has no preconditions.
    if !(meta.is_file()
        && !meta.file_type().is_symlink()
        && meta.uid() == unsafe { libc::geteuid() }
        && meta.mode() & 0o077 == 0)
    {
        return Err("Runtime configuration must be a private file owned by this user.".into());
    }
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "Runtime configuration {} is unreadable: {e}",
            path.display()
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Runtime configuration is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("Runtime configuration must be a JSON object.".into());
    }
    Ok(value)
}

/// Every place a runner secret may sit: the Mail token and OMP provider keys.
fn secret_slots(value: &Value) -> Vec<(Vec<String>, String)> {
    let mut slots = vec![(
        vec!["mail".to_owned(), "token".to_owned()],
        MAIL_SECRET.to_owned(),
    )];
    if let Some(providers) = value["models"]["providers"].as_object() {
        for name in providers.keys() {
            slots.push((
                vec![
                    "models".into(),
                    "providers".into(),
                    name.clone(),
                    "apiKey".into(),
                ],
                format!("runner-provider-{}", name.to_ascii_lowercase()),
            ));
        }
    }
    slots
}
fn at<'a>(value: &'a Value, path: &[String]) -> &'a Value {
    path.iter().fold(value, |v, key| &v[key.as_str()])
}
fn at_mut<'a>(value: &'a mut Value, path: &[String]) -> Option<&'a mut Value> {
    path.iter()
        .try_fold(value, |v, key| v.as_object_mut()?.get_mut(key.as_str()))
}
fn reference(id: &str) -> Value {
    json!({ MARKER: id })
}
fn referenced(value: &Value) -> Option<&str> {
    value.as_object().filter(|o| o.len() == 1)?[MARKER].as_str()
}

/// Replaces every secret reference with its decrypted value, in memory only.
pub fn resolve(value: &mut Value, secrets: &SecretStore) -> std::result::Result<(), String> {
    for (path, _) in secret_slots(value) {
        let Some(slot) = at_mut(value, &path) else {
            continue;
        };
        let Some(id) = referenced(slot).map(str::to_owned) else {
            continue;
        };
        match secrets.runner_secret(&id) {
            StoredToken::Present(secret) => *slot = json!(secret.as_str()),
            StoredToken::Missing => {
                return Err(format!(
                    "The runner secret for {} is not saved. Enter it in Settings → Runner.",
                    path.join(".")
                ));
            }
            StoredToken::Unreadable => {
                return Err(format!(
                    "The runner secret for {} is unreadable. Re-enter it in Settings → Runner.",
                    path.join(".")
                ));
            }
        }
    }
    Ok(())
}

/// Loads the active runner configuration for a service start. A settings-file
/// problem never prevents the service from starting: the runner stays off and
/// the reason is shown. `strict` keeps the headless contract that a broken
/// `CREWROOM_RUNTIME_CONFIG` fails startup.
pub fn load(data: &Path, strict: bool) -> anyhow::Result<(RunnerLoad, Option<Value>)> {
    let (source, path) = locate(data);
    let Some(path) = path else {
        return Ok((
            RunnerLoad {
                source,
                path: None,
                error: None,
            },
            None,
        ));
    };
    let loaded = read_private(&path).and_then(|mut value| {
        resolve(&mut value, &SecretStore::at(data))?;
        RuntimeConfig::parse(&value).map_err(|e| e.to_string())?;
        Ok(value)
    });
    match loaded {
        Ok(value) => Ok((
            RunnerLoad {
                source,
                path: Some(path),
                error: None,
            },
            Some(value),
        )),
        Err(error) if strict && source == "environment" => Err(anyhow::anyhow!(error)),
        Err(error) => Ok((
            RunnerLoad {
                source,
                path: Some(path),
                error: Some(error),
            },
            None,
        )),
    }
}

/// The same unresolved-ownership rule a pause uses, plus runs that are
/// running or pausing and could start processes at any moment.
pub(crate) fn owns_live(run: &Value, runtime: &Value) -> bool {
    array(&runtime["attempts"])
        .iter()
        .any(|a| a["state"] != "admitted" && a["ownership"] != "stopped")
        || matches!(run["status"].as_str(), Some("running" | "pausing"))
}

/// Runs that own live processes, or are running and could start them. A
/// runner reload pauses such runs, so it is offered only when this is empty.
pub(crate) async fn live_runs(state: &AppState) -> std::result::Result<Vec<Value>, ApiError> {
    operation(state, |s| {
        let mut live = Vec::new();
        for run in array(&s.store.snapshot()?["runs"]) {
            let Some(id) = run["id"].as_str() else {
                continue;
            };
            if owns_live(run, &s.store.runtime(id)?) {
                live.push(json!({"id":id,"title":run["title"],"status":run["status"]}));
            }
        }
        Ok(live)
    })
    .await
}

fn text_of(value: &Value) -> String {
    value.as_str().unwrap_or("").to_owned()
}
/// The form fields shown for a saved configuration.
fn form(raw: &Value) -> Value {
    json!({
        "root": text_of(&raw["root"]),
        "mailUrl": text_of(&raw["mail"]["url"]),
        "mailProjectRoot": text_of(&raw["mail"]["projectRoot"]),
        "dockerSocket": text_of(&raw["commandEngine"]["socket"]),
        "evaluationImage": text_of(&raw["evaluationImage"]),
        "ompBinary": text_of(&raw["ompBinary"]),
        "codexBinary": text_of(&raw["codex"]["binary"]),
        "codexAuthFile": text_of(&raw["codex"]["authFile"]),
        "claudeBinary": text_of(&raw["claudeCode"]["binary"]),
        "claudeHome": text_of(&raw["claudeCode"]["home"]),
    })
}
/// Advanced keys with every secret removed. Provider headers can carry
/// credentials, so their values are hidden too.
fn advanced(raw: &Value) -> Value {
    let mut value = raw.clone();
    if let Some(object) = value.as_object_mut() {
        for key in FORM_KEYS {
            object.remove(key);
        }
    }
    if let Some(providers) = value["models"]["providers"].as_object_mut() {
        for provider in providers.values_mut() {
            if let Some(provider) = provider.as_object_mut() {
                provider.remove("apiKey");
                if let Some(headers) = provider.get_mut("headers").and_then(Value::as_object_mut) {
                    for header in headers.values_mut() {
                        *header = json!("(hidden)");
                    }
                }
            }
        }
    }
    value
}
/// Where each secret comes from: `stored`, `inline` (plain text in an
/// environment-selected file), `unreadable` or `none`. Never the value.
fn secret_sources(raw: &Value, secrets: &SecretStore) -> Value {
    let status = |slot: &Value| -> &'static str {
        match referenced(slot) {
            Some(reference) => match secrets.runner_secret(reference) {
                StoredToken::Present(_) => "stored",
                StoredToken::Unreadable => "unreadable",
                StoredToken::Missing => "none",
            },
            None if slot.as_str().is_some_and(|s| !s.is_empty()) => "inline",
            None => "none",
        }
    };
    let mut providers = json!({});
    for (path, _) in secret_slots(raw).into_iter().skip(1) {
        providers[path[2].as_str()] = json!(status(at(raw, &path)));
    }
    json!({"mail": status(&raw["mail"]["token"]), "providers": providers})
}

fn item(
    id: &str,
    label: &str,
    state: &str,
    detail: impl Into<String>,
    remedy: Option<String>,
    required: bool,
) -> Value {
    json!({"id":id,"label":label,"state":state,"detail":detail.into(),"remedy":remedy,"required":required})
}
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}
fn root_item(config: &Value) -> Value {
    use std::os::unix::fs::MetadataExt;
    let Some(root) = config["root"].as_str().filter(|s| !s.is_empty()) else {
        return item(
            "runtime-root",
            "Runtime root",
            "missing",
            "No runtime folder is configured.",
            Some("Enter an absolute private folder for agent runtime files, such as a runner folder inside the Crewroom data folder, and save.".into()),
            true,
        );
    };
    let path = Path::new(root);
    match std::fs::symlink_metadata(path) {
        // SAFETY: geteuid has no preconditions.
        Ok(m) if m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0 => {
            item(
                "runtime-root",
                "Runtime root",
                "ready",
                format!("{root} · private folder owned by you"),
                None,
                true,
            )
        }
        Ok(_) => item(
            "runtime-root",
            "Runtime root",
            "failing",
            format!("{root} is not a private folder owned by you."),
            Some(format!("Run chmod 700 {root}, or choose another folder.")),
            true,
        ),
        Err(_) if path.parent().is_some_and(Path::exists) => item(
            "runtime-root",
            "Runtime root",
            "ready",
            format!("{root} · created privately (0700) when the runner starts"),
            None,
            true,
        ),
        Err(_) => item(
            "runtime-root",
            "Runtime root",
            "missing",
            format!("{root} and its parent folder do not exist."),
            Some("Create the parent folder, or choose a folder inside an existing one.".into()),
            true,
        ),
    }
}
async fn mail_item(config: &Value, sources: &Value) -> Value {
    let label = "Rust Agent Mail";
    let url = config["mail"]["url"].as_str().unwrap_or("");
    let start = "Start Rust Agent Mail on loopback with a bearer token (mcp-agent-mail serve --host 127.0.0.1 --port PORT, with HTTP_BEARER_TOKEN set), then enter its URL, project folder and token.";
    if url.is_empty() {
        return item(
            "mail",
            label,
            "missing",
            "No Mail endpoint is configured.",
            Some(start.into()),
            true,
        );
    }
    let token = config["mail"]["token"].as_str().unwrap_or("");
    if token.is_empty() {
        let detail = match sources["mail"].as_str() {
            Some("unreadable") => "The saved Mail token is unreadable.",
            _ => "No Mail token is saved.",
        };
        return item(
            "mail",
            label,
            "missing",
            detail,
            Some("Enter the token the Mail server was started with and save.".into()),
            true,
        );
    }
    let client = match MailClient::with_budget(url, token.into(), PROBE) {
        Ok(client) => client,
        Err(_) => {
            return item(
                "mail",
                label,
                "failing",
                format!("{url} is not a loopback HTTP endpoint."),
                Some("Use http://127.0.0.1:PORT/mcp/.".into()),
                true,
            );
        }
    };
    match client.probe().await {
        Ok(()) => item(
            "mail",
            label,
            "ready",
            format!("{url} answered the MCP handshake with the saved token."),
            None,
            true,
        ),
        Err(MailError::Http {
            status: status @ (401 | 403),
            ..
        }) => item(
            "mail",
            label,
            "failing",
            format!("{url} rejected the saved token (HTTP {status})."),
            Some("Enter the token the Mail server was started with.".into()),
            true,
        ),
        Err(error) => item(
            "mail",
            label,
            "failing",
            format!("{url} did not answer: {error}"),
            Some(start.into()),
            true,
        ),
    }
}
fn docker(socket: &str) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .unix_socket(socket)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE)
        .build()
        .ok()
}
async fn docker_items(config: &Value, images: Vec<String>) -> [Value; 2] {
    let socket = config["commandEngine"]["socket"].as_str().unwrap_or("");
    let image_label = "Pinned check images";
    if socket.is_empty() {
        return [
            item("docker", "Docker engine", "missing", "No Docker socket is configured.", Some("Enter the Docker engine socket (for example /var/run/docker.sock, or docker.sock in your user runtime folder for rootless Docker). Repository checks and evaluation run in pinned containers.".into()), true),
            item("check-images", image_label, "missing", "Not checked: no Docker socket is configured.", Some("Configure Docker first.".into()), true),
        ];
    }
    let Some(client) = docker(socket) else {
        return [
            item(
                "docker",
                "Docker engine",
                "failing",
                format!("{socket} cannot be used."),
                Some("Enter the path of the Docker engine socket.".into()),
                true,
            ),
            item(
                "check-images",
                image_label,
                "failing",
                "Not checked: Docker is unavailable.",
                None,
                true,
            ),
        ];
    };
    let ping = client.get("http://localhost/_ping").send().await;
    if !ping.as_ref().is_ok_and(|r| r.status().is_success()) {
        return [
            item(
                "docker",
                "Docker engine",
                "failing",
                format!("The Docker engine at {socket} did not answer."),
                Some("Start Docker, or correct the socket path.".into()),
                true,
            ),
            item(
                "check-images",
                image_label,
                "failing",
                "Not checked: Docker is unavailable.",
                None,
                true,
            ),
        ];
    }
    let docker_ready = item(
        "docker",
        "Docker engine",
        "ready",
        format!("{socket} answered."),
        None,
        true,
    );
    if images.is_empty() {
        return [
            docker_ready,
            item(
                "check-images",
                image_label,
                "ready",
                "No pinned check or evaluation images are configured.",
                None,
                true,
            ),
        ];
    }
    let mut missing = Vec::new();
    for image in &images {
        let present = client
            .get(format!("http://localhost/images/{image}/json"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        if !present {
            missing.push(image.clone());
        }
    }
    let images_item = if missing.is_empty() {
        item(
            "check-images",
            image_label,
            "ready",
            format!("{} pinned image(s) present.", images.len()),
            None,
            true,
        )
    } else {
        item(
            "check-images",
            image_label,
            "missing",
            format!(
                "{} of {} pinned image(s) are not present: {}",
                missing.len(),
                images.len(),
                missing.join(", ")
            ),
            Some(format!(
                "Pull them, for example: docker pull {}",
                missing[0]
            )),
            true,
        )
    };
    [docker_ready, images_item]
}
fn harness_items(config: Option<&Value>, catalog: &Value) -> Vec<Value> {
    let mut items = Vec::new();
    for (key, label, field) in [
        ("omp", "OMP harness", "OMP executable"),
        ("codex", "Codex harness", "Codex executable and auth file"),
        (
            "claude-code",
            "Claude Code harness",
            "Claude Code executable and home",
        ),
    ] {
        let entry = array(&catalog["harnesses"])
            .iter()
            .find(|h| h["harness"] == key)
            .cloned()
            .unwrap_or(Value::Null);
        let version = entry["identity"]["version"].as_str();
        let configured = config.is_some_and(|c| match key {
            "omp" => c["ompBinary"].as_str().is_some_and(|s| !s.is_empty()),
            "codex" => c["codex"]["binary"].is_string() && c["codex"]["authFile"].is_string(),
            _ => c["claudeCode"]["binary"].is_string() && c["claudeCode"]["home"].is_string(),
        });
        let id = format!("harness-{key}");
        let found = match (entry["outcome"].as_str(), version) {
            (Some("success"), Some(v)) => format!("Found {v}"),
            _ if entry["configured"] == true => {
                "Found; not probed yet (refresh models in Settings → Models & budget)".into()
            }
            _ => "Not found on the service PATH".into(),
        };
        if !configured {
            items.push(item(
                &id,
                label,
                "missing",
                format!("{found}; not configured for the runner."),
                Some(format!("Enter the {field} under Harnesses if agents should use it. At least one harness is needed.")),
                false,
            ));
            continue;
        }
        let compatibility = entry["compatibility"]
            .as_str()
            .unwrap_or("not verified for launch");
        let state = match entry["outcome"].as_str() {
            Some("success") => "ready",
            Some("error") => "failing",
            _ => "missing",
        };
        let (detail, remedy) = match state {
            "ready" => (
                format!("{} · {compatibility} · configured; access is not proven inference", version.unwrap_or("unknown version")),
                None,
            ),
            "failing" => (
                entry["error"].as_str().unwrap_or("The last probe failed.").to_owned(),
                Some(entry["remedy"].as_str().unwrap_or("Check the executable path, then refresh models in Settings → Models & budget.").to_owned()),
            ),
            _ => (
                "Configured but not probed yet.".into(),
                Some("Refresh models in Settings → Models & budget to probe it.".into()),
            ),
        };
        items.push(item(&id, label, state, detail, remedy, false));
    }
    items
}
fn provider_item(config: Option<&Value>, sources: &Value) -> Value {
    let label = "OMP provider entries";
    let omp = config.is_some_and(|c| c["ompBinary"].as_str().is_some_and(|s| !s.is_empty()));
    let providers: Vec<String> = config
        .and_then(|c| c["models"]["providers"].as_object())
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    if providers.is_empty() {
        return item(
            "omp-providers",
            label,
            "missing",
            "No OMP provider is defined.",
            Some("Add a provider under Advanced → models.providers (baseUrl and api), then enter its key. OMP lists models only from these providers.".into()),
            omp,
        );
    }
    let broken: Vec<&String> = providers
        .iter()
        .filter(|p| {
            matches!(
                sources["providers"][p.as_str()].as_str(),
                Some("unreadable")
            )
        })
        .collect();
    if !broken.is_empty() {
        return item(
            "omp-providers",
            label,
            "failing",
            format!(
                "Saved key unreadable for: {}",
                broken
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some("Re-enter those provider keys.".into()),
            omp,
        );
    }
    let described: Vec<String> = providers
        .iter()
        .map(|p| match sources["providers"][p.as_str()].as_str() {
            Some("stored") => format!("{p} (key stored encrypted)"),
            Some("inline") => format!("{p} (key in the configuration file)"),
            _ => format!("{p} (no key)"),
        })
        .collect();
    item(
        "omp-providers",
        label,
        "ready",
        format!(
            "{} · configured; access is not proven inference",
            described.join(", ")
        ),
        None,
        omp,
    )
}
fn profiles_item(config: Option<&Value>) -> Value {
    let count = config.map(|c| array(&c["catalog"]).len()).unwrap_or(0);
    if count == 0 {
        item(
            "profiles",
            "Execution profiles",
            "missing",
            "No verified execution profile is configured, so no listed model can run.",
            Some("Add one profile per model under Advanced → catalog, for example {\"adapter\":\"codex\",\"provider\":\"codex\",\"id\":\"MODEL\",\"api\":\"codex-app-server\",\"inputs\":[\"text\"],\"efforts\":[\"low\"],\"tools\":true}.".into()),
            true,
        )
    } else {
        item(
            "profiles",
            "Execution profiles",
            "ready",
            format!("{count} verified profile(s) configured."),
            None,
            true,
        )
    }
}

/// The workspace summary: no probes, no file contents and no secrets.
pub(crate) fn summary(state: &AppState) -> Value {
    let active = state
        .execution
        .as_ref()
        .is_some_and(|e| e.workflow.is_some());
    let (mut source, _) = locate(&state.runner.data);
    if source == "none" && active && state.runner.load.source == "embedded" {
        source = "embedded";
    }
    json!({
        "active": active,
        "source": source,
        "error": (!active).then(|| state.runner.load.error.clone()).flatten(),
    })
}

/// The complete Settings → Runner view. Probes are read-only and bounded.
pub(crate) async fn view(state: &AppState) -> std::result::Result<Value, ApiError> {
    let runner = &state.runner;
    let (mut source, path) = locate(&runner.data);
    let raw = path.as_deref().map(read_private);
    let secrets = SecretStore::at(&runner.data);
    let raw_value = match &raw {
        Some(Ok(v)) => Some(v.clone()),
        _ => None,
    };
    let file_error = match &raw {
        Some(Err(e)) => Some(e.clone()),
        _ => None,
    };
    let mut resolved = raw_value.clone();
    let mut resolve_error = None;
    if let Some(value) = resolved.as_mut()
        && let Err(error) = resolve(value, &secrets)
    {
        resolve_error = Some(error);
    }
    let parse_error = match (&resolved, &resolve_error) {
        (Some(v), None) => RuntimeConfig::parse(v).err().map(|e| e.to_string()),
        _ => None,
    };
    let active = state
        .execution
        .as_ref()
        .is_some_and(|e| e.workflow.is_some());
    if source == "none" && active && runner.load.source == "embedded" {
        source = "embedded";
    }
    let config = if source == "embedded" {
        runner.applied.clone()
    } else {
        resolved.clone()
    };
    let pending = match source {
        "embedded" => false,
        _ => {
            let valid = file_error.is_none() && resolve_error.is_none() && parse_error.is_none();
            (valid && resolved != runner.applied) || (!valid && raw.is_some() && active)
        }
    };
    let shown = if source == "embedded" {
        runner.applied.clone().unwrap_or(json!({}))
    } else {
        raw_value.clone().unwrap_or(json!({}))
    };
    let sources = secret_sources(&shown, &secrets);
    let live = live_runs(state).await?;
    let supervised = runner.supervised.load(Ordering::SeqCst);
    let restart = if !supervised {
        json!({"available":false,"reason":"This service cannot reload itself. Stop and start the service to apply runner changes."})
    } else if !live.is_empty() {
        json!({"available":false,"reason":format!("Applying reloads the runner, which would pause {} run(s) that own live agent processes. Pause or finish them first.", live.len())})
    } else {
        json!({"available":true,"reason":"Applying reloads the runner in place. No run owns live processes."})
    };
    // Prerequisites.
    let images = operation(state, |s| {
        let snapshot = s.store.snapshot()?;
        let mut images = Vec::new();
        for repository in array(&snapshot["repositories"]) {
            if !repository["retiredAt"].is_null() {
                continue;
            }
            for check in array(&repository["checks"]) {
                if let Some(image) = check["image"].as_str() {
                    images.push(image.to_owned());
                }
            }
        }
        Ok(images)
    })
    .await?;
    let mut images = images;
    if let Some(image) = config.as_ref().and_then(|c| c["evaluationImage"].as_str()) {
        images.push(image.to_owned());
    }
    images.sort();
    images.dedup();
    let empty = json!({});
    let probe_config = config.as_ref().unwrap_or(&empty);
    let catalog = crate::model_discovery(state);
    let mut prerequisites = vec![root_item(probe_config)];
    prerequisites.push(match &runner.tmux {
        Some(tmux) if executable(tmux) => item("tmux", "tmux", "ready", tmux.display().to_string(), None, true),
        Some(tmux) => item("tmux", "tmux", "missing", format!("{} is not an executable.", tmux.display()), Some("Install tmux with your package manager, or set CREWROOM_TMUX for the service, then restart it.".into()), true),
        None => item("tmux", "tmux", "missing", "This service was started without process supervision.", Some("Start the service with crewroomd or the Crewroom launcher.".into()), true),
    });
    prerequisites.push(if runner.helpers.is_empty() {
        item(
            "helpers",
            "Crewroom process helpers",
            "missing",
            "This service was started without the process helpers.",
            Some("Start the service with crewroomd or the Crewroom launcher.".into()),
            true,
        )
    } else if runner.helpers.iter().all(|p| executable(p)) {
        item(
            "helpers",
            "Crewroom process helpers",
            "ready",
            "crewroom-process and crewroom-host are installed beside the service.",
            None,
            true,
        )
    } else {
        item(
            "helpers",
            "Crewroom process helpers",
            "missing",
            "crewroom-process or crewroom-host is missing beside the service.",
            Some("Reinstall Crewroom so both helpers sit beside crewroomd.".into()),
            true,
        )
    });
    prerequisites.push(mail_item(probe_config, &sources).await);
    prerequisites.extend(docker_items(probe_config, images).await);
    prerequisites.extend(harness_items(config.as_ref(), &catalog));
    prerequisites.push(provider_item(config.as_ref(), &sources));
    prerequisites.push(profiles_item(config.as_ref()));
    let harness_ready = prerequisites.iter().any(|p| {
        p["id"].as_str().is_some_and(|i| i.starts_with("harness-")) && p["state"] == "ready"
    });
    let ready = harness_ready
        && prerequisites
            .iter()
            .all(|p| p["required"] != true || p["state"] == "ready");
    let error = file_error
        .or(resolve_error)
        .or(parse_error)
        .or_else(|| (!active).then(|| runner.load.error.clone()).flatten());
    let (label, editable, edit_reason) = match source {
        "environment" => (
            format!(
                "{ENVIRONMENT} selects {} for this service.",
                path.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            false,
            Some(format!(
                "Edit that file, or remove {ENVIRONMENT} from the service environment and restart it to manage the runner here."
            )),
        ),
        "settings" => (
            format!(
                "Saved from Settings → Runner in {}.",
                path.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            true,
            None,
        ),
        "embedded" => (
            "Supplied by the program that started this service.".into(),
            false,
            Some(
                "This service's runner configuration is fixed by the program that started it."
                    .into(),
            ),
        ),
        _ => (
            "No runner configuration yet. Fill in the form to create it.".into(),
            true,
            None,
        ),
    };
    Ok(json!({
        "source": {"kind": source, "path": path, "label": label},
        "active": active,
        "pending": pending,
        "error": error,
        "editable": editable,
        "editReason": edit_reason,
        "liveRuns": live,
        "restart": restart,
        "ready": ready,
        "note": "Ready means configured and reachable. Model access is confirmed only when an agent runs.",
        "form": form(&shown),
        "advanced": advanced(&shown),
        "secrets": sources,
        "prerequisites": prerequisites,
    }))
}

fn field<'a>(fields: &'a Value, key: &str) -> std::result::Result<&'a str, Error> {
    match &fields[key] {
        Value::Null => Ok(""),
        Value::String(s) => Ok(s.trim()),
        _ => Err(invalid(format!("{key} must be text."))),
    }
}
fn provider_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    {
        return Err(invalid(format!(
            "Provider name \"{name}\" must use lowercase letters, digits, '.', '_' or '-' so its key can be stored encrypted."
        )));
    }
    Ok(())
}

/// Builds and validates the configuration from the form, then writes the
/// secrets to the encrypted store and the file owner-only. Nothing is written
/// when validation fails.
pub(crate) fn save(data: &Path, body: &Value) -> Result<()> {
    let object = body
        .as_object()
        .ok_or_else(|| invalid("Send the runner settings as an object."))?;
    if let Some(key) = object
        .keys()
        .find(|k| !["fields", "advanced", "secrets", "clear"].contains(&k.as_str()))
    {
        return Err(invalid(format!("Unknown runner setting: {key}.")));
    }
    let (source, path) = locate(data);
    if source == "environment" {
        return Err(conflict(format!(
            "{ENVIRONMENT} selects {} for this service. Edit that file, or remove {ENVIRONMENT} from the service environment and restart it to manage the runner here.",
            path.map(|p| p.display().to_string()).unwrap_or_default()
        )));
    }
    let existing = match &path {
        Some(path) => read_private(path).unwrap_or_else(|_| json!({})),
        None => json!({}),
    };
    let fields = &body["fields"];
    if !fields.is_object() {
        return Err(invalid("Send the runner form fields."));
    }
    let mut config = match &body["advanced"] {
        Value::Null => json!({}),
        Value::Object(_) => body["advanced"].clone(),
        _ => return Err(invalid("Advanced settings must be a JSON object.")),
    };
    if let Some(key) = FORM_KEYS.iter().find(|k| config.get(**k).is_some()) {
        return Err(invalid(format!(
            "Set {key} with its field in the form, not under Advanced."
        )));
    }
    if config.get("catalog").is_none() {
        config["catalog"] = json!([]);
    }
    if config.get("policy").is_none() {
        config["policy"] = json!({});
    }
    let secrets_body = &body["secrets"];
    if !secrets_body.is_null() && !secrets_body.is_object() {
        return Err(invalid("Secrets must be an object."));
    }
    let clear: Vec<String> = match &body["clear"] {
        Value::Null => Vec::new(),
        value => serde_json::from_value(value.clone())
            .map_err(|_| invalid("clear must list secret names."))?,
    };
    // New secret values, keyed by store id; held in memory only.
    let mut new: BTreeMap<String, zeroize::Zeroizing<String>> = BTreeMap::new();
    if let Some(token) = secrets_body["mail"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        new.insert(
            MAIL_SECRET.into(),
            zeroize::Zeroizing::new(token.trim().to_owned()),
        );
    }
    let store = SecretStore::at(data);
    let mut cleared: Vec<String> = Vec::new();
    for name in &clear {
        let id = match name.strip_prefix("provider:") {
            Some(provider) => {
                provider_name(provider)?;
                format!("runner-provider-{provider}")
            }
            None if name == "mail" => MAIL_SECRET.into(),
            None => return Err(invalid(format!("Unknown runner secret: {name}."))),
        };
        cleared.push(id);
    }
    if let Some(providers) = config["models"]["providers"].as_object_mut() {
        for (name, provider) in providers.iter_mut() {
            provider_name(name)?;
            let id = format!("runner-provider-{name}");
            let provider = provider
                .as_object_mut()
                .ok_or_else(|| invalid(format!("Provider {name} must be an object.")))?;
            if provider.contains_key("headers") {
                return Err(invalid(format!(
                    "Provider {name} has headers, which can carry credentials. Use apiKey, or keep headers in a file selected by {ENVIRONMENT}."
                )));
            }
            match provider.remove("apiKey") {
                Some(Value::String(key)) if !key.trim().is_empty() => {
                    new.insert(id.clone(), zeroize::Zeroizing::new(key.trim().to_owned()));
                }
                None | Some(Value::Null) => {}
                Some(Value::String(_)) => {}
                Some(_) => {
                    return Err(invalid(format!("Provider {name} apiKey must be text.")));
                }
            }
            if let Some(key) = secrets_body["providers"][name.as_str()]
                .as_str()
                .filter(|s| !s.trim().is_empty())
            {
                new.insert(id.clone(), zeroize::Zeroizing::new(key.trim().to_owned()));
            }
            let stored = !cleared.contains(&id)
                && matches!(store.runner_secret(&id), StoredToken::Present(_));
            if new.contains_key(&id) || stored {
                provider.insert("apiKey".into(), reference(&id));
            }
        }
    } else if !config["models"].is_null() && !config["models"]["providers"].is_null() {
        return Err(invalid("models.providers must be an object."));
    }
    if let Some(name) = secrets_body["providers"].as_object().and_then(|p| {
        p.keys()
            .find(|k| config["models"]["providers"][k.as_str()].is_null())
    }) {
        return Err(invalid(format!(
            "Add provider {name} under Advanced → models.providers before entering its key."
        )));
    }
    // Form fields.
    let root = field(fields, "root")?;
    if root.is_empty() {
        return Err(invalid("Enter the runtime root folder."));
    }
    config["root"] = json!(root);
    let socket = existing["socket"]
        .as_str()
        .filter(|s| Path::new(s).parent() == Some(Path::new(root)))
        .map(str::to_owned)
        .unwrap_or_else(|| Path::new(root).join("tmux.sock").display().to_string());
    config["socket"] = json!(socket);
    let url = field(fields, "mailUrl")?;
    let project = field(fields, "mailProjectRoot")?;
    if url.is_empty() || project.is_empty() {
        return Err(invalid("Enter the Rust Agent Mail URL and project folder."));
    }
    config["mail"] = json!({
        "url": url,
        "projectRoot": project,
        "atcDisabled": true,
        "retentionSeconds": existing["mail"]["retentionSeconds"].as_f64().filter(|n| *n > 0.).map(|n| json!(n)).unwrap_or(json!(86400)),
        "token": reference(MAIL_SECRET),
    });
    let docker = field(fields, "dockerSocket")?;
    if !docker.is_empty() {
        let mut engine = existing["commandEngine"].clone();
        if !engine.is_object() {
            engine = json!({});
        }
        engine["socket"] = json!(docker);
        config["commandEngine"] = engine;
    }
    for (key, name) in [
        ("evaluationImage", "evaluationImage"),
        ("ompBinary", "ompBinary"),
    ] {
        let value = field(fields, name)?;
        if !value.is_empty() {
            config[key] = json!(value);
        }
    }
    for (key, binary, other, other_key) in [
        ("codex", "codexBinary", "codexAuthFile", "authFile"),
        ("claudeCode", "claudeBinary", "claudeHome", "home"),
    ] {
        let (binary, other) = (field(fields, binary)?, field(fields, other)?);
        if !binary.is_empty() || !other.is_empty() {
            let mut entry = existing[key].clone();
            if !entry.is_object() {
                entry = json!({});
            }
            entry["binary"] = json!(binary);
            entry[other_key] = json!(other);
            config[key] = entry;
        }
    }
    // Validate the resolved configuration before anything is written.
    let mut resolved = config.clone();
    for (slot_path, _) in secret_slots(&config) {
        let Some(slot) = at_mut(&mut resolved, &slot_path) else {
            continue;
        };
        let Some(id) = referenced(slot).map(str::to_owned) else {
            continue;
        };
        let value = match new.get(&id) {
            Some(value) => value.to_string(),
            None if cleared.contains(&id) => String::new(),
            None => match store.runner_secret(&id) {
                StoredToken::Present(value) => value.to_string(),
                _ => String::new(),
            },
        };
        if value.is_empty() {
            return Err(invalid(if id == MAIL_SECRET {
                "Enter the Rust Agent Mail token.".to_owned()
            } else {
                format!("Enter the key for {}.", slot_path.join("."))
            }));
        }
        *slot = json!(value);
    }
    RuntimeConfig::parse(&resolved).map_err(|e| invalid(e.to_string()))?;
    for (id, value) in &new {
        store.save_runner_secret(id, value)?;
    }
    for id in &cleared {
        if !new.contains_key(id) {
            store.clear_runner_secret(id)?;
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&config)?;
    bytes.push(b'\n');
    write_private(&data.join(SETTINGS_FILE), &bytes)
}

/// Asks the owning process to restart the service with the saved
/// configuration, when that is safe.
pub(crate) async fn request_reload(state: &AppState) -> std::result::Result<bool, ApiError> {
    if !state.runner.supervised.load(Ordering::SeqCst) || !live_runs(state).await?.is_empty() {
        return Ok(false);
    }
    state.runner.reload.send_modify(|v| *v = v.wrapping_add(1));
    Ok(true)
}

/// `POST /api/runner/settings` and `POST /api/runner/apply`.
pub(crate) async fn mutate(
    state: &AppState,
    action: &str,
    body: Value,
) -> std::result::Result<Value, ApiError> {
    match action {
        "settings" => {
            let data = state.runner.data.clone();
            tokio::task::spawn_blocking(move || save(&data, &body))
                .await
                .map_err(|_| ApiError::from(conflict("Runner settings save was interrupted.")))??;
            let applying = request_reload(state).await?;
            let notice = if applying {
                "Runner settings saved privately. The service is reloading the runner to apply them; this window reconnects automatically."
            } else if !state.runner.supervised.load(Ordering::SeqCst) {
                "Runner settings saved privately. This service cannot reload itself: stop and start the service to apply them."
            } else {
                "Runner settings saved privately. They apply when no run owns live agent processes; then choose Apply now."
            };
            state.changes.send_modify(|v| *v = v.saturating_add(1));
            Ok(json!({"saved":true,"applying":applying,"notice":notice}))
        }
        "apply" => {
            if body != json!({}) {
                return Err(invalid("Apply accepts no fields.").into());
            }
            if !state.runner.supervised.load(Ordering::SeqCst) {
                return Err(conflict("This service cannot reload itself. Stop and start the service to apply runner changes.").into());
            }
            let live = live_runs(state).await?;
            if !live.is_empty() {
                return Err(conflict(format!(
                    "Not applied: {} run(s) own live agent processes, and reloading the runner would pause them. Pause or finish them first.",
                    live.len()
                ))
                .into());
            }
            request_reload(state).await?;
            Ok(
                json!({"saved":false,"applying":true,"notice":"The service is reloading the runner; this window reconnects automatically."}),
            )
        }
        _ => Err(Error::NotFound("Unknown runner operation.".into()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_references_resolve_only_from_the_encrypted_store() {
        let root = tempfile::tempdir().unwrap();
        let store = SecretStore::open(root.path()).unwrap();
        let mut value = json!({"mail":{"token":reference(MAIL_SECRET)},"models":{"providers":{"zai":{"apiKey":reference("runner-provider-zai")}}}});
        assert!(
            resolve(&mut value.clone(), &store)
                .unwrap_err()
                .contains("mail.token")
        );
        store
            .save_runner_secret(MAIL_SECRET, "synthetic-mail")
            .unwrap();
        store
            .save_runner_secret("runner-provider-zai", "synthetic-key")
            .unwrap();
        resolve(&mut value, &store).unwrap();
        assert_eq!(value["mail"]["token"], "synthetic-mail");
        assert_eq!(
            value["models"]["providers"]["zai"]["apiKey"],
            "synthetic-key"
        );
        let file = std::fs::read_to_string(store.credentials_path()).unwrap();
        assert!(!file.contains("synthetic-mail") && !file.contains("synthetic-key"));
    }

    #[test]
    fn a_reload_waits_for_runs_that_own_live_processes() {
        let paused = json!({"status":"paused"});
        let stopped = json!({"attempts":[{"state":"exited","ownership":"stopped"},{"state":"admitted","ownership":"none"}]});
        assert!(!owns_live(&paused, &stopped));
        assert!(!owns_live(&json!({"status":"completed"}), &json!({})));
        let held = json!({"attempts":[{"state":"idle","ownership":"held"}]});
        assert!(owns_live(&paused, &held));
        let uncertain = json!({"attempts":[{"state":"exited","ownership":"uncertain"}]});
        assert!(owns_live(&paused, &uncertain));
        assert!(owns_live(&json!({"status":"running"}), &json!({})));
        assert!(owns_live(&json!({"status":"pausing"}), &json!({})));
    }

    #[test]
    fn advanced_view_never_carries_secrets() {
        let raw = json!({"root":"/r","mail":{"token":"inline-secret"},"catalog":[],"models":{"providers":{"p":{"apiKey":"inline-key","headers":{"x-key":"inline-header"},"baseUrl":"http://127.0.0.1:1"}}}});
        let shown = advanced(&raw).to_string();
        assert!(
            !shown.contains("inline-secret")
                && !shown.contains("inline-key")
                && !shown.contains("inline-header")
        );
        assert!(shown.contains("baseUrl"));
        assert!(advanced(&raw).get("root").is_none());
        let sources = secret_sources(&raw, &SecretStore::at(Path::new("/nonexistent")));
        assert_eq!(sources, json!({"mail":"inline","providers":{"p":"inline"}}));
    }
}
