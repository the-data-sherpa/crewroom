//! Registered host tool tasks. Their lifetime includes backend preparation,
//! container admission and final RPC delivery, so shutdown can drain all of it.
use super::*;
use crate::{container_command::ContainerCommand, host_protocol::ToolRoute};
use std::os::unix::fs::OpenOptionsExt;

impl Host {
    pub(super) fn tool(self: &Arc<Self>, frame: Value) {
        let (Some(id), Some(call), Some(name)) = (
            frame["id"].as_str(),
            frame["toolCallId"].as_str(),
            frame["toolName"].as_str(),
        ) else {
            self.fatal();
            return;
        };
        if id.is_empty() || call.is_empty() || name.is_empty() || !frame["arguments"].is_object() {
            self.fatal();
            return;
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(stringify(&json!([call, name, frame["arguments"]])).as_bytes())
        );
        let key = format!("tool:{id}");
        let admitted = {
            let mut activity = self.activity.lock().unwrap();
            if !activity.accepting {
                return;
            }
            if let Some(previous) = activity.tool_ids.get(id) {
                if previous == &fingerprint && activity.tasks.contains_key(&key) {
                    return;
                }
                // Do not deliver a contradictory result under an active RPC ID.
                drop(activity);
                self.fatal();
                return;
            }
            let reused = activity.calls.contains_key(call);
            activity.tool_ids.insert(id.into(), fingerprint.clone());
            activity.calls.insert(call.into(), fingerprint);
            match self.task(&mut activity, key, stringify(&frame).len()) {
                Ok((task, finished)) => Some((task, finished, reused)),
                Err(_) => None,
            }
        };
        let Some((task, finished, reused)) = admitted else {
            self.fatal();
            return;
        };
        let host = self.clone();
        tokio::spawn(async move {
            let execution = if reused {
                Err(conflict(
                    "Tool identity was already admitted; its retained outcome must be reconciled.",
                ))
            } else {
                host.execute_tool(&frame, &task.cancel).await
            };
            let (is_error, value) = match execution {
                Ok(value) => (false, stringify(&redact(&value, &host.secrets))),
                Err(error) => (
                    true,
                    redact(&json!(error.to_string()), &host.secrets)
                        .as_str()
                        .unwrap()
                        .to_owned(),
                ),
            };
            // Failure may follow a committed effect whose reply was lost.
            let result = json!({"type":"host_tool_result","id":frame["id"],"isError":is_error,"outcome":if is_error{"unresolved"}else{"confirmed"},"result":{"content":[{"type":"text","text":value}]}});
            let delivery = if let Some(rpc) = host.rpc.get() {
                rpc.resolve_tool(&result)
                    .await
                    .map(|_| Value::Null)
                    .map_err(|e| e.to_string())
            } else {
                Err("Runner is unavailable.".into())
            };
            finished.finish(delivery);
        });
    }
    async fn execute_tool(
        self: &Arc<Self>,
        frame: &Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        if cancel.is_cancelled() {
            return Err(conflict("Host tool was cancelled."));
        }
        match self.catalog.route(text(frame, "toolName")?)? {
            ToolRoute::Command => {
                let args = frame["arguments"].as_object().unwrap();
                if args.keys().any(|key| key != "commandId") {
                    return Err(conflict("Command is not admitted."));
                }
                let spec = self.config["tools"]
                    .as_array()
                    .and_then(|commands| {
                        commands
                            .iter()
                            .find(|c| c["id"] == frame["arguments"]["commandId"])
                    })
                    .ok_or_else(|| conflict("Command is not admitted."))?;
                self.run_tool(frame, spec.clone(), cancel).await
            }
            ToolRoute::Check => {
                let prepared = self
                    .backend_tool(
                        frame,
                        "coding/check-prepare",
                        frame["arguments"].clone(),
                        cancel,
                    )
                    .await?;
                let mut spec = prepared["spec"].clone();
                spec["id"] = frame["arguments"]["checkId"].clone();
                let execution = self.run_tool(frame, spec, cancel).await?;
                let mut result = self
                    .backend_tool(frame, "coding/check-finish", json!({}), cancel)
                    .await?;
                if !result.is_object() {
                    return Err(conflict("Invalid check completion reply."));
                }
                result["outputPreview"] = execution["output"].clone();
                Ok(result)
            }
            ToolRoute::Shell => {
                let prepared = self
                    .backend_tool(frame, "coding/shell", frame["arguments"].clone(), cancel)
                    .await?;
                let mut spec = prepared["spec"].clone();
                spec["id"] = json!("shell");
                let mut result = self.run_tool(frame, spec, cancel).await?;
                result["note"] = json!(
                    "Ad-hoc output over a read-only worktree. It is not check evidence; use run_check for required checks."
                );
                Ok(result)
            }
            ToolRoute::Backend(endpoint) => {
                self.backend_tool(frame, &endpoint, frame["arguments"].clone(), cancel)
                    .await
            }
        }
    }
    async fn backend_tool(
        &self,
        frame: &Value,
        endpoint: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let url = self
            .backend
            .join(&format!("internal/{endpoint}"))
            .map_err(|_| conflict("Invalid backend route."))?;
        let operation = async {
            let response=self.client.post(url).header("Content-Type","application/json").header("Origin",self.backend.origin().ascii_serialization()).bearer_auth(self.config["token"].as_str().unwrap())
                .body(stringify(&json!({"attemptId":self.config["attemptId"],"toolCallId":frame["toolCallId"],"arguments":args}))).send().await.map_err(|_|conflict("Backend response was interrupted; tool outcome is unknown."))?;
            let status = response.status();
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| {
                    conflict("Backend response was interrupted; tool outcome is unknown.")
                })?;
                if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                    return Err(conflict("Backend response exceeded its limit."));
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes)?;
            if !status.is_success() {
                return Err(conflict(
                    value["error"]
                        .as_str()
                        .unwrap_or("Tool operation was rejected."),
                ));
            }
            Ok(value)
        };
        tokio::select! {result=operation=>result,_=cancel.cancelled()=>Err(conflict("Host tool was cancelled; any backend receipt must be reconciled."))}
    }
    fn command(&self, root: &Path) -> Result<ContainerCommand> {
        let socket = self.config["commandEngine"]["socket"]
            .as_str()
            .ok_or_else(|| conflict("Contained command engine is unavailable."))?;
        ContainerCommand::new(root.into(), Path::new(socket))
    }
    async fn run_tool(
        self: &Arc<Self>,
        frame: &Value,
        spec: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        if cancel.is_cancelled() {
            return Err(conflict("Command stopped during preparation."));
        }
        let root = self.root.join("tools").join(format!(
            "{:x}",
            Sha256::digest(text(frame, "toolCallId")?.as_bytes())
        ));
        let job = self.command(&root)?;
        let retained = root.join("container.json").try_exists()?;
        if retained {
            let mut expected = spec.clone();
            expected["workspace"] = json!(std::fs::canonicalize(text(&spec, "workspace")?)?);
            let saved: Value = serde_json::from_slice(&read(&root.join("container.json"))?)?;
            if saved["spec"] != expected {
                return Err(conflict(
                    "Retained command does not match its admitted specification.",
                ));
            }
        }
        if !retained {
            let start = job.start(spec.clone()).await;
            if cancel.is_cancelled() {
                let stopped = job.stop().await?;
                if stopped["state"] != "stopped" {
                    return Err(conflict("Command launch ownership remains uncertain."));
                }
                return Err(conflict("Command stopped during admission."));
            }
            start?;
        }
        if cancel.is_cancelled() {
            let _ = job.stop().await?;
            return Err(conflict("Command was cancelled."));
        }
        self.observe(json!({"type":if retained{"tool_receipt_reused"}else{"tool_started"},"toolCallId":frame["toolCallId"],"commandId":spec["id"],"container":job.inspect().await["id"]}));
        let following = CancellationToken::new();
        let follow_cancel = following.clone();
        let follow_job = job.clone();
        let host = Arc::downgrade(self);
        let live = root.join("live.log");
        let observer = tokio::spawn(async move {
            let _ = follow_job
                .follow(
                    move |bytes| {
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .append(true)
                            .create(true)
                            .mode(0o600)
                            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                            .open(&live)
                        {
                            let _ = file.write_all(bytes);
                        }
                        if let Some(host) = host.upgrade() {
                            let text =
                                redact(&json!(String::from_utf8_lossy(bytes)), &host.secrets);
                            let output = host.pane.lock().unwrap().output(text.as_str().unwrap());
                            host.output(output);
                        }
                        Ok(())
                    },
                    follow_cancel,
                )
                .await;
        });
        let ownership =
            tokio::select! {value=job.wait()=>value,_=cancel.cancelled()=>job.stop().await};
        following.cancel();
        let _ = observer.await;
        let ownership = ownership?;
        let result = json!({"exitCode":ownership["exitCode"]});
        self.observe(json!({"type":"tool_exited","toolCallId":frame["toolCallId"],"result":result,"ownership":ownership}));
        if ownership["state"] != "stopped" {
            return Err(conflict("Tool descendants have unresolved ownership."));
        }
        let output = job.logs().await?;
        let path = root.join("output.log");
        if path.try_exists()? {
            if read(&path)? != output.as_bytes() {
                return Err(conflict("Saved command output does not match its receipt."));
            }
        } else {
            durable_new(&path, output.as_bytes())?;
        }
        job.remove().await?;
        if cancel.is_cancelled() {
            return Err(conflict(
                "Host tool was cancelled; its execution receipt is retained.",
            ));
        }
        let mut units = 0;
        let preview: String = output
            .chars()
            .rev()
            .take_while(|c| {
                units += c.len_utf16();
                units <= 16000
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Ok(json!({"result":result,"output":preview}))
    }
    pub(super) async fn stop_tools(&self) -> Result<()> {
        let parent = self.root.join("tools");
        if !parent.try_exists()? {
            return Ok(());
        }
        canonical(&parent)?;
        let mut unresolved = false;
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let root = entry.path();
            if !entry.file_type()?.is_dir() {
                unresolved = true;
                continue;
            }
            let result = async {
                let job = self.command(&root)?;
                let ownership = job.stop().await?;
                if ownership["state"] != "stopped" {
                    return Err(conflict("Tool ownership is unresolved."));
                }
                job.remove().await
            }
            .await;
            if result.is_err() {
                unresolved = true;
            }
        }
        if unresolved {
            Err(conflict(
                "Tool ownership or retained receipts remain unresolved.",
            ))
        } else {
            Ok(())
        }
    }
}
