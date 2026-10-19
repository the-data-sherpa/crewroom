//! Choosing a local folder for a repository (FOR-70).
//!
//! [`inspect`] reads a chosen folder without writing to it: it resolves the
//! enclosing checkout's root, refuses folders that cannot be configured with a
//! reason and a remedy, and prefills the repository form from Git (name, real
//! branches, the remote's default branch and the publish remotes with their
//! detected host).
//!
//! [`initialize`] is the only write. It runs only for a folder that inspection
//! reports as not a Git repository, after the operator confirms: `git init` on
//! the chosen branch and one empty initial commit. Existing files are never
//! staged or committed.
//!
//! Both run Git and the file system; call them on a blocking worker.
use crate::{
    Result,
    project_source::{observe, publish_remote_name},
    repositories::git,
    store::{Store, conflict, invalid, strict},
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Configured checkouts and Crewroom-owned worktrees a chosen folder must not
/// duplicate. Read from the store; inspection itself takes no lock.
#[derive(Default, Clone)]
pub struct Known {
    /// Active repositories: canonical path and name.
    pub configured: Vec<(String, String)>,
    /// Linked worktrees Crewroom created for runs.
    pub owned_worktrees: Vec<String>,
}

/// The branch a new repository is initialized on unless the operator picks another.
pub const INITIAL_BRANCH: &str = "main";
/// Author of the empty initial commit. The service ignores the operator's
/// global Git configuration, so it does not borrow their identity.
const INITIAL_AUTHOR: [&str; 4] = [
    "-c",
    "user.name=Crewroom",
    "-c",
    "user.email=crewroom@localhost.invalid",
];
const MAX_BRANCHES: usize = 200;

fn refused(path: &str, refusal: &str, reason: &str, remedy: &str) -> Value {
    json!({"path":path,"state":"refused","refusal":refusal,"reason":reason,"remedy":remedy})
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A `.git` entry in the folder or one of its parents, when Git itself
/// declined to read it (for example a checkout owned by another user).
fn enclosing_metadata(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .map(|p| p.join(".git"))
        .find(|p| std::fs::symlink_metadata(p).is_ok())
}

/// Reads a chosen folder. Never writes: Git runs with optional locks disabled
/// and no hooks, and nothing is created when the folder is not a repository.
pub fn inspect(requested: &str, known: &Known) -> Value {
    let requested = requested.trim();
    if requested.is_empty() || !Path::new(requested).is_absolute() {
        return refused(
            requested,
            "notAbsolute",
            "Use an absolute folder path.",
            "Choose the folder with Choose folder, or type a path that starts with /.",
        );
    }
    let metadata = match std::fs::metadata(requested) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return refused(
                requested,
                "missing",
                "This folder does not exist.",
                "Create the folder first, or choose an existing one.",
            );
        }
        Err(_) => {
            return refused(
                requested,
                "unreadable",
                "Crewroom cannot read this folder.",
                "Give your user read access to it, or choose another folder.",
            );
        }
    };
    if !metadata.is_dir() {
        return refused(
            requested,
            "notFolder",
            "This path is a file, not a folder.",
            "Choose the folder that contains the checkout.",
        );
    }
    let Ok(folder) = std::fs::canonicalize(requested) else {
        return refused(
            requested,
            "unreadable",
            "Crewroom cannot read this folder.",
            "Give your user read access to it, or choose another folder.",
        );
    };
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries.take(10_001).count(),
        Err(_) => {
            return refused(
                requested,
                "unreadable",
                "Crewroom cannot read this folder.",
                "Give your user read access to it, or choose another folder.",
            );
        }
    };
    let facts = git(
        &folder,
        &[
            "rev-parse",
            "--is-bare-repository",
            "--is-inside-git-dir",
            "--is-inside-work-tree",
        ],
    );
    let Ok(facts) = facts else {
        if let Some(metadata) = enclosing_metadata(&folder) {
            return refused(
                requested,
                "unreadable",
                &format!(
                    "Git could not read the repository metadata at {}.",
                    metadata.display()
                ),
                "Check that your user owns the checkout and that Git can open it, then choose it again.",
            );
        }
        let name = folder_name(&folder);
        return json!({
            "path":requested,"state":"notRepository","folder":folder.to_string_lossy(),
            "name":name,"initialBranch":INITIAL_BRANCH,"entries":entries.min(10_000),
            "reason":"This folder is not a Git repository.",
            "remedy":"Initialize a repository here, or choose a folder inside an existing checkout.",
        });
    };
    let facts: Vec<_> = facts.lines().collect();
    if facts.first() == Some(&"true") {
        return refused(
            requested,
            "bare",
            "This is a bare repository; it has no checkout to work in.",
            "Choose a folder with a working checkout, or clone this repository into one.",
        );
    }
    if facts.get(1) == Some(&"true") || facts.get(2) != Some(&"true") {
        return refused(
            requested,
            "gitMetadata",
            "This folder is inside Git's own metadata, not the checkout.",
            "Choose the checkout folder that contains .git.",
        );
    }
    let root = match git(&folder, &["rev-parse", "--show-toplevel"])
        .ok()
        .and_then(|root| std::fs::canonicalize(root).ok())
    {
        Some(root) => root,
        None => {
            return refused(
                requested,
                "unreadable",
                "Crewroom could not resolve this checkout's root folder.",
                "Choose the checkout's top-level folder.",
            );
        }
    };
    let root_text = root.to_string_lossy().into_owned();
    if known.owned_worktrees.contains(&root_text) {
        return refused(
            requested,
            "crewroomWorktree",
            "This is a worktree Crewroom created for a run.",
            "Choose the original checkout it was created from.",
        );
    }
    match std::fs::symlink_metadata(root.join(".git")) {
        Ok(m) if !m.file_type().is_symlink() && (m.is_file() || m.is_dir()) => {}
        _ => {
            return refused(
                requested,
                "unsafeMetadata",
                "This checkout's .git entry is a link or unreadable, so Crewroom will not use it.",
                "Replace the link with the repository metadata, or choose another checkout.",
            );
        }
    }
    if let Some((_, name)) = known.configured.iter().find(|(p, _)| *p == root_text) {
        return refused(
            requested,
            "alreadyConfigured",
            &format!("{name} already uses this checkout."),
            &format!("Edit {name} in the list above, or retire it before adding it again."),
        );
    }
    let branches: Vec<String> = git(
        &root,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|r| r.strip_prefix("refs/heads/"))
    .filter(|b| !b.is_empty())
    .take(MAX_BRANCHES)
    .map(str::to_owned)
    .collect();
    if branches.is_empty() {
        return refused(
            requested,
            "noCommits",
            "This repository has no commits yet, so it has no branch to target.",
            "Create a first commit in this checkout, then choose it again.",
        );
    }
    let current = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .filter(|b| branches.contains(b));
    let remote_names: Vec<String> = git(&root, &["remote"])
        .unwrap_or_default()
        .lines()
        .filter(|r| !r.is_empty() && !r.starts_with('-'))
        .map(str::to_owned)
        .collect();
    let mut choices = branches.clone();
    // The remote's default branch: the publish remote's HEAD, as a local branch
    // when one exists, otherwise as its remote-tracking ref.
    let first_guess = current.clone().unwrap_or_else(|| branches[0].clone());
    let publish = publish_remote_name(&root, &first_guess);
    let remote_default = publish.as_deref().and_then(|remote| {
        let head = git(
            &root,
            &[
                "symbolic-ref",
                "--quiet",
                &format!("refs/remotes/{remote}/HEAD"),
            ],
        )
        .ok()?;
        let branch = head.strip_prefix(&format!("refs/remotes/{remote}/"))?;
        if branches.iter().any(|b| b == branch) {
            Some(branch.to_owned())
        } else {
            let tracking = format!("{remote}/{branch}");
            choices.push(tracking.clone());
            Some(tracking)
        }
    });
    let default_branch = remote_default
        .clone()
        .or_else(|| current.clone())
        .unwrap_or_else(|| {
            ["main", "master"]
                .into_iter()
                .find(|b| branches.iter().any(|x| x == b))
                .map(str::to_owned)
                .unwrap_or_else(|| branches[0].clone())
        });
    let publish = publish_remote_name(&root, &default_branch);
    let remotes: Vec<Value> = remote_names
        .iter()
        .map(|name| {
            let address = git(&root, &["remote", "get-url", "--push", name])
                .ok()
                .filter(|a| !a.is_empty());
            let observed = observe(address.as_deref());
            json!({
                "name":name,"kind":observed["state"],"host":observed["host"],
                "repository":observed["repository"],"label":observed["label"],
                "publish":publish.as_deref()==Some(name.as_str()),
            })
        })
        .collect();
    let subfolder = root != folder;
    let mut view = json!({
        "path":requested,"state":"repository","root":root_text,"subfolder":subfolder,
        "name":folder_name(&root),"branches":choices,"defaultBranch":default_branch,
        "defaultSource":if remote_default.is_some() {"remote"} else if current.is_some() {"current"} else {"local"},
        "currentBranch":current,"remotes":remotes,"publishRemote":publish,
    });
    if subfolder {
        view["notice"] = json!(format!("Using repository root {root_text}"));
    }
    view
}

