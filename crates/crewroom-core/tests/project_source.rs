//! A project's ticket source suggested from its repository remote (FOR-47),
//! against real Git checkouts with synthetic remotes. Nothing here contacts a
//! remote: only the configured address is read.
mod support;
use crewroom_core::{
    project_source::{observe, read_remote, view},
    repositories::validate_update,
    store::Store,
};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn git(checkout: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(args)
            .output()
            .unwrap()
            .status
            .success(),
        "{args:?}"
    );
}
fn setup() -> (tempfile::TempDir, Store, Value) {
    let root = tempfile::tempdir().unwrap();
    let checkout = std::fs::canonicalize(root.path()).unwrap().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    git(&checkout, &["init", "-b", "main"]);
    git(
        &checkout,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    let store = Store::open(&root.path().join("workflow.sqlite"))
        .unwrap()
        .with_clock(|| 1000);
    let repo = store
        .create_repository(
            &json!({"name":"Fixture","path":checkout,"targetBranch":"main","checks":[]}),
        )
        .unwrap();
    (root, store, repo)
}
fn set_remote(repo: &Value, name: &str, url: &str) {
    let path = Path::new(repo["path"].as_str().unwrap());
    let _ = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "remove", name])
        .output();
    git(path, &["remote", "add", name, url]);
}
fn observed(store: &Store, repo: &Value) -> Value {
    let current = store.configured_repository(id(repo)).unwrap();
    observe(read_remote(&current).as_deref())
}

#[test]
fn the_publish_remote_is_read_from_the_checkout() {
    let (_root, store, repo) = setup();
    assert_eq!(observed(&store, &repo)["state"], "missing");
    set_remote(
        &repo,
        "upstream",
        "git@bitbucket.org:fixture/repository.git",
    );
    // The only remote is the publish remote.
    assert_eq!(observed(&store, &repo)["state"], "bitbucket");
    set_remote(
        &repo,
        "origin",
        "https://github.com/Octo-Fixture/Sandbox.git",
    );
    assert_eq!(
        observed(&store, &repo)["label"],
        "github.com/Octo-Fixture/Sandbox"
    );
    // A configured push remote wins over origin.
    let path = Path::new(repo["path"].as_str().unwrap());
    git(path, &["config", "branch.main.pushRemote", "upstream"]);
    assert_eq!(observed(&store, &repo)["state"], "bitbucket");
    // A push URL is what the repository publishes to.
    git(
        path,
        &[
            "remote",
            "set-url",
            "--push",
            "upstream",
            "https://gitlab.example/fixture/repository.git",
        ],
    );
    assert_eq!(observed(&store, &repo)["host"], "gitlab.example");
    // A credentialed remote is never classified or echoed.
    git(path, &["config", "--unset", "branch.main.pushRemote"]);
    set_remote(
        &repo,
        "origin",
        "https://fixture:synthetic-secret@github.com/o/r.git",
    );
    let credentialed = observed(&store, &repo);
    assert_eq!(credentialed["state"], "unrecognised");
    assert!(!credentialed.to_string().contains("synthetic-secret"));
}

