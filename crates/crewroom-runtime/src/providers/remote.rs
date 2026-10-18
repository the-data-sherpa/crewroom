//! Provider-neutral selection of the PR publication host from a repository's
//! configured publish remote. Bitbucket and github.com are implemented
//! providers; any other well-formed remote is reported as an unavailable
//! publication provider rather than silently ignored.
use super::*;
use std::path::PathBuf;

/// The PR host a repository publishes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicationProvider {
    Bitbucket,
    GitHub,
}
impl PublicationProvider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bitbucket => Provider::Bitbucket.name(),
            Self::GitHub => Provider::GitHub.name(),
        }
    }
    fn host(self) -> &'static str {
        match self {
            Self::Bitbucket => "bitbucket.org",
            Self::GitHub => "github.com",
        }
    }
    fn from_host(host: &str) -> Option<Self> {
        [Self::Bitbucket, Self::GitHub]
            .into_iter()
            .find(|p| p.host() == host)
    }
}

/// The provider resolved from a configured publish remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteSelection {
    /// A supported provider, with the remote's owner and repository name.
    Supported {
        provider: PublicationProvider,
        owner: String,
        name: String,
    },
    /// A well-formed remote on a host no publication provider implements.
    Unsupported { host: String },
}

/// A configured repository whose publish remote has no publication provider.
/// It carries no credential and is safe to project to the operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailablePublication {
    pub repository_path: PathBuf,
    pub host: String,
    pub reason: String,
}
impl UnavailablePublication {
    pub fn unsupported(repository_path: PathBuf, host: &str) -> Self {
        Self {
            repository_path,
            host: host.into(),
            reason: format!(
                "PR publication is unavailable: the publish remote host {host} has no supported publication provider. Local validation can complete; no PR will be created."
            ),
        }
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
}

/// Splits a publish remote into its lowercase host and repository path. Remotes
/// carrying credentials, queries or fragments are rejected so that metadata
/// never holds a secret.
fn split(remote: &str) -> Option<(String, String)> {
    if remote.is_empty()
        || remote.len() > 2048
        || remote.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let (host, path) = if remote.contains("://") {
        let url = url::Url::parse(remote).ok()?;
        let https = match url.scheme() {
            "https" => true,
            "ssh" => false,
            _ => return None,
        };
        if url.password().is_some()
            || (https && !url.username().is_empty())
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let host = match url.host()? {
            url::Host::Domain(host) => host.to_ascii_lowercase(),
            _ => return None,
        };
        (host, url.path().to_owned())
    } else {
        // scp-like `user@host:owner/name`; a `/` before `:` is a local path.
        let (authority, path) = remote.split_once(':')?;
        if authority.contains('/') || path.starts_with('/') {
            return None;
        }
        let (user, host) = authority.split_once('@')?;
        if user.is_empty() || user.contains('@') {
            return None;
        }
        (host.to_ascii_lowercase(), path.to_owned())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if !valid_host(&host)
        || path.is_empty()
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return None;
    }
    Some((host, path.into()))
}

/// Resolves the publication provider for a configured publish remote.
pub fn select(remote: &str) -> Result<RemoteSelection> {
    let (host, path) = split(remote).ok_or_else(invalid)?;
    let Some(provider) = PublicationProvider::from_host(&host) else {
        return Ok(RemoteSelection::Unsupported { host });
    };
    let (owner, name) = path.split_once('/').ok_or_else(invalid)?;
    if name.contains('/') {
        return Err(invalid());
    }
    Ok(RemoteSelection::Supported {
        provider,
        owner: owner.into(),
        name: name.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supported(owner: &str, name: &str) -> RemoteSelection {
        RemoteSelection::Supported {
            provider: PublicationProvider::Bitbucket,
            owner: owner.into(),
            name: name.into(),
        }
    }

    #[test]
    fn bitbucket_remotes_select_the_bitbucket_provider() {
        for remote in [
            "https://bitbucket.org/fixture/repository.git",
            "https://bitbucket.org/fixture/repository",
            "https://BitBucket.org/fixture/repository/",
            "ssh://git@bitbucket.org/fixture/repository.git",
            "git@bitbucket.org:fixture/repository.git",
            "git@bitbucket.org:fixture/repository",
        ] {
            assert_eq!(
                select(remote).unwrap(),
                supported("fixture", "repository"),
                "{remote}"
            );
        }
    }

    #[test]
    fn github_remotes_select_the_github_provider() {
        for remote in [
            "https://github.com/fixture/repository.git",
            "https://GitHub.com/fixture/repository",
            "ssh://git@github.com/fixture/repository.git",
            "git@github.com:fixture/repository.git",
        ] {
            assert_eq!(
                select(remote).unwrap(),
                RemoteSelection::Supported {
                    provider: PublicationProvider::GitHub,
                    owner: "fixture".into(),
                    name: "repository".into(),
                },
                "{remote}"
            );
        }
        assert_eq!(PublicationProvider::GitHub.name(), "GitHub");
        // GitHub Enterprise Server hosts are not github.com (FOR-50).
        assert_eq!(
            select("git@github.example.com:fixture/repository.git").unwrap(),
            RemoteSelection::Unsupported {
                host: "github.example.com".into()
            }
        );
        assert!(select("https://github.com/fixture/group/repository").is_err());
    }

    #[test]
    fn other_hosts_are_unsupported_not_silently_bitbucket() {
        for (remote, host) in [
            (
                "https://gitlab.example/fixture/repository.git",
                "gitlab.example",
            ),
            (
                "git@gitlab.example:fixture/repository.git",
                "gitlab.example",
            ),
            (
                "ssh://git@gitlab.example/group/sub/repository",
                "gitlab.example",
            ),
        ] {
            assert_eq!(
                select(remote).unwrap(),
                RemoteSelection::Unsupported { host: host.into() }
            );
        }
        let unavailable = UnavailablePublication::unsupported("/repo".into(), "gitlab.example");
        assert!(unavailable.reason.contains("gitlab.example"));
        assert!(unavailable.reason.contains("unavailable"));
    }

    #[test]
    fn malformed_or_credentialed_remotes_are_invalid() {
        for remote in [
            "",
            "https://user:synthetic-secret@bitbucket.org/fixture/repository.git",
            "https://user@bitbucket.org/fixture/repository.git",
            "https://bitbucket.org/fixture/repository.git?token=synthetic",
            "https://bitbucket.org/fixture/repository#x",
            "http://bitbucket.org/fixture/repository",
            "file:///tmp/repository",
            "/tmp/repository",
            "./repository",
            "https://bitbucket.org/fixture",
            "https://bitbucket.org/fixture/group/repository",
            "https://bitbucket.org/fixture/../repository",
            "https://127.0.0.1/fixture/repository",
            "bitbucket.org:fixture/repository",
            "git@bitbucket.org:/fixture/repository",
            "https://bitbucket.org/fixture/re pository",
        ] {
            assert!(select(remote).is_err(), "{remote}");
        }
    }
}
