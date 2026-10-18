//! Bounded owned JSONL process. Correlate client replies separately from server requests.
use crate::{CommandSpec, conflict, prepare_owned, stop_owned};
use crewroom_core::{Result, serialized::stringify};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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
    time::{Instant, sleep, timeout, timeout_at},
};

const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_REASSEMBLED_BYTES: usize = MAX_FRAME_BYTES;

type Reply = std::result::Result<Value, String>;
// A cancelled request keeps a tombstone until its late response arrives. Reusing
// an unresolved wire ID could otherwise accept that old response as new evidence.
type Pending = HashMap<String, (uuid::Uuid, Option<oneshot::Sender<Reply>>)>;
struct Inner {
    protocol: Protocol,
    input: AsyncMutex<Option<ChildStdin>>,
    pending: Mutex<Pending>,
    running: AtomicBool,
    closing: AtomicBool,
    event: Arc<dyn Fn(Value) + Send + Sync>,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Protocol {
    Codex,
    Claude,
}
impl Protocol {
    fn response_id(self, frame: &Value) -> Option<&str> {
        match self {
            Self::Codex
                if frame.get("method").is_none()
                    && (frame.get("result").is_some() || frame.get("error").is_some()) =>
            {
                frame["id"].as_str()
            }
            Self::Claude if frame["type"] == "control_response" => {
                frame["response"]["request_id"].as_str()
            }
            _ => None,
        }
    }
    fn response(self, frame: &Value) -> Reply {
        match self {
            Self::Codex if frame.get("result").is_some() && frame.get("error").is_none() => {
                Ok(frame["result"].clone())
            }
            Self::Claude if frame["response"]["subtype"] == "success" => {
                Ok(frame["response"]["response"].clone())
            }
            _ => Err("Runner rejected the control request.".into()),
        }
    }
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
            self.inner
                .fail("Native runner input dispatch was interrupted.");
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
pub struct JsonlRpc {
    inner: Arc<Inner>,
    pub ownership_root: PathBuf,
    pub pid: u32,
}
impl JsonlRpc {
    /// The helper path is supplied by the trusted installation, never a model request.
    pub async fn start(
        helper: &Path,
        spec: &CommandSpec,
        event: impl Fn(Value) + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::start_protocol(
            helper,
            spec,
            &spec.cwd.join("codex-process"),
            Protocol::Codex,
            event,
        )
        .await
    }
    pub async fn start_protocol(
        helper: &Path,
        spec: &CommandSpec,
        ownership_root: &Path,
        protocol: Protocol,
        event: impl Fn(Value) + Send + Sync + 'static,
    ) -> Result<Self> {
        let ownership_root = ownership_root.to_path_buf();
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
            .ok_or_else(|| conflict("Native runner process did not start."))?;
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let inner = Arc::new(Inner {
            protocol,
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
            let result = async {
                loop {
                    let mut line = Vec::new();
                    let mut deadline = None;
                    // Bound before allocation, including a peer that never sends a newline.
                    loop {
                        let available = if let Some(deadline) = deadline {
                            timeout_at(deadline, pipe.fill_buf())
                                .await
                                .map_err(|_| conflict("RPC physical frame read timed out."))??
                        } else {
                            pipe.fill_buf().await?
                        };
                        if available.is_empty() {
                            break;
                        }
                        deadline.get_or_insert_with(|| Instant::now() + Duration::from_secs(10));
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
                        return Ok::<(), crewroom_core::Error>(());
                    }
                    if !line.ends_with(b"\n") {
                        return Err(conflict("RPC stream ended inside a physical frame."));
                    }
                    {
                        let frame: Value = serde_json::from_slice(&line)?;
                        if !frame.is_object() {
                            return Err(conflict("Native runner frame is not an object."));
                        }
                        let reply = protocol
                            .response_id(&frame)
                            .and_then(|id| reader.pending.lock().unwrap().remove(id));
                        if let Some((_, reply)) = reply {
                            let Some(reply) = reply else {
                                continue;
                            };
                            let result = protocol.response(&frame);
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
                .unwrap_or_else(|| "Native runner output closed.".into());
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
                    waiter.fail("Native runner process exited.");
                    (waiter.event)(
                        json!({"type":"process_exit","code":status.code(),"signal":status.signal().map(signal_name)}),
                    );
                }
                Err(error) => {
                    waiter.fail("Native runner exit could not be observed.");
                    (waiter.event)(json!({"type":"process_exit","reason":error.to_string()}));
                }
            }
        });
        let rpc = Self {
            inner,
            ownership_root,
            pid,
        };
        Ok(rpc)
    }
    pub async fn send(&self, frame: &Value) -> Result<()> {
        if !self.inner.running.load(Ordering::SeqCst) {
            return Err(conflict("Native runner process is unavailable."));
        }
        // Native runner's input reader accepts full JSON lines, not output chunk envelopes.
        let mut bytes = stringify(frame).into_bytes();
        if bytes.len() > MAX_REASSEMBLED_BYTES {
            return Err(conflict("RPC command exceeded its byte limit."));
        }
        bytes.push(b'\n');
        let mut input = timeout(Duration::from_secs(20), self.inner.input.lock())
            .await
            .map_err(|_| conflict("Runner input queue timed out."))?;
        if !self.inner.running.load(Ordering::SeqCst) {
            return Err(conflict("Native runner process is unavailable."));
        }
        if self.inner.closing.load(Ordering::SeqCst)
            && frame["method"] != "turn/interrupt"
            && frame["request"]["subtype"] != "interrupt"
        {
            return Err(conflict("Native runner process is stopping."));
        }
        let Some(input) = input.as_mut() else {
            return Err(conflict("Native runner input is closed."));
        };
        let mut writing = Writing {
            inner: self.inner.clone(),
            complete: false,
        };
        if let Err(error) = timeout(Duration::from_secs(20), input.write_all(&bytes))
            .await
            .map_err(|_| conflict("Native runner input write timed out."))?
        {
            self.inner.fail("Native runner input failed.");
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
        if !args.is_object() {
            return Err(conflict(
                "Native runner request parameters must be an object.",
            ));
        }
        let frame = match self.inner.protocol {
            Protocol::Codex => json!({"method":kind,"id":id,"params":args}),
            Protocol::Claude => {
                let mut request = args;
                request["subtype"] = json!(kind);
                json!({"type":"control_request","request_id":id,"request":request})
            }
        };
        let (reply, result) = oneshot::channel();
        let registration = uuid::Uuid::new_v4();
        {
            let mut pending = self.inner.pending.lock().unwrap();
            if !self.inner.running.load(Ordering::SeqCst) {
                return Err(conflict("Native runner process is unavailable."));
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
            self.send(&frame).await?;
            result
                .await
                .map_err(|_| conflict("Native runner response channel closed."))?
                .map_err(|message| conflict(&message))
        })
        .await
        .map_err(|_| conflict(&format!("Native runner {kind} timed out.")))?
    }
    pub async fn close(&self) -> Result<()> {
        self.inner.closing.store(true, Ordering::SeqCst);
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
                    .unwrap_or("Native runner ownership remains uncertain."),
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
