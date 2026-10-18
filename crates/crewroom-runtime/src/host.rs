//! One independent host, one separately owned OMP process, authenticated local
//! control, and externally routed tools. No internal runner team bus is used.
mod tools;
use crate::{
    canonical, conflict, durable_new,
    host_journal::{EventJournal, InputJournal, saved_inputs},
    host_protocol::{HOST_INPUT_LIMIT, ToolCatalog, parse_request, redact},
    observation::ObservationStream,
    pane::PaneFormatter,
    provider_gateway::ProviderGateway,
    read,
    runner::Runner,
};
use axum::{
    Router,
    body::Body,
    http::{Method, Request, Response},
};
use crewroom_core::{Result, serialized::stringify};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex as AsyncMutex, Notify, watch},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use url::Url;

type Reply = std::result::Result<Value, String>;
#[derive(Clone)]
struct Task {
    cancel: CancellationToken,
    result: watch::Receiver<Option<Reply>>,
    bytes: usize,
}
struct Activity {
    accepting: bool,
    tasks: HashMap<String, Task>,
    tool_ids: HashMap<String, String>,
    calls: HashMap<String, String>,
}
struct Finished {
    host: Weak<Host>,
    key: String,
    result: watch::Sender<Option<Reply>>,
    complete: bool,
}
impl Finished {
    fn finish(mut self, result: Reply) {
        self.result.send_replace(Some(result));
        self.complete = true;
    }
}
impl Drop for Finished {
    fn drop(&mut self) {
        if !self.complete {
            self.result
                .send_replace(Some(Err("Host operation was interrupted.".into())));
        }
        if let Some(host) = self.host.upgrade() {
            host.activity.lock().unwrap().tasks.remove(&self.key);
            if !self.complete {
                host.fatal();
            }
        }
    }
}
struct Host {
    config: Value,
    root: PathBuf,
    helper: PathBuf,
    origin: String,
    backend: Url,
    secrets: Vec<String>,
    catalog: ToolCatalog,
    provider: OnceLock<ProviderGateway>,
    rpc: OnceLock<Runner>,
    identity: Mutex<Value>,
    activity: Mutex<Activity>,
    events: Mutex<EventJournal>,
    inputs: Mutex<InputJournal>,
    observation: Mutex<ObservationStream>,
    pane: Mutex<PaneFormatter>,
    output: std::sync::mpsc::SyncSender<String>,
    changed: Notify,
    stopping: CancellationToken,
    server_stop: CancellationToken,
    ready: AtomicBool,
    setup_done: AtomicBool,
    fault: AtomicBool,
    cleanup_started: AtomicBool,
    close_lock: AsyncMutex<()>,
    requests: tokio::sync::Semaphore,
    observers: tokio::sync::Semaphore,
    client: reqwest::Client,
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| conflict("Missing host configuration field."))
}
fn private_dir(path: &Path) -> Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    canonical(path)?;
    let m = std::fs::metadata(path)?;
    if m.mode() & 0o077 != 0 {
        return Err(conflict("Host directory must be private."));
    }
    Ok(())
}
fn local_backend(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| conflict("Invalid controller URL."))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(conflict("Controller URL must name one loopback origin."));
    }
    Ok(url)
}
fn response(status: u16, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .body(Body::from(stringify(&value)))
        .unwrap()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn stamp() -> String {
    let clock = (now() / 1000) as libc::time_t;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: both pointers remain valid for this synchronous call.
    if unsafe { libc::localtime_r(&clock, tm.as_mut_ptr()) }.is_null() {
        return String::new();
    }
    // SAFETY: successful localtime_r initialized the value.
    let tm = unsafe { tm.assume_init() };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
fn publish(path: &Path, value: &Value) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    durable_new(&temp, stringify(value).as_bytes())?;
    // Exclusive final publication; an old endpoint is never silently replaced.
    std::fs::hard_link(&temp, path)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    std::fs::remove_file(temp)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

pub async fn run(config_path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(config_path)?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.len() > 16 * 1024 * 1024
    {
        return Err(conflict("Host configuration must be a private owned file."));
    }
    let config: Value = serde_json::from_slice(&read(config_path)?)?;
    let root = PathBuf::from(text(&config, "root")?);
    canonical(&root)?;
    private_dir(&root)?;
    if root
        .join(crate::runner::process_dir(&config["model"])?)
        .try_exists()?
        || root.join("endpoint.json").try_exists()?
        || !saved_inputs(&root)?.is_empty()
    {
        return Err(conflict("Attempt execution root was already used."));
    }
    let events = EventJournal::open(&root)?;
    if events.sequence() != 0 {
        return Err(conflict("Attempt event journal was already used."));
    }
    let inputs = InputJournal::open(&root)?;
    let token = text(&config, "token")?;
    let provider_token = text(&config, "providerToken")?;
    if token == provider_token || token.len() < 16 || provider_token.len() < 16 {
        return Err(conflict(
            "Host and provider tokens must be distinct private credentials.",
        ));
    }
    let backend = local_backend(text(&config, "backend")?)?;
    let catalog = ToolCatalog::new(&config)?;
    let helper = config["helper"].as_str().map(PathBuf::from).unwrap_or(
        std::env::current_exe()?
            .parent()
            .unwrap()
            .join("crewroom-process"),
    );
    if !helper.is_absolute() {
        return Err(conflict("Host helper must be absolute."));
    }
    let mut secrets: Vec<String> =
        serde_json::from_value(config.get("secrets").cloned().unwrap_or(json!([])))?;
    secrets.extend([token.to_owned(), provider_token.to_owned()]);
    if let Some(key) = config["upstreamProvider"]["apiKey"].as_str() {
        secrets.push(key.into());
    }
    let port = match config.get("port") {
        None => 0,
        Some(v) => v
            .as_u64()
            .filter(|p| *p <= 65535)
            .ok_or_else(|| conflict("Invalid host port."))? as u16,
    };
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let (output, receiver) = std::sync::mpsc::sync_channel::<String>(32);
    // Detached bounded observation writer: an unread tmux pipe cannot delay fsync,
    // tool cancellation or runtime shutdown. It owns no execution authority.
    let output_secrets = secrets.clone();
    std::thread::spawn(move || {
        let mut pending = String::new();
        let keep = output_secrets.iter().map(String::len).max().unwrap_or(0);
        for part in receiver {
            pending.push_str(&part);
            for secret in &output_secrets {
                if !secret.is_empty() {
                    pending = pending.replace(secret, "[redacted]");
                }
            }
            let mut end = pending.len().saturating_sub(keep);
            while !pending.is_char_boundary(end) {
                end -= 1;
            }
            if std::io::stdout()
                .write_all(&pending.as_bytes()[..end])
                .is_err()
            {
                return;
            }
            pending.drain(..end);
            let _ = std::io::stdout().flush();
        }
        let _ = std::io::stdout().write_all(pending.as_bytes());
    });
    let host = Arc::new(Host {
        config,
        root,
        helper,
        origin,
        backend,
        secrets,
        catalog,
        provider: OnceLock::new(),
        rpc: OnceLock::new(),
        identity: Mutex::new(Value::Null),
        activity: Mutex::new(Activity {
            accepting: false,
            tasks: HashMap::new(),
            tool_ids: HashMap::new(),
            calls: HashMap::new(),
        }),
        events: Mutex::new(events),
        inputs: Mutex::new(inputs),
        observation: Mutex::new(ObservationStream::default()),
        pane: Mutex::new(PaneFormatter::default()),
        output,
        changed: Notify::new(),
        stopping: CancellationToken::new(),
        server_stop: CancellationToken::new(),
        ready: AtomicBool::new(false),
        setup_done: AtomicBool::new(false),
        fault: AtomicBool::new(false),
        cleanup_started: AtomicBool::new(false),
        close_lock: AsyncMutex::new(()),
        requests: tokio::sync::Semaphore::new(64),
        observers: tokio::sync::Semaphore::new(32),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| conflict("Backend transport initialization failed."))?,
    });
    if crate::runner::kind(&host.config["model"])? == "omp" {
        let weak = Arc::downgrade(&host);
        let provider = ProviderGateway::new(
            &host.root,
            host.config["model"].clone(),
            host.config["upstreamProvider"].clone(),
            text(&host.config, "providerToken")?.into(),
            host.config["budgetRoot"].as_str().map(Path::new),
            move |v| {
                if let Some(h) = weak.upgrade() {
                    h.observe(v);
                }
            },
        )?;
        host.provider.set(provider).ok().unwrap();
    }
    let handler = host.clone();
    let stop = host.server_stop.clone();
    let mut server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(move |request: Request<Body>| {
                let host = handler.clone();
                async move { host.serve(request).await }
            }),
        )
        .with_graceful_shutdown(stop.cancelled_owned())
        .await
    });
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    // Settle setup before close. Dropping a pending spawn/negotiation must never
    // let a late helper start after this host has certified stopped ownership.
    let setup = {
        let setup = host.setup();
        tokio::pin!(setup);
        loop {
            tokio::select! {
                result=&mut setup=>break result,
                _=term.recv()=>host.stop_admission(),
                _=interrupt.recv()=>host.stop_admission(),
            }
        }
    };
    host.setup_done.store(true, Ordering::SeqCst);
    if let Err(error) = setup {
        host.observe(json!({"type":"host_setup_failed","reason":error.to_string()}));
        host.fault.store(true, Ordering::SeqCst);
        if host.close().await.is_ok() {
            host.server_stop.cancel();
        } else {
            host.ready.store(true, Ordering::SeqCst);
        }
    } else if host.stopping.is_cancelled() {
        // Control/event callbacks can request shutdown just after endpoint
        // publication, before setup_done becomes visible. Honor that retained
        // request even when setup itself returned success.
        host.initiate_close();
    }
    let mut server_finished = false;
    loop {
        tokio::select! {
            result=&mut server,if !server_finished=>{
                server_finished=true;
                if !result.is_ok_and(|r|r.is_ok()){host.fault.store(true,Ordering::SeqCst);}
                if host.close().await.is_ok(){break;}
            },
            _=host.server_stop.cancelled()=>{if timeout(Duration::from_secs(3),&mut server).await.is_err(){server.abort();}break;},
            _=term.recv()=>{host.initiate_close();},
            _=interrupt.recv()=>{host.initiate_close();},
        }
    }
    if host.fault.load(Ordering::SeqCst) {
        Err(conflict("Host stopped after a runtime failure."))
    } else {
        Ok(())
    }
}

