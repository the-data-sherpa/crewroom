//! Read-only catalog facts and credential-free assignment previews.
use crate::{AuthorityState, array, text};
use crewroom_core::{Result, roles::validate_role, snapshot_briefs};
use crewroom_runtime::supervisor::workflow::configuration::assignment_prompt;
use serde_json::{Value, json};

pub(super) fn facts(state: &AuthorityState, workspace: &Value) -> Result<Value> {
    let mut facts = json!({});
    for role in array(&workspace["roles"]) {
        let id = text(role, "id")?;
        facts[id] = json!({"runs":0,"proposals":array(&workspace["learning"]).iter().filter(|p| p["status"]=="pending" && p["target"]["roleId"]==id).count(),"lockReason":if ["coordinator","triage","implementer","reviewer"].contains(&id) {"Required by the coding workflow; must stay enabled."} else {""},"modelError":"Runner integration is unavailable; model resolution is not verified."});
    }
    for role in array(&workspace["roles"]) {
        let (job, warning) = crewroom_core::model_jobs::job(role);
        facts[text(role, "id")?]["job"] = json!(job);
        facts[text(role, "id")?]["warning"] = json!(warning);
    }
    for run in array(&workspace["runs"]) {
        let runtime = state.store.runtime(text(run, "id")?)?;
        let used: std::collections::HashSet<_> = array(&runtime["assignments"])
            .iter()
            .filter_map(|a| a["role"]["id"].as_str())
            .collect();
        for id in used {
            if facts[id].is_object() {
                facts[id]["runs"] = json!(facts[id]["runs"].as_u64().unwrap_or(0) + 1);
            }
        }
    }
    Ok(facts)
}

/// A preview is a hypothetical one-role assignment over the selected saved ticket.
/// It never queues work, admits an assignment, or reads private provider settings.
pub(super) fn preview(state: &AuthorityState, body: &Value) -> Result<Value> {
    let mut role = validate_role(&body["role"])?;
    let task = state.store.get_task(text(body, "taskId")?)?;
    // FOR-88: with a project, the role as admitted there: its availability,
    // instructions, principles and model overrides. Admission resolves the
    // ticket's project, so the ticket must belong to it.
    let project = match body.get("repositoryId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(id.clone()),
        Some(_) => {
            return Err(crewroom_core::Error::Invalid(
                "repositoryId must be a string or null.".into(),
            ));
        }
    };
    if let Some(project) = &project {
        if task["repositoryId"] != project.as_str() {
            return Err(crewroom_core::Error::Invalid(
                "Choose a ticket from this project to preview its settings.".into(),
            ));
        }
        role = crewroom_core::project_settings::effective_roles(
            &[role],
            &state.store.project_settings()?,
            project,
        )
        .remove(0);
    }
    let name = if role["id"] == "coordinator" {
        "Coordinator"
    } else {
        "Specialist"
    };
    let criteria: Vec<_> = array(&task["criteria"]).iter().enumerate()
        .map(|(index, _)| json!({"index":index,"obligation":"Inspect this criterion within the role's responsibilities.","checkIds":[]})).collect();
    let request = json!({"name":name,"criteria":criteria,"dependsOn":[]});
    let briefs = snapshot_briefs(&[request], &task["criteria"], &[], &[])?;
    let principles =
        crewroom_core::principles::Principles::beside_roles(state.roles.path()).read()?;
    let brief = crewroom_core::principles::snapshot(&briefs[name], &role, &principles)?;
    let mut assignment = json!({"name":name,"coordinator":role["id"]=="coordinator","role":role,"input":role["purpose"],"brief":brief});
    if let Some(repository) = array(&state.store.snapshot()?["repositories"])
        .iter()
        .find(|r| r["id"] == task["repositoryId"])
        && let Some(guidance) = array(&repository["guidance"]).last()
    {
        assignment["projectGuidance"] = guidance.clone();
    }
    let run = json!({"purpose":"coding","taskSnapshot":task,"taskRevision":task["revision"]});
    let prompt = assignment_prompt(
        &run,
        &assignment,
        &assignment,
        &[assignment.clone()],
        None,
        false,
    )?;
    let scope = if project.is_some() {
        "Preview uses the saved ticket, the role as this project's settings resolve it and one hypothetical assignment with all criteria and no dependencies. The launch plan supplies actual obligations and teammates; repository instructions are captured when its worktree is prepared. No agent is started."
    } else {
        "Preview uses the saved ticket, current role draft and one hypothetical assignment with all criteria and no dependencies. The launch plan supplies actual obligations and teammates; repository instructions are captured when its worktree is prepared. No agent is started."
    };
    Ok(
        json!({"role":role,"task":task,"assignment":assignment,"prompt":prompt,"repositoryId":project,"scope":scope}),
    )
}
