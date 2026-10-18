//! Metadata-only acceptance entry point using the production discovery boundary.
use crewroom_runtime::model_catalog::{
    Harness,
    discovery::{Config, discover},
};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 4 {
        return Err("usage: model-discovery HARNESS BINARY PROCESS_HELPER PRIVATE_ROOT".into());
    }
    let harness = match args[0].as_str() {
        "omp" => Harness::Omp,
        "codex" => Harness::Codex,
        "claude-code" => Harness::ClaudeCode,
        _ => return Err("unknown harness".into()),
    };
    let config = Config {
        harness,
        binary: PathBuf::from(&args[1]),
        helper: PathBuf::from(&args[2]),
        path: std::env::var("PATH")?,
        omp_models: json!({"providers":{}}),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let receipt = discover(
        &config,
        &PathBuf::from(&args[3]),
        now,
        &CancellationToken::new(),
        Duration::from_secs(30),
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}