impl Host {
    async fn setup(self: &Arc<Self>) -> Result<()> {
        let weak = Arc::downgrade(self);
        let event: crate::runner::Event = Arc::new(move |frame| {
            if let Some(host) = weak.upgrade() {
                host.frame(frame);
            }
        });
        let tools = self.catalog.tools();
        let tool_catalog_hash = crewroom_core::comparisons::hash(&json!(tools));
        let (runner, identity) = crate::runner::open(
            &self.helper,
            &self.root,
            &self.config,
            &self.origin,
            tools,
            event,
            &self.stopping,
        )
        .await?;
        self.rpc.set(runner).ok().unwrap();
        *self.identity.lock().unwrap() = identity.clone();
        {
            let mut activity = self.activity.lock().unwrap();
            if self.fault.load(Ordering::SeqCst) || self.stopping.is_cancelled() {
                return Err(conflict("Runner stopped during setup."));
            }
            activity.accepting = true;
        }
        let mut ready = identity;
        ready["type"] = json!("host_ready");
        ready["toolCatalogHash"] = json!(tool_catalog_hash);
        ready["toolCatalogHashKind"] = json!("canonical-json-sha256");
        self.record(ready)?;
        self.ready.store(true, Ordering::SeqCst);
        publish(
            &self.root.join("endpoint.json"),
            &json!({"port":self.origin.rsplit(':').next().unwrap().parse::<u16>().unwrap()}),
        )?;
        Ok(())
    }
    fn record(&self, event: Value) -> Result<()> {
        let safe = redact(&event, &self.secrets);
        if let Some(projected) = self.observation.lock().unwrap().accept(&safe, now()) {
            self.events.lock().unwrap().append(projected)?;
            self.changed.notify_waiters();
        }
        let output = self.pane.lock().unwrap().frame(&safe, &stamp());
        self.output(output);
        Ok(())
    }
    fn output(&self, mut output: String) {
        let mut end = output.len().min(8192);
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        if !output.is_empty() {
            let _ = self.output.try_send(output);
        }
    }
    fn observe(self: &Arc<Self>, event: Value) {
        if self.record(event).is_err() {
            self.fatal();
        }
    }
    fn fatal(self: &Arc<Self>) {
        self.fault.store(true, Ordering::SeqCst);
        self.initiate_close();
    }
    fn initiate_close(self: &Arc<Self>) {
        self.stop_admission();
        if !self.setup_done.load(Ordering::SeqCst) {
            return;
        }
        if self.cleanup_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let host = self.clone();
        tokio::spawn(async move {
            if host.close().await.is_ok() {
                host.server_stop.cancel();
            } else {
                host.cleanup_started.store(false, Ordering::SeqCst);
            }
        });
    }
    fn stop_admission(&self) {
        let mut activity = self.activity.lock().unwrap();
        activity.accepting = false;
        self.stopping.cancel();
        for task in activity.tasks.values() {
            task.cancel.cancel();
        }
        self.changed.notify_waiters();
        if let Some(provider) = self.provider.get() {
            provider.close();
        }
    }
    fn frame(self: &Arc<Self>, mut frame: Value) {
        if frame["type"] == "prompt_result"
            && ["completed", "error"].iter().any(|v| frame["status"] == *v)
            && let Some(failure) = self.provider.get().and_then(ProviderGateway::failure)
        {
            frame["status"] = json!("error");
            frame["error"] = json!({"message":frame["error"].get("message").cloned().unwrap_or_else(||json!(format!("Provider {}; no usable final result was confirmed.",failure["kind"].as_str().unwrap_or("failure")))),"kind":failure["kind"],"retryable":failure["retryable"],"provider":self.config["model"]["resolved"]["provider"],"model":self.config["model"]["resolved"]["id"]});
        }
        // A visible terminal event must never precede its durable input status.
        if frame["type"] == "prompt_result"
            && let (Some(id), Some(status)) = (frame["id"].as_str(), frame["status"].as_str())
        {
            let result = self.inputs.lock().unwrap().completed(id, status);
            if result.is_err() {
                self.fatal();
                return;
            }
        }
        self.observe(frame.clone());
        match frame["type"].as_str().unwrap_or("") {
            "host_tool_call" => self.tool(frame),
            "host_tool_cancel" => {
                if let Some(id) = frame["targetId"].as_str()
                    && let Some(task) = self
                        .activity
                        .lock()
                        .unwrap()
                        .tasks
                        .get(&format!("tool:{id}"))
                {
                    task.cancel.cancel();
                }
            }
            "process_exit" | "rpc_stream_closed" if !self.stopping.is_cancelled() => {
                self.fatal();
            }
            _ => (),
        }
    }
    fn task(
        self: &Arc<Self>,
        activity: &mut Activity,
        key: String,
        bytes: usize,
    ) -> Result<(Task, Finished)> {
        if !activity.accepting {
            return Err(conflict("Host is stopping."));
        }
        if activity.tasks.len() >= 64
            || activity
                .tasks
                .values()
                .map(|t| t.bytes)
                .sum::<usize>()
                .saturating_add(bytes)
                > 64 * 1024 * 1024
        {
            return Err(conflict("Host pending operation limit reached."));
        }
        let (tx, rx) = watch::channel(None);
        let task = Task {
            cancel: CancellationToken::new(),
            result: rx,
            bytes,
        };
        activity.tasks.insert(key.clone(), task.clone());
        Ok((
            task,
            Finished {
                host: Arc::downgrade(self),
                key,
                result: tx,
                complete: false,
            },
        ))
    }
    async fn wait_task(mut task: Task) -> Result<Value> {
        loop {
            if let Some(result) = task.result.borrow().clone() {
                return result.map_err(|e| conflict(&e));
            }
            task.result
                .changed()
                .await
                .map_err(|_| conflict("Host operation lost its outcome."))?;
        }
    }
    async fn input(self: &Arc<Self>, body: Value) -> Result<Value> {
        let id = text(&body, "id")?.to_owned();
        let message = body["message"]
            .as_str()
            .ok_or_else(|| conflict("Input identity and message are required."))?
            .to_owned();
        let key = format!("input:{id}");
        let task = {
            let mut activity = self.activity.lock().unwrap();
            self.inputs.lock().unwrap().register(&id, &message)?;
            if let Some(task) = activity.tasks.get(&key) {
                task.clone()
            } else {
                if !activity.accepting {
                    return Err(conflict("Host is stopping."));
                }
                match self.inputs.lock().unwrap().disposition(&id)? {
                    Some("accepted") => {
                        return Err(conflict("Input dispatch outcome is uncertain."));
                    }
                    Some(_) => return Ok(json!({"received":true})),
                    None => (),
                }
                let (task, finished) = self.task(&mut activity, key, message.len())?;
                let pending = task.clone();
                let host = self.clone();
                tokio::spawn(async move {
                    let result = host
                        .dispatch_input(&id, &message, body["mode"] == "steer", &pending.cancel)
                        .await
                        .map(|_| json!({"received":true}))
                        .map_err(|e| e.to_string());
                    finished.finish(result);
                });
                task
            }
        };
        Self::wait_task(task).await
    }
    async fn dispatch_input(
        self: &Arc<Self>,
        id: &str,
        message: &str,
        steer: bool,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let rpc = self
            .rpc
            .get()
            .ok_or_else(|| conflict("Runner is unavailable."))?;
        let state = tokio::select! {value=rpc.state()=>value?,_=cancel.cancelled()=>return Err(conflict("Host is stopping."))};
        if !state["isCompacting"].is_boolean() || !state["isStreaming"].is_boolean() {
            return Err(conflict("Runner busy state is invalid."));
        }
        {
            let activity = self.activity.lock().unwrap();
            if !activity.accepting || cancel.is_cancelled() {
                return Err(conflict("Host is stopping."));
            }
            if state["isCompacting"] == true || state["isStreaming"] == true && !steer {
                return Err(conflict("Agent is busy."));
            }
            let accepted = self.inputs.lock().unwrap().accept(id, message);
            match accepted {
                Ok(true) => (),
                Ok(false) => return Ok(()),
                Err(error) => {
                    drop(activity);
                    self.fatal();
                    return Err(error);
                }
            }
        }
        tokio::select! {value=rpc.submit(id,message,steer)=>{value?;},_=cancel.cancelled()=>return Err(conflict("Host stopped during input dispatch."))};
        let result = self.inputs.lock().unwrap().received(id);
        if result.is_err() {
            self.fatal();
        }
        result
    }
    async fn events(self: &Arc<Self>, after: f64, wait: Duration) -> Result<Value> {
        let _observer = self
            .observers
            .try_acquire()
            .map_err(|_| conflict("Host observers are busy."))?;
        let notified = self.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let page = self.event_page(after)?;
        if !page.is_empty() || wait.is_zero() || self.stopping.is_cancelled() {
            return Ok(json!(page));
        }
        let _ = timeout(wait, notified).await;
        Ok(json!(self.event_page(after)?))
    }
    fn event_page(self: &Arc<Self>, after: f64) -> Result<Vec<Value>> {
        let result = self.events.lock().unwrap().page(after);
        if result.is_err() {
            self.fatal();
        }
        result
    }
    fn check_journals(self: &Arc<Self>) -> Result<()> {
        let result = self
            .inputs
            .lock()
            .unwrap()
            .disposition("")
            .map(|_| ())
            .and_then(|_| self.events.lock().unwrap().page(f64::INFINITY).map(|_| ()));
        if result.is_err() {
            self.fatal();
        }
        result
    }
    async fn serve(self: Arc<Self>, request: Request<Body>) -> Response<Body> {
        if !self.ready.load(Ordering::SeqCst) {
            return response(503, json!({"error":"Host is starting."}));
        }
        if request.uri().path().starts_with("/provider/") {
            let Some(provider) = self.provider.get() else {
                return response(404, json!({}));
            };
            return match provider.serve(request).await {
                Ok(r) => r,
                Err(e) => response(
                    409,
                    json!({"error":redact(&json!(e.to_string()),&self.secrets)}),
                ),
            };
        }
        if request.method() != Method::POST
            || request.uri().path() != "/"
            || request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                != Some(&format!(
                    "Bearer {}",
                    self.config["token"].as_str().unwrap()
                ))
            || request
                .headers()
                .get("origin")
                .is_some_and(|v| v.to_str().ok() != Some(&self.origin))
        {
            return response(403, json!({}));
        }
        let Ok(_request) = self.requests.try_acquire() else {
            return response(429, json!({"error":"Host control is busy."}));
        };
        let result=async {
            let mut stream=request.into_body().into_data_stream();let mut bytes=Vec::new();
            timeout(Duration::from_secs(10),async {
                while let Some(chunk)=stream.next().await {let chunk=chunk.map_err(|_|conflict("Host request was interrupted."))?;if bytes.len()+chunk.len()>HOST_INPUT_LIMIT*3{return Err(conflict("Request too large."));}bytes.extend_from_slice(&chunk);}Ok::<_,crewroom_core::Error>(())
            }).await.map_err(|_|conflict("Host request body timed out."))??;
            let body=parse_request(&bytes)?;
            if !["identity","close"].iter().any(|op|body["op"]==*op) {
                self.check_journals()?;
            }
            match body["op"].as_str().unwrap_or("") {
                "identity"=>Ok(self.identity.lock().unwrap().clone()),
                "state"=>Ok(redact(&self.rpc.get().ok_or_else(||conflict("Runner is unavailable."))?.state().await?,&self.secrets)),
                "events"=>self.events(body["after"].as_f64().unwrap_or(0.0),Duration::from_millis(body["wait"].as_f64().unwrap_or(0.0).clamp(0.0,25000.0) as u64)).await,
                "input_status"=>{let id=body["id"].as_str().unwrap_or("");let activity=self.activity.lock().unwrap();let inputs=self.inputs.lock().unwrap();Ok(json!({"disposition":inputs.disposition(id)?.unwrap_or(if activity.tasks.contains_key(&format!("input:{id}")){"admitting"}else{"absent"})}))},
                "input"=>self.input(body).await,
                "close"=>{let host=self.clone();tokio::spawn(async move {host.close().await?;host.server_stop.cancel();Ok::<_,crewroom_core::Error>(())}).await.map_err(|_|conflict("Host shutdown task failed."))??;Ok(json!({"stopped":true}))},
                _=>Err(conflict("Unknown host operation.")),
            }
        }.await;
        match result {
            Ok(value) => response(200, value),
            Err(error) => {
                // Recheck after awaits too: a changed/failed journal must never
                // become an ordinary retryable admission error.
                let _ = self.check_journals();
                response(
                    409,
                    json!({"error":redact(&json!(error.to_string()),&self.secrets)}),
                )
            }
        }
    }
    async fn close(self: &Arc<Self>) -> Result<()> {
        self.stop_admission();
        let _guard = self.close_lock.lock().await;
        if let Some(rpc) = self.rpc.get() {
            let _ = timeout(Duration::from_millis(500), rpc.interrupt()).await;
        }
        let tasks: Vec<_> = self
            .activity
            .lock()
            .unwrap()
            .tasks
            .values()
            .cloned()
            .collect();
        let drained = timeout(Duration::from_secs(40), async {
            for task in tasks {
                let _ = Self::wait_task(task).await;
            }
        })
        .await
        .is_ok();
        let stopped_tools = timeout(Duration::from_secs(30), self.stop_tools()).await;
        let stopped_omp = if let Some(rpc) = self.rpc.get() {
            rpc.close().await
        } else if self
            .root
            .join(crate::runner::process_dir(&self.config["model"])?)
            .try_exists()?
        {
            let ownership = crate::stop_owned(
                &self
                    .root
                    .join(crate::runner::process_dir(&self.config["model"])?),
            )
            .await;
            if ownership.state == "stopped" {
                Ok(())
            } else {
                Err(conflict("Runner setup ownership remains uncertain."))
            }
        } else {
            Ok(())
        };
        if !drained || !stopped_tools.is_ok_and(|value| value.is_ok()) || stopped_omp.is_err() {
            let reason = "Host tools, pending preparations or runner ownership remain unresolved.";
            let _ = self.record(json!({"type":"ownership_unresolved","reason":reason}));
            return Err(conflict(reason));
        }
        Ok(())
    }
}
