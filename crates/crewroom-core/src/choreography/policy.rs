use crate::{Result, store::invalid};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Responsibility {
    Writer,
    Navigator,
    Ui,
    Diagnosis,
    Documentation,
    Reviewer,
    Reporter,
    Consultant,
}
impl Responsibility {
    pub fn may_write(self) -> bool {
        matches!(self, Self::Writer | Self::Ui | Self::Documentation)
    }
    pub fn contributes(self) -> bool {
        !matches!(self, Self::Reviewer | Self::Reporter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    Proposed,
    Active,
    Blocked,
    Complete,
}
impl PackageState {
    /// These are lifecycle edges, not execution grants. Store transitions also
    /// check the caller, dependencies, receipts and current revision.
    pub fn permits(self, next: Self, explicit_rework: bool) -> bool {
        matches!(
            (self, next),
            (Self::Proposed, Self::Active)
                | (Self::Active, Self::Blocked | Self::Complete)
                | (Self::Blocked, Self::Active)
        ) || self == Self::Complete && next == Self::Active && explicit_rework
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledAssignment {
    pub name: String,
    pub role_id: String,
    pub responsibility: Responsibility,
    pub stage: u32,
    pub packages: Vec<String>,
    pub writes: bool,
    pub rationale: String,
    pub input: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    pub ui_required: bool,
    pub diagnosis_reason: Option<String>,
    pub assignments: Vec<ScheduledAssignment>,
}

fn prose(value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.chars().count() > max {
        return Err(invalid("Choreography text is empty or exceeds its limit."));
    }
    Ok(())
}

impl Schedule {
    pub fn parse(plan: &Value, input: &Value) -> Result<Self> {
        if input.to_string().len() > 100_000 {
            return Err(invalid("Choreography exceeds 100 KB."));
        }
        let schedule: Self = serde_json::from_value(input.clone())?;
        schedule.validate(plan)?;
        Ok(schedule)
    }
    pub fn validate(&self, plan: &Value) -> Result<()> {
        let packages = plan["content"]["workPackages"]
            .as_array()
            .ok_or_else(|| invalid("An accepted implementation plan is required."))?;
        // Reuse MH5's ID, dependency-cycle and criterion validation.
        let max_criterion = packages
            .iter()
            .flat_map(|p| p["criteria"].as_array().into_iter().flatten())
            .filter_map(Value::as_u64)
            .max()
            .unwrap_or(0);
        if max_criterion >= 100 {
            return Err(invalid("Invalid accepted package criteria."));
        }
        crate::intake::plan::validate(
            &plan["content"],
            &vec![Value::Null; max_criterion as usize + 1],
        )?;
        if !(4..=60).contains(&self.assignments.len()) {
            return Err(invalid(
                "Schedule the pair, documentation and independent review.",
            ));
        }
        let package_ids: HashSet<_> = packages.iter().filter_map(|p| p["id"].as_str()).collect();
        let mut names = HashSet::new();
        let mut stages: HashMap<u32, Vec<&ScheduledAssignment>> = HashMap::new();
        for member in &self.assignments {
            if member.name.is_empty()
                || member.name.len() > 61
                || !member.name.as_bytes()[0].is_ascii_alphabetic()
                || !member
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                || !names.insert(&member.name)
            {
                return Err(invalid(
                    "Scheduled assignment names must be distinct and valid.",
                ));
            }
            prose(&member.role_id, 100)?;
            prose(&member.rationale, 2000)?;
            prose(&member.input, 40000)?;
            if member.stage > 20
                || member.packages.is_empty()
                || member.packages.len() != member.packages.iter().collect::<HashSet<_>>().len()
                || member
                    .packages
                    .iter()
                    .any(|p| !package_ids.contains(p.as_str()))
            {
                return Err(invalid(
                    "Assignments must bind distinct accepted package IDs and a bounded stage.",
                ));
            }
            if member.writes && !member.responsibility.may_write()
                || member.responsibility == Responsibility::Writer && !member.writes
            {
                return Err(invalid(
                    "Navigators, diagnosis specialists and reviewers are read-only; writers require write responsibility.",
                ));
            }
            stages.entry(member.stage).or_default().push(member);
        }
        for members in stages.values() {
            if members.len() > 3 || members.iter().filter(|m| m.writes).count() > 1 {
                return Err(invalid(
                    "A stage permits three specialists and one designated writer.",
                ));
            }
        }
        if let Some(reason) = &self.diagnosis_reason {
            prose(reason, 2000)?;
        }
        if self
            .assignments
            .iter()
            .any(|m| m.responsibility == Responsibility::Diagnosis)
            != self.diagnosis_reason.is_some()
        {
            return Err(invalid(
                "Diagnosis requires a stated problem and an assigned responsibility.",
            ));
        }
        let last_contribution = self
            .assignments
            .iter()
            .filter(|m| m.responsibility.contributes())
            .map(|m| m.stage)
            .max()
            .unwrap_or(0);
        if self.ui_required
            && !self
                .assignments
                .iter()
                .any(|m| m.responsibility == Responsibility::Ui)
        {
            return Err(invalid("UI work requires a UI responsibility."));
        }
        for p in &package_ids {
            let bound: Vec<_> = self
                .assignments
                .iter()
                .filter(|m| m.packages.iter().any(|id| id == p))
                .collect();
            for responsibility in [
                Responsibility::Writer,
                Responsibility::Navigator,
                Responsibility::Documentation,
                Responsibility::Reviewer,
            ] {
                if bound
                    .iter()
                    .filter(|m| m.responsibility == responsibility)
                    .count()
                    != 1
                {
                    return Err(invalid(
                        "Every package needs one writer, navigator, documentation owner and independent reviewer.",
                    ));
                }
            }
            let writer = bound
                .iter()
                .find(|m| m.responsibility == Responsibility::Writer)
                .unwrap();
            let navigator = bound
                .iter()
                .find(|m| m.responsibility == Responsibility::Navigator)
                .unwrap();
            let reviewer = bound
                .iter()
                .find(|m| m.responsibility == Responsibility::Reviewer)
                .unwrap();
            if navigator.stage != writer.stage || reviewer.stage <= last_contribution {
                return Err(invalid(
                    "Pair participants share a stage; final review follows every contribution.",
                ));
            }
            if bound.iter().any(|m| {
                m.responsibility == Responsibility::Documentation && m.stage < writer.stage
            }) {
                return Err(invalid("Documentation closure follows implementation."));
            }
        }
        // Dependency packages must be closable before their dependents start.
        for p in packages {
            let id = p["id"].as_str().unwrap();
            let start = self
                .assignments
                .iter()
                .filter(|m| {
                    m.packages.iter().any(|v| v == id) && m.responsibility == Responsibility::Writer
                })
                .map(|m| m.stage)
                .min()
                .unwrap();
            for dep in p["dependsOn"].as_array().unwrap() {
                let dep = dep.as_str().unwrap();
                let finish = self
                    .assignments
                    .iter()
                    .filter(|m| {
                        m.responsibility.contributes() && m.packages.iter().any(|v| v == dep)
                    })
                    .map(|m| m.stage)
                    .max()
                    .unwrap();
                if finish >= start {
                    return Err(invalid(
                        "Package dependencies must finish in an earlier stage.",
                    ));
                }
            }
        }
        Ok(())
    }
}
