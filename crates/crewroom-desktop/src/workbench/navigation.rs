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
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256
}
fn read(path: &Path) -> Option<Saved> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > 65536
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
