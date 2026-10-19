//! FOR-73 live check: list the operator's OMP login through the production
//! listing boundary and print counts only. No model output, provider
//! configuration or credential is printed.
use crewroom_runtime::model_catalog::{
    Harness,
    discovery::Config,
    login::{self, Listing},
    store::Outcome,
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: omp-login-models OMP_BINARY PROCESS_HELPER PRIVATE_ROOT".into());
    }
    let home =
        login::home(&json!({"HOME": std::env::var("HOME")?})).ok_or("HOME holds no OMP login")?;
    // A bare name is resolved on PATH, as the unconfigured service does.
    let binary = if args[0].contains('/') {
        PathBuf::from(&args[0])
    } else {
        std::env::split_paths(&std::env::var("PATH")?)
            .map(|dir| dir.join(&args[0]))
            .find(|candidate| candidate.is_file())
            .ok_or("OMP is not on PATH")?
    };
    let config = Config {
        harness: Harness::Omp,
        binary,
        helper: PathBuf::from(&args[1]),
        path: std::env::var("PATH")?,
        omp_models: json!({"providers":{}}),
    };
    let offerings = login::discover(
        &config,
        &home,
        &PathBuf::from(&args[2]),
        0,
        &CancellationToken::new(),
        Duration::from_secs(60),
    )
    .await?;
    let listing = Listing {
        outcome: Outcome::Success,
        observed_at: 0,
        offerings,
        error: None,
    };
    // Launchability as a service without runner-configured providers sees it.
    let runner = json!({});
    let blocked: Vec<String> = login::unlaunchable(&listing, &runner)
        .map(|(_, reason)| reason)
        .collect();
    let subscription = blocked
        .iter()
        .filter(|r| r.contains("subscription sign-in"))
        .count();
    let serialized = serde_json::to_string(&listing)?;
    let summary = login::summary(Some(&listing), &runner);
    let credential_shaped = ["apiKey", "api_key", "secret", "password", "Bearer"]
        .iter()
        .any(|n| serialized.contains(n));
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "models": summary["models"],
            "providers": summary["providers"].as_array().map(Vec::len),
            "launchable": summary["launchable"],
            "notLaunchable": {"subscriptionSignIn": subscription, "notInRunnerConfiguration": blocked.len() - subscription},
            "credentialShapedTextInListing": credential_shaped,
        }))?
    );
    Ok(())
}
