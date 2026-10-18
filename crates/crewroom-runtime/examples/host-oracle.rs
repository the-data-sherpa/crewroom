//! Test-only JSON oracle for retained host contracts.
use crewroom_runtime::{
    host_journal::parse_saved_inputs,
    host_protocol::{ToolCatalog, ToolRoute, backend_routes, host_tools, redact},
};
use serde_json::{Value, json};
use std::io::Read;
fn main() {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).unwrap();
    let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
    let result: Vec<Value> = cases.iter().map(|v| match v["op"].as_str().unwrap() {
        "catalog" => {
            let config = &v["config"]; let tools = host_tools(config); let catalog = ToolCatalog::new(config);
            let routes: Vec<_> = v["names"].as_array().unwrap().iter().map(|name| catalog.as_ref().ok().and_then(|c| c.route(name.as_str().unwrap()).ok()).map(|r| match r { ToolRoute::Backend(s)=>json!({"backend":s}), ToolRoute::Command=>json!("command"), ToolRoute::Check=>json!("check"), ToolRoute::Shell=>json!("shell")}).unwrap_or(Value::Null)).collect();
            json!({"tools":tools,"backend":backend_routes(config),"valid":catalog.is_ok(),"routes":routes})
        },
        "redact" => redact(&v["value"],&serde_json::from_value::<Vec<String>>(v["secrets"].clone()).unwrap()),
        "inputs" => match parse_saved_inputs(v["text"].as_str().unwrap().as_bytes()) { Ok(inputs)=>json!({"ok":true,"inputs":inputs}), Err(_)=>json!({"ok":false}) },
        _=>panic!("Unknown oracle operation"),
    }).collect();
    println!("{}", crewroom_core::serialized::stringify(&json!(result)));
}
