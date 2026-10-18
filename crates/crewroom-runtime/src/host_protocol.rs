//! Host wire data. Tool availability is checked here; repository/evidence authority
//! remains in the controller. The embedded catalog has no TypeScript runtime dependency.
use crewroom_core::Result;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

pub const HOST_INPUT_LIMIT: usize = 150_000;
fn catalog() -> &'static Value {
    static CATALOG: OnceLock<Value> = OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(include_str!("host-tools.json")).unwrap())
}
fn yes(config: &Value, key: &str) -> bool {
    config[key] == true
}
fn group(name: &str) -> Vec<Value> {
    let catalog = catalog();
    catalog["groups"][name]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| catalog["definitions"][key.as_str().unwrap()].clone())
        .collect()
}
/// The same allowlist supplies both the advertised schema and backend authorization.
/// Evaluation never inherits coding completion, catalog writes, shell or publication.
fn evaluation_routes(config: &Value) -> BTreeMap<String, String> {
    let mut routes = BTreeMap::from([
        ("send_message".into(), "agent-message".into()),
        ("repository_read".into(), "evaluation/read".into()),
        ("repository_search".into(), "evaluation/search".into()),
    ]);
    if yes(config, "coordinator") {
        routes.extend([
            ("steering_receipt".into(), "steering-receipt".into()),
            ("playbook_status".into(), "evaluation/playbook".into()),
            (
                "record_playbook_step".into(),
                "evaluation/playbook-step".into(),
            ),
        ]);
    } else {
        routes.extend([
            ("write_file".into(), "evaluation/write".into()),
            ("edit_file".into(), "evaluation/edit".into()),
            ("run_check".into(), "evaluation/check".into()),
            ("handoff_assignment".into(), "evaluation/handoff".into()),
        ]);
    }
    routes
}
fn evaluation_tools(config: &Value) -> Vec<Value> {
    evaluation_routes(config).keys().map(|name| {
        if name == "handoff_assignment" {
            return json!({"name":name,"label":"Hand off assignment","loadMode":"essential",
                "description":"Record your final assignment status and explanation. This preserves your claim; it does not certify correctness.",
                "parameters":{"type":"object","properties":{"status":{"type":"string","enum":["complete","incomplete"]},"summary":{"type":"string"}},"required":["status","summary"],"additionalProperties":false}});
        }
        let mut tool = catalog()["definitions"][name].clone();
        if name == "run_check" {
            tool["description"] = json!("Execute a configured check on the current source in a networkless container. The receipt identifies the exact source checked.");
        }
        if name == "send_message" {
            tool["parameters"]["properties"]["to"]["enum"] = config.get("recipients").cloned().unwrap_or(json!([]));
        }
        tool
    }).collect()
}
pub fn host_tools(config: &Value) -> Vec<Value> {
    if config.get("purpose").is_some_and(|p| {
        !matches!(
            p.as_str(),
            Some("coding" | "research" | "explore" | "grill" | "evaluation")
        )
    }) {
        return Vec::new();
    }
    if config["purpose"] == "evaluation" {
        return evaluation_tools(config);
    }
    let c = yes(config, "coordinator");
    let research = yes(config, "research");
    let mut tools = group("common");
    // Staged policy keeps the assignment's native tool schema stable across
    // team rotation. The service validates the current recipient at dispatch;
    // an enum snapshot is a prompt hint, never message authority.
    if !yes(config, "choreography") {
        tools[0]["parameters"]["properties"]["to"]["enum"] =
            config.get("recipients").cloned().unwrap_or(json!([]));
    }
    if c && !yes(config, "refinement") {
        tools.extend(group("team"));
        tools.extend(group("playbook"));
        if !research {
            tools.extend(group("learning"));
        }
    }
    if yes(config, "intake") && c {
        tools.extend(group("intake"));
    } else if yes(config, "refinement") {
        tools.extend(group("refinement"));
    }
    if yes(config, "publication") {
        tools.extend(group(&format!("publication-{}", u8::from(c))));
    }
    if yes(config, "jira") {
        let fallback = yes(config, "researchCoordinatorReporter");
        tools.extend(group(&format!(
            "jira-{}-{}-{}",
            u8::from(yes(config, "reporter") || fallback),
            u8::from(research),
            u8::from(fallback)
        )));
    }
    if yes(config, "github") {
        let fallback = yes(config, "researchCoordinatorReporter");
        tools.extend(group(&format!(
            "github-{}-{}-{}",
            u8::from(yes(config, "reporter") || fallback),
            u8::from(research),
            u8::from(fallback)
        )));
    }
    if yes(config, "coding") {
        tools.extend(group(&format!(
            "coding-{}-{}",
            u8::from(c),
            u8::from(
                config
                    .get("commandEngine")
                    .is_some_and(|v| !v.is_null() && v != false)
            )
        )));
    }
    if yes(config, "choreography") && yes(config, "coding") {
        tools.extend(group(&format!("choreography-{}", u8::from(c))));
    }
    if research {
        tools.extend(group(&format!(
            "research-{}-{}",
            u8::from(c),
            u8::from(yes(config, "researchRepository"))
        )));
    }
    if let Some(commands) = config["tools"].as_array().filter(|a| !a.is_empty()) {
        let mut command = group("command");
        command[0]["parameters"]["properties"]["commandId"]["enum"] =
            json!(commands.iter().map(|c| &c["id"]).collect::<Vec<_>>());
        tools.extend(command);
    }
    tools
}

