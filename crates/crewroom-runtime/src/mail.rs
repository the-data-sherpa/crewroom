//! Fixed-endpoint Rust Agent Mail transport. A retry is permitted only when the
//! protocol establishes that it cannot repeat an ambiguous mutation.
use crewroom_core::{serialized::stringify, source_time};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Semaphore, watch},
    time::{Instant, sleep, timeout_at},
};
use url::Url;

#[derive(Debug, Clone)]
pub enum MailError {
    Timeout,
    Transport,
    Http { status: u16, retry_after: Duration },
    Protocol,
    Tool(String),
    Busy,
    Configuration,
}
impl fmt::Display for MailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "Rust Mail request budget exhausted."),
            Self::Transport => write!(
                f,
                "Rust Mail response was interrupted; mutation outcome may be unknown."
            ),
            Self::Http { status, .. } => write!(f, "Rust Mail request failed ({status})."),
            Self::Protocol => write!(f, "Rust Mail protocol request was rejected."),
            Self::Tool(reason) => f.write_str(reason),
            Self::Busy => write!(f, "Rust Mail pending request limit reached."),
            Self::Configuration => write!(
                f,
                "Rust Mail requires a loopback HTTP endpoint and a bounded request budget."
            ),
        }
    }
}
impl std::error::Error for MailError {}
type Result<T> = std::result::Result<T, MailError>;
enum Initialization {
    Empty,
    Pending(watch::Receiver<Option<Result<()>>>),
    Ready,
}
struct Inner {
    url: Url,
    token: String,
    session: Mutex<Option<String>>,
    initialized: Mutex<Initialization>,
    client: reqwest::Client,
    budget: Duration,
    pending: Semaphore,
}
#[derive(Clone)]
pub struct MailClient {
    inner: Arc<Inner>,
}
fn wall_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
/// HTTP Retry-After accepts seconds or a source timestamp, matching the retained
/// client. An invalid or past value supplies no additional delay. Fractional
/// milliseconds round upward, so a retry never precedes the server's deadline.
pub fn retry_after(value: Option<&str>, now: i64) -> Duration {
    let Some(raw) = value else {
        return Duration::ZERO;
    };
    let text = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let number = if text.is_empty() {
        Some(0.0)
    } else if let Some((digits, radix)) = [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ]
    .iter()
    .find_map(|(prefix, radix)| text.strip_prefix(prefix).map(|digits| (digits, *radix)))
    {
        if digits.is_empty() {
            None
        } else {
            digits.chars().try_fold(0.0, |acc, c| {
                c.to_digit(radix)
                    .map(|n| acc * f64::from(radix) + f64::from(n))
            })
        }
    } else {
        text.parse::<f64>().ok()
    };
    let millis = number
        .filter(|v| v.is_finite())
        .map(|v| v * 1000.0)
        .unwrap_or_else(|| {
            source_time::parse(raw)
                .map(|date| date.saturating_sub(now) as f64)
                .unwrap_or(0.0)
        });
    if millis.is_finite() {
        Duration::from_millis(millis.max(0.0).ceil() as u64)
    } else {
        Duration::ZERO
    }
}
fn wire_error(error: reqwest::Error) -> MailError {
    if error.is_timeout() {
        MailError::Timeout
    } else {
        MailError::Transport
    }
}
fn text_content(value: &Value, separator: &str) -> Result<String> {
    let content = value.as_array().ok_or(MailError::Protocol)?;
    let mut parts = Vec::new();
    for part in content {
        if part["type"] == "text" {
            parts.push(part["text"].as_str().ok_or(MailError::Protocol)?);
        }
    }
    Ok(parts.join(separator))
}
fn safe_busy(reason: &str) -> bool {
    serde_json::from_str::<Value>(reason).is_ok_and(|v| {
        v["error"]["recoverable"] == true
            && v["error"]["data"]["db_error_classification"]["safe_to_retry"] == true
    })
}
fn retry_delay(name: &str, args: &Value, error: &MailError, attempt: u32) -> Option<Duration> {
    if attempt >= 5 {
        return None;
    }
    let read = name.starts_with("fetch_") || name.starts_with("get_");
    let extra = match error {
        MailError::Transport if read => Duration::ZERO,
        MailError::Http {
            status: 503,
            retry_after,
        } if read => *retry_after,
        MailError::Http {
            status: 429,
            retry_after,
        } if read || name == "send_message" && args["idempotency_key"].is_string() => *retry_after,
        _ => return None,
    };
    let wait = Duration::from_millis(100u64 << (attempt - 1)).max(extra);
    (wait <= Duration::from_secs(5)).then_some(wait)
}
impl MailClient {
    pub fn new(url: &str, token: String) -> Result<Self> {
        Self::with_budget(url, token, Duration::from_secs(12))
    }
    pub fn with_budget(url: &str, token: String, budget: Duration) -> Result<Self> {
        let url = Url::parse(url).map_err(|_| MailError::Configuration)?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || budget.is_zero()
            || budget > Duration::from_secs(60)
        {
            return Err(MailError::Configuration);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| MailError::Configuration)?;
        Ok(Self {
            inner: Arc::new(Inner {
                url,
                token,
                session: Mutex::new(None),
                initialized: Mutex::new(Initialization::Empty),
                client,
                budget,
                pending: Semaphore::new(64),
            }),
        })
    }
    async fn rpc(
        &self,
        method: &str,
        params: Value,
        notification: bool,
        deadline: Instant,
    ) -> Result<Value> {
        if Instant::now() >= deadline {
            return Err(MailError::Timeout);
        }
        timeout_at(deadline, async {
            let id = uuid::Uuid::new_v4().to_string();
            let mut body = json!({"jsonrpc":"2.0","method":method,"params":params});
            if !notification {
                body["id"] = json!(id);
            }
            let mut request = self
                .inner
                .client
                .post(self.inner.url.clone())
                .header("Content-Type", "application/json")
                .header("Accept", "application/json, text/event-stream")
                .bearer_auth(&self.inner.token)
                .body(stringify(&body));
            if let Some(session) = self.inner.session.lock().unwrap().clone() {
                request = request.header("Mcp-Session-Id", session);
            }
            let response = request.send().await.map_err(wire_error)?;
            if !response.status().is_success() {
                return Err(MailError::Http {
                    status: response.status().as_u16(),
                    retry_after: retry_after(
                        response
                            .headers()
                            .get("Retry-After")
                            .and_then(|v| v.to_str().ok()),
                        wall_now(),
                    ),
                });
            }
            if let Some(value) = response.headers().get("Mcp-Session-Id") {
                let session = value.to_str().map_err(|_| MailError::Protocol)?;
                if session.len() > 4096 {
                    return Err(MailError::Protocol);
                }
                *self.inner.session.lock().unwrap() = Some(session.into());
            }
            let sse = response
                .headers()
                .get("Content-Type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|s| s.contains("text/event-stream"));
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(wire_error)?;
                if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                    return Err(MailError::Protocol);
                }
                bytes.extend_from_slice(&chunk);
            }
            if notification {
                return Ok(Value::Null);
            }
            let raw = std::str::from_utf8(&bytes).map_err(|_| MailError::Protocol)?;
            let result: Value = if sse {
                let mut found = None;
                for line in raw.lines().filter_map(|line| line.strip_prefix("data:")) {
                    let value: Value =
                        serde_json::from_str(line).map_err(|_| MailError::Protocol)?;
                    if value["id"] == id && found.replace(value).is_some() {
                        return Err(MailError::Protocol);
                    }
                }
                found.ok_or(MailError::Protocol)?
            } else {
                serde_json::from_str(raw).map_err(|_| MailError::Protocol)?
            };
            if result["id"] != id || result.get("error").is_some_and(|v| !v.is_null()) {
                return Err(MailError::Protocol);
            }
            result.get("result").cloned().ok_or(MailError::Protocol)
        })
        .await
        .map_err(|_| MailError::Timeout)?
    }
    async fn initialize(&self, deadline: Instant) -> Result<()> {
        let mut waiting = {
            let mut state = self.inner.initialized.lock().unwrap();
            match &*state {
                Initialization::Ready => return Ok(()),
                Initialization::Pending(receiver) => receiver.clone(),
                Initialization::Empty => {
                    let (sender, receiver) = watch::channel(None);
                    *state = Initialization::Pending(receiver.clone());
                    let client = self.clone();
                    // A caller disconnect cannot cancel initialization shared by
                    // another call. Every waiter sees this generation's result.
                    tokio::spawn(async move {
                        let result = async {
                            client.rpc("initialize", json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"crewroom","version":"0.1.0"}}), false, deadline).await?;
                            client.rpc("notifications/initialized", json!({}), true, deadline).await?;
                            Ok(())
                        }.await;
                        *client.inner.initialized.lock().unwrap() = if result.is_ok() {
                            Initialization::Ready
                        } else {
                            Initialization::Empty
                        };
                        sender.send_replace(Some(result));
                    });
                    receiver
                }
            }
        };
        timeout_at(deadline, async {
            loop {
                if let Some(result) = waiting.borrow().clone() {
                    return result;
                }
                waiting.changed().await.map_err(|_| MailError::Protocol)?;
            }
        })
        .await
        .map_err(|_| MailError::Timeout)?
    }
    /// Read-only readiness check (FOR-83): the MCP handshake only, with no tool
    /// call, so no message, agent or project is created.
    pub async fn probe(&self) -> Result<()> {
        self.initialize(Instant::now() + self.inner.budget).await
    }
    pub async fn call(&self, name: &str, args: Value) -> Result<Value> {
        self.call_before(name, args, Instant::now() + self.inner.budget)
            .await
    }
    pub(crate) async fn call_before(
        &self,
        name: &str,
        args: Value,
        deadline: Instant,
    ) -> Result<Value> {
        let deadline = deadline.min(Instant::now() + self.inner.budget);
        let _pending = self
            .inner
            .pending
            .try_acquire()
            .map_err(|_| MailError::Busy)?;
        self.initialize(deadline).await?;
        for attempt in 1..=5 {
            let response = match self
                .rpc(
                    "tools/call",
                    json!({"name":name,"arguments":args}),
                    false,
                    deadline,
                )
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let Some(wait) = retry_delay(name, &args, &error, attempt) else {
                        return Err(error);
                    };
                    if Instant::now() + wait >= deadline {
                        return Err(error);
                    }
                    sleep(wait).await;
                    continue;
                }
            };
            if !response.is_object() || response.get("isError").is_some_and(|v| !v.is_boolean()) {
                return Err(MailError::Protocol);
            }
            if response["isError"] != true {
                if let Some(structured) = response.get("structuredContent").filter(|v| !v.is_null())
                {
                    return Ok(structured.clone());
                }
                return serde_json::from_str(&text_content(&response["content"], "\n")?)
                    .map_err(|_| MailError::Protocol);
            }
            let reason = text_content(response.get("content").unwrap_or(&json!([])), " ")?;
            if !safe_busy(&reason) || attempt == 5 {
                let safe = crate::host_protocol::redact(
                    &json!(reason),
                    std::slice::from_ref(&self.inner.token),
                );
                let mut units = 0;
                let excerpt: String = safe
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take_while(|c| {
                        units += c.len_utf16();
                        units <= 300
                    })
                    .collect();
                return Err(MailError::Tool(format!(
                    "Rust Mail rejected {name}{}",
                    if excerpt.is_empty() {
                        ".".into()
                    } else {
                        format!(": {excerpt}")
                    }
                )));
            }
            let wait = Duration::from_millis(100u64 << (attempt - 1));
            if Instant::now() + wait >= deadline {
                return Err(MailError::Timeout);
            }
            sleep(wait).await;
        }
        unreachable!("Every fifth outcome returns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_after_dates_numbers_and_invalid_values_are_bounded() {
        let now = source_time::parse("2026-10-03T00:00:00Z").unwrap();
        for (raw, millis) in [
            (None, 0),
            (Some(""), 0),
            (Some("\u{feff}0.25\u{feff}"), 250),
            (Some("-3"), 0),
            (Some("0x10"), 16000),
            (Some("0b11"), 3000),
            (Some("0o10"), 8000),
            (Some("0x"), 0),
            (Some("no delay"), 0),
            (Some("Infinity"), 0),
            (Some("NaN"), 0),
            (Some("1e-4"), 1),
            (Some("Sat, 03 Oct 2026 00:00:02 GMT"), 2000),
            (Some("2026-10-02T00:00:00Z"), 0),
        ] {
            assert_eq!(
                retry_after(raw, now),
                Duration::from_millis(millis),
                "{raw:?}"
            );
        }
    }
    #[test]
    fn fixed_endpoint_and_deadline_are_required() {
        for url in [
            "https://127.0.0.1/mcp",
            "http://localhost/mcp",
            "http://example.com/mcp",
            "http://u:p@127.0.0.1/mcp",
            "http://127.0.0.1/mcp?q=1",
            "http://127.0.0.1/mcp#secret",
        ] {
            assert!(MailClient::new(url, "secret".into()).is_err());
        }
        for budget in [Duration::ZERO, Duration::from_secs(61)] {
            assert!(
                MailClient::with_budget("http://127.0.0.1/mcp", "secret".into(), budget).is_err()
            );
        }
        assert!(MailClient::new("http://127.0.0.1:9000/mcp/", "secret".into()).is_ok());
    }
    #[test]
    fn only_definitely_safe_outcomes_retry() {
        for name in [
            "send_message",
            "register_agent",
            "acknowledge_message",
            "mark_message_read",
        ] {
            for error in [
                MailError::Transport,
                MailError::Protocol,
                MailError::Timeout,
                MailError::Http {
                    status: 503,
                    retry_after: Duration::ZERO,
                },
            ] {
                assert!(retry_delay(name, &json!({"idempotency_key":"x"}), &error, 1).is_none());
            }
        }
        assert!(!safe_busy(r#"{"error":{"recoverable":true}}"#));
        assert!(!safe_busy(
            r#"{"error":{"recoverable":"true","data":{"db_error_classification":{"safe_to_retry":true}}}}"#
        ));
        assert!(safe_busy(
            r#"{"error":{"recoverable":true,"data":{"db_error_classification":{"safe_to_retry":true}}}}"#
        ));
        assert!(retry_delay("fetch_inbox", &json!({}), &MailError::Transport, 5).is_none());
    }
}
