//! Development-only source-date oracle. No store, processes or provider access.
use std::io::Read;
fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let values: Vec<String> = serde_json::from_str(&input).unwrap();
    let parsed: Vec<_> = values
        .iter()
        .map(|v| crewroom_core::source_time::parse(v))
        .collect();
    println!("{}", serde_json::to_string(&parsed).unwrap());
}
