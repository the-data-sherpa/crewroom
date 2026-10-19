//! Same daemon composition with explicitly injected loopback provider fixtures.
//! Production credentials and endpoints are never read by this test executable.
use crewroom_core::Result;
use crewroom_runtime::{
    providers::{Credential, Credentials, bitbucket::BitbucketClient, jira::JiraClient},
    publication_remote::Connection,
};
use crewroom_service::{Service, ServiceOptions};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
fn endpoint(v: &Value) -> Result<&str> {
    v.as_str()
        .filter(|v| v.starts_with("http://127.0.0.1:"))
        .ok_or_else(|| std::io::Error::other("Fixture requires loopback HTTP").into())
}
fn credentials(token: &'static str) -> Credentials {
    Arc::new(move || Box::pin(async move { Ok(Credential::Bearer(token.into())) }))
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let config: Value = serde_json::from_slice(&std::fs::read(std::env::args().nth(1).unwrap())?)?;
    let jira = config
        .get("jira")
        .map(|c| -> Result<_> {
            Ok(JiraClient::new(
                &c["source"],
                credentials("synthetic-jira-token"),
                Some(endpoint(&c["apiUrl"])?),
            )
            .map_err(std::io::Error::other)?)
        })
        .transpose()?;
    let publication = config["publicationRemote"]["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| -> Result<_> {
            let client = BitbucketClient::new(
                &c["repository"],
                credentials("synthetic-bitbucket-token"),
                Some(endpoint(&c["apiUrl"])?),
            )
            .map_err(std::io::Error::other)?;
            Connection::new(
                Path::new(c["repositoryPath"].as_str().unwrap()),
                &c["repository"],
                client,
                &c["policy"],
                Some(Path::new(c["fixtureGitPath"].as_str().unwrap())),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let bin = PathBuf::from(config["binaryDirectory"].as_str().unwrap());
    let options = ServiceOptions {
        runtime: Some(config["runtime"].clone()),
        helper: bin.join("crewroom-process"),
        host: bin.join("crewroom-host"),
        tmux: std::env::var_os("CREWROOM_TMUX")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(if cfg!(target_os = "macos") {
                    "/opt/homebrew/bin/tmux"
                } else {
                    "/usr/bin/tmux"
                })
            }),
        jira,
        publication,
        publication_unavailable: vec![],
        environment: config["environment"].clone(),
        runner: Default::default(),
        github: None,
    };
    let server = Service::start_with_options(
        Path::new(config["authority"].as_str().unwrap()),
        Path::new(config["roles"].as_str().unwrap()),
        config["nativePort"].as_u64().unwrap_or(0) as u16,
        options,
    )
    .await?;
    tokio::select! {_=interrupt.recv()=>{},_=terminate.recv()=>{}}
    server.close().await
}
