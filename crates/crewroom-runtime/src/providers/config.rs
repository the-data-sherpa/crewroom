//! Metadata is public; credential values stay in private callbacks and memory.
use super::{
    schema::*,
    secrets::{Integration, SecretStore, StoredToken},
    singleflight::SingleFlight,
    *,
};
use async_compression::tokio::bufread::GzipDecoder;
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, BufReader};
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;
#[derive(Clone, Debug)]
pub struct CredentialCommand {
    pub file: PathBuf,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_buffer: usize,
}
pub type Command =
    Arc<dyn Fn(CredentialCommand) -> BoxFuture<'static, Result<String>> + Send + Sync>;
pub type TokenEnvironment = Arc<dyn Fn() -> Option<String> + Send + Sync>;
/// Reads the encrypted install store on each request, so a token saved in
/// Settings applies without a restart and plaintext is never cached on disk.
pub type StoredTokens = Arc<dyn Fn() -> StoredToken + Send + Sync>;
pub fn stored_tokens(store: SecretStore, integration: Integration) -> StoredTokens {
    Arc::new(move || store.token(integration))
}
/// Resolves a saved token. `Ok(None)` falls through to the platform route.
fn stored_credential(
    stored: Option<&StoredTokens>,
    provider: Provider,
) -> Result<Option<Credential>> {
    match stored.map(|s| s()) {
        Some(StoredToken::Present(token)) => Ok(Some(Credential::Bearer(token.to_string()))),
        Some(StoredToken::Unreadable) => Err(unavailable(format!(
            "{}: {}. Save the token again in Settings → Integrations, or restore crewroom.secret from the backup that holds credentials.json.",
            provider.name(),
            secrets::UNREADABLE
        ))),
        _ => Ok(None),
    }
}
#[derive(Clone)]
pub struct JiraConnection {
    pub source: Value,
    pub credentials: Credentials,
}
#[derive(Clone)]
pub struct PublicationConnection {
    pub provider: remote::PublicationProvider,
    pub repository_path: PathBuf,
    pub repository: Value,
    pub policy: Value,
    pub credentials: Credentials,
}
/// Publication metadata resolved through each repository's publish remote.
/// Repositories whose remote has no provider stay visible as unavailable.
#[derive(Clone, Default)]
pub struct PublicationConfig {
    pub connections: Vec<PublicationConnection>,
    pub unavailable: Vec<remote::UnavailablePublication>,
}
fn credential_error(provider: Provider) -> Error {
    unavailable(match provider {
        Provider::Jira => {
            "Jira credentials are unavailable. Sign in to the configured account with ACLI on this machine."
        }
        Provider::Bitbucket => {
            "Bitbucket credentials are unavailable to the Crewroom server. Configure server access before publication."
        }
        Provider::GitHub => {
            "GitHub credentials are unavailable. Connect GitHub in Settings → Integrations, or set CREWROOM_GITHUB_TOKEN for the server."
        }
    })
}
fn command() -> Command {
    Arc::new(|spec| {
        Box::pin(async move {
            let mut child = tokio::process::Command::new(&spec.file)
                .args(&spec.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| invalid())?;
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            let result = tokio::time::timeout(spec.timeout, async {
                let (out, _) = tokio::try_join!(
                    bounded(stdout, spec.max_buffer),
                    bounded(stderr, spec.max_buffer)
                )?;
                let status = child.wait().await.map_err(|_| invalid())?;
                if !status.success() {
                    return Err(invalid());
                }
                Ok(String::from_utf8_lossy(&out).into_owned())
            })
            .await;
            match result {
                Ok(Ok(value)) => Ok(value),
                _ => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Err(invalid())
                }
            }
        })
    })
}
async fn bounded(reader: impl tokio::io::AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| invalid())?;
    if bytes.len() > limit {
        return Err(invalid());
    }
    Ok(bytes)
}
async fn run(
    command: &Command,
    file: &str,
    args: Vec<String>,
    seconds: u64,
    max_buffer: usize,
) -> Result<String> {
    command(CredentialCommand {
        file: file.into(),
        args,
        timeout: Duration::from_secs(seconds),
        max_buffer,
    })
    .await
}
fn json_file(path: &Path) -> Result<Option<Value>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(invalid()),
    }
}
/// Node Buffer.from accepts unpadded/base64url input and stops hex at its first
/// invalid pair. Keep decoding bounded by the trusted command output limit.
fn keyring_bytes(format: &str, value: &str) -> Result<Vec<u8>> {
    match format {
        "go-keyring-base64" => {
            let mut s = value
                .chars()
                .take_while(|c| *c != '=')
                .filter(|c| c.is_ascii_alphanumeric() || "+/-_".contains(*c))
                .map(|c| match c {
                    '-' => '+',
                    '_' => '/',
                    c => c,
                })
                .collect::<String>();
            if s.len() % 4 == 1 {
                s.pop();
            }
            GeneralPurpose::new(
                &alphabet::STANDARD,
                GeneralPurposeConfig::new()
                    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
                    .with_decode_allow_trailing_bits(true),
            )
            .decode(s)
            .map_err(|_| invalid())
        }
        "go-keyring-encoded" => {
            let mut bytes = vec![];
            for pair in value.as_bytes().chunks_exact(2) {
                let Some(a) = (pair[0] as char).to_digit(16) else {
                    break;
                };
                let Some(b) = (pair[1] as char).to_digit(16) else {
                    break;
                };
                bytes.push((a * 16 + b) as u8);
            }
            Ok(bytes)
        }
        _ => Err(invalid()),
    }
}
fn js_string(value: &Value, array_item: bool) -> String {
    match value {
        Value::Null if array_item => String::new(),
        Value::String(v) => v.clone(),
        Value::Array(a) => a
            .iter()
            .map(|v| js_string(v, true))
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
        _ => crewroom_core::serialized::stringify(value),
    }
}
struct JiraCredentials {
    source: Value,
    account: String,
    command: Command,
    clock: Clock,
    environment: TokenEnvironment,
    stored: Option<StoredTokens>,
    keychain: bool,
    renewing: SingleFlight<Credential>,
}
impl JiraCredentials {
    async fn read(&self) -> Result<Option<Credential>> {
        let output = run(
            &self.command,
            "/usr/bin/security",
            vec![
                "find-generic-password".into(),
                "-s".into(),
                "acli".into(),
                "-a".into(),
                format!(
                    "oauth:{}:{}",
                    self.source["cloudId"].as_str().unwrap(),
                    self.account
                ),
                "-w".into(),
            ],
            10,
            1024 * 1024,
        )
        .await?;
        let (format, value) = crewroom_core::store::trim_text(&output)
            .split_once(':')
            .ok_or_else(invalid)?;
        let bytes = keyring_bytes(format, value)?;
        let mut decoder = GzipDecoder::new(BufReader::new(std::io::Cursor::new(bytes)));
        decoder.multiple_members(true);
        let mut output = vec![];
        decoder
            .take(1024 * 1024 + 1)
            .read_to_end(&mut output)
            .await
            .map_err(|_| invalid())?;
        if output.len() > 1024 * 1024 {
            return Err(invalid());
        }
        let record: Value =
            serde_json::from_str(&String::from_utf8_lossy(&output)).map_err(|_| invalid())?;
        let expiry = record
            .get("expiry")
            .and_then(|v| crewroom_core::source_time::parse(&js_string(v, false)));
        Ok(record["access_token"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && expiry.is_some_and(|time| time > (self.clock)().saturating_add(30000))
            })
            .map(|s| Credential::Bearer(s.into())))
    }
    async fn credentials(self: &Arc<Self>) -> Result<Credential> {
        if let Some(token) = (self.environment)().filter(|s| !s.is_empty()) {
            return Ok(Credential::Bearer(token));
        }
        if let Some(credential) = stored_credential(self.stored.as_ref(), Provider::Jira)? {
            return Ok(credential);
        }
        if !self.keychain {
            return Err(unavailable(
                "Jira credentials are unavailable. Save a Jira token in Settings → Integrations, or set CREWROOM_JIRA_TOKEN in the server environment and restart the service. The macOS ACLI keychain is not available on this platform.",
            ));
        }
        let result = async {
            if let Some(token) = self.read().await? {
                return Ok(token);
            }
            let this = self.clone();
            self.renewing
                .run(async move {
                    // Any one-item search renews the ACLI session; the saved
                    // filter is optional since filters are per project (FOR-53).
                    let scope: Vec<String> = match this.source["filterId"].as_str() {
                        Some(filter) => vec!["--filter".into(), filter.into()],
                        None => vec!["--jql".into(), "order by created DESC".into()],
                    };
                    let mut args: Vec<String> =
                        vec!["jira".into(), "workitem".into(), "search".into()];
                    args.extend(scope);
                    args.extend(["--limit".into(), "1".into(), "--json".into()]);
                    run(
                        &this.command,
                        "/opt/homebrew/bin/acli",
                        args,
                        30,
                        2 * 1024 * 1024,
                    )
                    .await?;
                    this.read().await?.ok_or_else(invalid)
                })
                .await
        }
        .await;
        result.map_err(|_| credential_error(Provider::Jira))
    }
}
/// Production route: environment override, then the encrypted install store,
/// then (macOS only) the ACLI keychain.
pub fn load_jira_connection(path: &Path, secrets: SecretStore) -> Result<Option<JiraConnection>> {
    load_jira_connection_stored(
        path,
        Arc::new(|| std::env::var(Integration::Jira.environment()).ok()),
        stored_tokens(secrets, Integration::Jira),
    )
}
/// The production route with an explicit environment, without keychain commands.
pub fn load_jira_connection_stored(
    path: &Path,
    environment: TokenEnvironment,
    stored: StoredTokens,
) -> Result<Option<JiraConnection>> {
    load_jira_connection_options(
        path,
        command(),
        Arc::new(crewroom_core::now_ms),
        environment,
        Some(stored),
        cfg!(target_os = "macos"),
    )
}
pub fn load_jira_connection_with(
    path: &Path,
    command: Command,
    clock: Clock,
) -> Result<Option<JiraConnection>> {
    load_jira_connection_options(path, command, clock, Arc::new(|| None), None, true)
}
fn load_jira_connection_options(
    path: &Path,
    command: Command,
    clock: Clock,
    environment: TokenEnvironment,
    stored: Option<StoredTokens>,
    keychain: bool,
) -> Result<Option<JiraConnection>> {
    let Some(mut metadata) = json_file(path)? else {
        return Ok(None);
    };
    let account = account(&metadata["accountId"])?;
    object(&metadata)?;
    metadata.as_object_mut().unwrap().remove("accountId");
    let source = jira::source(&metadata)?;
    let inner = Arc::new(JiraCredentials {
        source: source.clone(),
        account,
        command,
        clock,
        environment,
        stored,
        keychain,
        renewing: SingleFlight::default(),
    });
    Ok(Some(JiraConnection {
        source,
        credentials: Arc::new(move || {
            let inner = inner.clone();
            Box::pin(async move { inner.credentials().await })
        }),
    }))
}
struct Cached {
    credential: Credential,
    expires: i64,
}
struct BitbucketCredentials {
    command: Command,
    clock: Clock,
    environment: TokenEnvironment,
    stored: Option<StoredTokens>,
    keychain: bool,
    cached: Mutex<Option<Cached>>,
    unavailable: AtomicBool,
    reading: SingleFlight<Credential>,
}
impl BitbucketCredentials {
    async fn credentials(self: &Arc<Self>) -> Result<Credential> {
        if let Some(token) = (self.environment)().filter(|s| !s.is_empty()) {
            return Ok(Credential::Bearer(token));
        }
        if let Some(credential) = stored_credential(self.stored.as_ref(), Provider::Bitbucket)? {
            return Ok(credential);
        }
        if !self.keychain {
            return Err(unavailable(
                "Bitbucket credentials are unavailable. Save a Bitbucket token in Settings → Integrations, or set CREWROOM_BITBUCKET_TOKEN in the server environment and restart the service. The macOS bkt keychain is not available on this platform.",
            ));
        }
        if let Some(cached) = self
            .cached
            .lock()
            .unwrap()
            .as_ref()
            .filter(|c| c.expires > (self.clock)().saturating_add(30000))
        {
            return Ok(cached.credential.clone());
        }
        if self.unavailable.load(Ordering::Acquire) {
            return Err(credential_error(Provider::Bitbucket));
        }
        let this = self.clone();
        self.reading
            .run(async move {
                // Another driven refresh may finish between the caller's cache
                // read and admission to this new flight on another OS thread.
                if let Some(cached) = this
                    .cached
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|c| c.expires > (this.clock)().saturating_add(30000))
                {
                    return Ok(cached.credential.clone());
                }
                if this.unavailable.load(Ordering::Acquire) {
                    return Err(credential_error(Provider::Bitbucket));
                }
                let result = this.read().await;
                match result {
                    Ok((credential, expires)) => {
                        *this.cached.lock().unwrap() = Some(Cached {
                            credential: credential.clone(),
                            expires,
                        });
                        Ok(credential)
                    }
                    Err(_) => {
                        this.unavailable.store(true, Ordering::Release);
                        Err(credential_error(Provider::Bitbucket))
                    }
                }
            })
            .await
            .map_err(|_| credential_error(Provider::Bitbucket))
    }
    async fn read(&self) -> Result<(Credential, i64)> {
        let raw = run(
            &self.command,
            "/opt/homebrew/bin/bkt",
            vec![
                "--context".into(),
                "conductor".into(),
                "auth".into(),
                "status".into(),
                "--json".into(),
            ],
            30,
            1024 * 1024,
        )
        .await?;
        let status: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
        if status["active_context"] != "conductor" {
            return Err(invalid());
        }
        let contexts = array(&status["contexts"])?;
        for v in contexts {
            string(&v["name"])?;
            string(&v["host"])?;
        }
        let hosts = array(&status["hosts"])?;
        for v in hosts {
            for k in ["key", "kind", "base_url", "auth_method", "token_source"] {
                string(&v[k])?;
            }
            if let Some(v) = v.get("username") {
                string(v)?;
            }
        }
        let context = contexts
            .iter()
            .find(|c| c["name"] == "conductor")
            .ok_or_else(invalid)?;
        let account = hosts
            .iter()
            .find(|h| h["key"] == context["host"])
            .ok_or_else(invalid)?;
        if account["key"] != "api.bitbucket.org"
            || account["kind"] != "cloud"
            || account["base_url"] != "https://api.bitbucket.org/2.0"
            || account["token_source"] != "keyring"
            || !["basic", "oauth"].contains(&account["auth_method"].as_str().unwrap())
        {
            return Err(invalid());
        }
        run(
            &self.command,
            "/opt/homebrew/bin/bkt",
            vec![
                "--context".into(),
                "conductor".into(),
                "api".into(),
                "/user".into(),
                "--json".into(),
            ],
            30,
            1024 * 1024,
        )
        .await?;
        let raw = run(
            &self.command,
            "/usr/bin/security",
            vec![
                "find-generic-password".into(),
                "-s".into(),
                "bkt".into(),
                "-a".into(),
                "host/api.bitbucket.org/token".into(),
                "-w".into(),
            ],
            10,
            1024 * 1024,
        )
        .await?;
        if account["auth_method"] == "basic" {
            let username = text(&account["username"], 1, usize::MAX, false)?;
            let token = raw
                .strip_suffix('\n')
                .map(|s| s.strip_suffix('\r').unwrap_or(s))
                .unwrap_or(&raw)
                .to_owned();
            if username.contains([':', '\r', '\n'])
                || token.is_empty()
                || token.contains(['\r', '\n'])
            {
                return Err(invalid());
            }
            Ok((
                Credential::Basic { username, token },
                (self.clock)().saturating_add(300000),
            ))
        } else {
            let value: Value = serde_json::from_str(&raw).map_err(|_| invalid())?;
            let token = text(&value["access_token"], 1, usize::MAX, false)?;
            let expiry = string(&value["expires_at"])?;
            let expiry = crewroom_core::source_time::parse(&expiry)
                .filter(|t| *t > (self.clock)().saturating_add(30000))
                .ok_or_else(invalid)?;
            Ok((Credential::Bearer(token), expiry))
        }
    }
}
/// Production route: environment override, then the encrypted install store,
/// then (macOS only) the bkt keychain.
pub fn load_publication_config(path: &Path, secrets: SecretStore) -> Result<PublicationConfig> {
    load_publication_config_options(
        path,
        command(),
        Arc::new(crewroom_core::now_ms),
        Arc::new(|| std::env::var(Integration::Bitbucket.environment()).ok()),
        Some(stored_tokens(secrets.clone(), Integration::Bitbucket)),
        cfg!(target_os = "macos"),
        Some(GitHubTokens {
            environment: Arc::new(|| std::env::var(Integration::GitHub.environment()).ok()),
            stored: Some(stored_tokens(secrets, Integration::GitHub)),
        }),
    )
}
/// Supported connections only; prefer [`load_publication_config`], which also
/// reports repositories whose publish remote has no provider.
pub fn load_publication_connections(
    path: &Path,
    secrets: SecretStore,
) -> Result<Vec<PublicationConnection>> {
    Ok(load_publication_config(path, secrets)?.connections)
}
pub fn load_publication_connections_with(
    path: &Path,
    command: Command,
    clock: Clock,
    environment: TokenEnvironment,
) -> Result<Vec<PublicationConnection>> {
    Ok(
        load_publication_config_options(path, command, clock, environment, None, true, None)?
            .connections,
    )
}
#[cfg(test)]
fn load_publication_connections_options(
    path: &Path,
    command: Command,
    clock: Clock,
    environment: TokenEnvironment,
    stored: Option<StoredTokens>,
    keychain: bool,
) -> Result<Vec<PublicationConnection>> {
    Ok(
        load_publication_config_options(path, command, clock, environment, stored, keychain, None)?
            .connections,
    )
}
/// The GitHub token route for publication: `CREWROOM_GITHUB_TOKEN`, then the
/// token saved in Settings → Integrations → GitHub (FOR-43). Read per request,
/// so connecting or disconnecting applies without a restart.
#[derive(Clone)]
pub struct GitHubTokens {
    pub environment: TokenEnvironment,
    pub stored: Option<StoredTokens>,
}
impl GitHubTokens {
    fn credentials(&self) -> Result<Credential> {
        if let Some(token) = (self.environment)().filter(|s| !s.trim().is_empty()) {
            return Ok(Credential::Bearer(token.trim().to_owned()));
        }
        stored_credential(self.stored.as_ref(), Provider::GitHub)?
            .ok_or_else(|| credential_error(Provider::GitHub))
    }
}
/// Each entry selects its provider from its optional `remote` (the publish
/// remote). Without one, the entry keeps the original Bitbucket contract. A
/// github.com remote requires `repository` as `{owner, name, id}` naming the
/// same repository; it authenticates with the GitHub token route.
fn load_publication_config_options(
    path: &Path,
    command: Command,
    clock: Clock,
    environment: TokenEnvironment,
    stored: Option<StoredTokens>,
    keychain: bool,
    github: Option<GitHubTokens>,
) -> Result<PublicationConfig> {
    let Some(metadata) = json_file(path)? else {
        return Ok(PublicationConfig::default());
    };
    let entries = array(&metadata)?;
    if entries.len() > 20 {
        return Err(invalid());
    }
    let inner = Arc::new(BitbucketCredentials {
        command,
        clock,
        environment,
        stored,
        keychain,
        cached: Mutex::new(None),
        unavailable: AtomicBool::new(false),
        reading: SingleFlight::default(),
    });
    let mut config = PublicationConfig::default();
    for entry in entries {
        strict(entry, &["repositoryPath", "repository", "policy", "remote"])?;
        let path = string(&entry["repositoryPath"])?;
        let repository_path = PathBuf::from(path);
        if !repository_path.is_absolute() {
            return Err(invalid());
        }
        let selected = entry
            .get("remote")
            .map(|v| remote::select(&string(v)?))
            .transpose()?;
        let provider = match selected {
            None => remote::PublicationProvider::Bitbucket,
            Some(remote::RemoteSelection::Unsupported { host }) => {
                config
                    .unavailable
                    .push(remote::UnavailablePublication::unsupported(
                        repository_path,
                        &host,
                    ));
                continue;
            }
            Some(remote::RemoteSelection::Supported {
                provider: remote::PublicationProvider::GitHub,
                owner,
                name,
            }) => {
                let repository = github_pulls::repository(&entry["repository"])?;
                if !repository["owner"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&owner))
                    || !repository["name"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&name))
                {
                    return Err(invalid());
                }
                let github = github.clone();
                config.connections.push(PublicationConnection {
                    provider: remote::PublicationProvider::GitHub,
                    repository_path,
                    repository,
                    policy: publication_policy(&entry["policy"])?,
                    credentials: Arc::new(move || {
                        let result = github.as_ref().map_or_else(
                            || Err(credential_error(Provider::GitHub)),
                            |g| g.credentials(),
                        );
                        Box::pin(async move { result })
                    }),
                });
                continue;
            }
            Some(remote::RemoteSelection::Supported {
                provider,
                owner,
                name,
            }) => {
                let configured = bitbucket::repository(&entry["repository"])?;
                if !configured["workspace"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&owner))
                    || !configured["slug"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(&name))
                {
                    return Err(invalid());
                }
                provider
            }
        };
        config.connections.push({
            let repository = bitbucket::repository(&entry["repository"])?;
            let normalized = publication_policy(&entry["policy"])?;
            let inner = inner.clone();
            PublicationConnection {
                provider,
                repository_path,
                repository,
                policy: normalized,
                credentials: Arc::new(move || {
                    let inner = inner.clone();
                    Box::pin(async move { inner.credentials().await })
                }),
            }
        });
    }
    Ok(config)
}
/// The accepted hosted-check policy: required status keys and pipeline steps,
/// plus optional steps. For GitHub, check runs are the pipeline steps and both
/// commit statuses and check runs are status keys.
fn publication_policy(policy: &Value) -> Result<Value> {
    strict(policy, &["statusKeys", "pipelineSteps", "optionalSteps"])?;
    let mut normalized = json!({});
    for key in ["statusKeys", "pipelineSteps", "optionalSteps"] {
        let values = match policy.get(key) {
            Some(v) => array(v)?.iter().map(string).collect::<Result<Vec<_>>>()?,
            None if key == "optionalSteps" => vec![],
            None => return Err(invalid()),
        };
        if key != "optionalSteps" && values.is_empty() {
            return Err(invalid());
        }
        normalized[key] = json!(values);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_credential_commands() -> Command {
        Arc::new(|_| panic!("Server token authentication must not invoke a credential command"))
    }

    #[tokio::test]
    async fn jira_server_token_stays_private_and_missing_linux_token_is_actionable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("jira.json");
        let source = json!({
            "siteUrl":"https://fixture.atlassian.net",
            "cloudId":"00000000-0000-4000-8000-000000000022",
            "filterId":"1","assigneeAccountId":"operator","name":"Fixture"
        });
        let mut metadata = source.clone();
        metadata["accountId"] = json!("operator");
        std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let token = Arc::new(Mutex::new(Some("synthetic-private-jira".to_owned())));
        let environment: TokenEnvironment = {
            let token = token.clone();
            Arc::new(move || token.lock().unwrap().clone())
        };
        for keychain in [true, false] {
            *token.lock().unwrap() = Some("synthetic-private-jira".into());
            let connection = load_jira_connection_options(
                &path,
                no_credential_commands(),
                Arc::new(|| 0),
                environment.clone(),
                None,
                keychain,
            )
            .unwrap()
            .unwrap();
            assert_eq!(connection.source, source);
            assert!(
                matches!((connection.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-private-jira")
            );
            *token.lock().unwrap() = Some("synthetic-rotated-jira".into());
            assert!(
                matches!((connection.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-rotated-jira")
            );
            if !keychain {
                for missing in [None, Some(String::new())] {
                    *token.lock().unwrap() = missing;
                    match (connection.credentials)().await {
                        Err(Error::Unavailable(error)) => {
                            assert!(error.message.contains("CREWROOM_JIRA_TOKEN"));
                            assert!(!error.message.contains("synthetic"));
                            assert!(!error.request_sent);
                        }
                        _ => panic!("Missing Linux token must be unavailable before dispatch"),
                    }
                }
            }
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
            metadata
        );
    }

    #[tokio::test]
    async fn bitbucket_linux_token_never_attempts_macos_commands_or_exposes_credentials() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bitbucket.json");
        let metadata = json!([{
            "repositoryPath":"/disposable/repository",
            "repository":{"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"},
            "policy":{"statusKeys":["pr"],"pipelineSteps":["Test"]}
        }]);
        std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        for token in [Some("synthetic-private-bitbucket"), None, Some("")] {
            let connection = load_publication_connections_options(
                &path,
                no_credential_commands(),
                Arc::new(|| 0),
                Arc::new(move || token.map(str::to_owned)),
                None,
                false,
            )
            .unwrap()
            .remove(0);
            assert_eq!(connection.repository, metadata[0]["repository"]);
            match (connection.credentials)().await {
                Ok(Credential::Bearer(value)) => assert_eq!(value, "synthetic-private-bitbucket"),
                Err(Error::Unavailable(error)) => {
                    assert!(token.is_none_or(str::is_empty));
                    assert!(error.message.contains("CREWROOM_BITBUCKET_TOKEN"));
                    assert!(!error.message.contains("synthetic"));
                    assert!(!error.request_sent);
                }
                _ => panic!("Unexpected token authentication result"),
            }
        }
    }

    #[test]
    fn publication_provider_resolves_from_the_configured_publish_remote() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bitbucket.json");
        let repository = json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"});
        let policy = json!({"statusKeys":["pr"],"pipelineSteps":["Test"]});
        let load = |metadata: Value| {
            std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
            load_publication_config_options(
                &path,
                no_credential_commands(),
                Arc::new(|| 0),
                Arc::new(|| None),
                None,
                false,
                None,
            )
        };
        // Without a remote, the original Bitbucket contract is unchanged.
        let legacy = load(json!([{"repositoryPath":"/disposable/legacy","repository":repository,"policy":policy}])).unwrap();
        assert_eq!(legacy.connections.len(), 1);
        assert!(legacy.unavailable.is_empty());
        assert_eq!(
            legacy.connections[0].provider,
            remote::PublicationProvider::Bitbucket
        );
        assert_eq!(legacy.connections[0].repository, repository);
        // A Bitbucket publish remote selects Bitbucket; an unsupported host is
        // reported as an unavailable provider instead of failing or vanishing.
        let mixed = load(json!([
            {"repositoryPath":"/disposable/bitbucket","remote":"git@bitbucket.org:fixture/repository.git","repository":repository,"policy":policy},
            {"repositoryPath":"/disposable/gitlab","remote":"https://gitlab.example/fixture/repository.git"}
        ]))
        .unwrap();
        assert_eq!(mixed.connections.len(), 1);
        assert_eq!(
            mixed.connections[0].provider,
            remote::PublicationProvider::Bitbucket
        );
        assert_eq!(
            mixed.connections[0].repository_path,
            PathBuf::from("/disposable/bitbucket")
        );
        assert_eq!(
            mixed.unavailable,
            vec![remote::UnavailablePublication::unsupported(
                "/disposable/gitlab".into(),
                "gitlab.example"
            )]
        );
        // A Bitbucket remote naming another repository, or a credentialed
        // remote, is invalid metadata rather than a silent mismatch.
        for remote in [
            "https://bitbucket.org/fixture/other.git",
            "https://user:synthetic-secret@bitbucket.org/fixture/repository.git",
        ] {
            let error = load(json!([{"repositoryPath":"/disposable/bitbucket","remote":remote,"repository":repository,"policy":policy}]))
                .err()
                .unwrap();
            assert!(!error.to_string().contains("synthetic-secret"));
        }
        assert!(load(json!([{"repositoryPath":"/disposable/bitbucket","remote":7,"repository":repository,"policy":policy}])).is_err());
    }

    #[tokio::test]
    async fn github_publish_remotes_use_the_github_token_route() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("bitbucket.json");
        let store = SecretStore::open(root.path()).unwrap();
        let environment = Arc::new(Mutex::new(None::<String>));
        let tokens = {
            let environment = environment.clone();
            GitHubTokens {
                environment: Arc::new(move || environment.lock().unwrap().clone()),
                stored: Some(stored_tokens(store.clone(), Integration::GitHub)),
            }
        };
        let policy = json!({"statusKeys":["test"],"pipelineSteps":["test"]});
        let load = |metadata: Value| {
            std::fs::write(&path, serde_json::to_vec(&metadata).unwrap()).unwrap();
            load_publication_config_options(
                &path,
                no_credential_commands(),
                Arc::new(|| 0),
                Arc::new(|| Some("synthetic-bitbucket-env".into())),
                None,
                false,
                Some(tokens.clone()),
            )
        };
        let remote = |name: &str| format!("git@github.com:Octo-Fixture/{name}.git");
        let config = load(json!([
            {"repositoryPath":"/disposable/github","remote":remote("sandbox"),"repository":{"owner":"octo-fixture","name":"sandbox","id":42},"policy":policy},
            {"repositoryPath":"/disposable/bitbucket","repository":{"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"},"policy":policy}
        ]))
        .unwrap();
        assert_eq!(config.connections.len(), 2);
        assert!(config.unavailable.is_empty());
        let github = &config.connections[0];
        assert_eq!(github.provider, remote::PublicationProvider::GitHub);
        assert_eq!(
            github.repository,
            json!({"provider":"github","owner":"octo-fixture","name":"sandbox","id":42,"uuid":"github:42"})
        );
        assert_eq!(github.policy["optionalSteps"], json!([]));
        // Bitbucket keeps its own token route alongside GitHub.
        assert_eq!(
            config.connections[1].provider,
            remote::PublicationProvider::Bitbucket
        );
        assert!(
            matches!((config.connections[1].credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-bitbucket-env")
        );
        // No token: the operator is told where to connect GitHub.
        match (github.credentials)().await {
            Err(Error::Unavailable(e)) => {
                assert!(
                    e.message
                        .contains("Connect GitHub in Settings → Integrations")
                );
                assert!(!e.request_sent);
            }
            _ => panic!("A missing GitHub token must be unavailable"),
        }
        // The saved token applies without a restart; the environment wins.
        store
            .save(Integration::GitHub, "github_pat_synthetic_stored", false)
            .unwrap();
        assert!(
            matches!((github.credentials)().await.unwrap(), Credential::Bearer(v) if v == "github_pat_synthetic_stored")
        );
        *environment.lock().unwrap() = Some("ghp_synthetic_env".into());
        assert!(
            matches!((github.credentials)().await.unwrap(), Credential::Bearer(v) if v == "ghp_synthetic_env")
        );
        // The configured repository must be the remote's repository, and GitHub
        // metadata needs its numeric id.
        for entry in [
            json!({"repositoryPath":"/disposable/github","remote":remote("other"),"repository":{"owner":"octo-fixture","name":"sandbox","id":42},"policy":policy}),
            json!({"repositoryPath":"/disposable/github","remote":remote("sandbox"),"repository":{"owner":"octo-fixture","name":"sandbox"},"policy":policy}),
            json!({"repositoryPath":"/disposable/github","remote":remote("sandbox"),"repository":{"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"},"policy":policy}),
            json!({"repositoryPath":"/disposable/github","remote":remote("sandbox"),"repository":{"owner":"octo-fixture","name":"sandbox","id":42},"policy":{"statusKeys":[],"pipelineSteps":["test"]}}),
        ] {
            assert!(load(json!([entry])).is_err(), "{entry}");
        }
    }

    #[tokio::test]
    async fn stored_tokens_authenticate_on_linux_and_environment_overrides_them() {
        let root = tempfile::tempdir().unwrap();
        let jira = root.path().join("jira.json");
        std::fs::write(&jira, serde_json::to_vec(&json!({
            "siteUrl":"https://fixture.atlassian.net",
            "cloudId":"00000000-0000-4000-8000-000000000022",
            "filterId":"1","assigneeAccountId":"operator","name":"Fixture","accountId":"operator"
        })).unwrap()).unwrap();
        let bitbucket = root.path().join("bitbucket.json");
        std::fs::write(&bitbucket, serde_json::to_vec(&json!([{
            "repositoryPath":"/disposable/repository",
            "repository":{"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"},
            "policy":{"statusKeys":["pr"],"pipelineSteps":["Test"]}
        }])).unwrap()).unwrap();
        let store = SecretStore::open(root.path()).unwrap();
        let environment = Arc::new(Mutex::new(None::<String>));
        let env = |integration| -> TokenEnvironment {
            let _ = integration;
            let environment = environment.clone();
            Arc::new(move || environment.lock().unwrap().clone())
        };
        let jira_connection = load_jira_connection_options(
            &jira,
            no_credential_commands(),
            Arc::new(|| 0),
            env(Integration::Jira),
            Some(stored_tokens(store.clone(), Integration::Jira)),
            false,
        )
        .unwrap()
        .unwrap();
        let publication = load_publication_connections_options(
            &bitbucket,
            no_credential_commands(),
            Arc::new(|| 0),
            env(Integration::Bitbucket),
            Some(stored_tokens(store.clone(), Integration::Bitbucket)),
            false,
        )
        .unwrap()
        .remove(0);
        // Nothing saved yet: the Linux message names both routes.
        match (jira_connection.credentials)().await {
            Err(Error::Unavailable(e)) => {
                assert!(e.message.contains("Settings → Integrations"));
                assert!(e.message.contains("CREWROOM_JIRA_TOKEN"));
                assert!(!e.request_sent);
            }
            _ => panic!("Missing stored token must be unavailable"),
        }
        // A token saved after connection load applies without a restart.
        store
            .save(Integration::Jira, "synthetic-stored-jira", false)
            .unwrap();
        store
            .save(Integration::Bitbucket, "synthetic-stored-bitbucket", false)
            .unwrap();
        assert!(
            matches!((jira_connection.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-stored-jira")
        );
        assert!(
            matches!((publication.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-stored-bitbucket")
        );
        // The environment variable wins over the stored value.
        *environment.lock().unwrap() = Some("synthetic-env-override".into());
        assert!(
            matches!((jira_connection.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-env-override")
        );
        assert!(
            matches!((publication.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-env-override")
        );
        *environment.lock().unwrap() = None;
        // A lost install key makes both integrations unreadable, without a crash.
        std::fs::remove_file(store.secret_path()).unwrap();
        for credentials in [&jira_connection.credentials, &publication.credentials] {
            match credentials().await {
                Err(Error::Unavailable(e)) => {
                    assert!(e.message.contains(secrets::UNREADABLE), "{}", e.message);
                    assert!(!e.message.contains("synthetic"));
                    assert!(!e.request_sent);
                }
                _ => panic!("Unreadable stored token must be unavailable"),
            }
        }
        // An environment override still works while the store is unreadable.
        *environment.lock().unwrap() = Some("synthetic-env-override".into());
        assert!(
            matches!((jira_connection.credentials)().await.unwrap(), Credential::Bearer(v) if v == "synthetic-env-override")
        );
    }

    #[tokio::test]
    async fn trusted_command_capture_stops_on_overflow_timeout_and_failure_without_exposing_output()
    {
        let root = tempfile::tempdir().unwrap();
        let pid = root.path().join("pid");
        let execute = command();
        for (script, deadline) in [
            (
                "echo $$ > \"$1\"; while :; do printf 'synthetic-private-output'; done",
                Duration::from_secs(10),
            ),
            (
                "echo $$ > \"$1\"; exec sleep 30",
                Duration::from_millis(100),
            ),
            (
                "echo $$ > \"$1\"; printf 'synthetic-private-output' >&2; exit 2",
                Duration::from_secs(10),
            ),
        ] {
            let started = std::time::Instant::now();
            let result = execute(CredentialCommand {
                file: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    script.into(),
                    "fixture".into(),
                    pid.to_string_lossy().into_owned(),
                ],
                timeout: deadline,
                max_buffer: 32,
            })
            .await;
            assert!(result.is_err());
            assert!(
                !result
                    .unwrap_err()
                    .to_string()
                    .contains("synthetic-private")
            );
            assert!(started.elapsed() < Duration::from_secs(3));
            let id = std::fs::read_to_string(&pid)
                .unwrap()
                .trim()
                .parse::<i32>()
                .unwrap();
            // SAFETY: signal 0 only checks the fixture PID; the exact Child was
            // killed and waited above before this observation.
            assert_eq!(unsafe { libc::kill(id, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        let result = execute(CredentialCommand {
            file: "/bin/sh".into(),
            args: vec!["-c".into(), "printf 'synthetic\\n'".into()],
            timeout: Duration::from_secs(2),
            max_buffer: 32,
        })
        .await
        .unwrap();
        assert_eq!(result, "synthetic\n");
    }
}
