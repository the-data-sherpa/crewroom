use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut data = PathBuf::from(
        std::env::var("CREWROOM_NATIVE_DATA_DIR").unwrap_or_else(|_| ".crewroom/rust".into()),
    );
    let mut port = 4318u16;
    let mut status = false;
    let mut background = false;
    let mut roles = std::env::var_os("CREWROOM_ROLE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".config/crewroom/roles.json")
        });
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                data = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--data-dir needs a path"))?
                    .into()
            }
            "--roles" => {
                roles = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--roles needs a path"))?
                    .into()
            }
            "--port" => {
                port = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--port needs a port"))?
                    .parse()?
            }
            "--status" => status = true,
            "--background" => background = true,
            "--help" => {
                println!(
                    "crewroomd [--data-dir PATH] [--roles PATH] [--port PORT] [--background | --status]\nDefault data: .crewroom/rust; port: 4318. Closing the native UI leaves this daemon running."
                );
                return Ok(());
            }
            _ => anyhow::bail!("Unknown option: {arg}"),
        }
    }
    anyhow::ensure!(!(background && status), "Choose --background or --status.");
    if background {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        std::fs::create_dir_all(&data)?;
        let metadata = std::fs::metadata(&data)?;
        // SAFETY: geteuid has no preconditions.
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "Workspace belongs to another user."
        );
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700))?;
        let log = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(data.join("service.log"))?;
        log.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        // Fork before creating Tokio's worker threads. A double fork and new
        // session let the service outlive the launching terminal's process group.
        daemonize::Daemonize::new()
            .working_directory(std::env::current_dir()?)
            .umask(0o077)
            .stderr(log.try_clone()?)
            .stdout(log)
            .start()?;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(data, roles, port, status))
}

async fn run(data: PathBuf, roles: PathBuf, port: u16, status: bool) -> anyhow::Result<()> {
    if status {
        use std::os::unix::fs::MetadataExt;
        let path = data.join("native-connection.json");
        let metadata = std::fs::symlink_metadata(&path)?;
        // SAFETY: geteuid has no preconditions.
        anyhow::ensure!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.mode() & 0o077 == 0
                && metadata.uid() == unsafe { libc::geteuid() },
            "Invalid native connection file."
        );
        let info: crewroom_service::ConnectionInfo = serde_json::from_slice(&std::fs::read(path)?)?;
        let origin = reqwest::Url::parse(&info.origin)?;
        anyhow::ensure!(
            origin.scheme() == "http"
                && origin.host_str() == Some("127.0.0.1")
                && origin.port().is_some()
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.path() == "/"
                && origin.query().is_none()
                && origin.fragment().is_none(),
            "Invalid local service origin."
        );
        let response = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(2))
            .build()?
            .get(format!("{}/api/health", info.origin))
            .bearer_auth(info.token)
            .send()
            .await?
            .error_for_status()?;
        let health: serde_json::Value = response.json().await?;
        anyhow::ensure!(
            health["service"] == "crewroom-rust" && health["protocol"] == 1,
            "Unexpected service."
        );
        println!(
            "Rust workflow service connected at {} (PID {}).",
            info.origin, info.pid
        );
        return Ok(());
    }
    // Install both handlers before startup can publish a connection or admit work.
    // Signals received during initialization remain pending until owned cleanup runs.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let options = crewroom_service::ServiceOptions::from_environment(&data)?;
    let server =
        crewroom_service::Service::start_with_options(&data, &roles, port, options).await?;
    println!("Crewroom Rust service listening on {}", server.info.origin);
    tokio::select! { _ = interrupt.recv() => {}, _ = terminate.recv() => {} }
    server.close().await
}
