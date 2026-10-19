//! Native login stays in the trusted host. Assignment processes receive only
//! ephemeral access tokens; all Crewroom refreshes share one filesystem lock.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::{Duration, sleep, timeout},
};

pub struct Credentials {
    pub token: String,
    pub account: String,
}
impl Credentials {
    pub fn reply(&self) -> Value {
        json!({"accessToken":self.token,"chatgptAccountId":self.account})
    }
    pub fn fingerprint(&self) -> String {
        super::profile::digest(self.token.as_bytes())
    }
}
pub(super) fn read_credentials(path: &Path) -> Result<Credentials> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| conflict("Existing Codex file login is unavailable."))?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.len() > 1024 * 1024
    {
        return Err(conflict("Codex login must be a private owned file."));
    }
    let value: Value =
        serde_json::from_reader(file).map_err(|_| conflict("Codex login is invalid."))?;
    if value["auth_mode"] != "chatgpt" {
        return Err(conflict(
            "Native Codex currently requires an existing ChatGPT file login.",
        ));
    }
    Ok(Credentials {
        token: text(&value["tokens"], "access_token")?.into(),
        account: text(&value["tokens"], "account_id")?.into(),
    })
}
fn fresh(token: &str) -> bool {
    token
        .split('.')
        .nth(1)
        .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok())
        .and_then(|v| v["exp"].as_u64())
        .is_some_and(|exp| {
            exp > SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + 120
        })
}
/// The filesystem lock every Crewroom reader or refresher of this Codex login
/// holds. Stable across separate runtime roots that use the same login.
pub fn lock_path(source: &Path) -> PathBuf {
    source.with_file_name(format!(
        ".crewroom-{}.lock",
        super::profile::digest(source.as_os_str().as_encoded_bytes())
    ))
}
pub async fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
    {
        return Err(conflict("Credential/session lock is not private."));
    }
    timeout(Duration::from_secs(45), async {
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(error);
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| conflict("Another Codex credential/session owner is still active."))??;
    let current = std::fs::symlink_metadata(path)?;
    if (current.dev(), current.ino()) != (meta.dev(), meta.ino()) {
        return Err(conflict("Credential/session lock changed identity."));
    }
    Ok(file)
}
pub async fn credentials(config: &Value, failed: Option<(&str, &str)>) -> Result<Credentials> {
    let source = Path::new(text(&config["codex"], "authFile")?);
    let owner = Path::new(text(&config["codex"], "authRoot")?);
    private_dir(owner)?;
    canonical(source)?;
    let lease = lock(&lock_path(source)).await?;
    let before = read_credentials(source)?;
    if let Some((account, _)) = failed
        && before.account != account
    {
        return Err(conflict(
            "Codex account changed; explicitly re-admit the assignment.",
        ));
    }
    if fresh(&before.token)
        && failed.is_none_or(|(_, fingerprint)| before.fingerprint() != fingerprint)
    {
        return Ok(before);
    }
    let home = owner.join(uuid::Uuid::new_v4().to_string());
    private_dir(&home)?;
    // The sole managed helper owns refresh. Never link native auth into an agent home.
    std::os::unix::fs::symlink(source, home.join("auth.json"))?;
    durable_new(
        &home.join("config.toml"),
        b"cli_auth_credentials_store = \"file\"\nproject_doc_max_bytes = 0\n",
    )?;
    let binary = text(&config["codex"], "binary")?;
    let mut command = Command::new(binary);
    command
        .args(["app-server", "--listen", "stdio://", "--strict-config"])
        .env_clear()
        .envs(super::profile::environment(&home, &home))
        .current_dir(&home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // The child retains the same lock even if the host dies before group cleanup.
    crate::helper::inherit_lease(command.as_std_mut(), &lease);
    let mut child = command.spawn()?;
    // The host's checked process group also owns this managed refresh child.
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let refreshed: Result<()> = timeout(Duration::from_secs(30),async {
        async fn call(input:&mut tokio::process::ChildStdin, output:&mut BufReader<tokio::process::ChildStdout>, id:u64, method:&str, params:Value)->Result<()> {
            input.write_all(format!("{}\n",stringify(&json!({"id":id,"method":method,"params":params}))).as_bytes()).await?;
            let mut total=0;
            loop {
                let mut line=Vec::new();
                loop {
                    let part=output.fill_buf().await?;
                    if part.is_empty(){return Err(conflict("Codex native refresh stream ended."));}
                    let n=part.iter().position(|b|*b==b'\n').map_or(part.len(),|i|i+1);
                    total+=n;
                    if total>1024*1024{return Err(conflict("Codex refresh exceeded its response bound."));}
                    let end=part[n-1]==b'\n';line.extend_from_slice(&part[..n]);output.consume(n);if end{break;}
                }
                let frame:Value=serde_json::from_slice(&line).map_err(|_|conflict("Invalid Codex refresh response."))?;
                if frame["id"]==id {
                    if frame.get("error").is_some() || frame.get("result").is_none(){return Err(conflict("Existing Codex login could not refresh. Reauthenticate the harness before resuming."));}
                    return Ok(());
                }
                if frame.get("id").is_some(){return Err(conflict("Unexpected request from Codex refresh owner."));}
            }
        }
        call(&mut input,&mut output,1,"initialize",json!({"clientInfo":{"name":"crewroom_auth","version":"0.1.0"}})).await?;
        input.write_all(b"{\"method\":\"initialized\"}\n").await?;
        call(&mut input,&mut output,2,"account/read",json!({"refreshToken":true})).await
    }).await.unwrap_or_else(|_|Err(conflict("Codex native refresh timed out.")));
    let _ = child.kill().await;
    let _ = child.wait().await;
    // Keep no credential copy. The managed helper may have updated the canonical login.
    let _ = std::fs::remove_file(home.join("auth.json"));
    refreshed?;
    let after = read_credentials(source)?;
    if after.account != before.account
        || !fresh(&after.token)
        || after.fingerprint() == before.fingerprint()
    {
        return Err(conflict(
            "Codex refresh did not establish fresh credentials for the same account.",
        ));
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn token(exp: u64) -> String {
        format!(
            "fixture.{}.signature",
            URL_SAFE_NO_PAD.encode(stringify(&json!({"exp":exp})))
        )
    }
    #[tokio::test]
    async fn concurrent_hosts_share_one_refresh_and_never_write_auth_into_agent_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("login.json");
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let old = token(1);
        let new = token(future);
        durable_new(&source,stringify(&json!({"auth_mode":"chatgpt","tokens":{"access_token":old,"refresh_token":"FAKE_REFRESH","account_id":"fixture"}})).as_bytes()).unwrap();
        let executable = root.join("codex");
        let script = format!(
            r#"#!/usr/bin/python3
import json,sys,os,time
from pathlib import Path
for line in sys.stdin:
 f=json.loads(line)
 if 'id' not in f:continue
 if f['method']=='account/read':
  time.sleep(.1)
  path=Path(os.environ['CODEX_HOME'])/'auth.json'
  value=json.loads(path.read_text());value['tokens']['access_token']={new:?};path.write_text(json.dumps(value))
  with open({counter:?},'a') as out:out.write('refresh\n')
 print(json.dumps({{'id':f['id'],'result':{{}}}}),flush=True)
"#,
            counter = root.join("refreshes").to_string_lossy()
        );
        crewroom_test_support::install_executable(&executable, script);
        let config =
            json!({"codex":{"binary":executable,"authFile":source,"authRoot":root.join("owner")}});
        let mut other = config.clone();
        other["codex"]["authRoot"] = json!(root.join("other-runtime"));
        let (a, b) = tokio::join!(credentials(&config, None), credentials(&other, None));
        let a = a.unwrap();
        let b = b.unwrap();
        assert_eq!(a.token, new);
        assert_eq!(b.token, new);
        assert_eq!(
            std::fs::read_to_string(root.join("refreshes")).unwrap(),
            "refresh\n"
        );
        let reused = credentials(
            &config,
            Some(("fixture", &super::super::profile::digest(old.as_bytes()))),
        )
        .await
        .unwrap();
        assert_eq!(reused.token, new);
        assert!(
            credentials(&config, Some(("different", &reused.fingerprint())))
                .await
                .is_err()
        );
        for entry in std::fs::read_dir(root.join("owner")).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert!(!path.join("auth.json").exists());
            }
        }
    }
    #[tokio::test]
    #[ignore = "subprocess entry point for the refresh parent-death regression"]
    async fn refresh_parent_worker() {
        let Ok(path) = std::env::var("CREWROOM_REFRESH_TEST_CONFIG") else {
            return;
        };
        let config: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let _ = credentials(&config, None).await;
    }
    #[tokio::test]
    async fn refresh_child_retains_source_lease_after_parent_death() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("login.json");
        durable_new(&source, stringify(&json!({"auth_mode":"chatgpt","tokens":{"access_token":token(1),"account_id":"fixture"}})).as_bytes()).unwrap();
        let executable = root.join("codex");
        let ready = root.join("ready");
        // Publish the PID by rename: the test polls for the path, and a plain
        // open('w') makes it visible before the PID is written.
        crewroom_test_support::install_executable(
            &executable,
            format!(
                "#!/usr/bin/python3\nimport os,time\nopen({0:?}+'.tmp','w').write(str(os.getpid()))\nos.rename({0:?}+'.tmp',{0:?})\ntime.sleep(60)\n",
                ready.to_str().unwrap()
            ),
        );
        let config = root.join("config.json");
        durable_new(&config, stringify(&json!({"codex":{"binary":executable,"authFile":source,"authRoot":root.join("first-runtime")}})).as_bytes()).unwrap();
        let mut parent = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runner::auth::tests::refresh_parent_worker",
                "--ignored",
            ])
            .env("CREWROOM_REFRESH_TEST_CONFIG", &config)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        timeout(Duration::from_secs(10), async {
            while !ready.exists() {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid: i32 = std::fs::read_to_string(&ready).unwrap().parse().unwrap();
        // Always stop the fake child, including after a failed assertion.
        struct Stop(i32);
        impl Drop for Stop {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                }
            }
        }
        let _stop = Stop(pid);
        parent.kill().await.unwrap();
        parent.wait().await.unwrap();
        let lock_path = source.with_file_name(format!(
            ".crewroom-{}.lock",
            super::super::profile::digest(source.as_os_str().as_encoded_bytes())
        ));
        let other = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EWOULDBLOCK)
        );
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        timeout(Duration::from_secs(5), async {
            loop {
                if unsafe { libc::flock(other.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn credentials_reject_public_files_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("auth.json");
        durable_new(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_credentials(&path).is_err());
        let link = root.path().join("alias");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_credentials(&link).is_err());
    }
}