/// Backend authorization uses the same purpose/actor policy as advertised tools.
/// Internal coding check routes are deliberately unavailable to evaluation workers.
pub fn permits_backend_route(config: &Value, route: &str) -> bool {
    let registered = host_tools(config);
    let routes = backend_routes(config);
    registered.iter().any(|tool| {
        tool["name"]
            .as_str()
            .and_then(|name| routes.get(name))
            .is_some_and(|r| r == route)
    }) || config["purpose"].as_str().unwrap_or("coding") == "coding"
        && (["coding/check-prepare", "coding/check-finish"].contains(&route)
            && registered.iter().any(|t| t["name"] == "run_check")
            || route == "coding/shell" && registered.iter().any(|t| t["name"] == "run_shell"))
}

/// Mirrors the retained routing table. Dispatch additionally requires registration,
/// because the table alone contains endpoints for tools absent from some roles.
pub fn backend_routes(config: &Value) -> BTreeMap<String, String> {
    if config["purpose"] == "evaluation" {
        return evaluation_routes(config);
    }
    let mut routes = BTreeMap::new();
    let mut add = |name: &str, route: &str| {
        routes.insert(name.into(), route.into());
    };
    let c = yes(config, "coordinator");
    if yes(config, "publication") {
        add("publication_status", "publication/status");
        if c {
            add("publish_change", "publication/prepare");
        }
    }
    if yes(config, "jira") {
        add("jira_updates", "jira/updates");
        if yes(config, "reporter") {
            add("jira_report", "jira/report");
        }
    }
    if yes(config, "github") {
        add("github_updates", "github/updates");
        if yes(config, "reporter") {
            add("github_report", "github/report");
        }
    }
    add("send_message", "agent-message");
    add("steering_receipt", "steering-receipt");
    if c && !yes(config, "refinement") {
        add("playbook_status", "playbook/status");
        add("propose_playbook", "playbook/propose");
        add("record_playbook_step", "playbook/step");
    }
    if yes(config, "intake") && c {
        add("repository_read", "intake/read");
        add("repository_search", "intake/search");
        add("ask_intake_questions", "intake/questions");
        add("propose_implementation_plan", "intake/plan");
    } else if yes(config, "refinement") {
        add("propose_refinement", "refinement-proposal");
    } else {
        add("list_roles", "agent-roles");
        add("propose_team", "team-proposal");
    }
    if yes(config, "coding") {
        for (name, path) in [
            ("choreography_status", "choreography"),
            ("propose_choreography", "choreography-propose"),
            ("record_package", "package"),
            ("pair_exchange", "pair-exchange"),
            ("checkpoint_implementation", "checkpoint"),
            ("coding_status", "status"),
            ("repository_read", "read"),
            ("claim_implementation", "claim"),
            ("write_file", "edit"),
            ("finish_implementation", "complete"),
            ("submit_review", "review"),
            ("assign_review", "review-assign"),
            ("check_output", "output"),
            ("repository_search", "search"),
            ("continue_target_conflict", "conflict-continue"),
        ] {
            add(name, &format!("coding/{path}"));
        }
        if !c {
            add("edit_file", "coding/patch");
        }
    }
    if yes(config, "research") {
        add("submit_research_artifact", "research/submit");
        if yes(config, "researchRepository") {
            add("repository_read", "research/read");
            add("repository_search", "research/search");
        }
        if c {
            add("request_research_completion", "research/complete");
        }
    }
    if yes(config, "jira") && yes(config, "researchCoordinatorReporter") {
        add("jira_report", "jira/report");
    }
    if yes(config, "github") && yes(config, "researchCoordinatorReporter") {
        add("github_report", "github/report");
    }
    if c && !yes(config, "refinement") && !yes(config, "research") {
        add("propose_learning", "learning-proposal");
    }
    routes
}