#[test]
fn confirmation_is_explicit_persisted_and_never_switched_by_a_remote_change() {
    let (root, store, repo) = setup();
    set_remote(&repo, "origin", "git@github.com:Octo-Fixture/Sandbox.git");
    let github = observed(&store, &repo);
    // Linking suggests GitHub with owner/repo prefilled, but saves nothing.
    let current = store.configured_repository(id(&repo)).unwrap();
    let suggested = view(&current, &github, &[], &[]);
    assert_eq!(suggested["state"], "unconfirmed");
    assert_eq!(suggested["suggestion"]["tickets"], "github");
    assert_eq!(
        suggested["suggestion"]["repository"],
        "Octo-Fixture/Sandbox"
    );
    assert!(current.get("ticketSource").is_none());

    let saved = store
        .confirm_ticket_source(
            id(&repo),
            &json!({"tickets":"github","repository":"Octo-Fixture/Sandbox"}),
            &github,
        )
        .unwrap();
    assert_eq!(saved["ticketSource"]["tickets"], "github");
    assert_eq!(saved["ticketSource"]["pullRequests"], "github");
    assert_eq!(
        saved["ticketSource"]["remote"]["label"],
        "github.com/Octo-Fixture/Sandbox"
    );
    // The repository revision is unchanged: drafts of its configuration stay valid.
    assert_eq!(saved["revision"], 1);
    // Confirming the same source again changes nothing.
    assert_eq!(
        store
            .confirm_ticket_source(
                id(&repo),
                &json!({"tickets":"github","repository":"octo-fixture/sandbox"}),
                &github,
            )
            .unwrap(),
        saved
    );
    // Editing the repository keeps the confirmed source.
    let mut update = json!({"name":"Renamed","path":repo["path"],"targetBranch":"main","checks":[],"revision":1,"requestId":uuid::Uuid::new_v4().to_string()});
    let edited = store
        .update_repository(id(&repo), &validate_update(&update).unwrap(), &update)
        .unwrap();
    assert_eq!(edited["ticketSource"], saved["ticketSource"]);
    update["revision"] = json!(2);

    // The remote moves to Bitbucket: a mismatch is reported, nothing changes.
    set_remote(&repo, "origin", "git@bitbucket.org:fixture/repository.git");
    let moved = observed(&store, &repo);
    let current = store.configured_repository(id(&repo)).unwrap();
    let warned = view(&current, &moved, &[], &[]);
    assert_eq!(warned["state"], "mismatch");
    assert!(
        warned["warning"]
            .as_str()
            .unwrap()
            .starts_with("Source no longer matches remote")
    );
    assert_eq!(current["ticketSource"], saved["ticketSource"]);
    // The Bitbucket suggestion needs a Jira site the remote cannot supply.
    assert_eq!(warned["suggestion"]["tickets"], "jira");
    assert_eq!(warned["suggestion"]["needsSite"], true);
    assert!(
        store
            .confirm_ticket_source(id(&repo), &json!({"tickets":"jira"}), &moved)
            .is_err()
    );
    let switched = store
        .confirm_ticket_source(
            id(&repo),
            &json!({"tickets":"jira","site":"https://fixture.atlassian.net"}),
            &moved,
        )
        .unwrap();
    assert_eq!(
        switched["ticketSource"]["site"],
        "https://fixture.atlassian.net"
    );
    assert_eq!(switched["ticketSource"]["pullRequests"], "bitbucket");
    assert_eq!(switched["ticketSourceHistory"][0], saved["ticketSource"]);
    // Persisted across reopen.
    drop(store);
    let reopened = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let current = reopened.configured_repository(id(&repo)).unwrap();
    assert_eq!(current["ticketSource"], switched["ticketSource"]);
    assert_eq!(view(&current, &moved, &[], &[])["state"], "confirmed");
}

#[test]
fn a_change_is_blocked_by_active_runs_and_imported_tickets_keep_their_source() {
    let (_root, store, repo) = setup();
    set_remote(&repo, "origin", "git@github.com:fixture/issues.git");
    let github = observed(&store, &repo);
    store
        .confirm_ticket_source(
            id(&repo),
            &json!({"tickets":"github","repository":"fixture/issues"}),
            &github,
        )
        .unwrap();
    // Tickets imported from the confirmed source, which now drives the sync.
    let source = store.github_source(id(&repo)).unwrap().unwrap()["source"].clone();
    assert_eq!(source["repository"], "fixture/issues");
    store
        .commit_github_sync(
            id(&repo),
            &source,
            &json!([{"id":7001,"number":1,"title":"Imported","body":"Synthetic","state":"open","html_url":"https://github.com/fixture/issues/issues/1","updated_at":"2026-10-18T08:00:00Z"}]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    let imported = store.snapshot().unwrap()["tasks"][0].clone();
    assert_eq!(imported["source"], "github");

    // Queued work referencing the repository blocks a change, naming it.
    let task = store
        .create_task(&json!({"title":"Active work","description":"","repositoryId":repo["id"]}))
        .unwrap();
    store
        .refine(
            id(&task),
            &json!({"revision":1,"sourceRevision":1,"brief":"Do it","criteria":["Done"]}),
        )
        .unwrap();
    support::accept_plan(&store, id(&task));
    let run = store.queue(id(&task), &json!({"revision":2})).unwrap();
    let runs = store.snapshot().unwrap()["runs"]
        .as_array()
        .unwrap()
        .clone();
    let blockers = crewroom_core::project_source::blocking_keys(&runs, id(&repo));
    assert_eq!(blockers.len(), 1);
    let current = store.configured_repository(id(&repo)).unwrap();
    let blocked_view = view(&current, &github, &blockers, &[]);
    assert!(
        blocked_view["changeBlocked"]
            .as_str()
            .unwrap()
            .contains("queued or open work")
    );
    let change = json!({"tickets":"jira","site":"https://fixture.atlassian.net"});
    let error = store
        .confirm_ticket_source(id(&repo), &change, &github)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Cannot change the ticket source"), "{error}");
    assert_eq!(
        store.configured_repository(id(&repo)).unwrap()["ticketSource"]["tickets"],
        "github"
    );

    // Once that work is cancelled the change is allowed, and the imported
    // ticket keeps its GitHub source, identity and history.
    store
        .cancel_queued(
            id(&run),
            &json!({"requestId":uuid::Uuid::new_v4().to_string()}),
        )
        .unwrap();
    store
        .confirm_ticket_source(id(&repo), &change, &github)
        .unwrap();
    let after = store.snapshot().unwrap()["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == imported["id"])
        .unwrap()
        .clone();
    for field in [
        "source",
        "sourceSite",
        "sourceId",
        "key",
        "sourceStatus",
        "sourceUrl",
    ] {
        assert_eq!(after[field], imported[field], "{field}");
    }
}
