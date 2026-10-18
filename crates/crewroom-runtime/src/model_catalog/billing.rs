//! Billing route of a model offering: which account pays for its inference.
//! Detected from the harness's own authentication configuration, as the
//! Crewroom launch would use it. Detection reads only the presence and kind of
//! credentials; it never returns, persists or logs a credential value. When
//! the configuration does not establish one route, the route is `unknown`.
use super::*;
use std::{
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingRoute {
    Subscription,
    Api,
    Unknown,
}
impl BillingRoute {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Api => "api",
            Self::Unknown => "unknown",
        }
    }
    /// Operator-facing name used in picker labels.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Api => "API",
            Self::Unknown => "unknown billing",
        }
    }
}

/// Routes reachable through one harness/provider and the configuration fact
/// that established them. `basis` names files and field kinds, never values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detection {
    pub routes: Vec<BillingRoute>,
    pub basis: String,
}
impl Detection {
    fn new(routes: Vec<BillingRoute>, basis: impl Into<String>) -> Self {
        Self {
            routes,
            basis: basis.into(),
        }
    }
    fn unknown(basis: impl Into<String>) -> Self {
        Self::new(vec![BillingRoute::Unknown], basis)
    }
}

/// Where each harness keeps the login Crewroom launches it with. Private:
/// never serialized to the workspace.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// Codex `auth.json` (runtime `codex.authFile`).
    pub codex_auth: Option<PathBuf>,
    /// Claude Code native HOME (runtime `claudeCode.home`).
    pub claude_home: Option<PathBuf>,
    /// Runtime `models.providers` definitions used for OMP.
    pub omp_providers: Value,
}
impl Sources {
    /// The native login locations for a service without a configured runner,
    /// from its own environment (`CODEX_HOME`, else `HOME/.codex`; `HOME`).
    pub fn native(environment: &Value) -> Self {
        let absolute = |key: &str| {
            environment[key]
                .as_str()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let home = absolute("HOME");
        Self {
            codex_auth: absolute("CODEX_HOME")
                .or_else(|| home.as_ref().map(|h| h.join(".codex")))
                .map(|dir| dir.join("auth.json")),
            claude_home: home,
            omp_providers: json!({}),
        }
    }
    pub fn detect(&self, harness: &Harness, provider: &str) -> Detection {
        match harness {
            Harness::Codex => match &self.codex_auth {
                Some(path) => codex(path),
                None => Detection::unknown("No Codex login file is configured."),
            },
            Harness::ClaudeCode => match &self.claude_home {
                Some(home) => claude_code(home),
                None => Detection::unknown("No Claude Code login home is configured."),
            },
            Harness::Omp => omp(&self.omp_providers[provider]),
        }
    }
    /// One summary row per harness (per configured provider for OMP) for the
    /// catalog view. Values are kinds and file names only.
    pub fn summary(&self, harness: &Harness) -> Value {
        match harness {
            Harness::Omp => {
                let providers = self.omp_providers.as_object();
                json!(
                    providers
                        .into_iter()
                        .flatten()
                        .map(|(name, _)| json!({"provider":name,"detection":self.detect(harness,name)}))
                        .collect::<Vec<_>>()
                )
            }
            Harness::Codex => {
                json!([{"provider":"codex","detection":self.detect(harness,"codex")}])
            }
            Harness::ClaudeCode => {
                json!([{"provider":"claude-code","detection":self.detect(harness,"claude-code")}])
            }
        }
    }
}

const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Read a JSON file without following a final symlink. Errors carry no content.
fn private_json(path: &Path) -> Option<Value> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    serde_json::from_reader(std::io::BufReader::new(file)).ok()
}
fn present(value: &Value) -> bool {
    value.as_str().is_some_and(|s| !s.trim().is_empty())
}

