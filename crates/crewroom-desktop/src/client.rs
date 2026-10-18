mod live;
use anyhow::{Context as _, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Deserialize)]
struct Connection {
    protocol: u32,
    origin: String,
    token: String,
}
fn connection(path: &Path) -> anyhow::Result<Connection> {
    let metadata = std::fs::symlink_metadata(path).context("Start the Rust service to connect.")?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!("The native connection file must be private and owned by this user.");
    }
    let value: Connection = serde_json::from_slice(&std::fs::read(path)?)?;
    let url = url::Url::parse(&value.origin)?;
    if value.protocol != 1
        || url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.token.len() != 64
        || !value.token.bytes().all(|c| c.is_ascii_hexdigit())
    {
        bail!("The native connection file is invalid.");
    }
    Ok(value)
}
pub enum Event {
    Superseded {
        path: String,
    },
    Workspace(Value),
    Connected(bool),
    Result {
        path: String,
        result: Result<Value, String>,
    },
    Read {
        generation: u64,
        path: String,
        result: Result<Value, String>,
    },
    Live {
        generation: u64,
        message: Value,
    },
    Error(String),
}
struct Command {
    path: String,
    body: Value,
    read: bool,
    generation: u64,
    stop_epoch: Option<(Arc<AtomicU64>, u64)>,
}
// Stop intent must reach the service even while a slow Resume awaits process startup.
pub(crate) fn stop_command(path: &str) -> bool {
    matches!(
        path.split('/').collect::<Vec<_>>().as_slice(),
        ["", "runs", _, "pause" | "cancel"]
    )
}
fn execute(
    http: &reqwest::blocking::Client,
    descriptor: &Path,
    events: &async_channel::Sender<Event>,
    request: Command,
) -> bool {
    if request
        .stop_epoch
        .as_ref()
        .is_some_and(|(epoch, sent)| epoch.load(Ordering::SeqCst) != *sent)
    {
        return events
            .send_blocking(Event::Superseded { path: request.path })
            .is_ok();
    }
    let result = (|| -> anyhow::Result<Value> {
        let config = connection(descriptor)?;
        let url = format!("{}/api{}", config.origin, request.path);
        let outgoing = if request.read {
            http.get(url)
        } else {
            let outgoing = http.post(url).json(&request.body);
            // A lifecycle command waits for owned processes to stop
            // or initialize. Multi-harness teams can exceed the read
            // deadline; the separate live observer still shows progress.
            if matches!(
                request.path.rsplit('/').next(),
                Some("start" | "resume" | "pause" | "cancel")
            ) {
                outgoing.timeout(Duration::from_secs(120))
            } else {
                outgoing
            }
        };
        let response = outgoing
            .bearer_auth(&config.token)
            .header("Origin", &config.origin)
            .send()
            .context(
                "Connection lost. Your draft is retained; check the saved state before retrying.",
            )?;
        let status = response.status();
        let data: Value = response.json()?;
        if !status.is_success() {
            bail!(
                "{}",
                data["error"]
                    .as_str()
                    .unwrap_or("Unable to save. Your draft is retained.")
            );
        }
        Ok(data)
    })()
    .map_err(|e| e.to_string());
    let event = if request.read {
        Event::Read {
            generation: request.generation,
            path: request.path,
            result,
        }
    } else {
        Event::Result {
            path: request.path,
            result,
        }
    };
    events.send_blocking(event).is_ok()
}
#[derive(Clone, Default, PartialEq, Eq)]
struct Selection {
    generation: u64,
    run: Option<String>,
}
pub struct Client {
    descriptor: PathBuf,
    commands: mpsc::Sender<Command>,
    stop_events: async_channel::Sender<Event>,
    stop_epochs: Mutex<std::collections::HashMap<String, Arc<AtomicU64>>>,
    alive: Arc<AtomicBool>,
    selection: Arc<Mutex<Selection>>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}
