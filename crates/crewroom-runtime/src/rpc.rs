//! One owned OMP process with correlated commands and an independent event stream.
use crate::{
    CommandSpec, conflict, prepare_owned,
    rpc_frame::{Decoder, MAX_FRAME_BYTES, MAX_REASSEMBLED_BYTES},
    stop_owned,
};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{Mutex as AsyncMutex, oneshot},
    time::{sleep, timeout},
};

type Reply = std::result::Result<Value, String>;
// A cancelled request keeps a tombstone until its late response arrives. Reusing
// an unresolved wire ID could otherwise accept that old response as new evidence.
type Pending = HashMap<String, (uuid::Uuid, Option<oneshot::Sender<Reply>>)>;
struct Inner {
    input: AsyncMutex<Option<ChildStdin>>,
    pending: Mutex<Pending>,
    running: AtomicBool,
    closing: AtomicBool,
    event: Arc<dyn Fn(Value) + Send + Sync>,
}
impl Inner {
    fn fail(&self, message: &str) {
        let mut pending = self.pending.lock().unwrap();
        self.running.store(false, Ordering::SeqCst);
        for (_, (_, reply)) in pending.drain() {
            if let Some(reply) = reply {
                let _ = reply.send(Err(message.into()));
            }
        }
    }
}
struct Request {
    inner: Arc<Inner>,
    id: String,
    registration: uuid::Uuid,
}
struct Writing {
    inner: Arc<Inner>,
    complete: bool,
}
impl Drop for Writing {
    fn drop(&mut self) {
        if !self.complete {
            // Cancelling write_all may leave a partial JSON line. Never allow a later
            // command to append to it or pretend that dispatch was absent.
            self.inner.fail("OMP input dispatch was interrupted.");
            let inner = self.inner.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    inner.input.lock().await.take();
                });
            }
        }
    }
}
impl Drop for Request {
    fn drop(&mut self) {
        if let Some((registration, reply)) = self.inner.pending.lock().unwrap().get_mut(&self.id)
            && *registration == self.registration
        {
            reply.take();
        }
    }
}
#[derive(Clone)]
pub struct OmpRpc {
    inner: Arc<Inner>,
    pub ownership_root: PathBuf,
    pub pid: u32,
}
struct Starting(Option<OmpRpc>);
impl Drop for Starting {
    fn drop(&mut self) {
        if let Some(rpc) = self.0.take() {
            // If negotiation is cancelled, retain the observable ownership root and
            // reconcile the real child. Host/controller death is reconciled externally.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = rpc.close().await;
                });
            }
        }
    }
}
pub type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>>;
pub trait RpcCommands {
    fn command<'a>(&'a self, kind: &'a str, args: Value) -> CommandFuture<'a>;
}
impl RpcCommands for OmpRpc {
    fn command<'a>(&'a self, kind: &'a str, args: Value) -> CommandFuture<'a> {
        Box::pin(OmpRpc::command(self, kind, args))
    }
}
impl OmpRpc {
    /// The helper path is supplied by the trusted installation, never a model request.
    pub async fn start(
        helper: &Path,
        spec: &CommandSpec,
        event: impl Fn(Value) + Send + Sync + 'static,
    ) -> Result<Self> {
        let ownership_root = spec.cwd.join("omp-process");
        prepare_owned(&ownership_root, spec)?;
        let mut child = Command::new(helper)
            .arg("exec")
            .arg(&ownership_root)
            .current_dir(&spec.cwd)
            .env_clear()
            .env("LANG", "C")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| conflict("OMP process did not start."))?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let inner = Arc::new(Inner {
            input: AsyncMutex::new(child.stdin.take()),
            pending: Mutex::new(HashMap::new()),
            running: AtomicBool::new(true),
            closing: AtomicBool::new(false),
            event: Arc::new(event),
        });
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        let reader = inner.clone();
        tokio::spawn(async move {
            let mut pipe = BufReader::new(stdout);
            let mut decoder = Decoder::default();
            let result = async {
                loop {
                    let mut line = Vec::new();
                    // Bound before allocation, including a peer that never sends a newline.
                    loop {
                        let available = pipe.fill_buf().await?;
                        if available.is_empty() {
                            break;
                        }
                        let count = available
                            .iter()
                            .position(|b| *b == b'\n')
                            .map_or(available.len(), |p| p + 1);
                        if line.len() + count > MAX_FRAME_BYTES {
                            return Err(conflict("RPC frame exceeded the transport limit."));
                        }
                        let finished = available[count - 1] == b'\n';
                        line.extend_from_slice(&available[..count]);
                        pipe.consume(count);
                        if finished {
                            break;
                        }
                    }
                    if line.is_empty() {
                        return decoder.finish();
                    }
                    if !line.ends_with(b"\n") {
                        return Err(conflict("RPC stream ended inside a physical frame."));
                    }
                    if let Some(frame) = decoder.line(&line)? {
                        let reply = if frame["type"] == "response" {
                            frame["id"]
                                .as_str()
                                .and_then(|id| reader.pending.lock().unwrap().remove(id))
                        } else {
                            None
                        };
                        if let Some((_, reply)) = reply {
                            let Some(reply) = reply else {
                                continue;
                            };
                            let result = if frame["success"] == true {
                                Ok(frame.get("data").cloned().unwrap_or(Value::Null))
                            } else {
                                Err(format!(
                                    "OMP rejected {}.",
                                    frame["command"].as_str().unwrap_or("command")
                                ))
                            };
                            let _ = reply.send(result);
                        } else {
                            (reader.event)(frame);
                        }
                    }
                }
            }
            .await;
            let message = result
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "OMP output closed.".into());
            reader.fail(&message);
            // A broken stream cannot admit more host tools or requests. Close stdin;
            // ownership remains observable until the caller verifies shutdown.
            (reader.event)(json!({"type":"rpc_stream_closed","reason":message}));
            let _ = timeout(Duration::from_millis(500), async {
                reader.input.lock().await.take();
            })
            .await;
        });
        let waiter = inner.clone();
        tokio::spawn(async move {
            use std::os::unix::process::ExitStatusExt;
            match child.wait().await {
                Ok(status) => {
                    waiter.fail("OMP process exited.");
                    (waiter.event)(
                        json!({"type":"process_exit","code":status.code(),"signal":status.signal().map(signal_name)}),
                    );
                }
                Err(error) => {
                    waiter.fail("OMP exit could not be observed.");
                    (waiter.event)(json!({"type":"process_exit","reason":error.to_string()}));
                }
            }
        });
        let rpc = Self {
            inner,
            ownership_root,
            pid,
        };
        let mut starting = Starting(Some(rpc.clone()));
        let negotiate = rpc
            .command("negotiate_protocol", json!({"protocolVersion":2}))
            .await;
        if !negotiate.as_ref().is_ok_and(|v| v["protocolVersion"] == 2) {
            starting.0.take();
            rpc.close().await?;
            return Err(conflict(
                "OMP protocol v2 negotiation failed. Assignment cannot execute.",
            ));
        }
        starting.0.take();
        Ok(rpc)
    }
    pub async fn send(&self, frame: &Value) -> Result<()> {
        if !self.inner.running.load(Ordering::SeqCst) {
            return Err(conflict("OMP process is unavailable."));
        }
        // OMP's input reader accepts full JSON lines, not output chunk envelopes.
        let mut bytes = stringify(frame).into_bytes();
        if bytes.len() > MAX_REASSEMBLED_BYTES {
            return Err(conflict("RPC command exceeded its byte limit."));
        }
        bytes.push(b'\n');
        let mut input = self.inner.input.lock().await;
        if !self.inner.running.load(Ordering::SeqCst) {
            return Err(conflict("OMP process is unavailable."));
        }
        if self.inner.closing.load(Ordering::SeqCst) && frame["type"] != "abort" {
            return Err(conflict("OMP process is stopping."));
        }
        let Some(input) = input.as_mut() else {
            return Err(conflict("OMP input is closed."));
        };
        let mut writing = Writing {
            inner: self.inner.clone(),
            complete: false,
        };
        if let Err(error) = timeout(Duration::from_secs(20), input.write_all(&bytes))
            .await
            .map_err(|_| conflict("OMP input write timed out."))?
        {
            self.inner.fail("OMP input failed.");
            return Err(error.into());
        }
        writing.complete = true;
        Ok(())
    }
    pub async fn command(&self, kind: &str, args: Value) -> Result<Value> {
        self.command_with_id(kind, args, &uuid::Uuid::new_v4().to_string())
            .await
    }
    pub async fn command_with_id(&self, kind: &str, args: Value, id: &str) -> Result<Value> {
        let mut frame = args
            .as_object()
            .cloned()
            .ok_or_else(|| conflict("RPC command arguments must be an object."))?;
        frame.insert("type".into(), json!(kind));
        frame.insert("id".into(), json!(id));
        let (reply, result) = oneshot::channel();
        let registration = uuid::Uuid::new_v4();
        {
            let mut pending = self.inner.pending.lock().unwrap();
            if !self.inner.running.load(Ordering::SeqCst) {
                return Err(conflict("OMP process is unavailable."));
            }
            if pending.contains_key(id) {
                return Err(conflict("RPC command ID is already pending or unresolved."));
            }
            if pending.len() >= 1024 {
                return Err(conflict("Too many unresolved RPC commands."));
            }
            pending.insert(id.into(), (registration, Some(reply)));
        }
        let _request = Request {
            inner: self.inner.clone(),
            id: id.into(),
            registration,
        };
        timeout(Duration::from_secs(20), async {
            self.send(&Value::Object(frame)).await?;
            result
                .await
                .map_err(|_| conflict("OMP response channel closed."))?
                .map_err(|message| conflict(&message))
        })
        .await
        .map_err(|_| conflict(&format!("OMP {kind} timed out.")))?
    }
    pub async fn close(&self) -> Result<()> {
        self.inner.closing.store(true, Ordering::SeqCst);
        if self.inner.running.load(Ordering::SeqCst) {
            let _ = timeout(Duration::from_millis(500), self.command("abort", json!({}))).await;
        }
        // A blocked writer must not prevent verified group shutdown. Stopping the
        // owned reader also releases a pipe write that has not returned yet.
        let _ = timeout(Duration::from_millis(500), async {
            self.inner.input.lock().await.take();
        })
        .await;
        sleep(Duration::from_millis(100)).await;
        let ownership = stop_owned(&self.ownership_root).await;
        if ownership.state != "stopped" {
            return Err(conflict(
                ownership
                    .reason
                    .as_deref()
                    .unwrap_or("OMP ownership remains uncertain."),
            ));
        }
        Ok(())
    }
}
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGTERM => "SIGTERM".into(),
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        _ => format!("signal {signal}"),
    }
}
