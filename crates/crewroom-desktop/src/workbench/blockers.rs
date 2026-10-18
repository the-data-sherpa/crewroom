//! Disabled decision and recovery controls name what blocks them and what clears it.
//! Each list is ordered so the first entry is the first step the operator can take.

const SAVE_BRIEF: &str = "save or discard brief changes";
const ACCEPT_JIRA: &str = "save the brief to accept the changed Jira requirements";

/// Renders "To {action}: a, b." for a non-empty blocker list.
pub(super) fn to(action: &str, blockers: &[String]) -> Option<String> {
    (!blockers.is_empty()).then(|| format!("To {action}: {}.", blockers.join(", ")))
}

/// A blocker hint with its own identity, so several from one call site stay distinct.
pub(super) fn line(
    id: impl Into<gpui_kit::SharedString>,
    action: &str,
    blockers: &[String],
) -> Option<gpui_kit::Div> {
    use gpui_kit::{ParentElement as _, Styled as _};
    to(action, blockers).map(|text| {
        gpui_kit::div()
            .text_sm()
            .text_color(gpui_kit::rgb(0x9eacb9))
            .child(crate::accessibility::named(id, text))
    })
}

/// Brief state shared by the plan, basis and adoption controls. A pending Jira
/// re-acceptance is cleared by the same Save brief that saves local edits.
fn brief(brief_dirty: bool, needs_reacceptance: bool) -> Option<String> {
    if needs_reacceptance {
        Some(ACCEPT_JIRA.into())
    } else if brief_dirty {
        Some(SAVE_BRIEF.into())
    } else {
        None
    }
}

#[derive(Default)]
pub(super) struct PlanDecision {
    pub brief_dirty: bool,
    pub needs_reacceptance: bool,
    pub stale: bool,
    pub waiting: bool,
    pub dirty_plan: bool,
    pub reason_missing: bool,
    /// No active intake: rejecting records the decision and no agent revises the plan.
    pub operator_plan: bool,
}
impl PlanDecision {
    /// Blockers shared by Accept plan and Request revision.
    pub(super) fn decide(&self) -> Vec<String> {
        let mut out: Vec<String> = brief(self.brief_dirty, self.needs_reacceptance)
            .into_iter()
            .collect();
        if self.stale || self.needs_reacceptance {
            out.push("refresh the repository basis and save a new plan version".into());
        }
        out
    }
    pub(super) fn accept(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.waiting {
            out.push("answer the open questions".into());
        }
        if self.dirty_plan {
            out.push("save your plan edits as a new version".into());
        }
        out
    }
    pub(super) fn reject(&self) -> Vec<String> {
        if self.reason_missing && self.operator_plan {
            vec!["enter a reason for rejecting".into()]
        } else if self.reason_missing {
            vec!["enter a reason for revision".into()]
        } else {
            Vec::new()
        }
    }
}

/// Refresh repository basis.
pub(super) fn refresh_basis(brief_dirty: bool, needs_reacceptance: bool) -> Vec<String> {
    brief(brief_dirty, needs_reacceptance).into_iter().collect()
}

/// Save plan draft. Content validation is reported by the editor itself.
pub(super) fn save_plan(brief_dirty: bool) -> Vec<String> {
    if brief_dirty {
        vec![SAVE_BRIEF.into()]
    } else {
        Vec::new()
    }
}

/// Use repository: an open run keeps the target it captured.
pub(super) fn change_target(open_run: bool) -> Vec<String> {
    if open_run {
        vec!["cancel the open run or let it finish".into()]
    } else {
        Vec::new()
    }
}

/// Adopt revision.
pub(super) fn adopt(
    runtime_fresh: bool,
    brief_dirty: bool,
    needs_reacceptance: bool,
    reason_missing: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if !runtime_fresh {
        out.push("wait for the current run status to load".into());
    }
    out.extend(brief(brief_dirty, needs_reacceptance));
    if reason_missing {
        out.push("enter an adoption reason".into());
    }
    out
}

/// Reconcile publication. `reason` is the service's unavailability reason.
pub(super) fn reconcile_publication(
    result_fresh: bool,
    available: bool,
    reason: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    if !result_fresh {
        out.push("wait for current results to load".into());
    }
    if !available {
        let reason = reason.trim().trim_end_matches('.');
        out.push(if reason.is_empty() {
            "publication is unavailable for this run".into()
        } else {
            reason.into()
        });
    }
    out
}

/// Reconcile Jira updates.
pub(super) fn reconcile_jira(configured: bool, pending: bool) -> Vec<String> {
    let mut out = Vec::new();
    if !configured {
        out.push("configure the Jira connection".into());
    }
    if !pending {
        out.push("no pending or uncertain update needs reconciling".into());
    }
    out
}

/// Refresh or retry from target.
pub(super) fn target_refresh(
    result_fresh: bool,
    runtime_fresh: bool,
    reason_missing: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if !result_fresh {
        out.push("wait for current results to load".into());
    }
    if !runtime_fresh {
        out.push("wait for the current run status to load".into());
    }
    if reason_missing {
        out.push("enter a reason for a fresh target attempt".into());
    }
    out
}

