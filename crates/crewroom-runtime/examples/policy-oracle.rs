//! Development-only pure policy comparisons; no runner, provider or workflow effects.
use crewroom_core::Result;
use crewroom_runtime::model_policy::*;
use serde_json::{Value, json};
#[derive(Default)]
struct Mock {
    replies: Value,
    calls: std::sync::Mutex<Vec<Value>>,
}
impl crewroom_runtime::rpc::RpcCommands for Mock {
    fn command<'a>(
        &'a self,
        kind: &'a str,
        args: Value,
    ) -> crewroom_runtime::rpc::CommandFuture<'a> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(json!({"type":kind,"args":args}));
            let v = &self.replies[kind];
            if let Some(error) = v["error"].as_str() {
                return Err(crewroom_core::Error::Conflict(error.into()));
            }
            Ok(v["value"].clone())
        })
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    let operations: Vec<Value> = serde_json::from_reader(std::io::stdin())?;
    let mut results = Vec::new();
    for v in &operations {
        if v["operation"] == "configure" {
            let mock = Mock {
                replies: v["replies"].clone(),
                ..Default::default()
            };
            let mut result = match configure_assignment(&mock, &v["plan"]).await {
                Ok(value) => json!({"value":value}),
                Err(error) => json!({"error":error.to_string()}),
            };
            result["calls"] = json!(*mock.calls.lock().unwrap());
            results.push(result);
            continue;
        }
        let run = || -> Result<Value> {
            let catalog = v["catalog"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            match v["operation"].as_str().unwrap_or("") {
                "resolve" => {
                    resolve_assignment(&v["role"], catalog, &v["policy"], &v["requirements"])
                }
                "fallback" => resolve_runtime_fallback(
                    &v["role"],
                    &v["current"],
                    catalog,
                    &v["policy"],
                    &v["requirements"],
                    &serde_json::from_value::<Vec<String>>(v["tried"].clone())?,
                )
                .map(|v| v.unwrap_or(Value::Null)),
                "identity" => verify_assignment_identity(&v["plan"], &v["input"]),
                "settings" => Ok(isolated_model_settings(&v["plan"])),
                _ => Err(crewroom_core::Error::Invalid(
                    "Unknown policy operation.".into(),
                )),
            }
        };
        results.push(match run() {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":error.to_string()}),
        });
    }
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
