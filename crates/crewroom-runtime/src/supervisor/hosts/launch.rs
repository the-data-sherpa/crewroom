use super::*;
use crate::{
    CheckedSpec, CommandSpec, describe_checked, durable_new, prepare_checked, validate_launch,
};
use std::{collections::BTreeMap, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    time::{Instant, sleep, timeout},
};

impl<S: StoreAccess> HostManager<S> {
    /// The workflow layer supplies snapshotted model/tool/prompt configuration.
    /// Process paths, callback origin and host credentials are bound here.
    pub fn launch(&self, attempt: &str, revision: u64, configuration: Value) -> Result<Task> {
        let saved = self.store(|s| Self::launch_guard(s, attempt, revision))?;
        let run = text(&saved, "runId")?.to_owned();
        if let Some(task) = self.0.tasks.keyed(&run, attempt) {
            let registry = self.0.registry.lock().unwrap();
            if registry.slots.get(attempt).is_some_and(|slot| {
                slot.launch.as_ref() == Some(&(revision, configuration.clone()))
            }) {
                return Ok(task);
            }
            return Err(conflict("Launch identity has different configuration."));
        }
        let slot = {
            let mut registry = self.0.registry.lock().unwrap();
            if registry.closing || registry.stopping.contains(&run) {
                return Err(conflict("Supervision is stopping this run."));
            }
            if registry.slots.contains_key(attempt) {
                return Err(conflict(
                    "Attempt launch is already registered; reconcile its saved state.",
                ));
            } else {
                if saved["state"] != "admitted" {
                    return Err(conflict(
                        "Reconcile the prior attempt launch before retrying.",
                    ));
                }
                let slot = Arc::new(Slot::new(
                    &run,
                    attempt,
                    self.root(attempt)?,
                    Some((revision, configuration.clone())),
                ));
                registry.slots.insert(attempt.into(), slot.clone());
                slot
            }
        };
        let this = self.clone();
        self.0
            .tasks
            .enqueue(&run, Some(attempt), move || async move {
                let result = this.launch_owned(&slot, revision, configuration).await;
                if result.is_err() {
                    slot.cancel.cancel();
                    let report = this.stop_attempt(&slot).await?;
                    if report["stopped"] != true {
                        return Err(Arc::new(conflict(
                            "Agent launch failed with unresolved ownership.",
                        )));
                    }
                }
                result.map_err(Arc::new)
            })
    }
    async fn launch_owned(
        &self,
        slot: &Arc<Slot>,
        revision: u64,
        mut config: Value,
    ) -> Result<Value> {
        if !config.is_object() {
            return Err(conflict("Host configuration must be an object."));
        }
        self.permitted(slot, revision, |s| {
            s.claim_attempt(&slot.attempt)?;
            Ok(())
        })?;
        crate::create_root(&slot.root)?;
        config["root"] = json!(slot.root);
        config["attemptId"] = json!(slot.attempt);
        config["backend"] = json!(self.0.options.backend);
        config["helper"] = json!(self.0.options.helper);
        config["token"] = json!(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        let config_path = slot.root.join("host.json");
        durable_new(&config_path, &serde_json::to_vec(&config)?)?;
        let token = uuid::Uuid::new_v4().to_string();
        let spec = CheckedSpec {
            root: slot.root.join("host-process"),
            token,
            command: CommandSpec {
                command: self.0.options.host.to_string_lossy().into(),
                args: vec![config_path.to_string_lossy().into()],
                cwd: slot.root.clone(),
                env: environment(),
            },
            epoch: self.0.authority.epoch().into(),
            controller_root: self.0.authority.root().into(),
        };
        let descriptor = describe_checked(&spec)?;
        // Persist descriptor before prepare/spawn; a missing manifest can then
        // be sealed after a controller crash without assuming non-execution.
        durable_new(
            &slot.root.join("host-intent.json"),
            &serde_json::to_vec(&json!({"descriptor":descriptor,"spec":spec}))?,
        )?;
        let pane = {
            let _lane = self.0.tmux.lock().await;
            self.permitted(slot, revision, |_| Ok(()))?;
            let existing = self
                .tmux_output(&["has-session", "-t", "crewroom"], true)
                .await?;
            let mut args = if existing.is_some() {
                vec![
                    "new-window",
                    "-d",
                    "-t",
                    "crewroom",
                    "-n",
                    &slot.attempt,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ]
            } else {
                vec![
                    "new-session",
                    "-d",
                    "-s",
                    "crewroom",
                    "-x",
                    "160",
                    "-y",
                    "40",
                    "-n",
                    &slot.attempt,
                    "-P",
                    "-F",
                    "#{pane_id}",
                ]
            }
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
            args.extend([
                self.0.options.helper.to_string_lossy().into(),
                "checked-owned".into(),
                descriptor.root.to_string_lossy().into(),
                descriptor.token.clone(),
                descriptor.spec_sha256.clone(),
                descriptor.epoch.clone(),
                descriptor.controller_root.to_string_lossy().into(),
            ]);
            let child = self.permitted(slot, revision, |s| {
                if s.get_attempt(&slot.attempt)?["state"] != "starting" {
                    return Err(conflict("Attempt is no longer awaiting launch."));
                }
                prepare_checked(&spec)?;
                validate_launch(
                    &descriptor,
                    self.0.authority.epoch(),
                    self.0.authority.root(),
                )?;
                Ok(self.tmux_command().args(args).spawn()?)
            })?;
            let output = command_output(child, false)
                .await?
                .ok_or_else(|| conflict("Tmux launch outcome is unavailable."))?;
            let pane = output.trim().to_owned();
            if !pane
                .strip_prefix('%')
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(conflict("Tmux did not return an exact pane identity."));
            }
            pane
        };
        self.permitted(slot, revision, |_| Ok(()))?;
        self.tmux_output(
            &["set-option", "-w", "-t", &pane, "remain-on-exit", "on"],
            false,
        )
        .await?;
        // ignore-size clients alone do not preserve an unattached window's size.
        // Owned runner dimensions are independent of passive desktop viewers.
        self.tmux_output(
            &["set-option", "-w", "-t", &pane, "window-size", "manual"],
            false,
        )
        .await?;
        let deadline = Instant::now()
            + Duration::from_secs(if crate::runner::kind(&config["model"])? != "omp" {
                90
            } else {
                20
            });
        let identity = loop {
            self.permitted(slot, revision, |_| Ok(()))?;
            if slot.root.join("endpoint.json").try_exists()? {
                let endpoint = private_json(&slot.root.join("endpoint.json"), 4096)?;
                let port = endpoint["port"]
                    .as_u64()
                    .filter(|p| *p > 0 && *p <= 65535)
                    .ok_or_else(|| conflict("Invalid host endpoint."))?;
                let client =
                    HostClient::new(&format!("http://127.0.0.1:{port}"), text(&config, "token")?)?;
                *slot.client.lock().unwrap() = Some(client.clone());
                if let Ok(identity) = client
                    .request(&json!({"op":"identity"}), &slot.cancel)
                    .await
                {
                    break identity;
                }
            }
            if Instant::now() >= deadline {
                return Err(conflict(
                    "Agent host did not establish its identity before the deadline.",
                ));
            }
            tokio::select! {_=slot.cancel.cancelled()=>return Err(conflict("Agent launch stopped during identity discovery.")),_=sleep(Duration::from_millis(25))=>{}}
        };
        crewroom_core::verify_runner_identity(&config["model"], &identity)?;
        let mut saved = json!({"hostPid":identity["hostPid"],"sessionId":identity["sessionId"],"hostUrl":slot.client.lock().unwrap().as_ref().unwrap().url().trim_end_matches('/'),"pane":pane,"runnerCwd":config.get("runnerCwd").cloned().unwrap_or(json!(slot.root))});
        for key in [
            "ompPid",
            "runnerKind",
            "observedModel",
            "runnerPid",
            "runnerVersion",
            "runnerProfile",
            "capabilities",
            "recoveryMode",
            "sessionDirectory",
        ] {
            if let Some(value) = identity.get(key) {
                saved[key] = value.clone();
            }
        }
        let mut proof = saved.clone();
        proof["id"] = json!(slot.attempt);
        proof["model"] = config["model"].clone();
        if self.owned_state(&proof).await?["state"] != "live" {
            return Err(conflict(
                "Host identity is not backed by its exact live OS processes.",
            ));
        }
        self.permitted(slot, revision, |s| {
            s.record_attempt_identity(&slot.attempt, &saved)?;
            Ok(())
        })?;
        self.register_observer(slot, true)?;
        Ok(saved)
    }
    fn tmux_command(&self) -> Command {
        let mut command = Command::new(&self.0.options.tmux);
        command
            .args(["-S"])
            .arg(&self.0.options.socket)
            .args(["-f", "/dev/null"])
            .env_clear()
            .envs(environment())
            .env("TMPDIR", &self.0.options.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }
    pub(super) async fn tmux_output(&self, args: &[&str], absent: bool) -> Result<Option<String>> {
        command_output(self.tmux_command().args(args).spawn()?, absent).await
    }
}
fn environment() -> BTreeMap<String, String> {
    [
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("HOME", std::env::var("HOME").unwrap_or_default()),
        ("LANG", "en_US.UTF-8".into()),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
async fn command_output(mut child: Child, absent: bool) -> Result<Option<String>> {
    let output = child.stdout.take().unwrap();
    timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        output.take(65537).read_to_end(&mut bytes).await?;
        if bytes.len() > 65536 {
            return Err(conflict(
                "Tmux response exceeded its bound; reconcile saved process ownership.",
            ));
        }
        let status = child.wait().await?;
        if absent && status.code() == Some(1) {
            return Ok(None);
        }
        if !status.success() {
            return Err(conflict(
                "Tmux operation failed; reconcile saved process ownership.",
            ));
        }
        Ok(Some(String::from_utf8(bytes).map_err(|_| {
            conflict("Tmux response is not valid UTF-8.")
        })?))
    })
    .await
    .map_err(|_| conflict("Tmux operation timed out; reconcile saved process ownership."))?
}
