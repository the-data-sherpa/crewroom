//! Independently observable native host; trusted controller supplies private config.
use std::path::Path;
#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 1 {
        eprintln!("Usage: crewroom-host PRIVATE_CONFIG.json");
        std::process::exit(2);
    }
    if crewroom_runtime::host::run(Path::new(&args[0]))
        .await
        .is_err()
    {
        // Details are retained in private journals where possible. Never print a
        // configuration/transport error that might contain a provider credential.
        eprintln!(
            "Native host stopped with an unresolved setup or execution error; inspect its private records."
        );
        std::process::exit(1);
    }
}
