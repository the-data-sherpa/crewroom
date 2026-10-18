//! Authenticated provider proxy. Only trusted configuration supplies upstream credentials.
use crate::{
    conflict, durable_new,
    provider_input::{ReasoningBudget, count, has_bounded_text_input},
    provider_usage::UsageReader,
    read,
};
use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, Request, Response},
};
use crewroom_core::{Result, serialized::stringify};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep_until},
};
use tokio_util::sync::CancellationToken;
use url::Url;
#[derive(Clone)]
pub struct ProviderGateway {
    inner: Arc<Inner>,
}
struct Inner {
    root: PathBuf,
    plan: Value,
    provider: Value,
    token: String,
    observe: Arc<dyn Fn(Value) + Send + Sync>,
    reasoning: Mutex<ReasoningBudget>,
    failure: Mutex<Option<Value>>,
    stopped: CancellationToken,
    client: reqwest::Client,
}
impl Inner {
    fn fail(&self, kind: &str, retryable: bool, status: Option<u16>) {
        let mut failure = json!({"kind":kind,"retryable":retryable});
        if let Some(status) = status {
            failure["status"] = json!(status);
        }
        *self.failure.lock().unwrap() = Some(failure.clone());
        failure["type"] = json!("provider_failure");
        (self.observe)(failure);
    }
}
fn empty(status: u16) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from("{}"))
        .unwrap()
}
fn denial(message: &str) -> Response<Body> {
    Response::builder()
        .status(403)
        .header("Content-Type", "application/json")
        .body(Body::from(stringify(
            &json!({"error":{"type":"crewroom_assignment_policy","message":message}}),
        )))
        .unwrap()
}
fn header<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    headers.get(key).and_then(|v| v.to_str().ok())
}
fn decoded_path(text: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut source = text.as_bytes().iter();
    while let Some(b) = source.next() {
        if *b != b'%' {
            bytes.push(*b);
            continue;
        }
        let mut hex = || {
            source
                .next()
                .and_then(|b| (*b as char).to_digit(16))
                .ok_or_else(|| conflict("Invalid provider model path."))
        };
        let a = hex()?;
        let b = hex()?;
        bytes.push((a * 16 + b) as u8);
    }
    String::from_utf8(bytes).map_err(|_| conflict("Invalid provider model path."))
}
impl ProviderGateway {
    pub fn new(
        root: &Path,
        plan: Value,
        provider: Value,
        token: String,
        budget_root: Option<&Path>,
        observe: impl Fn(Value) + Send + Sync + 'static,
    ) -> Result<Self> {
        if let Some(timeout) = provider.get("requestTimeoutMs")
            && count(timeout).is_none_or(|n| !(100.0..=3_600_000.0).contains(&n))
        {
            return Err(conflict("Provider request timeout must be 100–3600000 ms."));
        }
        if let Some(mode) = provider.get("authMode")
            && mode != "native"
            && mode != "bearer"
        {
            return Err(conflict("Unsupported provider authentication mode."));
        }
        let reasoning = ReasoningBudget::open(
            budget_root.unwrap_or(root),
            plan["resolved"]["selector"]
                .as_str()
                .ok_or_else(|| conflict("Missing assignment selector."))?,
        )?;
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| conflict("Provider transport initialization failed."))?;
        Ok(Self {
            inner: Arc::new(Inner {
                root: root.into(),
                plan,
                provider,
                token,
                observe: Arc::new(observe),
                reasoning: Mutex::new(reasoning),
                failure: Mutex::new(None),
                stopped: CancellationToken::new(),
                client,
            }),
        })
    }
    pub fn close(&self) {
        self.inner.stopped.cancel();
    }
    pub fn failure(&self) -> Option<Value> {
        self.inner.failure.lock().unwrap().clone()
    }
    pub async fn serve(&self, request: Request<Body>) -> Result<Response<Body>> {
        let inner = &self.inner;
        let url = Url::parse(&format!("http://127.0.0.1{}", request.uri()))
            .map_err(|_| conflict("Invalid provider route."))?;
        let authenticated = header(request.headers(), "authorization")
            == Some(&format!("Bearer {}", inner.token))
            || ["x-api-key", "x-goog-api-key"]
                .iter()
                .any(|h| header(request.headers(), h) == Some(inner.token.as_str()))
            || url
                .query_pairs()
                .find(|(k, _)| k == "key")
                .is_some_and(|(_, v)| v == inner.token);
        if !authenticated || request.method() != Method::POST || inner.stopped.is_cancelled() {
            return Ok(empty(403));
        }
        let (parts, body) = request.into_parts();
        let mut stream = body.into_data_stream();
        let mut raw = Vec::new();
        while let Some(chunk) = tokio::select! {value=stream.next()=>value,_=inner.stopped.cancelled()=>return Ok(empty(403))}
        {
            let chunk = chunk.map_err(|_| conflict("Provider request body was interrupted."))?;
            if raw.len() + chunk.len() > 12 * 1024 * 1024 {
                return Err(conflict("Provider request exceeded its limit."));
            }
            raw.extend_from_slice(&chunk);
        }
        let raw = String::from_utf8(raw)
            .map_err(|_| conflict("Provider request must contain valid UTF-8."))?;
        if raw.encode_utf16().count() > 4 * 1024 * 1024 {
            return Err(conflict("Provider request exceeded its limit."));
        }
        let body: Value = serde_json::from_str(&raw)?;
        let model = &inner.plan["resolved"];
        let api = model["api"].as_str().unwrap_or("");
        let base = Url::parse(inner.provider["baseUrl"].as_str().unwrap_or(""))
            .map_err(|_| conflict("Invalid upstream provider URL."))?;
        let base_path = base.path().strip_suffix('/').unwrap_or(base.path());
        let prefix = format!("/provider{base_path}");
        let Some(route) = url
            .path()
            .strip_prefix(&prefix)
            .filter(|s| s.starts_with('/'))
        else {
            return Ok(empty(403));
        };
        let wire = match api {
            "openai-responses" if route == "/responses" => {
                json!({"model":body["model"],"effort":body["reasoning"]["effort"]})
            }
            "anthropic-messages" if ["/messages", "/v1/messages"].contains(&route) => {
                json!({"model":body["model"],"thinking":body["thinking"]["type"],"budget":body["thinking"]["budget_tokens"],"effort":body["output_config"]["effort"]})
            }
            "google-generative-ai"
                if route.starts_with("/models/") && route.ends_with(":streamGenerateContent") =>
            {
                json!({"model":decoded_path(&route[8..route.len()-22])?,"effort":body["generationConfig"]["thinkingConfig"]["thinkingLevel"],"budget":body["generationConfig"]["thinkingConfig"]["thinkingBudget"],"includeThoughts":body["generationConfig"]["thinkingConfig"]["includeThoughts"]})
            }
            _ => return Ok(empty(403)),
        };
        // Match omission of undefined model fields in the retained wire baseline.
        let mut wire = wire;
        if api != "google-generative-ai" && body.get("model").is_none() {
            wire.as_object_mut().unwrap().remove("model");
        }
        let baseline = inner.root.join("provider-policy.json");
        let serialized =
            stringify(&json!({"selector":model["selector"],"effort":model["effort"],"wire":wire}));
        let invalid_effort = api == "openai-responses"
            && if model["effort"] == "off" {
                !wire["effort"].is_null() && wire["effort"] != "none"
            } else {
                wire["effort"] != model["effort"]
            };
        // The same lock orders first baseline admission and opaque-budget persistence.
        {
            let reasoning = inner.reasoning.lock().unwrap();
            let saved = match read(&baseline) {
                Ok(bytes) => Some(bytes),
                Err(crewroom_core::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    None
                }
                Err(e) => return Err(e),
            };
            if wire["model"] != model["id"]
                || invalid_effort
                || saved.as_ref().is_some_and(|b| b != serialized.as_bytes())
            {
                (inner.observe)(
                    json!({"type":"model_policy_rejected","requestedModel":model["selector"],"requestedEffort":model["effort"],"attempted":wire}),
                );
                return Ok(denial("Crewroom rejected a model or effort substitution."));
            }
            let limits = inner.provider["models"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|v| v["id"] == model["id"])
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let configured = if limits.len() == 1 {
                limits[0]
            } else {
                &Value::Null
            };
            let output = [
                &body["max_output_tokens"],
                &body["max_tokens"],
                &body["generationConfig"]["maxOutputTokens"],
                &configured["maxTokens"],
            ]
            .into_iter()
            .find(|v| !v.is_null())
            .unwrap_or(&Value::Null);
            let mut opaque_tokens = 0.0;
            let mut unknown = false;
            let bounded = has_bounded_text_input(api, &body, &mut |kind, value| {
                if let Some(tokens) = reasoning.tokens(kind, value) {
                    opaque_tokens += tokens;
                    true
                } else {
                    unknown = true;
                    false
                }
            });
            let context = count(&configured["contextWindow"]).filter(|n| *n > 0.0);
            let max = count(&configured["maxTokens"]).filter(|n| *n > 0.0);
            let output_count = count(output).filter(|n| *n > 0.0);
            let cause = match (context, max, output_count) {
                (None, _, _) | (_, None, _) => Some("model_limits"),
                (_, _, None) => Some("output_limit"),
                (Some(context), Some(max), Some(output)) => {
                    if output > max {
                        Some("output_limit")
                    } else if !bounded {
                        Some(if unknown {
                            "unknown_reasoning"
                        } else {
                            "unsupported_input"
                        })
                    } else if raw.len() as f64 + opaque_tokens + output > context {
                        Some("context_window")
                    } else {
                        None
                    }
                }
            };
            if let Some(cause) = cause {
                (inner.observe)(
                    json!({"type":"model_policy_rejected","code":"input_budget","cause":cause,"recovery":if ["context_window","unknown_reasoning"].contains(&cause) {"fresh_session"} else {"configuration"},"reason":"Provider input budget is unavailable or exceeded; multimodal input has no verified budget.","budget":{"inputBytes":raw.len(),"opaqueTokens":opaque_tokens,"outputTokens":output,"contextWindow":configured["contextWindow"],"maxOutputTokens":configured["maxTokens"],"method":"serialized-utf8-bytes-plus-reported-opaque-output"}}),
                );
                return Ok(denial(
                    "Input budget is unavailable or exceeded. Retain the work and configure verified model limits or shorten the input.",
                ));
            }
            (inner.observe)(
                json!({"type":"provider_budget","inputBytes":raw.len(),"opaqueTokens":opaque_tokens,"outputTokens":output,"contextWindow":configured["contextWindow"],"method":"serialized-utf8-bytes-plus-reported-opaque-output"}),
            );
            if saved.is_none() {
                durable_new(&baseline, serialized.as_bytes())?;
            }
        }
        if inner.stopped.is_cancelled() {
            return Ok(empty(403));
        }
        let mut upstream = Url::parse(&format!(
            "{}{}",
            base.origin().ascii_serialization(),
            url.path().strip_prefix("/provider").unwrap()
        ))
        .map_err(|_| conflict("Invalid upstream provider URL."))?;
        for key in ["beta", "alt"] {
            if let Some((_, value)) = url.query_pairs().find(|(k, _)| k == key) {
                upstream.query_pairs_mut().append_pair(key, &value);
            }
        }
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "content-type",
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        if let Some(configured) = inner.provider["headers"].as_object() {
            for (name, value) in configured {
                if ["authorization", "x-api-key", "x-goog-api-key"]
                    .contains(&name.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                headers.insert(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| conflict("Invalid upstream header name."))?,
                    reqwest::header::HeaderValue::from_str(
                        value
                            .as_str()
                            .ok_or_else(|| conflict("Invalid upstream header value."))?,
                    )
                    .map_err(|_| conflict("Invalid upstream header value."))?,
                );
            }
        }
        let key = inner.provider["apiKey"].as_str().unwrap_or("");
        let bearer = inner.provider["authMode"] == "bearer";
        let (auth, value) = if api == "anthropic-messages" && !bearer {
            ("x-api-key", key.to_owned())
        } else if api == "google-generative-ai" && !bearer {
            ("x-goog-api-key", key.to_owned())
        } else {
            ("authorization", format!("Bearer {key}"))
        };
        headers.insert(
            reqwest::header::HeaderName::from_static(auth),
            reqwest::header::HeaderValue::from_str(&value)
                .map_err(|_| conflict("Invalid provider credential encoding."))?,
        );
        if api == "anthropic-messages" {
            for name in ["anthropic-version", "anthropic-beta"] {
                if let Some(value) = parts.headers.get(name) {
                    headers.insert(name, value.clone());
                }
            }
        }
        if !headers.contains_key("accept-encoding") {
            headers.insert(
                "accept-encoding",
                reqwest::header::HeaderValue::from_static(if upstream.scheme() == "https" {
                    "br, gzip, deflate, zstd"
                } else {
                    "gzip, deflate"
                }),
            );
        }
        *inner.failure.lock().unwrap() = None;
        (inner.observe)(json!({"type":"provider_request","identity":wire}));
        let deadline = Instant::now()
            + Duration::from_millis(
                inner.provider["requestTimeoutMs"]
                    .as_f64()
                    .unwrap_or(300000.0) as u64,
            );
        let mut observation = RequestObservation::new(inner.clone());
        let response = tokio::select! {
            response=inner.client.post(upstream).headers(headers).body(raw).send()=>response.ok(),
            _=sleep_until(deadline)=>{observation.timed_out=true;None},
            _=inner.stopped.cancelled()=>None,
        };
        let Some(response) = response else {
            observation.finish(true);
            return Ok(Response::builder()
                .status(502)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    "{\"error\":{\"message\":\"Provider response was interrupted.\"}}",
                ))
                .unwrap());
        };
        // Redirects are a failed transport, as fetch redirect:error is in the retained host.
        if [301, 302, 303, 307, 308].contains(&response.status().as_u16())
            || crate::provider_body::validate(&response).is_err()
        {
            observation.finish(true);
            return Ok(Response::builder()
                .status(502)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    "{\"error\":{\"message\":\"Provider response was interrupted.\"}}",
                ))
                .unwrap());
        }
        let status = response.status().as_u16();
        observation.status = status;
        if !(200..300).contains(&status) {
            inner.fail(
                if [401, 403].contains(&status) {
                    "authorization"
                } else if status == 429 {
                    "rate_limit"
                } else if [408, 504].contains(&status) {
                    "timeout"
                } else if status >= 500 {
                    "unavailable"
                } else {
                    "rejected"
                },
                [408, 429].contains(&status) || status >= 500,
                Some(status),
            );
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_owned();
        observation.reader = Some(UsageReader::new(
            api,
            content_type.contains("text/event-stream"),
        ));
        let (tx, rx) = mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(4);
        let failed = Arc::new(AtomicBool::new(false));
        let stream_failed = failed.clone();
        tokio::spawn(async move {
            let mut body = Box::pin(crate::provider_body::stream(response));
            let mut interrupted = false;
            loop {
                let next = tokio::select! {
                    value=body.next()=>value,
                    _=sleep_until(deadline)=>{observation.timed_out=true;interrupted=true;None},
                    _=observation.inner.stopped.cancelled()=>{interrupted=true;None},
                    _=tx.closed()=>{interrupted=true;None},
                };
                match next {
                    None => break,
                    Some(Err(_)) => {
                        interrupted = true;
                        break;
                    }
                    Some(Ok(bytes)) => {
                        observation.reader.as_mut().unwrap().push(&bytes);
                        observation.collect();
                        // A runner can dispatch its next tool turn as soon as it sees
                        // the terminal event. Persist its budget before releasing that
                        // event, rather than racing the next request against EOF.
                        if let Some(output) = observation
                            .reader
                            .as_ref()
                            .unwrap()
                            .completed_output_tokens()
                            && !observation.opaque.is_empty()
                            && (200..300).contains(&observation.status)
                        {
                            let keys = observation.opaque.iter().cloned().collect();
                            if observation
                                .inner
                                .reasoning
                                .lock()
                                .unwrap()
                                .remember_keys(keys, output)
                                .is_err()
                            {
                                observation.budget_failed = true;
                                observation.inner.fail("budget_storage", false, None);
                                interrupted = true;
                                break;
                            }
                            observation.opaque.clear();
                        }
                        let sent = tokio::select! {value=tx.send(Ok(bytes))=>value.is_ok(),_=sleep_until(deadline)=>{observation.timed_out=true;false},_=observation.inner.stopped.cancelled()=>false};
                        if !sent {
                            interrupted = true;
                            break;
                        }
                    }
                }
            }
            observation.finish(interrupted);
            stream_failed.store(interrupted, Ordering::SeqCst);
        });
        let body = futures_util::stream::unfold((rx, failed), |(mut rx, failed)| async move {
            match rx.recv().await {
                Some(value) => Some((value, (rx, failed))),
                None if failed.swap(false, Ordering::SeqCst) => Some((
                    Err(std::io::Error::other("Provider response was interrupted.")),
                    (rx, failed),
                )),
                None => None,
            }
        });
        Ok(Response::builder()
            .status(status)
            .header("Content-Type", content_type)
            .header("Cache-Control", "no-store")
            .body(Body::from_stream(body))
            .unwrap())
    }
}
struct RequestObservation {
    inner: Arc<Inner>,
    reader: Option<UsageReader>,
    status: u16,
    timed_out: bool,
    finished: bool,
    opaque: HashSet<String>,
    budget_failed: bool,
}
impl RequestObservation {
    fn new(inner: Arc<Inner>) -> Self {
        Self {
            inner,
            reader: None,
            status: 0,
            timed_out: false,
            finished: false,
            opaque: HashSet::new(),
            budget_failed: false,
        }
    }
    fn collect(&mut self) {
        if let Some(reader) = self.reader.as_mut() {
            let budget = self.inner.reasoning.lock().unwrap();
            for (kind, value) in reader.take_opaque() {
                self.opaque.insert(budget.key(&kind, &value));
            }
        }
    }
    fn finish(&mut self, interrupted: bool) {
        if self.finished {
            return;
        }
        self.finished = true;
        if interrupted
            && !self.budget_failed
            && !self.inner.stopped.is_cancelled()
            && (self.status == 0 || (200..300).contains(&self.status))
        {
            self.inner.fail(
                if self.timed_out {
                    "timeout"
                } else {
                    "disconnect"
                },
                true,
                None,
            );
        }
        let receipt=self.reader.as_mut().map(|r|r.finish(self.status,interrupted)).unwrap_or_else(||json!({"type":"provider_usage","status":0,"state":"unavailable","reason":"The provider request failed before a response."}));
        self.collect();
        let mut persistence_failed = false;
        if receipt["state"] == "actual" {
            let result = self.inner.reasoning.lock().unwrap().remember_keys(
                self.opaque.drain().collect(),
                receipt["usage"]["output"].as_f64().unwrap(),
            );
            if result.is_err() {
                persistence_failed = true;
                self.inner.fail("budget_storage", false, None);
            }
        }
        (self.inner.observe)(receipt);
        if (200..300).contains(&self.status)
            && !interrupted
            && !persistence_failed
            && let Some(kind) = self.reader.as_ref().and_then(UsageReader::outcome)
        {
            self.inner.fail(kind, true, None);
        }
    }
}
impl Drop for RequestObservation {
    fn drop(&mut self) {
        self.finish(true);
    }
}
