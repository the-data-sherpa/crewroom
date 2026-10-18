//! Capture launch requests without granting process or repository authority.
use crate::{
    Result,
    briefs::snapshot_briefs,
    dependencies::coding,
    store::{Store, conflict, integer, invalid, remove, request_id, strict},
};
use serde_json::{Value, json};
use std::collections::HashSet;

fn string(value: &Value, minimum: usize, maximum: usize, trim: bool) -> Result<&str> {
    let value = value.as_str().ok_or_else(|| invalid("Expected text."))?;
    let value = if trim {
        crate::store::trim_text(value)
    } else {
        value
    };
    if !(minimum..=maximum).contains(&value.chars().count()) {
        return Err(invalid("Text is outside its permitted size."));
    }
    Ok(value)
}
fn strings(value: Option<&Value>, maximum: usize, length: usize) -> Result<Value> {
    let Some(value) = value else {
        return Ok(json!([]));
    };
    let values = value
        .as_array()
        .filter(|a| a.len() <= maximum)
        .ok_or_else(|| invalid("List is outside its permitted size."))?;
    values
        .iter()
        .map(|v| string(v, 1, length, false).map(|s| json!(s)))
        .collect::<Result<Vec<_>>>()
        .map(|v| json!(v))
}
/// Parse the same assignments as a queued launch without accepting repository
/// options or granting execution. Live admission still requires store guards.
pub fn parse_team(value: &Value) -> Result<Value> {
    strict(value, &["team"])?;
    let launch = parse_launch(value, None)?;
    Ok(json!({"team":launch["team"]}))
}
pub(crate) fn model_preview(input: &Value, output: &mut Value) -> Result<()> {
    if let Some(preview) = input.get("modelPreview") {
        let revision = preview
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| invalid("Invalid model preview revision."))?;
        output["modelPreview"] = json!(revision);
    }
    Ok(())
}

pub(crate) fn parse_launch(value: &Value, roles: Option<&[Value]>) -> Result<Value> {
    strict(value, &["team", "repository"])?;
    let team = value["team"]
        .as_array()
        .filter(|a| (1..=4).contains(&a.len()))
        .ok_or_else(|| invalid("A team needs one to four assignments."))?;
    let mut parsed = Vec::new();
    let mut names = HashSet::new();
    let mut coordinators = 0;
    for member in team {
        strict(
            member,
            &[
                "name",
                "roleId",
                "coordinator",
                "input",
                "criteria",
                "dependsOn",
                "modelPreview",
            ],
        )?;
        let name = string(&member["name"], 1, 61, false)?;
        if !name.as_bytes()[0].is_ascii_alphabetic()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(invalid("Invalid assignment name."));
        }
        let role = member["roleId"]
            .as_str()
            .ok_or_else(|| invalid("A role ID is required."))?;
        let coordinator = member["coordinator"]
            .as_bool()
            .ok_or_else(|| invalid("coordinator must be boolean."))?;
        coordinators += usize::from(coordinator);
        if !names.insert(name) {
            return Err(conflict(
                "Select one coordinator and distinct assignment names.",
            ));
        }
        let input = string(&member["input"], 1, 40000, false)?;
        let criteria = member.get("criteria").cloned().unwrap_or_else(|| json!([]));
        let criteria = criteria
            .as_array()
            .filter(|a| a.len() <= 100)
            .ok_or_else(|| invalid("At most 100 criterion obligations are allowed."))?;
        let mut obligations = Vec::new();
        for criterion in criteria {
            strict(criterion, &["index", "obligation", "checkIds"])?;
            obligations.push(json!({"index":integer(criterion,"index",0,u64::MAX)?,"obligation":string(&criterion["obligation"],1,2000,true)?,"checkIds":strings(criterion.get("checkIds"),100,200)?}));
        }
        parsed.push(json!({"name":name,"roleId":role,"coordinator":coordinator,"input":input,"criteria":obligations,"dependsOn":strings(member.get("dependsOn"),10,100)?}));
        model_preview(member, parsed.last_mut().unwrap())?;
    }
    if coordinators != 1 {
        return Err(conflict(
            "Select one coordinator and distinct assignment names.",
        ));
    }
    let mut repository = value
        .get("repository")
        .cloned()
        .unwrap_or_else(|| json!({"mode":"new"}));
    match repository["mode"].as_str() {
        Some("new") => strict(&repository, &["mode"])?,
        Some("continue") => {
            strict(&repository, &["mode", "branch", "expectedHead"])?;
            let branch = string(&repository["branch"], 1, 200, false)?;
            if branch.starts_with('-') {
                return Err(invalid("Use a branch name, not a Git option."));
            }
            let head = string(&repository["expectedHead"], 40, 64, false)?;
            if !head
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(invalid("Expected head must be a commit hash."));
            }
            repository = json!({"mode":"continue","branch":branch,"expectedHead":head});
        }
        _ => return Err(invalid("Select new or continue repository work.")),
    }
    if let Some(roles) = roles {
        for member in &parsed {
            if !roles
                .iter()
                .any(|r| r["id"] == member["roleId"] && r["enabled"] == true)
            {
                return Err(conflict("Selected role is unavailable."));
            }
        }
    }
    Ok(json!({"team":parsed,"repository":repository}))
}

impl Store {
    pub(crate) fn validate_launch_briefs(&self, task: &Value, launch: &Value) -> Result<()> {
        let repository_id = task["repositoryId"]
            .as_str()
            .ok_or_else(|| conflict("Select a target repository before saving launch details."))?;
        let repository = self.active_repository(repository_id)?;
        let checks = repository["checks"]
            .as_array()
            .map(|a| a.iter().map(|c| c["id"].clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        snapshot_briefs(
            launch["team"].as_array().unwrap(),
            &task["criteria"],
            &checks,
            &[],
        )?;
        Ok(())
    }
    pub fn set_queue_launch(&self, run_id: &str, input: &Value, roles: &[Value]) -> Result<Value> {
        strict(input, &["requestId", "launch"])?;
        let request = request_id(input)?;
        let launch = parse_launch(&input["launch"], None)?;
        let payload = json!({"runId":run_id,"launch":launch});
        self.transaction(|s| {
            if let Some(replay) = s.replay(request, "launch", &payload)? {
                return Ok(replay);
            }
            let mut run = s.get_run(run_id)?;
            if !coding(&run)
                || run.get("position").is_none()
                || !s.run_records("assignments", run_id)?.is_empty()
            {
                return Err(conflict(
                    "Launch details can change only on an unadmitted queued coding plan.",
                ));
            }
            if run.get("continuedFrom").is_some()
                && (launch["repository"]["mode"] != "continue"
                    || launch["repository"]["branch"] != run["continuedFrom"]["branch"]
                    || launch["repository"]["expectedHead"] != run["continuedFrom"]["expectedHead"])
            {
                return Err(conflict(
                    "A linked continuation must retain its selected branch and expected head.",
                ));
            }
            parse_launch(&input["launch"], Some(roles))?;
            s.validate_launch_briefs(&run["taskSnapshot"], &launch)?;
            s.validate_insertion_team(run_id, launch["team"].as_array().unwrap())?;
            run["launch"] = launch;
            remove(&mut run, "launchHold");
            s.save_run(&run)?;
            let saved = s.get_run(run_id)?;
            s.receipt(request, "launch", &payload, &saved)?;
            Ok(saved)
        })
    }
}
