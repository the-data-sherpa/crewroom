//! Per-dataset navigation only. Drafts, credentials and workflow state never enter this file.
use super::*;
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
#[derive(Default, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub(super) struct Saved {
    pub task: Option<String>,
    pub run: Option<String>,
    pub attempt: Option<String>,
    pub candidate: Option<String>,
    pub tab: String,
    /// The project sidebar (FOR-75): the current project, the operator's order and the
    /// collapsed groups. Files written before FOR-75 read with none of these set.
    #[serde(default)]
    pub projects: ProjectPrefs,
    /// The Overview's no-progress thresholds (FOR-77). Older files read with the defaults.
    #[serde(default)]
    pub overview: OverviewPrefs,
    /// Snoozed and settled tickets (FOR-78): operator presentation only. Files written before
    /// FOR-78 read with empty shelves.
    #[serde(default)]
    pub shelves: Shelves,
}
/// Minutes without observed progress before a running run is flagged in Running, and before
/// it is also listed under Needs you. A display preference of this machine only.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub(super) struct OverviewPrefs {
    pub stale_minutes: u32,
    pub promote_minutes: u32,
}
impl Default for OverviewPrefs {
    fn default() -> Self {
        Self {
            stale_minutes: 20,
            promote_minutes: 60,
        }
    }
}
/// The flag threshold stays within a day, and promotion always comes strictly later.
pub(super) const MAX_STALE_MINUTES: u32 = 24 * 60;
impl OverviewPrefs {
    pub(super) fn bounded(self) -> Self {
        let stale = self.stale_minutes.clamp(1, MAX_STALE_MINUTES - 1);
        Self {
            stale_minutes: stale,
            promote_minutes: self.promote_minutes.clamp(stale + 1, MAX_STALE_MINUTES),
        }
    }
}
/// Where the operator put tickets aside. Never a run, workflow or source fact: a snoozed or
/// settled ticket keeps running, and its source and workflow state are shown unchanged.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub(super) struct Shelves {
    #[serde(default)]
    pub snoozed: Vec<Snooze>,
    #[serde(default)]
    pub settled: Vec<String>,
    /// Tickets that returned from Snoozed, with the reason, until the operator dismisses it.
    #[serde(default)]
    pub woke: Vec<Woke>,
    /// Shelves the operator expanded, as `<group>/snoozed` or `<group>/settled`; collapsed
    /// otherwise.
    #[serde(default)]
    pub open: Vec<String>,
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) enum Watch {
    /// A PR review or comment the ticket's PR did not have when snoozed.
    Review,
    /// Hosted checks on the ticket's PR finish.
    Ci,
    /// The ticket's source status changes.
    Source,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Snooze {
    pub task: String,
    /// RFC 3339; the ticket wakes once this time has passed.
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub event: Option<Watch>,
    /// What the watched fact looked like when snoozed.
    #[serde(default)]
    pub baseline: Value,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub(super) struct Woke {
    pub task: String,
    pub reason: String,
}
/// Project ids in the operator's order, collapsed groups ([`UNASSIGNED`] included) and the
/// project New ticket and Import act in by default. Only ids; never project configuration.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub(super) struct ProjectPrefs {
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub collapsed: Vec<String>,
}
/// The collapse key of the group for tickets without a project.
pub(super) const UNASSIGNED: &str = "unassigned";
/// At most this many ids are kept per list, so the file stays small.
const PROJECT_IDS: usize = 200;
pub(super) struct Navigation {
    path: Option<PathBuf>,
    saved: Saved,
    pub restore: bool,
}
impl Navigation {
    pub fn new(descriptor: Option<&Path>) -> Self {
        let path = descriptor
            .and_then(Path::parent)
            .map(|p| p.join("native-ui-state.json"));
        let saved = path.as_ref().and_then(|p| read(p)).unwrap_or_default();
        Self {
            path,
            restore: true,
            saved,
        }
    }
    pub fn saved(&self) -> &Saved {
        &self.saved
    }
    fn save(&mut self, saved: Saved) -> Result<(), String> {
        if saved == self.saved {
            return Ok(());
        }
        self.saved = saved;
        if let Some(path) = &self.path {
            crewroom_core::authority::write_private(
                path,
                &serde_json::to_vec(&self.saved).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// Changes only the project preferences, keeping the saved ticket navigation.
    pub fn update_projects(
        &mut self,
        change: impl FnOnce(&mut ProjectPrefs),
    ) -> Result<(), String> {
        let mut saved = self.saved.clone();
        change(&mut saved.projects);
        for list in [&mut saved.projects.order, &mut saved.projects.collapsed] {
            let mut seen = std::collections::HashSet::new();
            list.retain(|id| valid_id(id) && seen.insert(id.clone()));
            list.truncate(PROJECT_IDS);
        }
        if saved
            .projects
            .current
            .as_ref()
            .is_some_and(|id| !valid_id(id))
        {
            saved.projects.current = None;
        }
        self.save(saved)
    }
    /// Changes only the Overview thresholds, kept within their bounds.
    pub fn update_overview(
        &mut self,
        change: impl FnOnce(&mut OverviewPrefs),
    ) -> Result<(), String> {
        let mut saved = self.saved.clone();
        change(&mut saved.overview);
        saved.overview = saved.overview.bounded();
        self.save(saved)
    }
    /// Changes only the shelves, keeping everything else.
    pub fn update_shelves(&mut self, change: impl FnOnce(&mut Shelves)) -> Result<(), String> {
        let mut saved = self.saved.clone();
        change(&mut saved.shelves);
        let shelves = &mut saved.shelves;
        let mut seen = std::collections::HashSet::new();
        shelves.snoozed.retain(|s| {
            valid_id(&s.task)
                && (s.until.is_some() || s.event.is_some())
                && seen.insert(s.task.clone())
        });
        let mut seen = std::collections::HashSet::new();
        shelves.woke.retain(|w| {
            valid_id(&w.task) && w.reason.len() <= REASON && seen.insert(w.task.clone())
        });
        for list in [&mut shelves.settled, &mut shelves.open] {
            let mut seen = std::collections::HashSet::new();
            list.retain(|id| valid_id(id) && seen.insert(id.clone()));
            list.truncate(PROJECT_IDS);
        }
        shelves.snoozed.truncate(PROJECT_IDS);
        shelves.woke.truncate(PROJECT_IDS);
        self.save(saved)
    }
}
/// The longest wake reason kept.
const REASON: usize = 256;
fn valid_shelves(s: &Shelves) -> bool {
    s.snoozed.len() <= PROJECT_IDS
        && s.woke.len() <= PROJECT_IDS
        && [&s.settled, &s.open]
            .iter()
            .all(|list| list.len() <= PROJECT_IDS && list.iter().all(|v| valid_id(v)))
        && s.snoozed.iter().all(|z| {
            valid_id(&z.task)
                && (z.until.is_some() || z.event.is_some())
                && z.until
                    .as_deref()
                    .is_none_or(|t| chrono::DateTime::parse_from_rfc3339(t).is_ok())
        })
        && s.woke
            .iter()
            .all(|w| valid_id(&w.task) && w.reason.len() <= REASON)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256
}
fn read(path: &Path) -> Option<Saved> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        // Shelves (FOR-78) hold up to 200 snoozes with their baselines.
        || metadata.len() > 262144
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return None;
    }
    let saved: Saved = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    // An empty tab: only project preferences were saved so far.
    if !["", "Brief", "Conversation", "Results", "History"].contains(&saved.tab.as_str())
        || [&saved.task, &saved.run, &saved.attempt, &saved.candidate]
            .iter()
            .any(|v| v.as_ref().is_some_and(|v| !valid_id(v)))
        || saved
            .projects
            .current
            .as_ref()
            .is_some_and(|v| !valid_id(v))
        || [&saved.projects.order, &saved.projects.collapsed]
            .iter()
            .any(|list| list.len() > PROJECT_IDS || list.iter().any(|v| !valid_id(v)))
        || !valid_shelves(&saved.shelves)
    {
        return None;
    }
    Some(saved)
}
impl Workbench {
    pub(super) fn save_navigation(&mut self) {
        if !self.connected || self.panel != Panel::Ticket || self.selected.is_none() {
            return;
        }
        let saved = Saved {
            task: self.selected.clone(),
            run: self.observation.id.clone(),
            attempt: self.observation.selected_attempt.clone(),
            candidate: self.observation.candidate.clone(),
            tab: self.tab.into(),
            projects: self.navigation.saved().projects.clone(),
            overview: self.navigation.saved().overview,
            shelves: self.navigation.saved().shelves.clone(),
        };
        if let Err(error) = self.navigation.save(saved) {
            self.notice = format!(
                "Navigation could not be saved for reopening: {error}. Workflow data is unaffected."
            );
            self.feedback_origin = Some(self.origin_at(crate::workbench::feedback::Place::Top));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restores_only_private_navigation_and_keeps_corrupt_or_foreign_state_out() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = root.path().join("native-connection.json");
        let mut n = Navigation::new(Some(&descriptor));
        let saved = Saved {
            task: Some("task".into()),
            run: Some("run".into()),
            attempt: Some("prior".into()),
            candidate: None,
            tab: "History".into(),
            projects: Default::default(),
            overview: Default::default(),
            shelves: Default::default(),
        };
        n.save(saved.clone()).unwrap();
        assert!(Navigation::new(Some(&descriptor)).saved() == &saved);
        let path = root.path().join("native-ui-state.json");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&path).is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, b"{broken").unwrap();
        assert!(read(&path).is_none());
    }
    #[test]
    fn shelves_read_back_and_a_malformed_snooze_keeps_the_file_out() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = root.path().join("native-connection.json");
        let path = root.path().join("native-ui-state.json");
        // A file written before FOR-78 has no shelves and still reads.
        crewroom_core::authority::write_private(&path, br#"{"tab":"Brief","task":"t"}"#).unwrap();
        let mut n = Navigation::new(Some(&descriptor));
        assert_eq!(n.saved().shelves, Shelves::default());
        n.update_shelves(|s| {
            s.snoozed.push(Snooze {
                task: "t".into(),
                until: None,
                event: Some(Watch::Ci),
                baseline: json!({"state":"running"}),
            });
            // Neither a time nor an event: dropped.
            s.snoozed.push(Snooze {
                task: "u".into(),
                until: None,
                event: None,
                baseline: Value::Null,
            });
            s.settled = vec!["v".into(), "v".into(), String::new()];
        })
        .unwrap();
        let shelves = Navigation::new(Some(&descriptor)).saved().shelves.clone();
        assert_eq!(shelves.snoozed.len(), 1);
        assert_eq!(shelves.settled, ["v"]);
        assert_eq!(
            Navigation::new(Some(&descriptor)).saved().task.as_deref(),
            Some("t")
        );
        crewroom_core::authority::write_private(
            &path,
            br#"{"tab":"Brief","shelves":{"snoozed":[{"task":"t","until":"not a time"}]}}"#,
        )
        .unwrap();
        assert!(read(&path).is_none());
    }
    #[test]
    fn project_preferences_survive_reopening_and_keep_ticket_navigation() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = root.path().join("native-connection.json");
        let path = root.path().join("native-ui-state.json");
        // A file written before FOR-75 has no project preferences and still reads.
        crewroom_core::authority::write_private(
            &path,
            br#"{"task":"task","run":null,"attempt":null,"candidate":null,"tab":"Brief"}"#,
        )
        .unwrap();
        let mut n = Navigation::new(Some(&descriptor));
        assert_eq!(n.saved().task.as_deref(), Some("task"));
        assert_eq!(n.saved().projects, ProjectPrefs::default());
        assert_eq!(n.saved().overview, OverviewPrefs::default());
        // Overview thresholds (FOR-77) are bounded, promotion stays later, and they reopen.
        n.update_overview(|o| {
            o.stale_minutes = 45;
            o.promote_minutes = 30;
        })
        .unwrap();
        assert_eq!(
            Navigation::new(Some(&descriptor)).saved().overview,
            OverviewPrefs {
                stale_minutes: 45,
                promote_minutes: 46
            }
        );
        n.update_overview(|o| o.stale_minutes = 0).unwrap();
        assert_eq!(n.saved().overview.stale_minutes, 1);
        n.update_overview(|o| *o = OverviewPrefs::default())
            .unwrap();
        n.update_projects(|p| {
            p.current = Some("b".into());
            p.order = vec!["b".into(), "a".into(), "b".into(), String::new()];
            p.collapsed = vec![UNASSIGNED.into()];
        })
        .unwrap();
        let reopened = Navigation::new(Some(&descriptor));
        assert_eq!(reopened.saved().task.as_deref(), Some("task"));
        assert_eq!(
            reopened.saved().projects,
            ProjectPrefs {
                current: Some("b".into()),
                order: vec!["b".into(), "a".into()],
                collapsed: vec![UNASSIGNED.into()],
            }
        );
        // Project preferences saved before any ticket was opened also read back.
        let fresh = tempfile::tempdir().unwrap();
        let descriptor = fresh.path().join("native-connection.json");
        Navigation::new(Some(&descriptor))
            .update_projects(|p| p.current = Some("a".into()))
            .unwrap();
        assert_eq!(
            Navigation::new(Some(&descriptor))
                .saved()
                .projects
                .current
                .as_deref(),
            Some("a")
        );
    }
}
