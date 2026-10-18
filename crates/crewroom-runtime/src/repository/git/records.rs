//! Decode records independently, and account for process roots even when their
//! description is missing. A directory alone never grants signaling authority.
use super::*;

#[derive(Default)]
pub(in crate::repository) struct Records {
    pub saved: Vec<(String, CheckedProcess)>,
    pub roots: Vec<(Option<String>, PathBuf)>,
    pub errors: Vec<(Option<String>, String)>,
}
impl Records {
    pub fn error(&mut self, run: Option<&str>, error: impl std::fmt::Display) {
        self.errors.push((run.map(String::from), error.to_string()));
    }
    pub fn extend(&mut self, other: Self) {
        self.saved.extend(other.saved);
        self.roots.extend(other.roots);
        self.errors.extend(other.errors);
    }
}
impl Git {
    pub(super) fn saved_records(&self) -> Records {
        let mut records = Records::default();
        if let Err(e) = self.read_jobs(&mut records) {
            records.error(None, e);
        }
        // These roots are named by completion, not run. Unknown roots therefore
        // hold any Stop until their ownership can be attributed.
        let promotions = self.jobs.parent().unwrap().join("promotion-processes");
        let read = || -> Result<Vec<PathBuf>> {
            match std::fs::symlink_metadata(&promotions) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(e.into()),
                Ok(m) if !m.is_dir() => {
                    return Err(conflict("Promotion directory changed identity."));
                }
                _ => (),
            }
            canonical(&promotions)?;
            Ok(std::fs::read_dir(&promotions)?
                .map(|entry| entry.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?)
        };
        match read() {
            Ok(roots) => records
                .roots
                .extend(roots.into_iter().map(|root| (None, root))),
            Err(e) => records.error(None, e),
        }
        records
    }
    fn read_jobs(&self, records: &mut Records) -> Result<()> {
        canonical(&self.jobs)?;
        for entry in std::fs::read_dir(&self.jobs)? {
            let root = match entry {
                Ok(entry) => entry.path(),
                Err(e) => {
                    records.error(None, e);
                    continue;
                }
            };
            let run = root
                .file_name()
                .and_then(|v| v.to_str())
                .filter(|v| crewroom_core::store::valid_uuid(v));
            let Some(run) = run else {
                records.error(None, "Invalid Git operation directory.");
                continue;
            };
            if let Err(e) = self.read_run(run, &root, records) {
                records.error(Some(run), e);
            }
        }
        Ok(())
    }
    pub(super) fn read_run(&self, run: &str, root: &Path, records: &mut Records) -> Result<()> {
        if !std::fs::symlink_metadata(root)?.is_dir() {
            return Err(conflict("Git operation directory changed identity."));
        }
        canonical(root)?;
        for entry in std::fs::read_dir(root)? {
            let read_one = || -> Result<Option<Intent>> {
                let path = entry?.path();
                if path.extension().and_then(|v| v.to_str()) != Some("json") {
                    records.roots.push((Some(run.into()), path));
                    return Ok(None);
                }
                let name = path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| conflict("Invalid retained Git record path."))?;
                let bytes = super::super::files::regular_bytes(root, name, 256 * 1024)?.0;
                let intent: Intent = serde_json::from_slice(&bytes)?;
                self.intent_identity(run, &intent)?;
                if path.file_stem().and_then(|s| s.to_str()) != Some(&intent.descriptor.token) {
                    return Err(conflict("Retained Git operation identity changed."));
                }
                Ok(Some(intent))
            };
            match read_one() {
                Ok(Some(intent)) => records.saved.push((run.into(), intent.descriptor)),
                Ok(None) => (),
                Err(e) => records.error(Some(run), e),
            }
        }
        Ok(())
    }
}
