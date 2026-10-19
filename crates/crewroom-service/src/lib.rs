mod execution;
mod github;
mod intake;
mod jira_refresh;
mod role_catalog;
mod routes;
mod runner;
mod scheduler;
mod ticket_sources;
mod views;
use crewroom_runtime::messaging::StoreAccess;
use crewroom_runtime::providers::secrets::{Integration, SecretStore};
use execution::Execution;
pub use execution::ServiceOptions;
pub use github::GitHubOptions;
pub use runner::RunnerLoad;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Path, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use crewroom_core::{
    Error,
    authority::{Authority, write_private},
    learning::Learning,
    roles::Roles,
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, watch};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub protocol: u32,
    pub origin: String,
    pub token: String,
    pub pid: u32,
}

struct AuthorityState {
    authority: Arc<Authority>,
    store: Store,
    roles: Roles,
}
#[derive(Clone)]
struct AppState {
    execution: Option<Arc<Execution>>,
    stopping: Arc<AtomicBool>,
    inner: Arc<Mutex<Option<AuthorityState>>>,
    info: ConnectionInfo,
    changes: watch::Sender<u64>,
    /// Encrypted provider tokens; plaintext never leaves the service.
    secrets: SecretStore,
    /// GitHub personal-access-token connection and its last observed check.
    github: Arc<github::GitHub>,
    /// Settings → Runner facts and the reload request channel (FOR-83).
    runner: Arc<runner::RunnerState>,
}
#[derive(Clone)]
struct Access(Arc<Mutex<Option<AuthorityState>>>);
impl StoreAccess for Access {
    fn with_store<T>(
        &self,
        f: impl FnOnce(&Store) -> crewroom_core::Result<T>,
    ) -> crewroom_core::Result<T> {
        let guard = self
            .0
            .lock()
            .map_err(|_| Error::Conflict("Workflow authority needs a restart.".into()))?;
        let inner = guard.as_ref().ok_or_else(|| {
            Error::Conflict("Crewroom is stopping. Saved work is retained.".into())
        })?;
        inner.authority.assert_current()?;
        f(&inner.store)
    }
}
impl Access {
    fn playbooks(&self) -> crewroom_core::Result<Vec<Value>> {
        let guard = self
            .0
            .lock()
            .map_err(|_| Error::Conflict("Workflow authority needs a restart.".into()))?;
        let state = guard
            .as_ref()
            .ok_or_else(|| Error::Conflict("Crewroom is stopping.".into()))?;
        state.authority.assert_current()?;
        crewroom_core::playbooks::Playbooks::beside_roles(&state.roles).read()
    }
    fn roles(&self) -> crewroom_core::Result<Vec<Value>> {
        let guard = self
            .0
            .lock()
            .map_err(|_| Error::Conflict("Workflow authority needs a restart.".into()))?;
        let inner = guard
            .as_ref()
            .ok_or_else(|| Error::Conflict("Crewroom is stopping.".into()))?;
        inner.authority.assert_current()?;
        // A failed approved catalog write remains pending and visible in the store.
        let _ = Learning::new(&inner.store, &inner.roles).reconcile();
        inner.roles.read()
    }
}
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn present(v: &Value) -> bool {
    !v.is_null() && v != false
}
fn text<'a>(v: &'a Value, key: &str) -> crewroom_core::Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| Error::Invalid(format!("Missing {key}.")))
}
pub struct Service {
    pub info: ConnectionInfo,
    owner: Option<ServiceOwner>,
}
struct ServiceOwner {
    info: ConnectionInfo,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    descriptor: PathBuf,
    state: AppState,
    timer_stop: watch::Sender<bool>,
    timer: tokio::task::JoinHandle<()>,
}

struct ApiError(Arc<Error>);
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(Arc::new(e))
    }
}
impl From<Arc<Error>> for ApiError {
    fn from(e: Arc<Error>) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.as_ref() {
            Error::Conflict(_)
            | Error::RepositoryConfigurationChanged(_)
            | Error::FeedbackResume { .. } => StatusCode::CONFLICT,
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            "Unable to complete this operation. Saved work has been retained.".into()
        } else {
            self.0.to_string()
        };
        (status, Json(json!({"error":message}))).into_response()
    }
}

