//! Fixed-origin provider clients. Private credentials and conservative transport
//! outcomes stay separate from durable workflow records and delivery decisions.
pub mod bitbucket;
mod bitbucket_schema;
pub mod config;
pub mod github;
pub mod github_issue_client;
pub mod github_pulls;
pub mod jira;
pub mod remote;
mod schema;
pub mod secrets;
mod singleflight;
mod transport;
use serde::Serialize;
pub use transport::{BeforeDispatch, Credential, Credentials};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Jira,
    Bitbucket,
    GitHub,
}
impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Jira => "Jira",
            Self::Bitbucket => "Bitbucket",
            Self::GitHub => "GitHub",
        }
    }
    fn unavailable(self) -> &'static str {
        match self {
            Self::Jira => {
                "Jira is unavailable. Check the server connection and credentials; saved work is retained."
            }
            Self::Bitbucket => "Bitbucket is unavailable. Saved delivery intents are retained.",
            Self::GitHub => "GitHub is unavailable. Check the network and the saved token.",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unavailable {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub request_sent: bool,
}
#[derive(Clone, Debug)]
pub enum Error {
    Unavailable(Unavailable),
    Invalid(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(v) => f.write_str(&v.message),
            Self::Invalid(v) => f.write_str(v),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn invalid() -> Error {
    Error::Invalid("Provider returned an invalid response.".into())
}
fn unavailable(message: impl Into<String>) -> Error {
    Error::Unavailable(Unavailable {
        message: message.into(),
        status: None,
        request_sent: false,
    })
}
