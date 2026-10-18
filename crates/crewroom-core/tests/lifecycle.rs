use crewroom_core::lifecycle::{Transition, run_label, transition};
use serde_json::{Value, json};
use std::io::Read;

#[test]
fn accepted_typescript_lifecycle_corpus_matches() {
    let mut source = String::new();
    flate2::read::GzDecoder::new(&include_bytes!("lifecycle-corpus.json.gz")[..])
        .read_to_string(&mut source)
        .unwrap();
    let cases: Vec<Value> = serde_json::from_str(&source).unwrap();
    assert_eq!(cases.len(), 4536);
    for (index, case) in cases.iter().enumerate() {
        let action: Transition = serde_json::from_value(case["action"].clone()).unwrap();
        let result = transition(&case["previous"], &action, case["now"].as_i64().unwrap());
        if let Some(error) = case["error"].as_str() {
            assert_eq!(result.unwrap_err().to_string(), error, "case {index}");
        } else {
            assert_eq!(result.unwrap(), case["expected"], "case {index}");
        }
    }
}

#[test]
fn waiting_and_cancellation_remain_distinct_from_execution() {
    assert_eq!(
        run_label(&json!({"status":"awaiting_runner","position":0})),
        "Queued"
    );
    assert_eq!(
        run_label(&json!({"status":"pausing","desiredState":"cancelled"})),
        "Cancelling · confirming stop"
    );
    assert_eq!(
        run_label(&json!({"status":"attention_required","desiredState":"cancelled"})),
        "Cancellation needs attention"
    );
}
