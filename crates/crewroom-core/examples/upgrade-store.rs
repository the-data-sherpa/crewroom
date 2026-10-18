//! Headless historical-upgrade harness; the supplied clock is fixture-only.
use crewroom_core::store::Store;
fn main() {
    let path = std::env::args().nth(1).expect("database path");
    let clock = std::env::args()
        .nth(2)
        .expect("fixture clock")
        .parse::<i64>()
        .expect("milliseconds");
    match Store::open_with_clock(std::path::Path::new(&path), move || clock) {
        Ok(_) => println!("{}", serde_json::json!({"ok":true})),
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"ok":false,"error":error.to_string()})
            );
            std::process::exit(1);
        }
    }
}