/// Codex: `auth_mode: chatgpt` with ChatGPT tokens is the subscription login;
/// `auth_mode: apikey` with `OPENAI_API_KEY` is API billing. A ChatGPT login
/// that also stores an API key can reach both routes.
pub fn codex(auth_file: &Path) -> Detection {
    let Some(value) = private_json(auth_file) else {
        return Detection::unknown("Codex login file (auth.json) is missing or unreadable.");
    };
    let tokens = present(&value["tokens"]["access_token"]);
    let key = present(&value["OPENAI_API_KEY"]);
    match value["auth_mode"].as_str() {
        Some("chatgpt") if tokens && key => Detection::new(
            vec![BillingRoute::Subscription, BillingRoute::Api],
            "Codex auth.json has a ChatGPT login (auth_mode chatgpt) and also stores an OpenAI API key.",
        ),
        Some("chatgpt") if tokens => Detection::new(
            vec![BillingRoute::Subscription],
            "Codex auth.json has a ChatGPT login (auth_mode chatgpt).",
        ),
        Some("apikey" | "api_key" | "apiKey") if key => Detection::new(
            vec![BillingRoute::Api],
            "Codex auth.json uses an OpenAI API key (auth_mode apikey).",
        ),
        _ => Detection::unknown(
            "Codex auth.json does not establish a ChatGPT login or an API key login.",
        ),
    }
}

/// Claude Code as Crewroom launches it: native HOME, no inherited
/// `ANTHROPIC_API_KEY`, no settings sources (so no `apiKeyHelper`). A claude.ai
/// OAuth login with inference scope and a subscription plan is subscription;
/// a Console login's stored API key is API billing. Both or neither: unknown.
pub fn claude_code(home: &Path) -> Detection {
    let credentials = private_json(&home.join(".claude/.credentials.json"));
    let oauth = credentials
        .as_ref()
        .map(|v| &v["claudeAiOauth"])
        .filter(|o| o.is_object());
    let subscription = oauth.is_some_and(|o| {
        present(&o["accessToken"])
            && present(&o["subscriptionType"])
            && o["scopes"]
                .as_array()
                .is_none_or(|s| s.iter().any(|v| v == "user:inference"))
    });
    let api =
        private_json(&home.join(".claude.json")).is_some_and(|v| present(&v["primaryApiKey"]));
    match (subscription, api) {
        (true, false) => Detection::new(
            vec![BillingRoute::Subscription],
            "Claude Code has a claude.ai subscription login (.credentials.json OAuth with a subscription plan).",
        ),
        (false, true) => Detection::new(
            vec![BillingRoute::Api],
            "Claude Code has a Console API key login (.claude.json primaryApiKey).",
        ),
        (true, true) => Detection::unknown(
            "Claude Code stores both a subscription login and a Console API key; which one it uses is not determined.",
        ),
        (false, false) if oauth.is_some() => Detection::unknown(
            "Claude Code has an OAuth login without a recorded subscription plan.",
        ),
        (false, false) => Detection::unknown(
            "No Claude Code login file was found in its home (.credentials.json); it may be in a system keychain.",
        ),
    }
}

/// First-party, per-token billed API hosts. Any other base URL (a gateway or
/// proxy) can front either kind of account, so it stays unknown.
const API_HOSTS: &[&str] = &[
    "api.openai.com",
    "api.anthropic.com",
    "generativelanguage.googleapis.com",
    "openrouter.ai",
];