/// Learning Approve and Reject.
pub(super) fn learning(reason_missing: bool) -> Vec<String> {
    if reason_missing {
        vec!["enter a decision reason".into()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hint(action: &str, blockers: Vec<String>) -> String {
        to(action, &blockers).unwrap_or_default()
    }

    #[test]
    fn empty_blockers_render_no_hint() {
        assert_eq!(to("accept", &[]), None);
        assert_eq!(to("decide", &PlanDecision::default().decide()), None);
        assert_eq!(to("accept", &PlanDecision::default().accept()), None);
        assert_eq!(to("save the plan", &save_plan(false)), None);
        assert_eq!(to("change the target", &change_target(false)), None);
        assert_eq!(to("adopt", &adopt(true, false, false, false)), None);
        assert_eq!(
            to("reconcile", &reconcile_publication(true, true, "")),
            None
        );
        assert_eq!(to("reconcile Jira", &reconcile_jira(true, true)), None);
        assert_eq!(to("refresh", &target_refresh(true, true, false)), None);
        assert_eq!(to("decide", &learning(false)), None);
    }

    #[test]
    fn plan_decision_names_each_blocker() {
        let dirty = PlanDecision {
            brief_dirty: true,
            ..Default::default()
        };
        assert_eq!(
            hint("decide", dirty.decide()),
            "To decide: save or discard brief changes."
        );
        let stale = PlanDecision {
            stale: true,
            ..Default::default()
        };
        assert_eq!(
            hint("decide", stale.decide()),
            "To decide: refresh the repository basis and save a new plan version."
        );
        let waiting = PlanDecision {
            waiting: true,
            dirty_plan: true,
            ..Default::default()
        };
        assert_eq!(
            hint("accept", waiting.accept()),
            "To accept: answer the open questions, save your plan edits as a new version."
        );
        let reason = PlanDecision {
            reason_missing: true,
            ..Default::default()
        };
        assert_eq!(
            hint("request revision", reason.reject()),
            "To request revision: enter a reason for revision."
        );
        let operator = PlanDecision {
            reason_missing: true,
            operator_plan: true,
            ..Default::default()
        };
        assert_eq!(
            hint("reject the plan", operator.reject()),
            "To reject the plan: enter a reason for rejecting."
        );
    }

    #[test]
    fn jira_reacceptance_names_save_brief_before_refresh() {
        let changed = PlanDecision {
            needs_reacceptance: true,
            brief_dirty: true,
            stale: true,
            ..Default::default()
        };
        assert_eq!(
            hint("decide", changed.decide()),
            "To decide: save the brief to accept the changed Jira requirements, refresh the repository basis and save a new plan version."
        );
        assert_eq!(
            hint("refresh the basis", refresh_basis(false, true)),
            "To refresh the basis: save the brief to accept the changed Jira requirements."
        );
        assert_eq!(
            hint("refresh the basis", refresh_basis(true, false)),
            "To refresh the basis: save or discard brief changes."
        );
    }

    #[test]
    fn plan_draft_and_target_name_their_blockers() {
        assert_eq!(
            hint("save the plan", save_plan(true)),
            "To save the plan: save or discard brief changes."
        );
        assert_eq!(
            hint("change the target", change_target(true)),
            "To change the target: cancel the open run or let it finish."
        );
    }

    #[test]
    fn adoption_names_each_blocker() {
        assert_eq!(
            hint("adopt", adopt(false, false, false, false)),
            "To adopt: wait for the current run status to load."
        );
        assert_eq!(
            hint("adopt", adopt(true, true, false, false)),
            "To adopt: save or discard brief changes."
        );
        assert_eq!(
            hint("adopt", adopt(true, true, true, false)),
            "To adopt: save the brief to accept the changed Jira requirements."
        );
        assert_eq!(
            hint("adopt", adopt(true, false, false, true)),
            "To adopt: enter an adoption reason."
        );
    }

    #[test]
    fn recovery_controls_name_each_blocker() {
        assert_eq!(
            hint(
                "reconcile",
                reconcile_publication(
                    true,
                    false,
                    "Bitbucket connection unavailable for this repository."
                )
            ),
            "To reconcile: Bitbucket connection unavailable for this repository."
        );
        assert_eq!(
            hint("reconcile", reconcile_publication(true, false, "")),
            "To reconcile: publication is unavailable for this run."
        );
        assert_eq!(
            hint("reconcile", reconcile_publication(false, true, "")),
            "To reconcile: wait for current results to load."
        );
        assert_eq!(
            hint("reconcile Jira", reconcile_jira(false, true)),
            "To reconcile Jira: configure the Jira connection."
        );
        assert_eq!(
            hint("reconcile Jira", reconcile_jira(true, false)),
            "To reconcile Jira: no pending or uncertain update needs reconciling."
        );
        assert_eq!(
            hint("refresh from target", target_refresh(false, true, false)),
            "To refresh from target: wait for current results to load."
        );
        assert_eq!(
            hint("refresh from target", target_refresh(true, false, false)),
            "To refresh from target: wait for the current run status to load."
        );
        assert_eq!(
            hint("retry from target", target_refresh(true, true, true)),
            "To retry from target: enter a reason for a fresh target attempt."
        );
        assert_eq!(
            hint("decide", learning(true)),
            "To decide: enter a decision reason."
        );
    }
}
