//! Pure retained-policy comparison. These inputs are explicitly simulated.
fn main() -> crewroom_core::Result<()> {
    let cases: Vec<serde_json::Value> = serde_json::from_reader(std::io::stdin())?;
    let results: Vec<_> = cases
        .iter()
        .map(|v| {
            crewroom_runtime::publication::assess(
                &v["record"],
                v.get("local"),
                // `true` is the retired service's Jira-only conflict; a string names the source.
                v["sourceConflict"]
                    .as_str()
                    .or((v["sourceConflict"] == true).then_some("Jira")),
            )
        })
        .collect();
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
