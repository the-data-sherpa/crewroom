use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{StreamExt, future::BoxFuture};
use reqwest::{
    Method,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue},
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// Deliberately has no Debug/Serialize implementation: credentials never become
/// diagnostic output or workflow state.
#[derive(Clone)]
pub enum Credential {
    Bearer(String),
    Basic { username: String, token: String },
}
pub type Credentials = Arc<dyn Fn() -> BoxFuture<'static, Result<Credential>> + Send + Sync>;
pub type BeforeDispatch = Arc<dyn Fn() -> BoxFuture<'static, Result<()>> + Send + Sync>;
#[derive(Clone)]
pub(super) struct Transport {
    provider: Provider,
    client: reqwest::Client,
    base: String,
    credentials: Credentials,
}
impl Transport {
    pub fn new(provider: Provider, base: String, credentials: Credentials) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| unavailable(provider.unavailable()))?;
        Ok(Self {
            provider,
            client,
            base,
            credentials,
        })
    }
    pub async fn request(
        &self,
        path: &str,
        method: Method,
        body: Option<Value>,
        before: Option<&BeforeDispatch>,
    ) -> Result<Option<Value>> {
        let mut sent = false;
        let result = async {
            let credential = (self.credentials)().await?;
            let bad = || {
                unavailable(format!(
                    "{} credentials are unavailable.",
                    self.provider.name()
                ))
            };
            let token = match &credential {
                Credential::Bearer(t) => t,
                Credential::Basic { token, .. } => token,
            };
            if token.is_empty() || token.contains(['\r', '\n']) {
                return Err(bad());
            }
            let authorization = match credential {
                Credential::Bearer(token) => format!("Bearer {token}"),
                Credential::Basic { username, token } => {
                    if self.provider == Provider::Jira
                        || username.is_empty()
                        || username.contains([':', '\r', '\n'])
                    {
                        return Err(bad());
                    }
                    format!("Basic {}", STANDARD.encode(format!("{username}:{token}")))
                }
            };
            if let Some(before) = before {
                before().await?;
            }
            // This is a conservative dispatch boundary, not evidence of wire
            // delivery. Node fetch can reject headers after this point too.
            sent = true;
            let header: Vec<u8> = authorization
                .trim_matches([' ', '\t'])
                .chars()
                .map(|c| u8::try_from(c as u32).map_err(|_| invalid()))
                .collect::<Result<_>>()?;
            let authorization = HeaderValue::from_bytes(&header).map_err(|_| invalid())?;
            tokio::time::timeout(Duration::from_secs(15), async {
                let mut request = self
                    .client
                    .request(method, format!("{}{path}", self.base))
                    .header(AUTHORIZATION, authorization)
                    .header(ACCEPT, "application/json");
                if let Some(body) = body {
                    request = request
                        .header(CONTENT_TYPE, "application/json")
                        .body(crewroom_core::serialized::stringify(&body));
                }
                let response = request.send().await.map_err(|_| invalid())?;
                let status = response.status();
                // Node redirect:error treats redirect responses as unavailable,
                // not a known rejection of the attempted provider mutation.
                if [301, 302, 303, 307, 308].contains(&status.as_u16()) {
                    return Err(invalid());
                }
                if !status.is_success() {
                    let retained = match self.provider {
                        Provider::Jira => "Saved work is retained.",
                        Provider::Bitbucket => "Saved delivery intents are retained.",
                        Provider::GitHub => "Saved work is retained.",
                    };
                    return Err(Error::Unavailable(Unavailable {
                        message: format!(
                            "{} returned HTTP {}. {retained}",
                            self.provider.name(),
                            status.as_u16()
                        ),
                        status: Some(status.as_u16()),
                        request_sent: true,
                    }));
                }
                let stream = crate::provider_body::stream(response);
                tokio::pin!(stream);
                let mut bytes = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| invalid())?;
                    if bytes.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 {
                        return Err(unavailable(format!(
                            "{} response exceeded its size limit.",
                            self.provider.name()
                        )));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if bytes.is_empty() {
                    return Ok(None);
                }
                let text = String::from_utf8_lossy(&bytes);
                // Keep serde_json's nesting and Unicode-scalar checks as explicit
                // native refusals. They remain sent/unknown for mutations.
                serde_json::from_str(&text).map(Some).map_err(|_| invalid())
            })
            .await
            .map_err(|_| invalid())?
        }
        .await;
        result.map_err(|error| match error {
            Error::Unavailable(mut error) => {
                error.request_sent = sent;
                Error::Unavailable(error)
            }
            Error::Invalid(_) => Error::Unavailable(Unavailable {
                message: self.provider.unavailable().into(),
                status: None,
                request_sent: sent,
            }),
        })
    }
}

/// Public constructors accept a deliberate loopback fixture override only.
pub(super) fn fixture_origin(value: &str, provider: Provider) -> Result<String> {
    let url = url::Url::parse(value).map_err(|_| invalid())?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some_and(|s| !s.is_empty())
        || url.fragment().is_some_and(|s| !s.is_empty())
    {
        return Err(Error::Invalid(format!(
            "Fixture {} endpoints must use a loopback origin.",
            provider.name()
        )));
    }
    Ok(url.origin().ascii_serialization())
}
