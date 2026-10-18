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
}
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
}
fn read(path: &Path) -> Option<Saved> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > 8192
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return None;
    }
    let saved: Saved = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if !["Brief", "Conversation", "Results", "History"].contains(&saved.tab.as_str())
        || [&saved.task, &saved.run, &saved.attempt, &saved.candidate]
            .iter()
            .any(|v| v.as_ref().is_some_and(|v| v.is_empty() || v.len() > 256))
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
}
