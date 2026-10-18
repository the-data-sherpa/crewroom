//! Development-only serialization oracle for captured record identities.
use std::io::Read;
fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let values: Vec<serde_json::Value> = serde_json::from_str(&input).unwrap();
    let serialized: Vec<_> = values
        .iter()
        .map(crewroom_core::serialized::stringify)
        .collect();
    println!("{}", serde_json::to_string(&serialized).unwrap());
}
