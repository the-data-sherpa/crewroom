//! Provider-neutral PR host. Publication, readiness observation, feedback and
//! continuation verification reach the provider only through this interface.
//! Bitbucket and GitHub implement it; every call delegates to the provider
//! client, which returns the same normalized publication shapes.
use crate::{
    providers::{
        BeforeDispatch, Result,
        bitbucket::{self, BitbucketClient},
        github_pulls::{self, GitHubPullClient},
        remote::PublicationProvider,
    },
    publication_git::Remote,
    repository::text,
};
use crewroom_core::Error;
use serde_json::Value;

#[derive(Clone)]
pub enum PullRequestHost {
    Bitbucket(BitbucketClient),
    GitHub(GitHubPullClient),
}
impl From<BitbucketClient> for PullRequestHost {
    fn from(client: BitbucketClient) -> Self {
        Self::Bitbucket(client)
    }
}
impl From<GitHubPullClient> for PullRequestHost {
    fn from(client: GitHubPullClient) -> Self {
        Self::GitHub(client)
    }
}
impl PullRequestHost {
    pub fn provider(&self) -> PublicationProvider {
        match self {
            Self::Bitbucket(_) => PublicationProvider::Bitbucket,
            Self::GitHub(_) => PublicationProvider::GitHub,
        }
    }
    /// The provider's normalized repository identity.
    pub fn repository(&self) -> &Value {
        match self {
            Self::Bitbucket(client) => &client.repository,
            Self::GitHub(client) => &client.repository,
        }
    }
    /// Rejects configured repository metadata that does not name this host's
    /// repository.
    pub(crate) fn check_configured(&self, repository: &Value) -> crewroom_core::Result<()> {
        match self {
            Self::Bitbucket(client) => {
                if bitbucket::repository(repository).map_err(|_| {
                    Error::Invalid("Invalid configured Bitbucket repository.".into())
                })? != client.repository
                {
                    return Err(Error::Invalid(
                        "Configured Bitbucket client identity changed.".into(),
                    ));
                }
                Ok(())
            }
            Self::GitHub(client) => {
                if github_pulls::repository(repository)
                    .map_err(|_| Error::Invalid("Invalid configured GitHub repository.".into()))?
                    != client.repository
                {
                    return Err(Error::Invalid(
                        "Configured GitHub client identity changed.".into(),
                    ));
                }
                Ok(())
            }
        }
    }
    /// The Git transport remote for pushes and reference observation.
    pub(crate) fn git_remote(&self) -> crewroom_core::Result<Remote> {
        match self {
            Self::Bitbucket(client) => Ok(Remote::Bitbucket {
                workspace: text(&client.repository, "workspace")?.into(),
                slug: text(&client.repository, "slug")?.into(),
            }),
            Self::GitHub(client) => Ok(Remote::GitHub {
                owner: text(&client.repository, "owner")?.into(),
                name: text(&client.repository, "name")?.into(),
            }),
        }
    }
    pub async fn verify_repository(&self) -> Result<Value> {
        match self {
            Self::Bitbucket(client) => client.verify_repository().await,
            Self::GitHub(client) => client.verify_repository().await,
        }
    }
    pub async fn pull_requests(&self, branch: &str, target: &str) -> Result<Vec<Value>> {
        match self {
            Self::Bitbucket(client) => client.pull_requests(branch, target).await,
            Self::GitHub(client) => client.pull_requests(branch, target).await,
        }
    }
    pub async fn pull_request(&self, id: i64) -> Result<Value> {
        match self {
            Self::Bitbucket(client) => client.pull_request(id).await,
            Self::GitHub(client) => client.pull_request(id).await,
        }
    }
    pub async fn comments(&self, id: i64) -> Result<Value> {
        match self {
            Self::Bitbucket(client) => client.comments(id).await,
            Self::GitHub(client) => client.comments(id).await,
        }
    }
    pub async fn observe(&self, id: i64) -> Result<Value> {
        match self {
            Self::Bitbucket(client) => client.observe(id).await,
            Self::GitHub(client) => client.observe(id).await,
        }
    }
    pub async fn create_pull_request(
        &self,
        input: &Value,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        match self {
            Self::Bitbucket(client) => client.create_pull_request(input, before).await,
            Self::GitHub(client) => client.create_pull_request(input, before).await,
        }
    }
    pub async fn update_pull_request(
        &self,
        id: i64,
        input: &Value,
        before: Option<&BeforeDispatch>,
    ) -> Result<Value> {
        match self {
            Self::Bitbucket(client) => client.update_pull_request(id, input, before).await,
            Self::GitHub(client) => client.update_pull_request(id, input, before).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Credential, Credentials};
    use serde_json::json;
    use std::sync::Arc;

    fn host() -> PullRequestHost {
        let credentials: Credentials = Arc::new(|| {
            Box::pin(async { Ok(Credential::Bearer("synthetic-bitbucket-token".into())) })
        });
        BitbucketClient::new(
            &json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"}),
            credentials,
            None,
        )
        .unwrap()
        .into()
    }

    #[test]
    fn bitbucket_host_keeps_its_identity_and_transport_remote() {
        let host = host();
        assert_eq!(host.provider(), PublicationProvider::Bitbucket);
        assert_eq!(host.provider().name(), "Bitbucket");
        assert_eq!(
            host.repository(),
            &json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"})
        );
        match host.git_remote().unwrap() {
            Remote::Bitbucket { workspace, slug } => {
                assert_eq!(
                    (workspace.as_str(), slug.as_str()),
                    ("fixture", "repository")
                )
            }
            _ => panic!("A Bitbucket host must resolve to its Bitbucket remote"),
        }
    }

    #[test]
    fn github_host_keeps_its_identity_and_transport_remote() {
        let credentials: Credentials =
            Arc::new(|| Box::pin(async { Ok(Credential::Bearer("ghp_synthetic".into())) }));
        let host: PullRequestHost = GitHubPullClient::new(
            &json!({"owner":"octo-fixture","name":"sandbox","id":42}),
            credentials,
            None,
        )
        .unwrap()
        .into();
        assert_eq!(host.provider(), PublicationProvider::GitHub);
        assert_eq!(host.repository()["uuid"], "github:42");
        match host.git_remote().unwrap() {
            Remote::GitHub { owner, name } => {
                assert_eq!((owner.as_str(), name.as_str()), ("octo-fixture", "sandbox"))
            }
            _ => panic!("A GitHub host must resolve to its GitHub remote"),
        }
        host.check_configured(&json!({"owner":"octo-fixture","name":"sandbox","id":42}))
            .unwrap();
        host.check_configured(host.repository()).unwrap();
        assert_eq!(
            host.check_configured(&json!({"owner":"octo-fixture","name":"sandbox","id":7}))
                .unwrap_err()
                .to_string(),
            Error::Invalid("Configured GitHub client identity changed.".into()).to_string()
        );
        assert_eq!(
            host.check_configured(&json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"}))
                .unwrap_err()
                .to_string(),
            Error::Invalid("Invalid configured GitHub repository.".into()).to_string()
        );
    }

    #[test]
    fn bitbucket_host_rejects_other_configured_repositories_with_original_messages() {
        let host = host();
        host.check_configured(&json!({"workspace":"fixture","slug":"repository","uuid":"{00000000-0000-4000-8000-000000000011}"}))
            .unwrap();
        let changed = host
            .check_configured(&json!({"workspace":"fixture","slug":"other","uuid":"{00000000-0000-4000-8000-000000000011}"}))
            .unwrap_err();
        assert_eq!(
            changed.to_string(),
            Error::Invalid("Configured Bitbucket client identity changed.".into()).to_string()
        );
        let invalid = host
            .check_configured(&json!({"workspace":"fixture","slug":"repository"}))
            .unwrap_err();
        assert_eq!(
            invalid.to_string(),
            Error::Invalid("Invalid configured Bitbucket repository.".into()).to_string()
        );
    }
}
