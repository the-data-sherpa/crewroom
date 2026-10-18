//! Pure compatibility fixture. No upstream provider or shared configuration is used.
use base64::{Engine, engine::general_purpose::STANDARD};
use crewroom_core::Result;
use crewroom_runtime::{
    provider_input::{ReasoningBudget, has_bounded_text_input},
    provider_usage::UsageReader,
};
use serde_json::{Value, json};
fn main() -> Result<()> {
    let cases: Vec<Value> = serde_json::from_reader(std::io::stdin())?;
    let mut results = vec![];
    for v in cases {
        let result = match v["op"].as_str().unwrap() {
            "input" => {
                let mut calls = vec![];
                let result = has_bounded_text_input(
                    v["api"].as_str().unwrap(),
                    &v["body"],
                    &mut |kind, value| {
                        calls.push(json!([kind, value]));
                        v["opaque"] == true
                    },
                );
                json!({"value":result,"opaque":calls})
            }
            "usage" => {
                let mut reader = UsageReader::new(v["api"].as_str().unwrap(), v["stream"] == true);
                let mut opaque = vec![];
                let chunks = if let Some(preset) = v["preset"].as_str() {
                    let event = r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":2}}}"#;
                    let body = r#"{"object":"response","status":"completed","usage":{"input_tokens":1,"output_tokens":2}}"#;
                    match preset {
                        "json-at-limit" | "json-over-limit" => vec![
                            format!(
                                "{}{body}",
                                " ".repeat(
                                    8 * 1024 * 1024 - body.len()
                                        + usize::from(preset == "json-over-limit")
                                )
                            )
                            .into_bytes(),
                        ],
                        "line-at-limit" | "line-over-limit" => vec![
                            ":".repeat(1024 * 1024 + usize::from(preset == "line-over-limit"))
                                .into_bytes(),
                            format!("\ndata: {event}\n\n").into_bytes(),
                        ],
                        _ => panic!("unknown usage preset"),
                    }
                } else {
                    v["chunks"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|c| STANDARD.decode(c.as_str().unwrap()).unwrap())
                        .collect()
                };
                for chunk in chunks {
                    reader.push(&chunk);
                    opaque.extend(reader.take_opaque());
                }
                let mut receipts = vec![];
                for _ in 0..v["finishes"].as_u64().unwrap_or(1) {
                    receipts.push(reader.finish(
                        v["status"].as_u64().unwrap() as u16,
                        v["interrupted"] == true,
                    ));
                    opaque.extend(reader.take_opaque());
                }
                json!({"receipts":receipts,"outcome":reader.outcome(),"opaque":opaque})
            }
            "budget" => {
                let root = tempfile::tempdir()?;
                if let Some(saved) = v.get("saved") {
                    std::fs::write(root.path().join("reasoning-budget.json"), saved.to_string())?;
                }
                let run = || -> Result<Value> {
                    let selector = v["selector"].as_str().unwrap();
                    let mut budget = ReasoningBudget::open(root.path(), selector)?;
                    let mut receipts = vec![];
                    for operation in v["operations"].as_array().unwrap() {
                        if operation["type"] == "reopen" {
                            budget = ReasoningBudget::open(root.path(), selector)?;
                        } else if operation["type"] == "remember" {
                            let blocks = operation["blocks"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|b| {
                                    (
                                        b["kind"].as_str().unwrap().into(),
                                        b["value"].as_str().unwrap().into(),
                                    )
                                })
                                .collect::<Vec<_>>();
                            budget.remember(&blocks, operation["output"].as_f64().unwrap())?;
                        } else {
                            receipts.push(json!(budget.tokens(
                                operation["kind"].as_str().unwrap(),
                                operation["value"].as_str().unwrap()
                            )));
                        }
                    }
                    let saved = std::fs::read(root.path().join("reasoning-budget.json"))
                        .ok()
                        .map(|b| serde_json::from_slice::<Value>(&b).unwrap());
                    Ok(json!({"receipts":receipts,"saved":saved}))
                };
                match run() {
                    Ok(v) => v,
                    Err(e) => json!({"error":e.to_string()}),
                }
            }
            _ => panic!("unknown operation"),
        };
        results.push(result);
    }
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