/// `git init` on `branch` and one empty commit, after the operator confirmed.
/// Refuses unless [`inspect`] reports the folder as not a Git repository, and
/// never stages or commits existing files. Returns the folder's new inspection.
pub fn initialize(body: &Value, known: &Known) -> Result<Value> {
    strict(body, &["path", "targetBranch", "confirmed", "requestId"])?;
    if body["confirmed"] != true {
        return Err(invalid(
            "Confirm initialization first. Nothing was written.",
        ));
    }
    let path = body["path"]
        .as_str()
        .ok_or_else(|| invalid("Choose a folder to initialize."))?;
    let branch = body["targetBranch"]
        .as_str()
        .map(str::trim)
        .filter(|b| !b.is_empty() && b.chars().count() <= 200)
        .ok_or_else(|| invalid("Target branch is required (up to 200 characters)."))?;
    let before = inspect(path, known);
    if before["state"] != "notRepository" {
        return Err(conflict(&format!(
            "Nothing was written: {}",
            before["reason"]
                .as_str()
                .filter(|_| before["state"] == "refused")
                .unwrap_or("this folder is already a Git repository.")
        )));
    }
    let folder = PathBuf::from(before["folder"].as_str().unwrap());
    let invalid_branch = format!("'{branch}' is not a valid branch name. Nothing was written.");
    if branch.starts_with('-') {
        return Err(invalid(&invalid_branch));
    }
    git(&folder, &["check-ref-format", "--branch", branch])
        .map_err(|_| invalid(&invalid_branch))?;
    let metadata = folder.join(".git");
    if std::fs::symlink_metadata(&metadata).is_ok() {
        return Err(conflict(
            "This folder already has a .git entry. Nothing was written.",
        ));
    }
    let created = (|| {
        git(&folder, &["init", "--quiet", "--initial-branch", branch])?;
        let mut commit: Vec<&str> = INITIAL_AUTHOR.to_vec();
        commit.extend([
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--quiet",
            "--allow-empty",
            "--no-verify",
            "--message",
            "Initial commit",
        ]);
        git(&folder, &commit)?;
        let count = git(&folder, &["rev-list", "--count", "HEAD"])?;
        let tree = git(&folder, &["ls-tree", "HEAD"])?;
        let index = git(&folder, &["ls-files", "--cached"])?;
        let head = git(&folder, &["symbolic-ref", "--short", "HEAD"])?;
        if count != "1" || !tree.is_empty() || !index.is_empty() || head != branch {
            return Err(conflict("Initialization did not produce one empty commit."));
        }
        Ok(())
    })();
    if let Err(error) = created {
        // Only the metadata this call created is removed; files are untouched.
        let _ = std::fs::remove_dir_all(&metadata);
        return Err(conflict(&format!(
            "Initialization failed and was undone: {error}"
        )));
    }
    Ok(inspect(path, known))
}

impl Store {
    /// Paths a chosen folder must not duplicate. `exclude` names the
    /// repository being edited, whose own checkout stays acceptable.
    pub fn known_folders(&self, exclude: Option<&str>) -> Result<Known> {
        let configured = self
            .all("repositories")?
            .into_iter()
            .filter(|r| r.get("retiredAt").is_none() && r["id"].as_str() != exclude)
            .filter_map(|r| {
                let saved = r["path"].as_str()?.to_owned();
                let path = std::fs::canonicalize(&saved)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(saved);
                Some((
                    path,
                    r["name"].as_str().unwrap_or("A repository").to_owned(),
                ))
            })
            .collect();
        let owned_worktrees = self
            .all("repository_work")?
            .into_iter()
            .filter_map(|w| w["path"].as_str().map(str::to_owned))
            .collect();
        Ok(Known {
            configured,
            owned_worktrees,
        })
    }
}