#[derive(Debug, PartialEq, Eq)]
pub enum ToolRoute {
    Backend(String),
    Command,
    Check,
    Shell,
}
pub struct ToolCatalog {
    tools: Vec<Value>,
    routes: BTreeMap<String, String>,
}
impl ToolCatalog {
    pub fn new(config: &Value) -> Result<Self> {
        for key in [
            "coordinator",
            "refinement",
            "intake",
            "research",
            "researchRepository",
            "publication",
            "jira",
            "github",
            "reporter",
            "researchCoordinatorReporter",
            "coding",
            "choreography",
        ] {
            if config.get(key).is_some_and(|v| !v.is_boolean()) {
                return Err(crate::conflict("Invalid host tool capability."));
            }
        }
        if !config["recipients"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
            || config.get("tools").is_some_and(|v| {
                !v.as_array().is_some_and(|a| {
                    a.iter()
                        .all(|c| c["id"].as_str().is_some_and(|id| !id.is_empty()))
                })
            })
        {
            return Err(crate::conflict(
                "Invalid host tool recipients or command identities.",
            ));
        }
        if config
            .get("commandEngine")
            .is_some_and(|v| !v.is_null() && v != false && !v.is_object())
        {
            return Err(crate::conflict("Invalid host command engine."));
        }
        let mut command_ids = std::collections::HashSet::new();
        if config["tools"].as_array().is_some_and(|a| {
            a.iter()
                .any(|c| !command_ids.insert(c["id"].as_str().unwrap()))
        }) {
            return Err(crate::conflict("Configured command identity is ambiguous."));
        }
        let tools = host_tools(config);
        let mut names = std::collections::HashSet::new();
        if tools
            .iter()
            .any(|t| !names.insert(t["name"].as_str().unwrap()))
        {
            return Err(crate::conflict("Host tool capabilities overlap."));
        }
        Ok(Self {
            tools,
            routes: backend_routes(config),
        })
    }
    pub fn tools(&self) -> &[Value] {
        &self.tools
    }
    pub fn route(&self, name: &str) -> Result<ToolRoute> {
        if !self.tools.iter().any(|t| t["name"] == name) {
            return Err(crate::conflict("Host tool is unavailable."));
        }
        if let Some(route) = self
            .routes
            .get(name)
            .filter(|r| r.starts_with("evaluation/"))
        {
            return Ok(ToolRoute::Backend(route.clone()));
        }
        match name {
            "run_command" => Ok(ToolRoute::Command),
            "run_check" => Ok(ToolRoute::Check),
            "run_shell" => Ok(ToolRoute::Shell),
            _ => self
                .routes
                .get(name)
                .cloned()
                .map(ToolRoute::Backend)
                .ok_or_else(|| crate::conflict("Host tool is unavailable.")),
        }
    }
}

pub fn redact(value: &Value, secrets: &[String]) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|v| redact(v, secrets)).collect()),
        Value::Object(items) => Value::Object(
            items
                .iter()
                .filter(|(key, _)| {
                    ![
                        "apiKey",
                        "headers",
                        "authorization",
                        "token",
                        "providerPayload",
                    ]
                    .iter()
                    .any(|k| key.eq_ignore_ascii_case(k))
                })
                .map(|(key, v)| (key.clone(), redact(v, secrets)))
                .collect(),
        ),
        Value::String(text) => Value::String(
            secrets
                .iter()
                .filter(|s| !s.is_empty())
                .fold(text.clone(), |text, secret| {
                    text.replace(secret, "[redacted]")
                }),
        ),
        _ => value.clone(),
    }
}

/// Byte chunks must be assembled before decoding: a transport may split a UTF-8 scalar.
pub fn parse_request(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > HOST_INPUT_LIMIT * 3 {
        return Err(crate::conflict("Request too large."));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| crate::conflict("Host request must contain valid UTF-8."))?;
    if text.encode_utf16().count() > HOST_INPUT_LIMIT {
        return Err(crate::conflict("Request too large."));
    }
    Ok(serde_json::from_str(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(config: &Value) -> Vec<String> {
        host_tools(config)
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn github_issue_delivery_tools_follow_the_reporter_policy() {
        let reporter = json!({"purpose":"coding","coordinator":false,"recipients":[],"github":true,"reporter":true});
        let tools = names(&reporter);
        assert!(
            tools.contains(&"github_updates".into()) && tools.contains(&"github_report".into())
        );
        assert!(!tools.iter().any(|t| t.starts_with("jira_")));
        let catalog = ToolCatalog::new(&reporter).unwrap();
        assert_eq!(
            catalog.route("github_report").unwrap(),
            ToolRoute::Backend("github/report".into())
        );
        assert!(permits_backend_route(&reporter, "github/updates"));
        // Other specialists may read receipts but not queue updates.
        let reader = json!({"purpose":"coding","coordinator":false,"recipients":[],"github":true});
        assert_eq!(
            names(&reader)
                .iter()
                .filter(|t| t.starts_with("github_"))
                .count(),
            1
        );
        assert!(!permits_backend_route(&reader, "github/report"));
        // A research coordinator without a reporter may only queue its outcome.
        let fallback = json!({"purpose":"research","coordinator":true,"research":true,"recipients":[],"github":true,"researchCoordinatorReporter":true});
        let tool = host_tools(&fallback)
            .into_iter()
            .find(|t| t["name"] == "github_report")
            .unwrap();
        assert_eq!(
            tool["parameters"]["properties"]["kind"]["enum"],
            json!(["research_outcome"])
        );
        assert!(permits_backend_route(&fallback, "github/report"));
        // Without GitHub delivery nothing GitHub-specific is offered.
        let without =
            json!({"purpose":"coding","coordinator":false,"recipients":[],"reporter":true});
        assert!(!names(&without).iter().any(|t| t.starts_with("github_")));
        assert!(
            ToolCatalog::new(&json!({"purpose":"coding","recipients":[],"github":"yes"})).is_err()
        );
    }
}