impl Client {
    pub fn descriptor_path(&self) -> PathBuf {
        self.descriptor.clone()
    }
    pub fn connect(path: PathBuf) -> (Self, async_channel::Receiver<Event>) {
        let saved_descriptor = path.clone();
        let (send, receive) = async_channel::bounded(32);
        let (commands, requests) = mpsc::channel::<Command>();
        let alive = Arc::new(AtomicBool::new(true));
        let selection = Arc::new(Mutex::new(Selection::default()));
        let descriptor = path.clone();
        let events = send.clone();
        let stop_events = send.clone();
        std::thread::spawn(move || {
            let http = reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .build()
                .expect("HTTP client");
            while let Ok(request) = requests.recv() {
                if !execute(&http, &descriptor, &events, request) {
                    break;
                }
            }
        });
        let alive_copy = alive.clone();
        let selected = selection.clone();
        std::thread::spawn(move || live::observe(path, alive_copy, selected, send));
        (
            Self {
                descriptor: saved_descriptor,
                commands,
                stop_events,
                stop_epochs: Mutex::new(Default::default()),
                alive,
                selection,
            },
            receive,
        )
    }
    pub fn watch_run(&self, run: Option<String>) -> u64 {
        let mut selection = self.selection.lock().unwrap();
        if selection.run != run {
            selection.run = run;
            selection.generation = selection.generation.wrapping_add(1);
        }
        selection.generation
    }
    pub fn read(&self, path: String) -> Result<(), String> {
        self.commands
            .send(Command {
                path,
                body: Value::Null,
                read: true,
                generation: self.selection.lock().unwrap().generation,
                stop_epoch: None,
            })
            .map_err(|_| "Connection closed. Retained data may be stale.".into())
    }
    pub fn send(&self, path: String, body: Value) -> Result<(), String> {
        let stop_epoch = if let Some((run, action)) = path.rsplit_once('/')
            && ["start", "resume", "pause", "cancel"].contains(&action)
        {
            let mut epochs = self.stop_epochs.lock().unwrap();
            let epoch = epochs.entry(run.into()).or_default().clone();
            if stop_command(&path) {
                epoch.fetch_add(1, Ordering::SeqCst);
                None
            } else {
                let current = epoch.load(Ordering::SeqCst);
                Some((epoch, current))
            }
        } else {
            None
        };
        let command = Command {
            path,
            body,
            read: false,
            generation: 0,
            stop_epoch,
        };
        if stop_command(&command.path) {
            let descriptor = self.descriptor.clone();
            let events = self.stop_events.clone();
            std::thread::spawn(move || {
                let http = reqwest::blocking::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(15))
                    .build()
                    .expect("HTTP client");
                execute(&http, &descriptor, &events, command);
            });
            Ok(())
        } else {
            self.commands
                .send(command)
                .map_err(|_| "Connection closed. Draft retained.".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn refuses_remote_or_public_connection_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("connection.json");
        let write = |origin: &str| {
            std::fs::write(
                &path,
                json!({"origin":origin,"protocol":1,"token":"a".repeat(64)}).to_string(),
            )
            .unwrap()
        };
        write("http://example.com:4318");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(connection(&path).is_err());
        write("http://127.0.0.1:4318");
        assert!(connection(&path).is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(connection(&path).is_err());
    }
    #[test]
    fn resume_can_finish_after_the_read_deadline_without_reporting_connection_loss() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        let server = std::thread::spawn(move || {
            while !finished.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    if buffer[..count].starts_with(b"POST /api/runs/fixture/resume ") {
                        std::thread::sleep(Duration::from_millis(16_500));
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"status\":\"running\"}").unwrap();
                        break;
                    }
                    let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                } else {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        });
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("connection.json");
        std::fs::write(
            &path,
            json!({"protocol":1,"origin":origin,"token":"a".repeat(64)}).to_string(),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (client, events) = Client::connect(path);
        client
            .send("/runs/fixture/resume".into(), json!({}))
            .unwrap();
        while let Ok(event) = events.recv_blocking() {
            if let Event::Result { result, .. } = event {
                done.store(true, Ordering::Relaxed);
                drop(client);
                server.join().unwrap();
                assert_eq!(result.unwrap()["status"], "running");
                return;
            }
        }
        panic!("Resume result was lost");
    }
    #[test]
    fn cancel_bypasses_a_pending_resume_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (cancel_tx, cancel_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Arc::new(Mutex::new(release_rx));
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let server = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let resume = resume_tx.clone();
                        let cancel = cancel_tx.clone();
                        let release = release.clone();
                        std::thread::spawn(move || {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let mut buffer = [0; 4096];
                            let count = stream.read(&mut buffer).unwrap_or(0);
                            let request = String::from_utf8_lossy(&buffer[..count]);
                            if request.starts_with("POST /api/runs/fixture/resume ") {
                                resume.send(()).unwrap();
                                release.lock().unwrap().recv().unwrap();
                            } else if request.starts_with("POST /api/runs/fixture/cancel ") {
                                cancel.send(()).unwrap();
                            } else {
                                let _=stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                return;
                            }
                            let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("connection.json");
        std::fs::write(
            &path,
            json!({"protocol":1,"origin":origin,"token":"a".repeat(64)}).to_string(),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (client, _events) = Client::connect(path);
        client
            .send("/runs/fixture/resume".into(), json!({}))
            .unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client
            .send("/runs/fixture/cancel".into(), json!({}))
            .unwrap();
        let delivered = cancel_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        drop(client);
        stopped.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert!(
            delivered.is_ok(),
            "Cancel must reach the service before Resume completes"
        );
    }
    #[test]
    fn pause_supersedes_resume_queued_behind_a_read() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (pause_tx, pause_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Arc::new(Mutex::new(release_rx));
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let server = std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let resume = resume_tx.clone();
                        let pause = pause_tx.clone();
                        let release = release.clone();
                        std::thread::spawn(move || {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let mut buffer = [0; 4096];
                            let count = stream.read(&mut buffer).unwrap_or(0);
                            let request = String::from_utf8_lossy(&buffer[..count]);
                            if request.starts_with("GET /api/slow ") {
                                resume.send(()).unwrap();
                                release.lock().unwrap().recv().unwrap();
                            } else if request.starts_with("POST /api/runs/fixture/pause ") {
                                pause.send(()).unwrap();
                            } else {
                                let _=stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                                return;
                            }
                            let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("connection.json");
        std::fs::write(
            &path,
            json!({"protocol":1,"origin":origin,"token":"a".repeat(64)}).to_string(),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (client, events) = Client::connect(path);
        client.read("/slow".into()).unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client
            .send("/runs/fixture/resume".into(), json!({}))
            .unwrap();
        client
            .send("/runs/fixture/pause".into(), json!({}))
            .unwrap();
        let delivered = pause_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut superseded = false;
        while std::time::Instant::now() < deadline {
            if let Ok(Event::Superseded { path }) = events.try_recv() {
                assert_eq!(path, "/runs/fixture/resume");
                superseded = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(client);
        stopped.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert!(
            superseded,
            "Queued Resume must not reach the service after Pause"
        );
        assert!(
            delivered.is_ok(),
            "Pause must reach the service while an older read is pending"
        );
    }
}