/// OMP runs through Crewroom's provider gateway with the configured provider
/// key; its own OAuth logins are never used by Crewroom launches.
pub fn omp(provider: &Value) -> Detection {
    if !provider.is_object() {
        return Detection::unknown("This OMP provider is not defined in the runner configuration.");
    }
    let host = provider["baseUrl"]
        .as_str()
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_owned));
    match host {
        Some(host) if present(&provider["apiKey"]) && API_HOSTS.contains(&host.as_str()) => {
            Detection::new(
                vec![BillingRoute::Api],
                format!("OMP provider uses an API key with {host}."),
            )
        }
        Some(_) => Detection::unknown(
            "OMP provider uses a gateway or host whose billing Crewroom cannot determine.",
        ),
        None => Detection::unknown("OMP provider has no valid base URL."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crewroom_core::authority::write_private;

    const CANARY: &str = "billing-secret-canary";

    fn write(path: &Path, value: Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_private(path, value.to_string().as_bytes()).unwrap();
    }
    fn routes(d: &Detection) -> Vec<&'static str> {
        d.routes.iter().map(BillingRoute::as_str).collect()
    }

    #[test]
    fn codex_auth_modes_map_to_routes_without_exposing_values() {
        let root = tempfile::tempdir().unwrap();
        let auth = root.path().join("auth.json");
        let cases = [
            (
                json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"access_token":CANARY,"account_id":CANARY}}),
                vec!["subscription"],
            ),
            (
                json!({"auth_mode":"apikey","OPENAI_API_KEY":CANARY}),
                vec!["api"],
            ),
            (
                json!({"auth_mode":"chatgpt","OPENAI_API_KEY":CANARY,"tokens":{"access_token":CANARY}}),
                vec!["subscription", "api"],
            ),
            (json!({"auth_mode":"chatgpt","tokens":{}}), vec!["unknown"]),
            (json!({"auth_mode":"apikey"}), vec!["unknown"]),
            (json!({"OPENAI_API_KEY":CANARY}), vec!["unknown"]),
        ];
        for (value, expected) in cases {
            write(&auth, value.clone());
            let detection = codex(&auth);
            assert_eq!(routes(&detection), expected, "{value}");
            assert!(!serde_json::to_string(&detection).unwrap().contains(CANARY));
        }
        assert_eq!(
            routes(&codex(&root.path().join("missing.json"))),
            ["unknown"]
        );
        std::fs::write(&auth, "not json").unwrap();
        assert_eq!(routes(&codex(&auth)), ["unknown"]);
        // A symlinked login is not followed.
        let link = root.path().join("link.json");
        write(&auth, json!({"auth_mode":"apikey","OPENAI_API_KEY":CANARY}));
        std::os::unix::fs::symlink(&auth, &link).unwrap();
        assert_eq!(routes(&codex(&link)), ["unknown"]);
    }

    #[test]
    fn claude_logins_map_to_routes_and_ambiguity_is_unknown() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        let credentials = home.join(".claude/.credentials.json");
        let account = home.join(".claude.json");
        assert_eq!(routes(&claude_code(home)), ["unknown"]);
        write(
            &credentials,
            json!({"claudeAiOauth":{"accessToken":CANARY,"refreshToken":CANARY,"scopes":["user:inference"],"subscriptionType":"pro"}}),
        );
        write(&account, json!({"oauthAccount":{"accountUuid":CANARY}}));
        let detection = claude_code(home);
        assert_eq!(routes(&detection), ["subscription"]);
        assert!(!serde_json::to_string(&detection).unwrap().contains(CANARY));
        write(
            &account,
            json!({"oauthAccount":{"accountUuid":CANARY},"primaryApiKey":CANARY}),
        );
        assert_eq!(routes(&claude_code(home)), ["unknown"]);
        std::fs::remove_file(&credentials).unwrap();
        assert_eq!(routes(&claude_code(home)), ["api"]);
        write(&account, json!({}));
        write(
            &credentials,
            json!({"claudeAiOauth":{"accessToken":CANARY,"scopes":["user:inference"]}}),
        );
        assert_eq!(routes(&claude_code(home)), ["unknown"]);
        write(
            &credentials,
            json!({"claudeAiOauth":{"accessToken":CANARY,"scopes":["user:profile"],"subscriptionType":"max"}}),
        );
        assert_eq!(routes(&claude_code(home)), ["unknown"]);
    }

    #[test]
    fn omp_providers_are_api_only_on_first_party_hosts() {
        let api = json!({"apiKey":CANARY,"baseUrl":"https://api.openai.com/v1"});
        let detection = omp(&api);
        assert_eq!(routes(&detection), ["api"]);
        assert!(!serde_json::to_string(&detection).unwrap().contains(CANARY));
        for provider in [
            json!({"apiKey":CANARY,"baseUrl":"http://127.0.0.1:5555"}),
            json!({"baseUrl":"https://api.anthropic.com"}),
            json!({"apiKey":CANARY,"baseUrl":"not a url"}),
            Value::Null,
        ] {
            assert_eq!(routes(&omp(&provider)), ["unknown"], "{provider}");
        }
    }

    #[test]
    fn native_sources_follow_codex_home_and_home() {
        let sources = Sources::native(&json!({"HOME":"/home/fixture"}));
        assert_eq!(
            sources.codex_auth.unwrap(),
            Path::new("/home/fixture/.codex/auth.json")
        );
        assert_eq!(sources.claude_home.unwrap(), Path::new("/home/fixture"));
        let sources = Sources::native(&json!({"HOME":"/home/fixture","CODEX_HOME":"/srv/codex"}));
        assert_eq!(
            sources.codex_auth.unwrap(),
            Path::new("/srv/codex/auth.json")
        );
        let sources = Sources::native(&json!({"HOME":"relative"}));
        assert!(sources.codex_auth.is_none() && sources.claude_home.is_none());
        assert_eq!(
            routes(&sources.detect(&Harness::Codex, "codex")),
            ["unknown"]
        );
    }
}
