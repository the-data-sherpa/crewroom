//! Durable, independently reconcilable Docker command ownership over a local Unix socket.
use crate::{conflict, durable_new, flock, read};
use crewroom_core::{Error, Result, serialized::stringify};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::{Instant, sleep, timeout};
use tokio_util::sync::CancellationToken;
const LIMIT: usize = 4 * 1024 * 1024;
const ENGINE_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Debug)]
struct EngineError(Option<u16>);
impl EngineError {
    fn local() -> Self {
        Self(None)
    }
    fn error(self) -> Error {
        conflict(&format!(
            "Command engine response is uncertain (HTTP {:?}).",
            self.0
        ))
    }
}
#[derive(Clone)]
pub struct ContainerCommand {
    pub root: PathBuf,
    client: reqwest::Client,
}
fn stopped(id: Option<&Value>, exit: Option<&Value>) -> Value {
    let mut v = json!({"state":"stopped"});
    if let Some(id) = id {
        v["id"] = id.clone();
    }
    if let Some(exit) = exit {
        v["exitCode"] = exit.clone();
    }
    v
}
fn uncertain(reason: &str) -> Value {
    json!({"state":"uncertain","reason":reason})
}
pub(crate) fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn exit_code(v: &Value) -> Option<i64> {
    v.as_f64()
        .filter(|n| {
            n.is_finite() && n.fract() == 0.0 && *n >= i32::MIN as f64 && *n <= i32::MAX as f64
        })
        .map(|n| n as i64)
}
fn digest(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_spec(spec: &Value) -> Result<()> {
    if !spec.is_object()
        || !spec["image"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .is_some_and(digest)
        || !["workspace", "command"]
            .iter()
            .all(|k| spec[k].as_str().is_some_and(|s| Path::new(s).is_absolute()))
        || !spec["args"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(conflict(
            "Commands require a pinned local image, an absolute workspace, and an absolute executable.",
        ));
    }
    for (key, min, max, message) in [
        ("timeoutMs", 100., 3_600_000., "Invalid command timeout."),
        (
            "memoryMiB",
            128.,
            8192.,
            "Command memory limit must be an integer from 128 to 8192 MiB.",
        ),
    ] {
        if let Some(v) = spec.get(key)
            && crate::provider_input::count(v).is_none_or(|n| n < min || n > max)
        {
            return Err(conflict(message));
        }
    }
    for key in ["tmpExecutable", "readOnlyWorkspace"] {
        if let Some(v) = spec.get(key)
            && !v.is_boolean()
        {
            return Err(conflict("Invalid command mount policy."));
        }
    }
    Ok(())
}
fn temporary(spec: &Value) -> String {
    format!(
        "rw,nosuid,{}size=67108864",
        if spec["tmpExecutable"] == true {
            "exec,"
        } else {
            ""
        }
    )
}
fn canonical_root(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        return Err(conflict("Container ownership directory must be absolute."));
    }
    let mut expected = PathBuf::new();
    for c in root.components() {
        match c {
            Component::ParentDir => {
                expected.pop();
            }
            Component::CurDir => (),
            _ => expected.push(c.as_os_str()),
        }
    }
    #[cfg(target_os = "macos")]
    for alias in ["/var", "/tmp"] {
        if let Ok(suffix) = expected.strip_prefix(alias) {
            expected = std::fs::canonicalize(alias)?.join(suffix);
            break;
        }
    }
    if std::fs::canonicalize(root)? != expected {
        return Err(conflict("Container ownership path contains a symlink."));
    }
    Ok(())
}
impl ContainerCommand {
    pub fn new(root: PathBuf, socket: &Path) -> Result<Self> {
        if !socket.is_absolute() {
            return Err(conflict("Use a local Unix socket for command containment."));
        }
        let client = reqwest::Client::builder()
            .unix_socket(socket)
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| conflict("Command engine initialization failed."))?;
        Ok(Self { root, client })
    }
    fn path(&self) -> PathBuf {
        self.root.join("container.json")
    }
    fn read(&self) -> Result<Value> {
        canonical_root(&self.root)?;
        let v: Value = serde_json::from_slice(&read(&self.path())?)?;
        if v["root"].as_str() != self.root.to_str() {
            return Err(conflict(
                "Container ownership directory changed or is unrecorded.",
            ));
        }
        let token = v["token"]
            .as_str()
            .ok_or_else(|| conflict("Missing container ownership token."))?;
        if uuid::Uuid::parse_str(token).is_err()
            || v["name"] != format!("crewroom-{token}")
            || v.get("id")
                .is_some_and(|id| !id.as_str().is_some_and(digest))
        {
            return Err(conflict("Invalid container ownership identity."));
        }
        valid_spec(&v["spec"])?;
        let invalid =
            || conflict("Container command journal is invalid; retain it for inspection.");
        for key in [
            "startRequested",
            "startConfirmed",
            "timedOut",
            "stopRequested",
            "removed",
            "removalRequested",
        ] {
            if v.get(key).is_some_and(|value| !value.is_boolean()) {
                return Err(invalid());
            }
        }
        if v.get("startedAt")
            .is_some_and(|value| crate::provider_input::count(value).is_none())
            || v["startRequested"] == true
                && (v.get("id").is_none() || v.get("startedAt").is_none())
            || v["startConfirmed"] == true && v["startRequested"] != true
            || v["timedOut"] == true && v["startRequested"] != true
            || v.get("completed").is_some_and(|value| {
                !value.is_object()
                    || exit_code(&value["exitCode"]).is_none()
                    || v["startConfirmed"] != true
            })
            || v["removalRequested"] == true && (!v["receipt"].is_object() || v.get("id").is_none())
            || v["removed"] == true && v["removalRequested"] != true
        {
            return Err(invalid());
        }
        if let Some(receipt) = v.get("receipt")
            && (!receipt.is_object()
                || receipt["spec"] != v["spec"]
                || receipt["containerId"] != v["id"]
                || receipt
                    .get("user")
                    .is_some_and(|user| Some(user) != v.get("user"))
                || v.get("id").is_none()
                || receipt["started"].as_bool() != Some(v["startConfirmed"] == true)
                || receipt["timedOut"].as_bool() != Some(v["timedOut"] == true)
                || receipt["completed"].as_bool()
                    != Some(
                        v["completed"].is_object()
                            && v["completed"]["exitCode"] == receipt["exitCode"]
                            && v["timedOut"] != true,
                    )
                || exit_code(&receipt["exitCode"]).is_none()
                || !receipt["output"].is_string()
                || !receipt["platform"].is_string())
        {
            return Err(invalid());
        }
        Ok(v)
    }
    fn open_lock(&self, name: &str) -> Result<File> {
        canonical_root(&self.root)?;
        Ok(OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.root.join(name))?)
    }
    async fn lock(&self, name: &str, wait: Duration) -> Result<File> {
        let file = self.open_lock(name)?;
        let until = Instant::now() + wait;
        loop {
            if flock(&file, libc::LOCK_EX | libc::LOCK_NB)? {
                return Ok(file);
            }
            if Instant::now() >= until {
                return Err(conflict("Container operation is still in progress."));
            }
            sleep(Duration::from_millis(5)).await;
        }
    }
    fn launch_active(&self) -> Result<bool> {
        Ok(!flock(
            &self.open_lock("launch.lock")?,
            libc::LOCK_SH | libc::LOCK_NB,
        )?)
    }
    fn write(&self, value: &Value, new: bool) -> Result<()> {
        canonical_root(&self.root)?;
        let tmp = self
            .root
            .join(format!("container.{}.tmp", uuid::Uuid::new_v4()));
        durable_new(&tmp, stringify(value).as_bytes())?;
        if new {
            std::fs::hard_link(&tmp, self.path())?;
            std::fs::remove_file(&tmp)?;
        } else {
            std::fs::rename(&tmp, self.path())?;
        }
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    async fn change<T>(
        &self,
        expected: &Value,
        edit: impl FnOnce(&mut Value) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.lock("journal.lock", ENGINE_TIMEOUT).await?;
        let mut value = self.read()?;
        if ["root", "token", "name", "spec"]
            .iter()
            .any(|key| value[key] != expected[key])
            || expected.get("id").is_some_and(|id| value["id"] != *id)
        {
            return Err(conflict("Container ownership changed during an operation."));
        }
        let result = edit(&mut value)?;
        self.write(&value, false)?;
        Ok(result)
    }
    async fn update(&self, expected: &Value, changes: Value) -> Result<()> {
        self.change(expected, |v| {
            for (k, value) in changes.as_object().unwrap() {
                if v["receipt"].is_object()
                    && ["stopRequested", "timedOut", "completed"].contains(&k.as_str())
                {
                    continue;
                }
                if k == "id" && !v["id"].is_null() && v["id"] != *value {
                    return Err(conflict("Container identity changed."));
                }
                v[k] = value.clone();
            }
            Ok(())
        })
        .await
    }
    async fn raw(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> std::result::Result<Vec<u8>, EngineError> {
        timeout(ENGINE_TIMEOUT, async {
            let mut req = self
                .client
                .request(method.clone(), format!("http://localhost{path}"))
                .header("Content-Type", "application/json");
            if let Some(body) = body {
                req = req.body(stringify(&body));
            }
            let response = req.send().await.map_err(|_| EngineError::local())?;
            let status = response.status().as_u16();
            let admitted = match method {
                reqwest::Method::GET => status == 200,
                reqwest::Method::POST if path.starts_with("/containers/create?") => status == 201,
                reqwest::Method::POST if path.ends_with("/start") => [204, 304].contains(&status),
                _ => status == 204,
            };
            if !admitted {
                return Err(EngineError(Some(status)));
            }
            let mut result = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| EngineError::local())?;
                if result.len() + chunk.len() > LIMIT {
                    return Err(EngineError::local());
                }
                result.extend_from_slice(&chunk);
            }
            Ok(result)
        })
        .await
        .unwrap_or_else(|_| Err(EngineError::local()))
    }
    async fn api(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> std::result::Result<Value, EngineError> {
        let bytes = self.raw(method, path, body).await?;
        if bytes.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_slice(&bytes).map_err(|_| EngineError::local())
        }
    }
    fn reference(intent: &Value) -> &str {
        intent
            .get("id")
            .unwrap_or(&intent["name"])
            .as_str()
            .unwrap()
    }
    async fn remote(&self, intent: &Value) -> std::result::Result<Value, EngineError> {
        let found = self
            .api(
                reqwest::Method::GET,
                &format!("/containers/{}/json", Self::reference(intent)),
                None,
            )
            .await?;
        let spec = &intent["spec"];
        let host = &found["HostConfig"];
        let mounts = host["Mounts"].as_array();
        let actual_mounts = found["Mounts"].as_array();
        if found["Config"]["Labels"]["io.crewroom.command"] != intent["token"]
            || intent.get("id").is_some_and(|id| *id != found["Id"])
            || !found["Id"].as_str().is_some_and(digest)
            || found["Image"] != spec["image"]
            || found["Config"]["NetworkDisabled"] != true
            || found["Config"]["WorkingDir"] != "/workspace"
            || intent
                .get("user")
                .is_some_and(|user| *user != found["Config"]["User"])
            || found["Config"]["Cmd"]
                != json!(
                    std::iter::once(spec["command"].clone())
                        .chain(spec["args"].as_array().unwrap().iter().cloned())
                        .collect::<Vec<_>>()
                )
            || [
                "Binds",
                "VolumesFrom",
                "Devices",
                "DeviceRequests",
                "CapAdd",
            ]
            .iter()
            .any(|k| !host[k].is_null() && !host[k].as_array().is_some_and(Vec::is_empty))
            || ["PidMode", "UsernsMode", "UTSMode"]
                .iter()
                .any(|k| host[k] != "")
            || host["IpcMode"] != "private"
            || actual_mounts.is_none_or(|m| {
                m.len() != 1
                    || m[0]["Type"] != "bind"
                    || m[0]["Source"] != spec["workspace"]
                    || m[0]["Destination"] != "/workspace"
                    || m[0]["RW"] != json!(spec["readOnlyWorkspace"] != true)
            })
            || host["NetworkMode"] != "none"
            || host["Privileged"] != false
            || host["ReadonlyRootfs"] != true
            || host["Memory"].as_f64()
                != Some(spec["memoryMiB"].as_f64().unwrap_or(1024.) * 1024. * 1024.)
            || host["Tmpfs"]["/tmp"] != temporary(spec)
            || host["CapDrop"] != json!(["ALL"])
            || host["SecurityOpt"] != json!(["no-new-privileges:true"])
            || host["PidsLimit"].as_f64() != Some(256.)
            || host["NanoCpus"].as_f64() != Some(2_000_000_000.)
            || host["RestartPolicy"]["Name"] != "no"
            || mounts.is_none_or(|m| {
                m.len() != 1
                    || m[0]["Type"] != "bind"
                    || m[0]["Source"] != spec["workspace"]
                    || m[0]["Target"] != "/workspace"
                    || m[0].get("ReadOnly").is_some_and(|v| !v.is_boolean())
                    || m[0]["ReadOnly"].as_bool().unwrap_or(false)
                        != (spec["readOnlyWorkspace"] == true)
            })
            || found["Config"]["Tty"] != false
        {
            return Err(EngineError::local());
        }
        if intent.get("id").is_none() {
            self.update(intent, json!({"id":found["Id"]}))
                .await
                .map_err(|_| EngineError::local())?;
        }
        Ok(found)
    }
    pub async fn start(&self, mut spec: Value) -> Result<()> {
        valid_spec(&spec)?;
        if self.path().exists() {
            return Err(conflict(
                "Command already has a durable intent; reconcile it instead of restarting.",
            ));
        }
        let mut ancestor = self.root.as_path();
        while !ancestor.exists() {
            ancestor = ancestor
                .parent()
                .ok_or_else(|| conflict("Missing container root ancestor."))?;
        }
        canonical_root(ancestor)?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)?;
        canonical_root(&self.root)?;
        let lease=self.lock("launch.lock",Duration::ZERO).await.map_err(|_|conflict("Command already has a durable intent or pending admission; reconcile it instead of restarting."))?;
        spec["workspace"] = json!(std::fs::canonicalize(spec["workspace"].as_str().unwrap())?);
        let token = uuid::Uuid::new_v4().to_string();
        // CapDrop=ALL removes root's DAC bypass. Run as the local owner so private
        // 0700 candidate exports remain readable without weakening host modes.
        // SAFETY: these process identity queries have no preconditions.
        let user = unsafe { format!("{}:{}", libc::geteuid(), libc::getegid()) };
        let intent = json!({"root":self.root,"token":token,"name":format!("crewroom-{token}"),"spec":spec,"user":user});
        {
            let _lock = self.lock("journal.lock", ENGINE_TIMEOUT).await?;
            if std::fs::symlink_metadata(self.path()).is_ok() {
                return Err(conflict(
                    "Command already has a durable intent; reconcile it instead of restarting.",
                ));
            }
            self.write(&intent, true)?;
        }
        // Dropping the caller's future cannot cancel an admitted launch between engine actions.
        let owned = self.clone();
        tokio::spawn(async move {
            let _lease = lease;
            owned.launch().await
        })
        .await
        .map_err(|_| conflict("Command launch outcome is uncertain."))?
    }
    async fn launch(&self) -> Result<()> {
        let intent = self.read()?;
        let spec = &intent["spec"];
        let mut configuration = json!({
        "Image":spec["image"],"Entrypoint":[],"Cmd":std::iter::once(spec["command"].clone()).chain(spec["args"].as_array().unwrap().iter().cloned()).collect::<Vec<_>>(),"WorkingDir":"/workspace","Env":["HOME=/tmp"],"Labels":{"io.crewroom.command":intent["token"]},"NetworkDisabled":true,"Tty":false,
        "HostConfig":{"NetworkMode":"none","CapDrop":["ALL"],"SecurityOpt":["no-new-privileges:true"],"ReadonlyRootfs":true,"PidsLimit":256,"Memory":spec["memoryMiB"].as_f64().unwrap_or(1024.)*1024.*1024.,"NanoCpus":2_000_000_000,"RestartPolicy":{"Name":"no"},"Mounts":[{"Type":"bind","Source":spec["workspace"],"Target":"/workspace","ReadOnly":spec["readOnlyWorkspace"]==true}],"Tmpfs":{"/tmp":temporary(spec)}}
             });
        // Saved pre-MH6 intents retain their original image-user contract.
        if let Some(user) = intent.get("user") {
            configuration["User"] = user.clone();
        }
        let created = self
            .api(
                reqwest::Method::POST,
                &format!(
                    "/containers/create?name={}",
                    intent["name"].as_str().unwrap()
                ),
                Some(configuration),
            )
            .await
            .map_err(EngineError::error)?;
        if !created["Id"].as_str().is_some_and(digest) {
            return Err(conflict("Command engine returned an invalid container ID."));
        }
        self.update(&intent, json!({"id":created["Id"]})).await?;
        let launch = self
            .change(&intent, |v| {
                if v["stopRequested"] == true {
                    return Ok(false);
                }
                v["startRequested"] = json!(true);
                v["startedAt"] = json!(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|_| conflict("Command clock is unavailable."))?
                        .as_millis() as u64
                );
                Ok(true)
            })
            .await?;
        if !launch {
            return Ok(());
        }
        self.api(
            reqwest::Method::POST,
            &format!("/containers/{}/start", created["Id"].as_str().unwrap()),
            None,
        )
        .await
        .map_err(EngineError::error)?;
        self.update(&intent, json!({"startConfirmed":true})).await
    }
    async fn inspect_inner(&self) -> Result<Value> {
        if !present(&self.root)? {
            return Ok(stopped(None, None));
        }
        canonical_root(&self.root)?;
        if self.launch_active()? {
            return Ok(uncertain("Command admission is still in progress."));
        }
        if !present(&self.path())? {
            return Ok(stopped(None, None));
        }
        let intent = self.read()?;
        if intent["removed"] == true {
            return Ok(stopped(intent.get("id"), intent["receipt"].get("exitCode")));
        }
        let found = match self.remote(&intent).await {
            Ok(found) => found,
            Err(e) => {
                if e.0 == Some(404)
                    && intent["removalRequested"] == true
                    && intent["receipt"].is_object()
                    && intent.get("id").is_some()
                {
                    self.update(&intent, json!({"removed":true})).await?;
                    return Ok(stopped(intent.get("id"), intent["receipt"].get("exitCode")));
                }
                return Err(e.error());
            }
        };
        if !["Running", "Restarting"]
            .iter()
            .all(|key| found["State"][key].is_boolean())
        {
            return Ok(uncertain("Command engine process state is invalid."));
        }
        if found["State"]["Running"] == true || found["State"]["Restarting"] == true {
            return Ok(json!({"state":"running","id":found["Id"]}));
        }
        if intent["startRequested"] == true
            && intent["startConfirmed"] != true
            && found["State"]["Status"] == "created"
        {
            return Ok(
                json!({"state":"uncertain","id":found["Id"],"reason":"Command start outcome is unresolved."}),
            );
        }
        if found["State"]["Pid"].as_f64() != Some(0.)
            || !["created", "exited", "dead"]
                .iter()
                .any(|s| found["State"]["Status"] == *s)
            || exit_code(&found["State"]["ExitCode"]).is_none()
        {
            return Ok(
                json!({"state":"uncertain","id":found["Id"],"reason":"Container termination is unconfirmed."}),
            );
        }
        Ok(stopped(found.get("Id"), found["State"].get("ExitCode")))
    }
    pub async fn inspect(&self) -> Value {
        self.inspect_inner()
            .await
            .unwrap_or_else(|_| uncertain("Command engine ownership cannot be confirmed."))
    }
    pub async fn stop(&self) -> Result<Value> {
        if !present(&self.root)? || !present(&self.path())? {
            return Ok(self.inspect().await);
        }
        let intent = match self.read() {
            Ok(v) => v,
            Err(_) => {
                return Ok(uncertain(
                    "Container ownership directory cannot be confirmed.",
                ));
            }
        };
        if intent["removed"] == true {
            return Ok(self.inspect().await);
        }
        if !self.launch_active()? {
            let state = self.inspect().await;
            let current = self.read()?;
            if state["state"] == "stopped"
                && current["startConfirmed"] == true
                && current["stopRequested"] != true
                && state.get("exitCode").is_some()
            {
                self.update(
                    &current,
                    json!({"completed":{"exitCode":state["exitCode"]}}),
                )
                .await?;
                return Ok(state);
            }
        }
        self.update(&intent, json!({"stopRequested":true})).await?;
        let lease = match self.lock("launch.lock", Duration::from_secs(15)).await {
            Ok(lease) => lease,
            Err(_) => return Ok(uncertain("Command admission has not settled.")),
        };
        drop(lease);
        let state = self.inspect().await;
        if state["state"] != "running" {
            return Ok(state);
        }
        let _ = self
            .api(
                reqwest::Method::POST,
                &format!(
                    "/containers/{}/kill?signal=SIGKILL",
                    state["id"].as_str().unwrap()
                ),
                None,
            )
            .await;
        Ok(self.inspect().await)
    }
    pub async fn wait(&self) -> Result<Value> {
        loop {
            let state = self.inspect().await;
            let intent = self.read()?;
            if state["state"] != "running" {
                if state["state"] == "stopped"
                    && intent["startConfirmed"] == true
                    && intent["stopRequested"] != true
                    && state.get("exitCode").is_some()
                {
                    self.update(&intent, json!({"completed":{"exitCode":state["exitCode"]}}))
                        .await?;
                }
                return Ok(state);
            }
            if let (Some(limit), Some(start)) = (
                intent["spec"]["timeoutMs"].as_f64(),
                intent["startedAt"].as_f64(),
            ) && SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| conflict("Command clock is unavailable."))?
                .as_millis() as f64
                >= start + limit
            {
                self.update(&intent, json!({"timedOut":true})).await?;
                return self.stop().await;
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
    pub async fn logs(&self) -> Result<String> {
        let intent = self.read()?;
        if let Some(output) = intent["receipt"]["output"].as_str() {
            return Ok(output.into());
        }
        self.remote(&intent).await.map_err(EngineError::error)?;
        let bytes = self
            .raw(
                reqwest::Method::GET,
                &format!(
                    "/containers/{}/logs?stdout=true&stderr=true",
                    Self::reference(&intent)
                ),
                None,
            )
            .await
            .map_err(EngineError::error)?;
        let mut frames = LogFrames::default();
        let mut output = Vec::new();
        for frame in frames.push(&bytes)? {
            output.extend_from_slice(&frame);
        }
        frames.finish()?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }
    pub async fn receipt(&self) -> Result<Value> {
        let saved = self.read()?;
        if saved["receipt"].is_object() {
            return Ok(saved["receipt"].clone());
        }
        let state = self.inspect().await;
        if state["state"] != "stopped" || state.get("exitCode").is_none() {
            return Err(conflict("Command completion remains unconfirmed."));
        }
        let mut intent = self.read()?;
        if !intent["completed"].is_object()
            && intent["startConfirmed"] == true
            && intent["stopRequested"] != true
            && intent["timedOut"] != true
        {
            self.update(&intent, json!({"completed":{"exitCode":state["exitCode"]}}))
                .await?;
            intent = self.read()?;
        }
        let image = self
            .api(
                reqwest::Method::GET,
                &format!("/images/{}/json", intent["spec"]["image"].as_str().unwrap()),
                None,
            )
            .await
            .map_err(EngineError::error)?;
        let os = image["Os"]
            .as_str()
            .ok_or_else(|| conflict("Command image platform is unavailable."))?;
        let arch = image["Architecture"]
            .as_str()
            .ok_or_else(|| conflict("Command image platform is unavailable."))?;
        let output = self.logs().await?;
        if self.inspect().await != state {
            return Err(conflict(
                "Command state changed while final output was read.",
            ));
        }
        self.change(&intent, |v| {
            if !v["receipt"].is_object() {
                v["receipt"] = json!({"spec":v["spec"],"started":v["startConfirmed"]==true,"completed":v["completed"].is_object()&&v["completed"]["exitCode"]==state["exitCode"]&&v["timedOut"]!=true,"timedOut":v["timedOut"]==true,"exitCode":state["exitCode"],"output":output,"platform":format!("{os}/{arch}"),"containerId":state["id"]});
                if let Some(user) = v.get("user").cloned() {
                    v["receipt"]["user"] = user;
                }
            }
            Ok(v["receipt"].clone())
        })
        .await
    }
    pub async fn follow(
        &self,
        mut on_chunk: impl FnMut(&[u8]) -> Result<()>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let intent = self.read()?;
        self.remote(&intent).await.map_err(EngineError::error)?;
        let response = tokio::select! {_=cancel.cancelled()=>return Ok(()),v=timeout(ENGINE_TIMEOUT,self.client.get(format!("http://localhost/containers/{}/logs?stdout=true&stderr=true&follow=true",Self::reference(&intent))).send())=>v.map_err(|_|conflict("Command observation timed out."))?.map_err(|_|conflict("Command observation was interrupted."))?};
        if !response.status().is_success() {
            return Err(conflict("Command observation is unavailable."));
        }
        let mut stream = response.bytes_stream();
        let mut frames = LogFrames::default();
        loop {
            let next = tokio::select! {_=cancel.cancelled()=>return Ok(()),v=stream.next()=>v};
            let Some(chunk) = next else {
                return frames.finish();
            };
            let chunk = chunk.map_err(|_| conflict("Command observation was interrupted."))?;
            for part in chunk.chunks(64 * 1024) {
                for frame in frames.push(part)? {
                    on_chunk(&frame)?;
                }
            }
        }
    }
    pub async fn remove(&self) -> Result<()> {
        let state = self.inspect().await;
        if state["state"] != "stopped" || state.get("id").is_none() {
            return Ok(());
        }
        let intent = self.read()?;
        if intent["removed"] == true {
            return Ok(());
        }
        self.receipt().await?;
        self.update(&intent, json!({"removalRequested":true}))
            .await?;
        match self
            .api(
                reqwest::Method::DELETE,
                &format!("/containers/{}", state["id"].as_str().unwrap()),
                None,
            )
            .await
        {
            Ok(_) => (),
            Err(e) if e.0 == Some(404) => (),
            Err(e) => return Err(e.error()),
        };
        self.update(&intent, json!({"removed":true})).await
    }
}
/// Bounded Docker non-TTY multiplex framing. Missing headers/payloads cannot certify final output.
#[derive(Default)]
pub struct LogFrames {
    pending: Vec<u8>,
}
impl LogFrames {
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut output = Vec::new();
        while !bytes.is_empty() {
            let target = if self.pending.len() < 8 {
                8
            } else {
                let n = u32::from_be_bytes(self.pending[4..8].try_into().unwrap()) as usize;
                if n > LIMIT || self.pending[0] > 2 || self.pending[1..4] != [0, 0, 0] {
                    return Err(conflict("Invalid command output frame."));
                }
                8 + n
            };
            let take = (target - self.pending.len()).min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == target && target >= 8 && self.pending.len() >= 8 {
                let size = u32::from_be_bytes(self.pending[4..8].try_into().unwrap()) as usize;
                if size > LIMIT || self.pending[0] > 2 || self.pending[1..4] != [0, 0, 0] {
                    return Err(conflict("Invalid command output frame."));
                }
                if self.pending.len() == 8 + size {
                    output.push(self.pending.split_off(8));
                    self.pending.clear();
                }
            }
        }
        Ok(output)
    }
    pub fn finish(&self) -> Result<()> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(conflict("Incomplete command output."))
        }
    }
}
