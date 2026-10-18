//! Role/model/brief snapshots, before any process or durable admission.
use super::{array, configuration::RuntimeConfig, text};
use crate::conflict;
use crewroom_core::{Result, parse_team, snapshot_briefs};
use serde_json::{Value, json};

pub fn prepare_assignments(
    config: &RuntimeConfig,
    run: &Value,
    work: Option<&Value>,
    checks: &[Value],
    input: &Value,
    roles: &[Value],
) -> Result<Vec<Value>> {
    if let Some(reason) = run["sourceConflict"].as_str().filter(|s| !s.is_empty()) {
        return Err(conflict(reason));
    }
    let principles = config.principles()?;
    let request = parse_team(input)?;
    let team = array(&request["team"]);
    let purpose = crewroom_core::run_purpose(run)?;
    if !matches!(purpose, "coding" | "research" | "explore" | "grill") {
        return Err(conflict(
            "This purpose cannot use ordinary assignment preparation.",
        ));
    }
    if matches!(purpose, "explore" | "grill")
        && (team.len() != 1 || team[0]["coordinator"] != true || work.is_some())
    {
        return Err(conflict(
            "Refinement admits only its coordinator and never prepares repository work.",
        ));
    }
    if purpose == "research" && work.is_some() {
        return Err(conflict("Research cannot admit coding repository work."));
    }
    let briefs = snapshot_briefs(team, &run["taskSnapshot"]["criteria"], checks, &[])?;
    team.iter()
        .map(|item| {
            let role = roles
                .iter()
                .find(|r| r["id"] == item["roleId"] && r["enabled"] == true)
                .ok_or_else(|| conflict("Selected role is unavailable."))?;
            validate_research_role(purpose, role, item)?;
            let name = text(item, "name")?;
            let model=config.assignment_model(role)?;
            crate::model_catalog::policy::verify_preview(&model,&item["modelPreview"])?;
            Ok(
                json!({"name":name,"coordinator":item["coordinator"],"input":item["input"],
            "role":role,"model":model,"brief":crewroom_core::principles::snapshot(&briefs[name],role,&principles)?}),
            )
        })
        .collect()
}

pub fn validate_research_role(purpose: &str, role: &Value, item: &Value) -> Result<()> {
    if purpose == "research" {
        if matches!(role["id"].as_str(), Some("implementer" | "reviewer")) {
            return Err(conflict("Research cannot admit coding specialists."));
        }
        if item["coordinator"] != true
            && role["id"] != "delivery-reporter"
            && array(&item["criteria"]).is_empty()
        {
            return Err(conflict(
                "Research specialists need an accepted criterion obligation in their brief.",
            ));
        }
    }
    Ok(())
}
