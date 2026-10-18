//! Fixed-loopback host control. Credentials cannot follow a redirect, proxy,
//! caller-selected route or an unbounded response.
use crate::{
    conflict,
    host_journal::MAX_RECORD_BYTES,
    host_protocol::{HOST_INPUT_LIMIT, parse_request},
};
use crewroom_core::{Result, serialized::stringify};
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

pub(crate) fn origin(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| conflict("Invalid host origin."))?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none_or(|p| p == 0)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(conflict("Host control requires one fixed loopback origin."));
    }
    Ok(url)
}
#[derive(Clone)]
pub struct HostClient {
    origin: Url,
    token: String,
    client: reqwest::Client,
}
impl HostClient {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        if token.len() < 16 || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(conflict("Invalid private host credential."));
        }
        Ok(Self {
            origin: origin(url)?,
            token: token.into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .build()
                .map_err(|_| conflict("Unable to create local host transport."))?,
        })
    }
    pub fn url(&self) -> &str {
        self.origin.as_str()
    }
    pub fn authenticates(&self, authorization: &str) -> bool {
        let expected = format!("Bearer {}", self.token);
        let presented = authorization.as_bytes();
        let wanted = expected.as_bytes();
        if presented.len() != wanted.len() {
            return false;
        }
        presented
            .iter()
            .zip(wanted)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
    }
    pub async fn request(&self, body: &Value, cancel: &CancellationToken) -> Result<Value> {
        let bytes = stringify(body).into_bytes();
        if bytes.len() > HOST_INPUT_LIMIT * 3 {
            return Err(conflict("Host request exceeds its bounded input limit."));
        }
        let parsed = parse_request(&bytes)?;
        let millis = match parsed["op"].as_str() {
            Some("close") => 15000,
            Some("events") => {
                parsed["wait"].as_f64().unwrap_or(0.0).clamp(0.0, 25000.0) as u64 + 3000
            }
            _ => 3000,
        };
        let operation = async {
            let response = self
                .client
                .post(self.origin.clone())
                .bearer_auth(&self.token)
                .header("Content-Type", "application/json")
                .header("Origin", self.origin.as_str().trim_end_matches('/'))
                .body(bytes)
                .send()
                .await
                .map_err(|_| conflict("Agent host control is unavailable."))?;
            let status = response.status();
            // A page may contain one maximum-size event plus its JSON envelope.
            let limit = MAX_RECORD_BYTES + 4096;
            if response.content_length().is_some_and(|n| n > limit as u64) {
                return Err(conflict("Host response exceeds its bounded size."));
            }
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| conflict("Host response was interrupted."))?;
                if body.len() + chunk.len() > limit {
                    return Err(conflict("Host response exceeds its bounded size."));
                }
                body.extend_from_slice(&chunk);
            }
            // Host error bodies may contain runtime/provider information. Keep
            // them in the host journal, not unauthenticated error surfaces.
            if !status.is_success() {
                return Err(conflict("Agent host refused the control request."));
            }
            serde_json::from_slice(&body).map_err(|_| conflict("Host response is not valid JSON."))
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(conflict("Host observation or control was cancelled.")),
            result = tokio::time::timeout(Duration::from_millis(millis),operation) => result.map_err(|_|conflict("Agent host request timed out."))?,
        }
    }
}
