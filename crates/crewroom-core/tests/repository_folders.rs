//! Choosing a repository folder (FOR-70): read-only inspection outcomes and
//! confirmed initialization, against real Git checkouts with synthetic
//! remotes. Nothing here contacts a remote.
use crewroom_core::{
    repository_folders::{Known, initialize, inspect},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}");
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_owned()
}
fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap();
    (dir, path)
}
fn checkout(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q", "-b", "main"]);
    git(
        path,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}
/// Every entry below `path` with its modification time and length.
fn tree(path: &Path) -> BTreeMap<PathBuf, (SystemTime, u64)> {
    let mut entries = BTreeMap::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            }
            entries.insert(entry.path(), (metadata.modified().unwrap(), metadata.len()));
        }
    }
    entries
}

#[test]
fn unusable_paths_are_refused_with_reason_and_remedy() {
    let (_dir, root) = root();
    let file = root.join("file.txt");
    std::fs::write(&file, "x").unwrap();
    for (path, refusal) in [
        ("relative/path".to_owned(), "notAbsolute"),
        ("".to_owned(), "notAbsolute"),
        (text(&root.join("missing")).to_owned(), "missing"),
        (text(&file).to_owned(), "notFolder"),
    ] {
        let view = inspect(&path, &Known::default());
        assert_eq!(view["state"], "refused", "{path}");
        assert_eq!(view["refusal"], refusal, "{path}");
        assert!(view["reason"].as_str().is_some_and(|r| !r.is_empty()));
        assert!(view["remedy"].as_str().is_some_and(|r| !r.is_empty()));
    }
    let locked = root.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let readable = std::fs::read_dir(&locked).is_ok();
    let view = inspect(text(&locked), &Known::default());
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    if !readable {
        assert_eq!(view["refusal"], "unreadable");
        assert_eq!(view["reason"], "Crewroom cannot read this folder.");
    }
}

#[test]
fn a_subfolder_resolves_to_its_root_and_prefills_from_git() {
    let (_dir, root) = root();
    let repo = root.join("Sandbox");
    checkout(&repo);
    git(&repo, &["branch", "develop"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:Octo-Fixture/Sandbox.git",
        ],
    );
    git(
        &repo,
        &[
            "remote",
            "add",
            "mirror",
            "https://bitbucket.org/fixture/sandbox.git",
        ],
    );
    git(
        &repo,
        &[
            "remote",
            "add",
            "backup",
            "https://fixture:synthetic-secret@git.example/fixture/sandbox.git",
        ],
    );
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/develop", "HEAD"],
    );
    git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    );
    let sub = repo.join("src/nested");
    std::fs::create_dir_all(&sub).unwrap();
    let before = tree(&repo);
    let view = inspect(text(&sub), &Known::default());
    assert_eq!(tree(&repo), before, "inspection must not write");
    assert_eq!(view["state"], "repository");
    assert_eq!(view["root"], text(&repo));
    assert_eq!(view["subfolder"], true);
    assert_eq!(
        view["notice"],
        format!("Using repository root {}", repo.display())
    );
    assert_eq!(view["name"], "Sandbox");
    assert_eq!(view["branches"], json!(["develop", "main"]));
    assert_eq!(
        view["defaultBranch"], "develop",
        "the remote's default branch"
    );
    assert_eq!(view["defaultSource"], "remote");
    assert_eq!(view["currentBranch"], "main");
    assert_eq!(view["publishRemote"], "origin");
    let remotes: BTreeMap<_, _> = view["remotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap().to_owned(), r.clone()))
        .collect();
    assert_eq!(remotes["origin"]["kind"], "github");
    assert_eq!(remotes["origin"]["host"], "github.com");
    assert_eq!(remotes["origin"]["repository"], "Octo-Fixture/Sandbox");
    assert_eq!(remotes["origin"]["publish"], true);
    assert_eq!(remotes["mirror"]["kind"], "bitbucket");
    assert_eq!(remotes["mirror"]["host"], "bitbucket.org");
    assert_eq!(remotes["backup"]["kind"], "unrecognised");
    assert!(!view.to_string().contains("synthetic-secret"));

    let root_view = inspect(text(&repo), &Known::default());
    assert_eq!(root_view["subfolder"], false);
    assert!(root_view["notice"].is_null());
}