impl Service {
    /// Intake-only embedding, without an installed process helper or runner.
    pub async fn start(data: &FilePath, role_path: &FilePath, port: u16) -> anyhow::Result<Self> {
        Self::start_owned(data, role_path, port, None).await
    }
    pub async fn start_with_options(
        data: &FilePath,
        role_path: &FilePath,
        port: u16,
        options: ServiceOptions,
    ) -> anyhow::Result<Self> {
        Self::start_owned(data, role_path, port, Some(options)).await
    }
    async fn start_owned(
        data: &FilePath,
        roles: &FilePath,
        port: u16,
        options: Option<ServiceOptions>,
    ) -> anyhow::Result<Self> {
        let (send, receive) = oneshot::channel();
        let (data, roles) = (data.to_owned(), roles.to_owned());
        tokio::spawn(async move {
            let result = Self::start_inner(&data, &roles, port, options).await;
            if let Err(Ok(service)) = send.send(result) {
                let _ = service.close().await;
            }
        });
        receive
            .await
            .map_err(|_| anyhow::anyhow!("Daemon initialization was interrupted."))?
    }
    async fn start_inner(
        data: &FilePath,
        role_path: &FilePath,
        port: u16,
        mut options: Option<ServiceOptions>,
    ) -> anyhow::Result<Self> {
        let authority = Arc::new(if options.is_some() {
            Authority::acquire_for_execution(data)?
        } else {
            Authority::acquire(data)?
        });
        let data = std::fs::canonicalize(data)?;
        // First run creates the install key; existing encrypted values keep theirs.
        let secrets = SecretStore::open(&data)?;
        let github = Arc::new(github::GitHub::new(
            match options.as_mut().and_then(|o| o.github.take()) {
                Some(github) => github,
                None => GitHubOptions::production()?,
            },
            secrets.clone(),
        ));
        let store = Store::open(&data.join("workflow.sqlite"))?;
        let roles = Roles::new(role_path);
        Learning::new(&store, &roles).reconcile()?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let info = ConnectionInfo {
            protocol: 1,
            origin: format!("http://{}", listener.local_addr()?),
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            pid: std::process::id(),
        };
        let (changes, _) = watch::channel(0u64);
        let runner = Arc::new(runner::RunnerState::new(&data, options.as_ref()));
        let inner = Arc::new(Mutex::new(Some(AuthorityState {
            authority: authority.clone(),
            store,
            roles,
        })));
        let execution = options
            .map(|options| {
                Execution::new(
                    Access(inner.clone()),
                    authority,
                    &data,
                    role_path,
                    &info.origin,
                    options,
                    changes.clone(),
                    &github,
                )
            })
            .transpose()?;
        let state = AppState {
            inner,
            info: info.clone(),
            changes,
            execution,
            stopping: Arc::new(AtomicBool::new(false)),
            secrets,
            github,
            runner,
        };
        let app = Router::new()
            .route("/api/health", get(health))
            .route("/api/workspace", get(workspace))
            .route("/api/live", get(live))
            .route("/internal/{*path}", axum::routing::post(internal))
            .route("/api/{*path}", get(read).post(mutate))
            .fallback(|| async {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error":"API operation not found."})),
                )
            })
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
            .layer(middleware::from_fn_with_state(state.clone(), boundary))
            .with_state(state.clone());
        let descriptor = data.join("native-connection.json");
        let (shutdown, stop) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = stop.await;
            })
            .await
        });
        let (timer_stop, mut stopping) = watch::channel(false);
        let background = state.clone();
        let timer = tokio::spawn(async move {
            let mut tick = 0u64;
            loop {
                tokio::select! { _ = stopping.changed() => break, _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {} }
                let Some(e) = &background.execution else {
                    continue;
                };
                if !e.ready.load(Ordering::SeqCst) {
                    continue;
                }
                tick += 1;
                if let Some(j) = &e.jira
                    && let Err(error) = j.flush().await
                {
                    eprintln!("Jira reconciliation failed: {error}");
                }
                if let Err(error) = e.github.flush().await {
                    eprintln!("GitHub issue delivery reconciliation failed: {error}");
                }
                if tick.is_multiple_of(3) {
                    if let Err(error) = e.publication.flush().await {
                        eprintln!("Publication reconciliation failed: {error}");
                    }
                    if let Ok(records) = e.store.with_store(|s| s.publications()) {
                        for record in records {
                            if record["pr"]["state"] == "confirmed"
                                && let Some(id) = record["runId"].as_str()
                            {
                                let _ = e.publication.observe_feedback(id).await;
                            }
                        }
                    }
                }
                if tick.is_multiple_of(12)
                    && let Err(error) = e.sync.refresh_all().await
                {
                    eprintln!("Jira refresh failed: {error}");
                }
                let _ = e.notify_all();
            }
        });
        let owner = ServiceOwner {
            info: info.clone(),
            shutdown: Some(shutdown),
            task,
            descriptor,
            state,
            timer_stop,
            timer,
        };
        let server = Self {
            info,
            owner: Some(owner),
        };
        let initialized = async {
            let owner = server.owner.as_ref().unwrap();
            if let Some(e) = &owner.state.execution {
                e.initialize().await?;
            }
            write_private(&owner.descriptor, &serde_json::to_vec(&server.info)?)?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(error) = initialized {
            let cleanup = server.close().await;
            return Err(error.context(format!("Startup failed; cleanup: {cleanup:?}")));
        }
        Ok(server)
    }
    /// Requests to restart this service with the saved runner configuration
    /// (FOR-83). Taking the receiver promises that the caller closes this
    /// service and starts it again when the value changes; until then Settings
    /// → Runner reports that a manual stop and start is needed.
    pub fn reload_requests(&self) -> watch::Receiver<u64> {
        let runner = &self.owner.as_ref().expect("Service owner").state.runner;
        runner.supervised.store(true, Ordering::SeqCst);
        runner.reload.subscribe()
    }
    pub async fn close(mut self) -> anyhow::Result<()> {
        let owner = self.owner.take().expect("Service owner");
        // Once polled, close is driven even when its caller disappears.
        tokio::spawn(owner.close()).await?
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            let close = async move {
                if let Err(error) = owner.close().await {
                    eprintln!("Daemon cleanup needs inspection: {error}");
                }
            };
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(close);
            } else {
                std::thread::spawn(move || {
                    if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        runtime.block_on(close);
                    }
                });
            }
        }
    }
}
impl ServiceOwner {
    async fn close(mut self) -> anyhow::Result<()> {
        self.state.stopping.store(true, Ordering::SeqCst);
        self.state.changes.send_replace(u64::MAX);
        self.timer_stop.send_replace(true);
        let cleanup = if let Some(e) = &self.state.execution {
            e.close().await
        } else {
            Ok(())
        };
        self.timer.await?;
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.await??;
        if std::fs::read(&self.descriptor)
            .ok()
            .and_then(|v| serde_json::from_slice::<ConnectionInfo>(&v).ok())
            .is_some_and(|c| c.token == self.info.token)
        {
            std::fs::remove_file(&self.descriptor)?;
        }
        self.state
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("Workflow authority lock poisoned"))?
            .take();
        cleanup?;
        Ok(())
    }
}

