//! Development-only workflow input oracle. No processes, Mail or provider calls.
use crewroom_core::{Result, parse_team, snapshot_briefs};
use crewroom_runtime::supervisor::workflow::{
    configuration::{HostAssignment, RuntimeConfig},
    context::*,
    preparation::prepare_assignments,
};
use serde_json::{Value, json};
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn optional<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get(k).filter(|v| !v.is_null())
}
fn execute(v: &Value) -> Result<Value> {
    match v["op"].as_str().unwrap_or("") {
        "team" => parse_team(&v["input"]),
        "briefs" => snapshot_briefs(
            array(&v["team"]),
            &v["criteria"],
            array(&v["checks"]),
            array(&v["existing"]),
        ),
        "present" => Ok(json!(present_briefs(&v["own"], array(&v["team"]))?)),
        "coding" => Ok(coding_context(optional(v, "coding"))?.unwrap_or(Value::Null)),
        "context" => continuation_context(ContinuationContext {
            coding: optional(v, "coding"),
            work: optional(v, "work"),
            publication: optional(v, "publication"),
            steering: array(&v["steering"]),
            messages: array(&v["messages"]),
            notices: array(&v["notices"]),
        }),
        "session" => Ok(json!(session_needs_replacement(
            array(&v["events"]),
            v["attempt"].as_str().unwrap_or("")
        ))),
        "history" => Ok(json!(input_history(
            array(&v["inputs"]),
            v["attempt"].as_str().unwrap_or("")
        ))),
        "input" => host_input(
            v["id"].as_str().unwrap_or(""),
            &v["message"],
            v["mode"].as_str(),
        ),
        op => {
            let config = RuntimeConfig::parse(&v["config"])?;
            match op {
                "config" => Ok(
                    json!({"root":config.root(),"socket":config.socket(),"ports":config.host_ports()}),
                ),
                "prepare" => Ok(json!(prepare_assignments(
                    &config,
                    &v["run"],
                    optional(v, "work"),
                    array(&v["checks"]),
                    &v["input"],
                    array(&v["roles"])
                )?)),
                "refinement" => config.refinement_team(&v["run"], array(&v["roles"])),
                "host" => config.host_configuration(HostAssignment {
                    run: &v["run"],
                    assignment: &v["assignment"],
                    attempt: &v["attempt"],
                    team: array(&v["team"]),
                    work: optional(v, "work"),
                    previous: optional(v, "previous"),
                    resume_session: v["resumeSession"] == true,
                    port: v["port"].as_u64().unwrap_or(0) as u16,
                    provider_token: v["providerToken"].as_str().unwrap_or(""),
                    environment: &v["environment"],
                    publication: v["publication"] == true,
                    jira: v["jira"] == true,
                }),
                _ => Err(crewroom_core::Error::Invalid(
                    "Unknown workflow input operation.".into(),
                )),
            }
        }
    }
}
fn main() -> Result<()> {
    let operations: Vec<Value> = serde_json::from_reader(std::io::stdin())?;
    let replies: Vec<_> = operations
        .iter()
        .map(|v| match execute(v) {
            Ok(value) => json!({"value":value}),
            Err(e) => json!({"error":e.to_string()}),
        })
        .collect();
    println!("{}", serde_json::to_string(&replies)?);
    Ok(())
}
