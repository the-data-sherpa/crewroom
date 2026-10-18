use crewroom_core::Result;
use crewroom_runtime::{
    observation::ObservationStream,
    pane::{PaneFormatter, summarize_arguments},
};
use serde_json::{Value, json};
fn main() -> Result<()> {
    let cases: Vec<Value> = serde_json::from_reader(std::io::stdin())?;
    let mut stream = ObservationStream::default();
    let mut pane = PaneFormatter::default();
    let results: Vec<_> = cases
        .iter()
        .map(|v| match v["op"].as_str().unwrap() {
            "observe" => stream
                .accept(&v["event"], v["now"].as_i64().unwrap())
                .unwrap_or(Value::Null),
            "compact" => crewroom_core::compact_observation(&v["event"]),
            "frame" => json!(pane.frame(&v["event"], "12:34:56")),
            "output" => json!(pane.output(v["chunk"].as_str().unwrap())),
            "summary" => json!(summarize_arguments(&v["value"], 90)),
            _ => panic!("unknown operation"),
        })
        .collect();
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
