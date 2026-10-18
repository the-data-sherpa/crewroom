//! One bounded push connection observes the workspace and the selected run.
use super::*;
use tungstenite::{Message, client::IntoClientRequest};

pub(super) fn observe(
    path: PathBuf,
    alive: Arc<AtomicBool>,
    selection: Arc<Mutex<Selection>>,
    send: async_channel::Sender<Event>,
) {
    let mut last_error = String::new();
    let mut retained = Selection::default();
    let mut cursor = 0;
    while alive.load(Ordering::Relaxed) {
        let result = (|| -> anyhow::Result<()> {
            let config = connection(&path)?;
            let mut request = format!("{}/api/live", config.origin.replacen("http://", "ws://", 1))
                .into_client_request()?;
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {}", config.token).parse()?);
            let port = url::Url::parse(&config.origin)?.port().unwrap();
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            let stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5))
                .context("Cannot connect to the Rust workflow service.")?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            let (mut socket, _) = tungstenite::client(request, stream)
                .context("Cannot connect to the Rust workflow service.")?;
            // Selection changes are coalesced. Bound idle reaction and handshake waits.
            socket
                .get_mut()
                .set_read_timeout(Some(Duration::from_millis(250)))?;
            socket.send(Message::Text(
                json!({"op":"subscribe","topic":"workspace"})
                    .to_string()
                    .into(),
            ))?;
            let mut subscribed: Option<Selection> = None;
            while alive.load(Ordering::Relaxed) {
                let wanted = selection.lock().unwrap().clone();
                if subscribed.as_ref() != Some(&wanted) {
                    if let Some(previous) = &subscribed
                        && let Some(id) = &previous.run
                    {
                        for kind in ["run", "result", "events"] {
                            socket.send(Message::Text(
                                json!({"op":"unsubscribe","topic":format!("{kind}:{id}")})
                                    .to_string()
                                    .into(),
                            ))?;
                        }
                    }
                    if retained != wanted {
                        retained = wanted.clone();
                        cursor = 0;
                    }
                    if let Some(id) = &wanted.run {
                        for kind in ["run", "result", "events"] {
                            let mut message =
                                json!({"op":"subscribe","topic":format!("{kind}:{id}")});
                            if kind == "events" {
                                message["after"] = json!(cursor);
                            }
                            socket.send(Message::Text(message.to_string().into()))?;
                        }
                    }
                    subscribed = Some(wanted.clone());
                }
                match socket.read() {
                    Ok(Message::Text(text)) => {
                        let data: Value = serde_json::from_str(&text)?;
                        let topic = data["topic"].as_str().unwrap_or("");
                        if topic == "workspace" {
                            if data["kind"] == "state" {
                                last_error.clear();
                                send.send_blocking(Event::Connected(true))?;
                                send.send_blocking(Event::Workspace(data["data"].clone()))?;
                            } else if data["kind"] == "error" {
                                bail!(
                                    "{}",
                                    data["error"].as_str().unwrap_or("Workspace unavailable.")
                                );
                            }
                        } else if let Some(id) = &wanted.run
                            && topic.split_once(':').is_some_and(|(kind, run)| {
                                run == id && ["run", "result", "events"].contains(&kind)
                            })
                            && *selection.lock().unwrap() == wanted
                        {
                            let after = if data["kind"] == "events" {
                                data["after"].as_u64()
                            } else {
                                None
                            };
                            send.send_blocking(Event::Live {
                                generation: wanted.generation,
                                message: data,
                            })?;
                            // Advance only after the UI's bounded delivery queue accepts the page.
                            if let Some(after) = after {
                                cursor = cursor.max(after);
                            }
                        }
                        // A failed optional run/result topic does not disconnect the workspace.
                    }
                    Ok(Message::Ping(_)) => socket.flush()?,
                    Ok(Message::Close(_)) => {
                        bail!("Connection lost. Retained activity may be out of date.")
                    }
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(e) => return Err(e.into()),
                    _ => (),
                }
            }
            let _ = socket.close(None);
            Ok(())
        })();
        if !alive.load(Ordering::Relaxed) {
            break;
        }
        if send.send_blocking(Event::Connected(false)).is_err() {
            break;
        }
        if let Err(error) = result {
            let message = error.to_string();
            if message != last_error {
                last_error = message.clone();
                if send.send_blocking(Event::Error(message)).is_err() {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{TcpListener, TcpStream},
        os::unix::fs::PermissionsExt,
        time::Instant,
    };
    type Socket = tungstenite::WebSocket<TcpStream>;
    fn send(socket: &mut Socket, value: Value) {
        socket
            .send(Message::Text(value.to_string().into()))
            .unwrap();
    }
    fn subscription(socket: &mut Socket, topic: &str) -> Value {
        loop {
            let message = socket.read().unwrap();
            if let Message::Text(text) = message {
                let v: Value = serde_json::from_str(&text).unwrap();
                if v["op"] == "subscribe" && v["topic"] == topic {
                    return v;
                }
            }
        }
    }
    // Tungstenite's callback contract owns the HTTP error response.
    #[allow(clippy::result_large_err)]
    fn accept(listener: &TcpListener) -> Socket {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        tungstenite::accept_hdr(
            stream,
            |request: &tungstenite::handshake::server::Request, response| {
                assert_eq!(
                    request.headers()["authorization"],
                    format!("Bearer {}", "a".repeat(64))
                );
                Ok(response)
            },
        )
        .unwrap()
    }
    #[test]
    fn run_topics_reconnect_with_cursor_and_isolate_optional_errors_and_new_selection() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let path = root.path().join("connection.json");
        std::fs::write(&path,json!({"protocol":1,"origin":format!("http://{}", listener.local_addr().unwrap()),"token":"a".repeat(64)}).to_string()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (finish, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut socket = accept(&listener);
            subscription(&mut socket, "workspace");
            send(
                &mut socket,
                json!({"topic":"workspace","kind":"state","data":{"tasks":[{"id":"ticket"}]}}),
            );
            subscription(&mut socket, "run:a");
            send(
                &mut socket,
                json!({"topic":"run:a","kind":"state","data":{"run":{"id":"a"}}}),
            );
            subscription(&mut socket, "result:a");
            send(
                &mut socket,
                json!({"topic":"result:a","kind":"error","error":"Result unavailable"}),
            );
            assert_eq!(subscription(&mut socket, "events:a")["after"], 0);
            send(
                &mut socket,
                json!({"topic":"events:a","kind":"events","events":[{"cursor":205}],"after":205}),
            );
            socket.close(None).unwrap();
            drop(socket);
            let mut socket = accept(&listener);
            subscription(&mut socket, "workspace");
            send(
                &mut socket,
                json!({"topic":"workspace","kind":"state","data":{"tasks":[{"id":"ticket"}]}}),
            );
            assert_eq!(subscription(&mut socket, "events:a")["after"], 205);
            send(
                &mut socket,
                json!({"topic":"events:a","kind":"events","events":[{"cursor":206}],"after":206}),
            );
            assert_eq!(subscription(&mut socket, "events:b")["after"], 0);
            send(
                &mut socket,
                json!({"topic":"run:a","kind":"state","data":{"run":{"id":"wrong-run"}}}),
            );
            send(
                &mut socket,
                json!({"topic":"run:b","kind":"state","data":{"run":{"id":"b"}}}),
            );
            send(
                &mut socket,
                json!({"topic":"events:b","kind":"events","events":[{"cursor":1}],"after":1}),
            );
            finished.recv_timeout(Duration::from_secs(8)).unwrap();
        });
        let (client, events) = Client::connect(path);
        let generation_a = client.watch_run(Some("a".into()));
        let mut generation_b = None;
        let mut error_seen = false;
        let mut reconnect_seen = false;
        let mut run_b = false;
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < until, "live topic test timed out");
            match events.try_recv() {
                Ok(Event::Workspace(v)) => {
                    assert!(v["tasks"].is_array(), "A run state replaced workspace")
                }
                Ok(Event::Connected(false)) => {
                    assert!(error_seen);
                    reconnect_seen = true;
                }
                Ok(Event::Live {
                    generation,
                    message,
                }) => {
                    assert_ne!(message["data"]["run"]["id"], "wrong-run");
                    match message["topic"].as_str().unwrap() {
                        "result:a" => {
                            assert_eq!(message["kind"], "error");
                            error_seen = true;
                        }
                        "events:a" => {
                            assert_eq!(generation, generation_a);
                            if message["after"] == 206 {
                                assert!(reconnect_seen);
                                generation_b = Some(client.watch_run(Some("b".into())));
                            }
                        }
                        "run:b" => {
                            assert_eq!(Some(generation), generation_b);
                            run_b = true;
                        }
                        "events:b" => {
                            assert!(run_b);
                            assert_eq!(Some(generation), generation_b);
                            break;
                        }
                        _ => (),
                    }
                }
                Ok(_) => (),
                Err(async_channel::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => panic!("{e}"),
            }
        }
        finish.send(()).unwrap();
        worker.join().unwrap();
        drop(client);
        drop(events);
    }
}
