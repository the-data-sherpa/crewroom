//! FOR-73: models available through the operator's own OMP login, beyond the
//! providers in Crewroom's runner configuration. Metadata only: the listing
//! runs `omp models --json` with the service's HOME, keeps only identity and
//! capability fields, and never reads, returns or stores a credential.
//!
//! Crewroom launches OMP through its provider gateway, which supports only
//! API-key providers defined in the runner configuration. A login model is
//! therefore launchable only when its provider is defined there; every other
//! login model stays listed with the reason it cannot launch.
use super::{
    discovery::{self, Config, Process, bounded, private_dir},
    *,
};
use std::{collections::BTreeMap, path::Path};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

/// OMP providers whose login is a subscription (OAuth) sign-in rather than
/// an API key. Names only; Crewroom never inspects the stored login.
pub const SUBSCRIPTION_PROVIDERS: &[&str] = &[
    "openai-codex",
    "anthropic",
    "github-copilot",
    "google-gemini-cli",
    "google-antigravity",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Listing {
    pub outcome: store::Outcome,
    pub observed_at: u64,
    #[serde(default)]
    pub offerings: Vec<Offering>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The login home to list from: an absolute HOME that holds an OMP login.
pub fn home(environment: &Value) -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(environment["HOME"].as_str()?);
    (home.is_absolute() && home.join(".omp").is_dir()).then_some(home)
}

/// List the login's models through the configured OMP executable.
pub async fn discover(
    config: &Config,
    home: &Path,
    root: &Path,
    observed_at: u64,
    cancel: &CancellationToken,
    limit: Duration,
) -> Result<Vec<Offering>> {
    if config.harness != Harness::Omp {
        return Err(conflict("Only OMP has a login model listing."));
    }
    private_dir(root)?;
    let env = BTreeMap::from([
        ("HOME".into(), home.to_string_lossy().into()),
        ("PATH".into(), config.path.clone()),
        ("LANG".into(), "C.UTF-8".into()),
        ("NO_COLOR".into(), "1".into()),
    ]);
    let args = ["models", "--json", "--no-extensions"]
        .map(String::from)
        .to_vec();
    let mut process = Process::start(config, root, "login", args, &env).await?;
    let result = bounded(&mut process, cancel, limit, |p| Box::pin(p.collect())).await;
    process.close().await?;
    let value: Value =
        serde_json::from_slice(&result?).map_err(|_| conflict("Invalid OMP login listing."))?;
    // Only identity and capability fields are read; anything else in the
    // output, including any credential-shaped field, is dropped here.
    discovery::normalize(&Harness::Omp, &value["models"], observed_at)
}

/// Why a login model cannot launch, or none when its provider is defined in
/// the runner configuration (and so is listed through the runner instead).
pub fn launch_blocker(provider: &str, runner_providers: &Value) -> Option<String> {
    if runner_providers[provider].is_object() {
        return None;
    }
    Some(if SUBSCRIPTION_PROVIDERS.contains(&provider) {
        format!(
            "Listed by your OMP login ({provider}), which is a subscription sign-in. Crewroom's provider gateway launches only API-key providers defined in the runner configuration."
        )
    } else {
        format!(
            "Listed by your OMP login ({provider}). Crewroom's provider gateway launches only API-key providers defined in the runner configuration; add {provider} there with an API key to launch it."
        )
    })
}

/// Login offerings not already covered by a runner-configured provider.
pub fn unlaunchable<'a>(
    listing: &'a Listing,
    runner_providers: &'a Value,
) -> impl Iterator<Item = (&'a Offering, String)> + 'a {
    listing
        .offerings
        .iter()
        .filter_map(move |o| launch_blocker(&o.provider, runner_providers).map(|r| (o, r)))
}

/// The OMP harness row's summary of the login listing, in operator words.
pub fn summary(listing: Option<&Listing>, runner_providers: &Value) -> Value {
    let Some(listing) = listing else {
        return Value::Null;
    };
    if listing.outcome != store::Outcome::Success {
        return json!({"outcome":listing.outcome,"error":listing.error,"models":0,"launchable":0});
    }
    let blocked = unlaunchable(listing, runner_providers).count();
    let mut providers: Vec<&str> = listing
        .offerings
        .iter()
        .map(|o| o.provider.as_str())
        .collect();
    providers.sort_unstable();
    providers.dedup();
    json!({
        "outcome": listing.outcome,
        "models": listing.offerings.len(),
        "launchable": listing.offerings.len() - blocked,
        "providers": providers,
        "note": format!(
            "Your OMP login lists {} models from {} providers; {} can launch through Crewroom's provider gateway, which supports only API-key providers defined in the runner configuration.",
            listing.offerings.len(),
            providers.len(),
            listing.offerings.len() - blocked
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchability_follows_the_runner_configuration_and_names_the_reason() {
        let providers = json!({"configured":{"apiKey":"x"}});
        assert_eq!(launch_blocker("configured", &providers), None);
        let subscription = launch_blocker("openai-codex", &providers).unwrap();
        assert!(
            subscription.contains("subscription sign-in"),
            "{subscription}"
        );
        assert!(
            subscription.contains("only API-key providers"),
            "{subscription}"
        );
        let other = launch_blocker("zai", &providers).unwrap();
        assert!(other.contains("add zai there with an API key"), "{other}");
    }

    #[test]
    fn home_requires_an_absolute_home_with_an_omp_login() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(home(&json!({"HOME":root.path()})), None);
        std::fs::create_dir(root.path().join(".omp")).unwrap();
        assert_eq!(
            home(&json!({"HOME":root.path()})),
            Some(root.path().to_path_buf())
        );
        assert_eq!(home(&json!({"HOME":"relative"})), None);
        assert_eq!(home(&json!({})), None);
    }
}
