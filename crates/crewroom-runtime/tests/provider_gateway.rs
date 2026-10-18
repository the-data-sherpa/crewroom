use axum::{
    Router,
    body::{Body, Bytes},
    http::{Request, Response, StatusCode},
};
use crewroom_runtime::provider_gateway::ProviderGateway;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    task::JoinHandle,
    time::{sleep, timeout},
};
struct Upstream {
    url: String,
    task: JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn upstream(router: Router) -> Upstream {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Upstream { url, task }
}
fn plan(api: &str) -> Value {
    json!({"resolved":{"selector":"fixture/model","id":"model","api":api,"provider":"fixture","effort":"off"}})
}
fn config(base: &str) -> Value {
    json!({"baseUrl":format!("{base}/v1"),"apiKey":"private-key","requestTimeoutMs":100.0,"models":[{"id":"model","contextWindow":20000000,"maxTokens":1000}]})
}
fn request(route: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(route)
        .header("Authorization", "Bearer host-token")
        .body(Body::from(body.to_string()))
        .unwrap()
}
async fn read(response: Response<Body>) -> std::result::Result<Vec<u8>, axum::Error> {
    let mut stream = response.into_body().into_data_stream();
    let mut bytes = vec![];
    while let Some(chunk) = stream.next().await {
        bytes.extend(chunk?);
    }
    Ok(bytes)
}
fn gateway(
    root: &std::path::Path,
    api: &str,
    provider: Value,
    events: &Arc<Mutex<Vec<Value>>>,
) -> ProviderGateway {
    let events = events.clone();
    ProviderGateway::new(
        root,
        plan(api),
        provider,
        "host-token".into(),
        None,
        move |e| events.lock().unwrap().push(e),
    )
    .unwrap()
}
#[tokio::test]
async fn authentication_route_and_query_boundaries_do_not_forward_host_credentials() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let captured = received.clone();
    let server = upstream(Router::new().fallback(move |r: Request<Body>| {
        let captured = captured.clone();
        async move {
            captured
                .lock()
                .unwrap()
                .push((r.uri().to_string(), r.headers().clone()));
            "{}"
        }
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let gateway = gateway(
        root.path(),
        "google-generative-ai",
        config(&server.url),
        &events,
    );
    let body = json!({"contents":[{"parts":[{"text":"fixture"}]}]});
    for auth in ["bearer", "api", "google", "query"] {
        let mut r = request(
            "/provider/v1/models/model:streamGenerateContent?key=host-token&beta=one&beta=two&alt=sse&secret=never",
            body.clone(),
        );
        if auth != "query" {
            *r.uri_mut()="/provider/v1/models/model:streamGenerateContent?beta=one&beta=two&alt=sse&secret=never".parse().unwrap();
        }
        r.headers_mut().remove("authorization");
        match auth {
            "bearer" => {
                r.headers_mut()
                    .insert("authorization", "Bearer host-token".parse().unwrap());
            }
            "api" => {
                r.headers_mut()
                    .insert("x-api-key", "host-token".parse().unwrap());
            }
            "google" => {
                r.headers_mut()
                    .insert("x-goog-api-key", "host-token".parse().unwrap());
            }
            _ => (),
        };
        let response = gateway.serve(r).await.unwrap();
        assert_eq!(response.status(), 200);
        read(response).await.unwrap();
    }
    assert_eq!(received.lock().unwrap().len(), 4);
    for (uri, headers) in received.lock().unwrap().iter() {
        assert_eq!(
            uri,
            "/v1/models/model:streamGenerateContent?beta=one&alt=sse"
        );
        assert_eq!(headers["x-goog-api-key"], "private-key");
        assert!(!headers.contains_key("authorization"));
        assert!(!headers.contains_key("x-api-key"));
    }
    for route in [
        "/provider/v1/other",
        "/provider/elsewhere/models/model:streamGenerateContent",
    ] {
        assert_eq!(
            gateway
                .serve(request(route, body.clone()))
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let mut unauth = request("/provider/v1/models/model:streamGenerateContent", body);
    unauth.headers_mut().remove("authorization");
    assert_eq!(gateway.serve(unauth).await.unwrap().status(), 403);
    assert_eq!(received.lock().unwrap().len(), 4);
    assert!(
        !serde_json::to_string(&*events.lock().unwrap())
            .unwrap()
            .contains("private-key")
    );
}
#[tokio::test]
async fn redirects_are_not_followed_and_cancelled_headers_get_one_unavailable_receipt() {
    let calls = Arc::new(Mutex::new(0));
    let seen = calls.clone();
    let server = upstream(Router::new().fallback(move |r: Request<Body>| {
        let seen = seen.clone();
        async move {
            *seen.lock().unwrap() += 1;
            if r.uri().path().contains("redirect") {
                Response::builder()
                    .status(307)
                    .header("Location", "/stolen")
                    .body(Body::empty())
                    .unwrap()
            } else {
                sleep(Duration::from_secs(5)).await;
                Response::new(Body::from("{}"))
            }
        }
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let mut c = config(&server.url);
    c["baseUrl"] = json!(format!("{}/redirect", server.url));
    let g = gateway(root.path(), "openai-responses", c, &events);
    assert_eq!(
        g.serve(request(
            "/provider/redirect/responses",
            json!({"model":"model","input":"fixture"})
        ))
        .await
        .unwrap()
        .status(),
        502
    );
    assert_eq!(*calls.lock().unwrap(), 1);
    assert_eq!(g.failure().unwrap()["kind"], "disconnect");
    let second = tempfile::tempdir().unwrap();
    let g = gateway(
        second.path(),
        "openai-responses",
        config(&server.url),
        &events,
    );
    assert!(
        timeout(
            Duration::from_millis(30),
            g.serve(request(
                "/provider/v1/responses",
                json!({"model":"model","input":"fixture"})
            ))
        )
        .await
        .is_err()
    );
    assert_eq!(g.failure().unwrap()["kind"], "disconnect");
    let saved = events.lock().unwrap();
    assert_eq!(
        saved
            .iter()
            .filter(|e| e["type"] == "provider_usage" && e["state"] == "unavailable")
            .count(),
        2
    );
}
#[tokio::test]
async fn a_stalled_reader_retains_body_interruption_even_when_the_channel_is_full() {
    let server = upstream(Router::new().fallback(|| async {
        Response::builder()
            .header("Content-Type", "text/plain")
            .body(Body::from_stream(
                futures_util::stream::repeat(Bytes::from(vec![b'x'; 65536]))
                    .map(Ok::<_, std::io::Error>),
            ))
            .unwrap()
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let g = gateway(
        root.path(),
        "openai-responses",
        config(&server.url),
        &events,
    );
    let response = g
        .serve(request(
            "/provider/v1/responses",
            json!({"model":"model","input":"fixture"}),
        ))
        .await
        .unwrap();
    sleep(Duration::from_millis(250)).await;
    assert!(read(response).await.is_err());
    assert_eq!(g.failure().unwrap()["kind"], "timeout");
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "provider_usage" && e["state"] == "unavailable")
            .count(),
        1
    );
}
#[tokio::test]
async fn stop_cancels_an_incomplete_request_body_before_provider_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let g = gateway(
        root.path(),
        "openai-responses",
        config("http://127.0.0.1:1"),
        &events,
    );
    let r = Request::builder()
        .method("POST")
        .uri("/provider/v1/responses")
        .header("Authorization", "Bearer host-token")
        .body(Body::from_stream(futures_util::stream::pending::<
            std::result::Result<Bytes, std::io::Error>,
        >()))
        .unwrap();
    let run = g.clone();
    let waiting = tokio::spawn(async move { run.serve(r).await.unwrap() });
    sleep(Duration::from_millis(20)).await;
    g.close();
    assert_eq!(
        timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap()
            .status(),
        403
    );
    assert!(events.lock().unwrap().is_empty());
}
#[tokio::test]
async fn simultaneous_first_efforts_pin_one_policy_and_reopen_cannot_change_it() {
    let calls = Arc::new(Mutex::new(0));
    let captured = calls.clone();
    let server = upstream(Router::new().fallback(move || {
        let captured = captured.clone();
        async move {
            *captured.lock().unwrap() += 1;
            "{}"
        }
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let g = gateway(
        root.path(),
        "anthropic-messages",
        config(&server.url),
        &events,
    );
    let make = |effort: &str| {
        request(
            "/provider/v1/messages",
            json!({"model":"model","messages":[{"content":"fixture"}],"output_config":{"effort":effort}}),
        )
    };
    let (a, b) = tokio::join!(g.serve(make("low")), g.serve(make("high")));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(
        [a.status(), b.status()]
            .iter()
            .filter(|v| **v == StatusCode::OK)
            .count(),
        1
    );
    read(a).await.unwrap();
    read(b).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), 1);
    let saved: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("provider-policy.json")).unwrap())
            .unwrap();
    g.close();
    let g = gateway(
        root.path(),
        "anthropic-messages",
        config(&server.url),
        &events,
    );
    let other = if saved["wire"]["effort"] == "low" {
        "high"
    } else {
        "low"
    };
    assert_eq!(g.serve(make(other)).await.unwrap().status(), 403);
}
#[tokio::test]
async fn split_utf8_request_content_is_preserved_exactly() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let received = captured.clone();
    let server = upstream(Router::new().fallback(move |body: Bytes| {
        let received = received.clone();
        async move {
            *received.lock().unwrap() = body.to_vec();
            "{}"
        }
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let g = gateway(
        root.path(),
        "openai-responses",
        config(&server.url),
        &events,
    );
    let raw = json!({"model":"model","input":"résumé🦀"})
        .to_string()
        .into_bytes();
    let chunks = raw
        .iter()
        .map(|b| Ok::<_, std::io::Error>(Bytes::from(vec![*b])))
        .collect::<Vec<_>>();
    let r = Request::builder()
        .method("POST")
        .uri("/provider/v1/responses")
        .header("Authorization", "Bearer host-token")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap();
    read(g.serve(r).await.unwrap()).await.unwrap();
    assert_eq!(*captured.lock().unwrap(), raw);
}
#[tokio::test]
async fn failed_budget_persistence_keeps_actual_usage_but_refuses_unknown_replay() {
    let response=json!({"object":"response","status":"completed","output":[{"type":"reasoning","encrypted_content":"opaque"}],"usage":{"input_tokens":3,"output_tokens":7}}).to_string();
    let server = upstream(Router::new().fallback(move || {
        let response = response.clone();
        async move { response }
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let g = gateway(
        root.path(),
        "openai-responses",
        config(&server.url),
        &events,
    );
    std::fs::create_dir(root.path().join("reasoning-budget.json")).unwrap();
    read(
        g.serve(request(
            "/provider/v1/responses",
            json!({"model":"model","input":"fixture"}),
        ))
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "provider_usage" && e["state"] == "actual")
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "provider_failure" && e["kind"] == "budget_storage")
    );
    let replay =
        json!({"model":"model","input":[{"type":"reasoning","encrypted_content":"opaque"}]});
    assert_eq!(
        g.serve(request("/provider/v1/responses", replay))
            .await
            .unwrap()
            .status(),
        403
    );
}

#[tokio::test]
async fn terminal_stream_budget_is_durable_before_a_tool_can_start_the_next_request() {
    let terminal = format!(
        "data: {}\n\n",
        json!({"type":"response.completed","response":{"output":[{"type":"reasoning","encrypted_content":"early-opaque"},{"type":"function_call","name":"repository_read"}],"usage":{"input_tokens":3,"output_tokens":7}}})
    );
    let server = upstream(Router::new().fallback(move || {
        let terminal = terminal.clone();
        async move {
            let body = futures_util::stream::once(async move {
                Ok::<_, std::io::Error>(Bytes::from(terminal))
            })
            .chain(futures_util::stream::once(async {
                sleep(Duration::from_secs(2)).await;
                Ok(Bytes::from("data: [DONE]\n\n"))
            }));
            Response::builder()
                .header("Content-Type", "text/event-stream")
                .body(Body::from_stream(body))
                .unwrap()
        }
    }))
    .await;
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let mut config = config(&server.url);
    config["requestTimeoutMs"] = json!(5000);
    let g = gateway(root.path(), "openai-responses", config, &events);
    let response = g
        .serve(request(
            "/provider/v1/responses",
            json!({"model":"model","input":"fixture"}),
        ))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("response.completed"));
    let response=g.serve(request("/provider/v1/responses",json!({"model":"model","input":[{"type":"reasoning","encrypted_content":"early-opaque"}]}))).await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(root.path().join("reasoning-budget.json").is_file());
    // Transport receipt remains pending until EOF; budget proof is a separate fact.
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "provider_usage")
    );
    g.close();
    drop(stream);
    drop(response);
}

#[tokio::test]
async fn http_failure_taxonomy_and_body_bytes_are_preserved() {
    let server = upstream(Router::new().fallback(|r: Request<Body>| async move {
        let status = r
            .uri()
            .path()
            .split('/')
            .nth(1)
            .unwrap()
            .parse::<u16>()
            .unwrap();
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(Body::from("{\"error\":\"scripted\"}"))
            .unwrap()
    }))
    .await;
    for (status, kind, retryable) in [
        (300, "rejected", false),
        (400, "rejected", false),
        (401, "authorization", false),
        (403, "authorization", false),
        (408, "timeout", true),
        (429, "rate_limit", true),
        (500, "unavailable", true),
        (503, "unavailable", true),
        (504, "timeout", true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let mut c = config(&server.url);
        c["baseUrl"] = json!(format!("{}/{status}", server.url));
        let g = gateway(root.path(), "openai-responses", c, &events);
        let response = g
            .serve(request(
                &format!("/provider/{status}/responses"),
                json!({"model":"model","input":"fixture"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(read(response).await.unwrap(), b"{\"error\":\"scripted\"}");
        assert_eq!(
            g.failure(),
            Some(json!({"kind":kind,"retryable":retryable,"status":status}))
        );
        let events = events.lock().unwrap();
        let usage = events
            .iter()
            .filter(|e| e["type"] == "provider_usage")
            .collect::<Vec<_>>();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0]["state"], "unavailable");
    }
}
#[tokio::test]
async fn close_and_client_disconnect_end_active_response_ownership() {
    let server = upstream(Router::new().fallback(|| async {
        let first = futures_util::stream::once(async {
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: {}\n\n"))
        });
        Response::builder()
            .header("Content-Type", "text/event-stream")
            .body(Body::from_stream(
                first.chain(futures_util::stream::pending()),
            ))
            .unwrap()
    }))
    .await;
    for close in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let events = Arc::new(Mutex::new(vec![]));
        let mut c = config(&server.url);
        c["requestTimeoutMs"] = json!(10000);
        let g = gateway(root.path(), "openai-responses", c, &events);
        let response = g
            .serve(request(
                "/provider/v1/responses",
                json!({"model":"model","input":"fixture"}),
            ))
            .await
            .unwrap();
        if close {
            g.close();
            assert!(read(response).await.is_err());
        } else {
            drop(response);
        }
        timeout(Duration::from_secs(1), async {
            while !events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "provider_usage")
            {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let failure = g.failure();
        if close {
            assert!(failure.is_none());
        } else {
            assert_eq!(failure.unwrap()["kind"], "disconnect");
        }
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == "provider_usage")
                .count(),
            1
        );
    }
}
