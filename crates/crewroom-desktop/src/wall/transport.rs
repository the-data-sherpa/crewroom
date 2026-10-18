use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

#[derive(Deserialize)]
struct Connection {
    protocol: u32,
    origin: String,
    token: String,
}
pub(super) fn get(descriptor: &Path, path: &str) -> anyhow::Result<Value> {
    let stat = std::fs::symlink_metadata(descriptor)
        .context("Native service connection is unavailable.")?;
    if !stat.is_file()
        || stat.file_type().is_symlink()
        || stat.uid() != unsafe { libc::geteuid() }
        || stat.mode() & 0o077 != 0
    {
        bail!("Native connection file is not private and owned.");
    }
    let c: Connection = serde_json::from_slice(&std::fs::read(descriptor)?)?;
    let u = url::Url::parse(&c.origin)?;
    if c.protocol != 1
        || u.scheme() != "http"
        || u.host_str() != Some("127.0.0.1")
        || u.port().is_none()
        || u.path() != "/"
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || c.token.len() != 64
        || !c.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("Invalid native connection.");
    }
    let response = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?
        .get(format!("{}/api{path}", c.origin))
        .bearer_auth(c.token)
        .send()
        .context("Cannot reach the native observation service.")?;
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        bail!("Observation exceeds the bounded response limit.");
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    if !status.is_success() {
        bail!(
            "{}",
            v["error"].as_str().unwrap_or("Observation unavailable.")
        );
    }
    Ok(v)
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Attachment {
    pub version: u8,
    pub run_id: String,
    pub attempt_id: String,
    pub pane: String,
    pub window: String,
    pub session: String,
    pub tmux: PathBuf,
    pub socket: PathBuf,
    pub directory: PathBuf,
    pub read_only: bool,
    pub socket_device: u64,
    pub socket_inode: u64,
    pub host_pid: u32,
}
impl Attachment {
    pub fn validate(&self, run: &str, attempt: &str) -> anyhow::Result<()> {
        let numbered = |s: &str, prefix: char| {
            s.strip_prefix(prefix)
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        };
        if self.version != 1
            || !self.read_only
            || self.run_id != run
            || self.attempt_id != attempt
            || self.session != "crewroom"
            || !numbered(&self.pane, '%')
            || !numbered(&self.window, '@')
            || self.host_pid < 2
        {
            bail!("Terminal identity is invalid.");
        }
        for path in [&self.tmux, &self.directory] {
            if !path.is_absolute() || std::fs::canonicalize(path)? != *path {
                bail!("Terminal path changed identity.");
            }
        }
        if self.socket.parent() != Some(self.directory.as_path()) {
            bail!("Terminal socket left its private directory.");
        }
        let directory = std::fs::metadata(&self.directory)?;
        let socket = std::fs::symlink_metadata(&self.socket)?;
        if directory.uid() != unsafe { libc::geteuid() }
            || directory.mode() & 0o077 != 0
            || !socket.file_type().is_socket()
            || socket.uid() != directory.uid()
            || socket.dev() != self.socket_device
            || socket.ino() != self.socket_inode
        {
            bail!("Terminal socket ownership changed.");
        }
        Ok(())
    }
    fn arguments(&self, viewer: &str) -> Vec<String> {
        vec![
            "-S".into(),
            self.socket.to_string_lossy().into_owned(),
            "-f".into(),
            "/dev/null".into(),
            "new-session".into(),
            "-E".into(),
            "-s".into(),
            viewer.into(),
            "-t".into(),
            self.session.clone(),
            "-f".into(),
            "read-only,ignore-size".into(),
            ";".into(),
            "set-option".into(),
            "-t".into(),
            viewer.into(),
            "destroy-unattached".into(),
            "on".into(),
            ";".into(),
            "set-option".into(),
            "-t".into(),
            viewer.into(),
            "status".into(),
            "off".into(),
            ";".into(),
            "select-window".into(),
            "-t".into(),
            format!("{viewer}:{}", self.window),
            ";".into(),
            "refresh-client".into(),
            "-L".into(),
            "10000".into(),
        ]
    }
}
/// Ghostty starts this mode instead of receiving credentials or arbitrary shell commands.
/// An attached grouped session has independent window selection; it disappears on detach.
pub(crate) fn observe(descriptor: &Path, run: &str, attempt: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    uuid::Uuid::parse_str(run)?;
    uuid::Uuid::parse_str(attempt)?;
    let value = get(
        descriptor,
        &format!("/runs/{run}/attempts/{attempt}/terminal"),
    )?;
    let a: Attachment = serde_json::from_value(value)?;
    a.validate(run, attempt)?;
    let viewer = format!("crewroom-view-{}", uuid::Uuid::new_v4());
    let error = Command::new(&a.tmux)
        .args(a.arguments(&viewer))
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .current_dir(&a.directory)
        .exec();
    Err(error.into())
}
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observer_command_is_readonly_separate_and_self_cleaning() {
        let a = Attachment {
            version: 1,
            run_id: "r".into(),
            attempt_id: "a".into(),
            pane: "%7".into(),
            window: "@4".into(),
            session: "crewroom".into(),
            tmux: "/bin/tmux".into(),
            socket: "/private/t".into(),
            directory: "/private".into(),
            read_only: true,
            socket_device: 1,
            socket_inode: 2,
            host_pid: 5,
        };
        let args = a.arguments("crewroom-view-test");
        assert!(args.iter().any(|s| s == "read-only,ignore-size"));
        assert!(args.iter().any(|s| s == "destroy-unattached"));
        assert!(args.iter().any(|s| s == "crewroom-view-test:@4"));
        assert_eq!(&args[args.len() - 3..], ["refresh-client", "-L", "10000"]);
        assert!(
            !args
                .iter()
                .any(|s| ["send-keys", "resize-pane", "kill-server", "-d"].contains(&s.as_str()))
        );
        assert_eq!(quote("it's $(private)"), "'it'\\''s $(private)'");
    }
}