async fn boundary(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let internal = request.uri().path().starts_with("/internal/");
    if state.stopping.load(Ordering::SeqCst)
        || (!internal
            && request.method() == axum::http::Method::POST
            && state
                .execution
                .as_ref()
                .is_some_and(|e| !e.ready.load(Ordering::SeqCst)))
    {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"Workflow recovery or shutdown is in progress. Saved work is retained."}))).into_response();
    }
    let headers = request.headers();
    let host = state.info.origin.trim_start_matches("http://");
    let origin_ok = headers
        .get(header::ORIGIN)
        .is_none_or(|h| h.to_str().ok() == Some(&state.info.origin));
    let host_ok = headers.get(header::HOST).and_then(|h| h.to_str().ok()) == Some(host);
    let cross_site =
        headers.get("sec-fetch-site").and_then(|h| h.to_str().ok()) == Some("cross-site");
    let actual = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .as_bytes();
    let expected = format!("Bearer {}", state.info.token);
    let authenticated = actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.bytes())
            .fold(0u8, |v, (a, b)| v | (a ^ b))
            == 0;
    if !origin_ok || !host_ok || cross_site || (!internal && !authenticated) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Authenticated local Crewroom access is required."})),
        )
            .into_response();
    }
    if request.method() == axum::http::Method::POST
        && headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) != Some(&state.info.origin)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Mutations require the Crewroom origin."})),
        )
            .into_response();
    }
    if request.method() == axum::http::Method::POST
        && headers
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .is_none_or(|s| s.split(';').next().unwrap_or("").trim() != "application/json")
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Mutations require JSON requests."})),
        )
            .into_response();
    }
    let mut response = next.run(request).await;
    for (key, value) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("cache-control", "no-store"),
    ] {
        response.headers_mut().insert(
            header::HeaderName::from_static(key),
            header::HeaderValue::from_static(value),
        );
    }
    response
}

