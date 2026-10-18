//! A bounded loopback HTTP peer, not live provider delivery.
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub(super) struct Provider {
    pub origin: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Provider {
    pub fn new(handler: impl Fn(&str) -> (u16, Value) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            for socket in listener.incoming() {
                let mut socket = socket.unwrap();
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0u8; 2048];
                while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                    let count = socket.read(&mut bytes).unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&bytes[..count]);
                    assert!(request.len() < 16384);
                }
                let request = String::from_utf8(request).unwrap();
                let header = request.lines().next().unwrap();
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer synthetic-native-provider")
                );
                let (status, body) = handler(header);
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).unwrap();
            }
        });
        Self {
            origin: format!("http://{address}"),
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.origin.trim_start_matches("http://"));
        self.worker.take().unwrap().join().unwrap();
    }
}
pub(super) fn credentials() -> crewroom_runtime::providers::Credentials {
    Arc::new(|| {
        Box::pin(async {
            Ok(crewroom_runtime::providers::Credential::Bearer(
                "synthetic-native-provider".into(),
            ))
        })
    })
}