#[test]
fn default_branch_falls_back_to_tracking_ref_then_current_branch() {
    let (_dir, root) = root();
    let tracked = root.join("tracked");
    checkout(&tracked);
    git(
        &tracked,
        &[
            "remote",
            "add",
            "origin",
            "https://git.example/fixture/tracked.git",
        ],
    );
    git(
        &tracked,
        &["update-ref", "refs/remotes/origin/trunk", "HEAD"],
    );
    git(
        &tracked,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    let view = inspect(text(&tracked), &Known::default());
    assert_eq!(view["defaultBranch"], "origin/trunk");
    assert_eq!(view["branches"], json!(["main", "origin/trunk"]));
    assert_eq!(view["remotes"][0]["kind"], "other");
    assert_eq!(view["remotes"][0]["host"], "git.example");

    let local = root.join("local");
    checkout(&local);
    git(&local, &["checkout", "-q", "-b", "feature"]);
    let view = inspect(text(&local), &Known::default());
    assert_eq!(
        view["defaultBranch"], "feature",
        "current branch without a remote"
    );
    assert_eq!(view["defaultSource"], "current");
    assert_eq!(view["remotes"], json!([]));
    assert!(view["publishRemote"].is_null());
}

#[test]
fn bare_metadata_unborn_and_owned_or_configured_checkouts_are_refused() {
    let (_dir, root) = root();
    let bare = root.join("bare.git");
    std::fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare"]);
    let view = inspect(text(&bare), &Known::default());
    assert_eq!(view["refusal"], "bare");
    assert!(
        view["remedy"]
            .as_str()
            .unwrap()
            .contains("working checkout")
    );

    let repo = root.join("repo");
    checkout(&repo);
    let view = inspect(text(&repo.join(".git")), &Known::default());
    assert_eq!(view["refusal"], "gitMetadata");

    let unborn = root.join("unborn");
    std::fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init", "-q", "-b", "main"]);
    assert_eq!(
        inspect(text(&unborn), &Known::default())["refusal"],
        "noCommits"
    );

    let owned = Known {
        owned_worktrees: vec![text(&repo).to_owned()],
        ..Known::default()
    };
    let view = inspect(text(&repo.join(".")), &owned);
    assert_eq!(view["refusal"], "crewroomWorktree");
    assert_eq!(
        view["remedy"],
        "Choose the original checkout it was created from."
    );

    let linked = root.join("linked");
    checkout(&linked);
    let data = root.join("elsewhere.git");
    std::fs::rename(linked.join(".git"), &data).unwrap();
    std::os::unix::fs::symlink(&data, linked.join(".git")).unwrap();
    assert_eq!(
        inspect(text(&linked), &Known::default())["refusal"],
        "unsafeMetadata"
    );

    let store = Store::open(&root.join("workflow.sqlite")).unwrap();
    let saved = store
        .create_repository(&json!({"name":"Fixture","path":repo,"targetBranch":"main","checks":[]}))
        .unwrap();
    let known = store.known_folders(None).unwrap();
    let view = inspect(text(&repo), &known);
    assert_eq!(view["refusal"], "alreadyConfigured");
    assert_eq!(view["reason"], "Fixture already uses this checkout.");
    assert_eq!(
        view["remedy"],
        "Edit Fixture in the list above, or retire it before adding it again."
    );
    // The repository being edited keeps its own checkout.
    let known = store.known_folders(saved["id"].as_str()).unwrap();
    assert_eq!(inspect(text(&repo), &known)["state"], "repository");
}

#[test]
fn a_plain_folder_offers_initialization_and_inspection_writes_nothing() {
    let (_dir, root) = root();
    let folder = root.join("New project");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("notes.md"), "draft").unwrap();
    let before = tree(&folder);
    let view = inspect(text(&folder), &Known::default());
    assert_eq!(tree(&folder), before);
    assert_eq!(view["state"], "notRepository");
    assert_eq!(view["name"], "New project");
    assert_eq!(view["initialBranch"], "main");
    assert_eq!(view["entries"], 1);
}

fn initialize_body(folder: &Path, branch: &str, confirmed: bool) -> Value {
    json!({"path":folder,"targetBranch":branch,"confirmed":confirmed})
}

#[test]
fn confirmed_initialization_creates_one_empty_commit_and_leaves_files_untracked() {
    let (_dir, root) = root();
    let folder = root.join("project");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("existing.txt"), "keep me out of Git").unwrap();
    let before = tree(&folder);

    // Declining, or an invalid branch, writes nothing.
    let error = initialize(&initialize_body(&folder, "trunk", false), &Known::default())
        .unwrap_err()
        .to_string();
    assert_eq!(error, "Confirm initialization first. Nothing was written.");
    for branch in ["-bad", "bad..name", ""] {
        assert!(initialize(&initialize_body(&folder, branch, true), &Known::default()).is_err());
    }
    assert_eq!(tree(&folder), before);

    let view = initialize(&initialize_body(&folder, "trunk", true), &Known::default()).unwrap();
    assert_eq!(view["state"], "repository");
    assert_eq!(view["branches"], json!(["trunk"]));
    assert_eq!(view["defaultBranch"], "trunk");
    assert_eq!(git(&folder, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(git(&folder, &["ls-tree", "HEAD"]), "");
    assert_eq!(git(&folder, &["ls-files", "--cached"]), "");
    assert_eq!(git(&folder, &["status", "--porcelain"]), "?? existing.txt");
    assert_eq!(
        std::fs::read_to_string(folder.join("existing.txt")).unwrap(),
        "keep me out of Git"
    );

    // A second request, or one for an existing repository, is refused.
    let error = initialize(&initialize_body(&folder, "trunk", true), &Known::default())
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        "Nothing was written: this folder is already a Git repository."
    );
    assert_eq!(git(&folder, &["rev-list", "--count", "HEAD"]), "1");
    let sub = folder.join("inner");
    std::fs::create_dir(&sub).unwrap();
    assert!(initialize(&initialize_body(&sub, "main", true), &Known::default()).is_err());
    assert!(!sub.join(".git").exists());
    let missing = root.join("missing");
    let error = initialize(&initialize_body(&missing, "main", true), &Known::default())
        .unwrap_err()
        .to_string();
    assert_eq!(error, "Nothing was written: This folder does not exist.");
    assert!(!missing.exists());
}