async fn operation<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&mut AuthorityState) -> crewroom_core::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    let stopping = state.stopping.clone();
    let state = state.inner.clone();
    tokio::task::spawn_blocking(move || {
        if stopping.load(Ordering::SeqCst) {
            return Err(Error::Conflict("Crewroom is stopping.".into()));
        }
        let mut inner = state
            .lock()
            .map_err(|_| Error::Conflict("Workflow authority needs a restart.".into()))?;
        let inner = inner.as_mut().ok_or_else(|| {
            Error::Conflict("Crewroom is stopping. Saved work is retained.".into())
        })?;
        inner.authority.assert_current()?;
        f(inner)
    })
    .await
    .map_err(|_| {
        ApiError::from(Error::Conflict(
            "Workflow operation was interrupted.".into(),
        ))
    })?
    .map_err(ApiError::from)
}
fn snapshot(inner: &AuthorityState) -> crewroom_core::Result<Value> {
    let mut value = inner.store.snapshot()?;
    let mut intake = json!({});
    for task in value["tasks"].as_array().into_iter().flatten() {
        let id = task["id"].as_str().unwrap();
        intake[id] = inner.store.intake_view(id)?;
    }
    value["intake"] = intake;
    value["roles"] = json!(inner.roles.read()?);
    value["playbooks"] =
        json!(crewroom_core::playbooks::Playbooks::beside_roles(&inner.roles).read()?);
    value["principles"] =
        json!(crewroom_core::principles::Principles::beside_roles(inner.roles.path()).read()?);
    value["modelJobs"] = crewroom_core::model_jobs::read(
        &inner.roles.path().with_file_name("model-jobs.json"),
        &json!({}),
    )?;
    value["roleCatalog"] = role_catalog::facts(inner, &value)?;
    Ok(value)
}
async fn health(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let available = state
        .execution
        .as_ref()
        .is_some_and(|e| e.ready.load(Ordering::SeqCst) && e.workflow.is_some());
    let scheduling = state
        .execution
        .as_ref()
        .and_then(|e| e.scheduler.as_ref())
        .and_then(|s| s.error());
    operation(&state, move |_| Ok(json!({"service":"crewroom-rust","protocol":1,"executionAvailable":available,"schedulingError":scheduling}))).await.map(Json)
}
/// Where each integration's credential comes from. Never the value itself.
fn credentials_view(state: &AppState) -> Value {
    let mut view = json!({});
    for integration in Integration::ALL {
        let environment = match integration {
            Integration::GitHub => state.github.environment(),
            _ => std::env::var(integration.environment()).ok(),
        };
        view[integration.id()] = state.secrets.status(integration, environment.as_deref());
    }
    view
}
/// Connects, checks or disconnects GitHub. Responses carry status only.
async fn github_mutation(state: &AppState, action: &str, body: Value) -> Result<Value, ApiError> {
    match action {
        "connect" => state.github.connect(body).await?,
        "check" => state.github.check(body).await?,
        "disconnect" => state.github.disconnect(body).await?,
        _ => return Err(Error::NotFound("Unknown GitHub operation.".into()).into()),
    }
    state.changes.send_modify(|v| *v += 1);
    Ok(json!({"github": state.github.view(), "credentials": credentials_view(state)}))
}
/// Saves or clears one integration's token in the encrypted install store.
async fn credentials_mutation(
    state: &AppState,
    integration: &str,
    clear: bool,
    body: Value,
) -> Result<Value, ApiError> {
    let integration = Integration::parse(integration)?;
    let secrets = state.secrets.clone();
    let result = tokio::task::spawn_blocking(move || {
        if clear {
            if body != json!({}) {
                return Err(Error::Invalid("Clear accepts no fields.".into()));
            }
            return secrets.clear(integration);
        }
        let fields = body
            .as_object()
            .ok_or_else(|| Error::Invalid("Send a token object.".into()))?;
        if fields
            .keys()
            .any(|k| k != "token" && k != "replaceUnreadable")
        {
            return Err(Error::Invalid(
                "Credentials accept only token and replaceUnreadable.".into(),
            ));
        }
        let token = fields
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("Enter a token.".into()))?;
        let replace = match fields.get("replaceUnreadable") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(Error::Invalid(
                    "replaceUnreadable must be a boolean.".into(),
                ));
            }
        };
        secrets.save(integration, token, replace)
    })
    .await
    .map_err(|_| ApiError::from(Error::Conflict("Credential update was interrupted.".into())))?;
    result?;
    state.changes.send_modify(|v| *v += 1);
    Ok(json!({"credentials": credentials_view(state)}))
}
async fn workspace_value(state: &AppState) -> Result<Value, ApiError> {
    let mut value = operation(state, |s| snapshot(s)).await?;
    let runs = array(&value["runs"]).to_vec();
    value["credentials"] = credentials_view(state);
    value["github"] = state.github.view();
    value["githubIssues"] = github::issues::workspace_view(state).await?;
    if let Some(e) = &state.execution {
        if let Some(runs) = value["runs"].as_array_mut() {
            runs.retain(|r| r["purpose"] != "evaluation");
        }
        views::workspace(e, &mut value)?;
        if let Some(workflow) = &e.workflow {
            let policy = workflow.model_job_policy();
            value["modelJobs"] = operation(state, move |s| {
                crewroom_core::model_jobs::read(
                    &s.roles.path().with_file_name("model-jobs.json"),
                    &policy,
                )
            })
            .await?;
            if value["modelJobs"]["legacyDefault"] == true {
                value["modelJobs"]["legacyEffort"] =
                    workflow.model_job_policy()["default"]["effort"].clone();
            }
            value["modelCatalog"] = workflow.model_catalog();
            for role in array(&value["roles"]).to_vec() {
                let id = text(&role, "id")?;
                match workflow.role_model(&role) {
                    Ok(model) => {
                        value["roleCatalog"][id]["model"] = model;
                        value["roleCatalog"][id]["modelError"] = Value::Null;
                    }
                    Err(error) => value["roleCatalog"][id]["modelError"] = json!(error.to_string()),
                }
            }
        }
    }
    value["modelDiscovery"] = model_discovery(state);
    value["runner"] = runner::summary(state);
    value["allowance"] = allowance_view(state);
    ticket_sources::project(&mut value, &runs).await?;
    Ok(value)
}
/// FOR-74: always projected, so every harness shows a reading or the reason
/// it has none.
fn allowance_view(state: &AppState) -> Value {
    match &state.execution {
        Some(e) => e.allowance.view(),
        None => {
            json!({"state":"unavailable","note":crewroom_runtime::model_catalog::allowance::NOTE,
            "harnesses":crewroom_runtime::model_catalog::allowance::HARNESSES.iter().map(|h|
                crewroom_runtime::model_catalog::allowance::Reading::unavailable(h,"This service was started without model discovery.",None)).collect::<Vec<_>>()})
        }
    }
}
fn unavailable_catalog(reason: &str, remedy: &str) -> Value {
    json!({"state":"unavailable","reason":reason,"remedy":remedy,"harnesses":[],"choices":[],"offerings":[]})
}
/// Always projected: model pickers render either catalog facts or the
/// operator-facing cause and remedy, never a silent free-text fallback.
fn model_discovery(state: &AppState) -> Value {
    let Some(e) = &state.execution else {
        return unavailable_catalog(
            "This service was started without model discovery.",
            "Start the service from the Crewroom launcher or crewroomd, then reopen Crewroom.",
        );
    };
    if let Some(workflow) = &e.workflow {
        return workflow.catalog_view().unwrap_or_else(|error| {
            unavailable_catalog(
                &error.to_string(),
                "Refresh models. If this persists, check the runner model configuration and restart the service.",
            )
        });
    }
    match &e.discovery {
        Some(catalog) => crewroom_runtime::model_catalog::unconfigured::view(
            catalog,
            &e.billing,
            crewroom_runtime::model_catalog::store::now(),
        ),
        None => unavailable_catalog(
            "Model discovery is unavailable in this service.",
            "Restart the service to retry model discovery.",
        ),
    }
}
async fn workspace(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    workspace_value(&state).await.map(Json)
}
async fn read(
    State(state): State<AppState>,
    Path(path): Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    if path == "model-catalog" {
        if let Some(workflow) = state.execution.as_ref().and_then(|e| e.workflow.as_ref()) {
            return workflow.catalog_view().map(Json).map_err(ApiError::from);
        }
        return Ok(Json(model_discovery(&state)));
    }
    if path == "allowance" {
        return Ok(Json(allowance_view(&state)));
    }
    if path == "repository-folders/inspect" {
        return repository_folder(&state, None, query).await.map(Json);
    }
    if path == "runner" {
        return runner::view(&state).await.map(Json);
    }
    if let Some(e) = &state.execution
        && let Some(value) = views::read(e, &path, &query).await?
    {
        return Ok(Json(value));
    }
    operation(&state, move |s| {
        let parts: Vec<_> = path.split('/').collect();
        match parts.as_slice() {
            ["comparisons"] => Ok(json!(s.store.comparisons()?)),
            ["comparisons", id] => s.store.get_comparison(id),
            ["tasks", id, "revisions"] => Ok(json!(s.store.requirement_revisions(id)?)),
            ["runs", id, "runtime"] => {
                let mut view = s.store.runtime(id)?;
                view["run"] = s.store.get_run(id)?;
                view["teamProposals"] = json!(s.store.team_proposals(id)?);
                view["commands"] = views::commands(&s.store.command_events(id)?);
                Ok(view)
            }
            ["runs", id, "events"] => {
                s.store
                    .agent_event_page(id, views::after(&query)?, views::limit(&query)?)
            }
            _ => Err(Error::NotFound("API operation not found.".into())),
        }
    })
    .await
    .map(Json)
}
async fn internal(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let workflow = state
        .execution
        .as_ref()
        .and_then(|e| e.workflow.as_ref())
        .ok_or_else(unavailable)?;
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let result = workflow.tool(&path, authorization, body).await?;
    if let Some(e) = &state.execution {
        e.changed();
    }
    Ok(Json(result))
}
/// Inspects a chosen folder (read-only), or initializes it after the
/// operator confirmed (`body`). The store lock is held only to read the
/// configured checkouts; Git runs on a blocking worker without it (FOR-70).
async fn repository_folder(
    state: &AppState,
    body: Option<Value>,
    query: std::collections::HashMap<String, String>,
) -> Result<Value, ApiError> {
    let exclude = query.get("repository").cloned();
    let known = operation(state, move |s| s.store.known_folders(exclude.as_deref())).await?;
    let initializing = body.is_some();
    let result = tokio::task::spawn_blocking(move || match body {
        Some(body) => crewroom_core::repository_folders::initialize(&body, &known),
        None => Ok(crewroom_core::repository_folders::inspect(
            query.get("path").map(String::as_str).unwrap_or(""),
            &known,
        )),
    })
    .await
    .map_err(|_| ApiError::from(Error::Conflict("Folder inspection was interrupted.".into())))??;
    if initializing {
        state.changes.send_modify(|v| *v += 1);
    }
    Ok(result)
}
fn unavailable() -> ApiError {
    Error::Conflict(
        "Rust runner integration is not configured. No agent was started or stopped. Open Settings → Runner to configure it.".into(),
    )
    .into()
}
async fn wait(task: crewroom_runtime::supervisor::tasks::Task) -> Result<Value, ApiError> {
    task.wait().await.map_err(ApiError::from)
}
async fn mutate(
    State(state): State<AppState>,
    Path(path): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    // Runner settings sit outside the workflow operation queue: a reload
    // drains that queue while it closes the service.
    if let Some(action) = path.strip_prefix("runner/") {
        return runner::mutate(&state, action, body).await.map(Json);
    }
    if let Some(e) = &state.execution {
        let owned = state.clone();
        let task = e.operations.enqueue(
            &format!("api-{}", uuid::Uuid::new_v4()),
            None,
            move || async move {
                let execution = owned.execution.clone();
                let result = mutate_inner(owned, path, body).await.map_err(|e| e.0);
                if let Some(e) = execution {
                    e.changed();
                }
                result
            },
        )?;
        wait(task).await.map(Json)
    } else {
        mutate_inner(state, path, body).await.map(Json)
    }
}
async fn mutate_inner(state: AppState, path: String, body: Value) -> Result<Value, ApiError> {
    if state.stopping.load(Ordering::SeqCst) {
        return Err(Error::Conflict("Crewroom is stopping. Saved work is retained.".into()).into());
    }
    let segments = path.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["repositories", id, "github-issues", action] => {
            return github::issues::mutate(&state, id, action, body).await;
        }
        ["repositories", id, "ticket-source"] => {
            return ticket_sources::confirm(&state, id, body).await;
        }
        ["repository-folders", "initialize"] => {
            return repository_folder(&state, Some(body), Default::default()).await;
        }
        ["github", action] => return github_mutation(&state, action, body).await,
        // A GitHub token is saved only after GitHub accepts it.
        ["credentials", "github"] => {
            return Err(Error::Invalid(
                "Use Connect GitHub so the token is validated before it is saved.".into(),
            )
            .into());
        }
        ["credentials", "github", "clear"] => {
            return github_mutation(&state, "disconnect", body).await;
        }
        ["credentials", integration] => {
            return credentials_mutation(&state, integration, false, body).await;
        }
        ["credentials", integration, "clear"] => {
            return credentials_mutation(&state, integration, true, body).await;
        }
        _ => {}
    }
    if segments.len() >= 3 && segments[0] == "runs" && segments[2] != "steering" {
        let id = segments[1].to_owned();
        let evaluation = operation(&state, move |s| match s.store.get_run(&id) {
            Ok(run) => Ok(run["purpose"] == "evaluation"),
            Err(Error::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        })
        .await?;
        if evaluation {
            return Err(Error::Conflict("Use the comparison controls; ordinary coding actions cannot alter evaluation runs.".into()).into());
        }
    }
    if path == "comparisons" {
        let replay_body = body.clone();
        if let Some(receipt) = operation(&state, move |s| {
            s.store.comparison_create_replay(&replay_body)
        })
        .await?
        {
            return Ok(receipt);
        }
    }
    if let ["comparisons", id, "adoption"] = segments.as_slice() {
        let id = (*id).to_owned();
        let replay_body = body.clone();
        if let Some(receipt) = operation(&state, move |s| {
            s.store.comparison_adoption_replay(&id, &replay_body)
        })
        .await?
        {
            return Ok(receipt);
        }
    }
    if path == "comparisons/preflight"
        && state
            .execution
            .as_ref()
            .and_then(|e| e.workflow.as_ref())
            .is_none()
    {
        return Ok(
            json!({"canStart":false,"reasons":["Rust runner integration is not configured. No comparison can start."]}),
        );
    }
    if path == "model-catalog/refresh" {
        if body != json!({}) {
            return Err(
                Error::Invalid("Refresh accepts no configuration or credentials.".into()).into(),
            );
        }
        let execution = state.execution.as_ref().ok_or_else(unavailable)?;
        if let Some(workflow) = &execution.workflow {
            return workflow.refresh_model_catalog().map_err(ApiError::from);
        }
        let catalog = execution.discovery.as_ref().ok_or_else(unavailable)?;
        catalog.refresh()?;
        return Ok(model_discovery(&state));
    }
    if path == "allowance/refresh" {
        if body != json!({}) {
            return Err(
                Error::Invalid("Refresh accepts no configuration or credentials.".into()).into(),
            );
        }
        let execution = state.execution.as_ref().ok_or_else(unavailable)?;
        return execution.refresh_allowance().map_err(ApiError::from);
    }
    if path == "model-preview" {
        let role = crewroom_core::roles::validate_role(&body["role"])?;
        let workflow = state
            .execution
            .as_ref()
            .and_then(|e| e.workflow.as_ref())
            .ok_or_else(unavailable)?;
        return Ok(match workflow.role_model(&role) {
            Ok(model) => json!({"model":model,"error":null}),
            Err(error) => json!({"model":null,"error":error.to_string()}),
        });
    }
    if path == "model-jobs" {
        let policy = state
            .execution
            .as_ref()
            .and_then(|e| e.workflow.as_ref())
            .map(|w| w.model_job_policy())
            .unwrap_or_else(|| json!({}));
        let result = operation(&state, move |s| {
            crewroom_core::model_jobs::save(
                &s.roles.path().with_file_name("model-jobs.json"),
                &policy,
                &body,
            )
        })
        .await?;
        state.changes.send_modify(|v| *v += 1);
        if let Some(e) = &state.execution {
            e.changed();
        }
        return Ok(result);
    }
    if path == "role-preview" {
        let mut preview = operation(&state, move |s| role_catalog::preview(s, &body)).await?;
        let workspace = workspace_value(&state).await?;
        preview["catalogRevision"] = workspace["modelDiscovery"]["revision"].clone();
        preview["modelJobsRevision"] = workspace["modelJobs"]["revision"].clone();
        if let Some(workflow) = state.execution.as_ref().and_then(|e| e.workflow.as_ref()) {
            match workflow.role_model(&preview["role"]) {
                Ok(model) => {
                    if model["selection"].is_object() {
                        preview["catalogRevision"] = model["selection"]["catalogRevision"].clone();
                    }
                    preview["modelJobsRevision"] = model["settingsRevision"].clone();
                    preview["assignment"]["model"] = model;
                    preview["modelError"] = Value::Null;
                }
                Err(error) => preview["modelError"] = json!(error.to_string()),
            }
        } else {
            preview["modelError"] = json!("Runner integration is unavailable.");
        }
        return Ok(preview);
    }
    if let Some(value) = intake::mutate(&state, &path, &body).await? {
        return Ok(value);
    }
    if let Some(e) = &state.execution
        && let Some(value) = routes::mutate(e, &path, &body).await?
    {
        e.changed();
        return Ok(value);
    }
    if path == "repositories" {
        let input = body.clone();
        if let Some(replay) = operation(&state, move |s| s.store.repository_replay(&input)).await? {
            return Ok(replay);
        }
        let validated =
            tokio::task::spawn_blocking(move || crewroom_core::repositories::validate(&body))
                .await
                .map_err(|_| {
                    ApiError::from(Error::Conflict(
                        "Repository validation was interrupted.".into(),
                    ))
                })?
                .map_err(ApiError::from)?;
        let result = operation(&state, move |s| s.store.save_repository(&validated)).await?;
        state.changes.send_modify(|v| *v += 1);
        return Ok(result);
    }
    if let ["tasks", id, "research-runs"] = path.split('/').collect::<Vec<_>>().as_slice() {
        let id = (*id).to_owned();
        let prepared = operation(&state, move |s| s.store.prepare_research(&id, &body)).await?;
        let pinned = tokio::task::spawn_blocking(move || prepared.validate())
            .await
            .map_err(|_| {
                ApiError::from(Error::Conflict(
                    "Research validation was interrupted.".into(),
                ))
            })?
            .map_err(ApiError::from)?;
        let result = operation(&state, move |s| s.store.queue_pinned_research(&pinned)).await?;
        state.changes.send_modify(|v| *v += 1);
        return Ok(result);
    }
    // Git preflight must not hold the workflow mutex while observations wait.
    // Replay precedes Git so a saved edit remains replayable after a checkout disappears.
    if let ["repositories", id] = path.split('/').collect::<Vec<_>>().as_slice() {
        let id = (*id).to_owned();
        let replay_id = id.clone();
        let replay_body = body.clone();
        let replay = operation(&state, move |s| {
            Ok(s.store
                .repository_update_replay(&replay_id, &replay_body)?
                .unwrap_or(Value::Null))
        })
        .await?;
        if !replay.is_null() {
            return Ok(replay);
        }
        let preflight = body.clone();
        let validated = tokio::task::spawn_blocking(move || {
            crewroom_core::repositories::validate_update(&preflight)
        })
        .await
        .map_err(|_| {
            ApiError::from(Error::Conflict(
                "Repository validation was interrupted.".into(),
            ))
        })?
        .map_err(ApiError::from)?;
        let result = operation(&state, move |s| {
            s.store.update_repository(&id, &validated, &body)
        })
        .await?;
        state.changes.send_modify(|v| *v += 1);
        return Ok(result);
    }
    let affected = path.split('/').nth(1).map(str::to_owned);
    let result = operation(&state, move |s| {
        let parts: Vec<_> = path.split('/').collect();
        match parts.as_slice() {
            ["tasks"] => s.store.create_task(&body),
            ["import", "jira"] => s.store.import_jira(&body),
            ["tasks", id, "implementation-plans"] => s.store.save_implementation_plan(id, &body),
            ["intakes", id, "answers"] => s.store.answer_intake(id, &body),
            ["intakes", id, "cancel"] => s.store.cancel_intake(id, &body),
            ["intakes", id, "revalidate"] => s.store.revalidate_intake(id, &body),
            ["tasks", id, "refinement"] => s.store.refine(id, &body),
            ["tasks", id, "repository"] => s.store.assign_repository(id, &body),
            ["tasks", id, "dependencies"] => s.store.set_dependencies(id, &body),
            ["tasks", id, "notes"] => s.store.add_note(id, &body),
            ["tasks", id, "runs"] => s.store.queue_with_playbooks(
                id,
                &body,
                &s.roles.read()?,
                &crewroom_core::playbooks::Playbooks::beside_roles(&s.roles).read()?,
            ),
            ["playbooks"] => {
                crewroom_core::playbooks::Playbooks::beside_roles(&s.roles).save(&body)
            }
            ["runs", id, "playbook"] => s.store.select_playbook(
                id,
                &body,
                &crewroom_core::playbooks::Playbooks::beside_roles(&s.roles).read()?,
            ),
            ["tasks", id, "refinement-runs"] => s.store.queue_refinement(id, &body),
            ["runs", id, "adopt-requirements"] => s.store.adopt_requirements(id, &body),
            ["runs", id, "close"] => s.store.close_refinement(id),
            ["runs", id, "steering"] => s.store.steer(id, &body),
            ["queue", "order"] => s.store.reorder_queue(&body),
            ["execution-capacity"] => s.store.set_execution_capacity(&body),
            ["queue", id, "remove"] => s.store.cancel_queued(id, &body),
            ["queue", id, "launch"] | ["runs", id, "launch"] => {
                s.store.set_queue_launch(id, &body, &s.roles.read()?)
            }
            ["refinement-proposals", id, "decision"] => s.store.decide_refinement(id, &body),
            ["learning"] => Learning::new(&s.store, &s.roles).propose_operator(&body),
            ["learning", id, "decision"] => Learning::new(&s.store, &s.roles).decide(id, &body),
            ["principles"] => {
                crewroom_core::principles::Principles::beside_roles(s.roles.path()).save(&body)
            }
            ["principles", id, "delete"] => {
                crewroom_core::principles::Principles::beside_roles(s.roles.path()).delete(
                    id,
                    body["revision"]
                        .as_u64()
                        .ok_or_else(|| Error::Invalid("Principle revision is required.".into()))?,
                    &s.roles,
                )
            }
            ["roles"] => {
                if let Some(id) = body["draftProposalId"].as_str() {
                    let proposal = s.store.role_draft_proposal(id)?;
                    if proposal["roleDraft"]["id"] != body["id"] {
                        return Err(Error::Conflict("Draft role identity changed.".into()));
                    }
                    Ok(json!(s.roles.create_from_draft(&body, id)?))
                } else {
                    Ok(json!(s.roles.create(&body)?))
                }
            }
            ["role-drafts"] => Err(Error::Conflict(
                "Runner drafting is unavailable. Use the manual role form.".into(),
            )),
            ["roles", id] => Ok(json!(s.roles.update(id, &body)?)),
            ["repositories", id, "retire"] => s.store.retire_repository(id, &body),
            [
                "runs",
                _,
                "start" | "resume" | "pause" | "cancel" | "specialists" | "continue-later",
            ] => Err(Error::Conflict(
                "Rust runner integration is not available. No agent was started or stopped.".into(),
            )),
            _ => Err(Error::NotFound("API operation not found.".into())),
        }
    })
    .await?;
    state.changes.send_modify(|v| *v += 1);
    if let Some(e) = &state.execution {
        if let Some(id) = affected
            && let Some(w) = &e.workflow
        {
            w.notify(&id);
        }
        e.changed();
    }
    Ok(result)
}
async fn live(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(16 * 1024)
        .on_upgrade(move |socket| observe(socket, state))
}
async fn send_live(socket: &mut WebSocket, value: Value) -> bool {
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.send(Message::Text(value.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    )
}
async fn observe(mut socket: WebSocket, state: AppState) {
    use std::collections::BTreeMap;
    let mut changes = state.changes.subscribe();
    let mut subscriptions = BTreeMap::<String, (Option<Value>, u64)>::new();
    loop {
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(message))) => {
                    let value = serde_json::from_str::<Value>(&message).unwrap_or(Value::Null);
                    let topic = value["topic"].as_str().unwrap_or("");
                    let valid = value.as_object().is_some_and(|v| v.keys().all(|k| ["op","topic","after"].contains(&k.as_str()))) && !topic.is_empty() && topic.len()<=300 && value.get("after").is_none_or(|a| a.as_u64().is_some());
                    if !valid || !["subscribe","unsubscribe"].contains(&value["op"].as_str().unwrap_or("")) {
                        if !send_live(&mut socket,json!({"topic":"","kind":"error","error":"Invalid live request."})).await {break;} continue;
                    }
                    if value["op"]=="unsubscribe" {subscriptions.remove(topic);continue;}
                    if subscriptions.len()>=64 && !subscriptions.contains_key(topic) {if !send_live(&mut socket,json!({"topic":topic,"kind":"error","error":"Too many live subscriptions."})).await {break;}continue;}
                    subscriptions.insert(topic.into(),(None,value["after"].as_u64().unwrap_or(0)));
                },
                Some(Ok(Message::Ping(_)))|Some(Ok(Message::Pong(_)))=>continue,
                _=>break,
            },
            result=changes.changed()=>{if result.is_err() || *changes.borrow_and_update()==u64::MAX {break;}}
        }
        for (topic, (last, cursor)) in &mut subscriptions {
            let parts: Vec<_> = topic.split(':').collect();
            if let ["events", id] = parts.as_slice() {
                loop {
                    if state.stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let id = (*id).to_owned();
                    let after = *cursor;
                    let page =
                        operation(&state, move |s| s.store.agent_event_page(&id, after, 200)).await;
                    match page {
                        Ok(page) => {
                            *cursor = page["after"].as_u64().unwrap_or(*cursor);
                            if !array(&page["events"]).is_empty() && !send_live(&mut socket,json!({"topic":topic,"kind":"events","events":page["events"],"after":cursor})).await {return;}
                            if page["hasMore"] != true {
                                break;
                            }
                        }
                        Err(error) => {
                            if !send_live(
                                &mut socket,
                                json!({"topic":topic,"kind":"error","error":error.0.to_string()}),
                            )
                            .await
                            {
                                return;
                            }
                            break;
                        }
                    }
                }
                continue;
            }
            let result = match parts.as_slice() {
                ["workspace"] => workspace_value(&state).await,
                [kind, id] if ["run", "result"].contains(kind) => {
                    let path = format!(
                        "runs/{id}/{}",
                        if *kind == "run" { "runtime" } else { "result" }
                    );
                    read(
                        State(state.clone()),
                        Path(path),
                        axum::extract::Query(Default::default()),
                    )
                    .await
                    .map(|v| v.0)
                }
                _ => Err(Error::NotFound("Unknown live topic.".into()).into()),
            };
            let event = match result {
                Ok(data) => json!({"topic":topic,"kind":"state","data":data}),
                Err(error) => json!({"topic":topic,"kind":"error","error":error.0.to_string()}),
            };
            if last.as_ref() != Some(&event) {
                *last = Some(event.clone());
                if !send_live(&mut socket, event).await {
                    return;
                }
            }
        }
    }
}
