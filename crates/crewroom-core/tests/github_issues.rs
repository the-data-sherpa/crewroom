//! GitHub Issues intake records, per project (FOR-45), and the schema-34
//! migration of the former workspace-level source. Transport is tested in the
//! service against a simulated GitHub API.
mod support;
use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};

fn issue(number: u64, title: &str, state: &str, updated: &str) -> Value {
    json!({"id":5000+number,"number":number,"title":title,"body":"Synthetic body","state":state,"html_url":format!("https://github.com/Fixture/Issues/issues/{number}"),"updated_at":updated})
}

#[test]
fn sync_matches_on_identity_keeps_refinements_and_records_failures_without_partial_imports() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let project = support::github_project(&store, root.path(), "Fixture/Issues");
    // Confirming GitHub Issues starts the project with open issues only.
    let started = store.github_source(&project).unwrap().unwrap();
    assert_eq!(started["state"], "not_synced");
    assert_eq!(
        started["source"],
        json!({"repository":"Fixture/Issues","labels":[],"assignee":null,"milestone":null,"state":"open"})
    );
    // The repository comes from the confirmed source, never from the filters.
    assert!(
        store
            .save_github_filters(&project, &json!({"repository":"fixture/other"}))
            .is_err()
    );
    let saved = store
        .save_github_filters(&project, &json!({"labels":["crewroom"]}))
        .unwrap();
    assert_eq!(saved["state"], "not_synced");
    let source = saved["source"].clone();
    // Saving the same source again keeps its state.
    assert_eq!(
        store
            .save_github_filters(&project, &json!({"labels":"crewroom"}))
            .unwrap(),
        saved
    );

    let state = store
        .commit_github_sync(
            &project,
            &source,
            &json!([
                issue(1, "First", "open", "2026-10-18T08:00:00Z"),
                issue(2, "Second", "closed", "2026-10-18T08:00:00Z")
            ]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    assert_eq!(state["state"], "connected");
    assert_eq!(state["issueCount"], 2);
    let tasks = store.snapshot().unwrap()["tasks"].clone();
    let first = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["key"] == "#1")
        .unwrap()
        .clone();
    assert_eq!(first["source"], "github");
    assert_eq!(first["sourceSite"], "https://github.com/fixture/issues");
    assert_eq!(first["sourceStatus"], "open");
    assert_eq!(first["project"], "Fixture/Issues");
    // A ticket first imported by the project's source targets that project.
    assert_eq!(first["repositoryId"], project);
    let id = first["id"].as_str().unwrap().to_owned();
    store
        .refine(
            &id,
            &json!({"revision":1,"sourceRevision":1,"brief":"Operator brief","criteria":["Kept"]}),
        )
        .unwrap();

    // Closing the issue is a status change, not a requirement change.
    store
        .commit_github_sync(
            &project,
            &source,
            &json!([issue(1, "First", "closed", "2026-10-18T09:00:00Z")]),
            &json!([]),
            &json!([]),
        )
        .unwrap();
    let closed = store.get_task(&id).unwrap();
    assert_eq!(closed["sourceStatus"], "closed");
    assert_eq!(closed["needsReacceptance"], false);
    // A body change asks for re-acceptance and keeps the operator's brief.
    let mut changed = issue(1, "First", "closed", "2026-10-18T10:00:00Z");
    changed["body"] = json!("Synthetic body, revised");
    store
        .commit_github_sync(&project, &source, &json!([changed]), &json!([]), &json!([]))
        .unwrap();
    let revised = store.get_task(&id).unwrap();
    assert_eq!(revised["needsReacceptance"], true);
    assert_eq!(revised["brief"], "Operator brief");
    assert_eq!(revised["sourceRevision"], 2);

    // An older observation, a pull request, or a duplicate saves nothing.
    let before = store.snapshot().unwrap()["tasks"].clone();
    for bad in [
        json!([issue(1, "Older", "open", "2026-10-18T07:00:00Z")]),
        json!([{"id":5009,"number":9,"title":"PR","body":"","state":"open","html_url":"https://github.com/fixture/issues/pull/9","pull_request":{}}]),
        json!([
            issue(3, "Dup", "open", "2026-10-18T11:00:00Z"),
            issue(3, "Dup", "open", "2026-10-18T11:00:00Z")
        ]),
    ] {
        assert!(
            store
                .commit_github_sync(&project, &source, &bad, &json!([]), &json!([]))
                .is_err()
        );
    }
    assert_eq!(store.snapshot().unwrap()["tasks"], before);

    // A failure keeps the last complete sync and the imported tickets.
    let synced_at = store.github_source(&project).unwrap().unwrap()["lastSyncedAt"].clone();
    store
        .fail_github_sync(
            &project,
            &source,
            "rate_limited",
            "GitHub rate-limited this request.",
        )
        .unwrap();
    let failed = store.github_source(&project).unwrap().unwrap();
    assert_eq!(failed["state"], "unavailable");
    assert_eq!(failed["reason"], "rate_limited");
    assert_eq!(failed["lastSyncedAt"], synced_at);

    // Issues GitHub no longer serves are recorded with the sync, validated,
    // and replaced by the next complete sync.
    let gone = json!([{"taskId": id, "key": "#1", "status": 410}]);
    let state = store
        .commit_github_sync(&project, &source, &json!([]), &json!([]), &gone)
        .unwrap();
    assert_eq!(state["unreadable"], gone);
    assert_eq!(state["state"], "connected");
    assert!(state.get("reason").is_none());
    for bad in [
        json!([{"taskId": id, "key": "1", "status": 410}]),
        json!([{"taskId": id, "key": "#1"}]),
        json!({"taskId": id}),
    ] {
        assert!(
            store
                .commit_github_sync(&project, &source, &json!([]), &json!([]), &bad)
                .is_err(),
            "{bad}"
        );
    }
    let state = store
        .commit_github_sync(&project, &source, &json!([]), &json!([]), &json!([]))
        .unwrap();
    assert_eq!(state["unreadable"], json!([]));

    // Replacing the filters starts over; a sync of the old filters is refused
    // and a late failure for it is ignored. Earlier tickets stay.
    store
        .save_github_filters(&project, &json!({"state":"all"}))
        .unwrap();
    assert!(
        store
            .commit_github_sync(&project, &source, &json!([]), &json!([]), &json!([]))
            .is_err()
    );
    store
        .fail_github_sync(&project, &source, "rejected", "stale")
        .unwrap();
    let replaced = store.github_source(&project).unwrap().unwrap();
    assert_eq!(replaced["state"], "not_synced");
    assert_eq!(replaced["source"]["state"], "all");
    assert_eq!(store.get_task(&id).unwrap()["brief"], "Operator brief");
}

#[test]
fn each_project_has_its_own_source_driven_by_its_confirmed_ticket_source() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    // A project without a confirmed GitHub source cannot save filters or sync.
    let plain = support::project(&store, root.path(), "plain");
    assert_eq!(store.github_source(&plain).unwrap(), None);
    for error in [
        store
            .save_github_filters(&plain, &json!({"state":"all"}))
            .unwrap_err(),
        store
            .save_github_status_labels(&plain, &json!({"inProgress":"x"}))
            .unwrap_err(),
        store
            .commit_github_sync(
                &plain,
                &json!({"repository":"fixture/issues","labels":[],"assignee":null,"milestone":null,"state":"open"}),
                &json!([]),
                &json!([]),
                &json!([]),
            )
            .unwrap_err(),
    ] {
        assert!(error.to_string().contains("ticket source"), "{error}");
    }

    // Two projects keep separate filters and sync results.
    let first = support::github_project(&store, root.path(), "fixture/issues");
    let second = support::github_project(&store, root.path(), "fixture/other");
    store
        .save_github_filters(&first, &json!({"labels":"crewroom","state":"all"}))
        .unwrap();
    let other = store.github_source(&second).unwrap().unwrap();
    assert_eq!(other["source"]["labels"], json!([]));
    assert_eq!(other["source"]["repository"], "fixture/other");
    let ids: Vec<Value> = store
        .github_sources()
        .unwrap()
        .iter()
        .map(|s| s["repositoryId"].clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&json!(first)) && ids.contains(&json!(second)));

    // Re-confirming the same repository (any spelling) keeps its filters.
    let source = store.github_source(&first).unwrap().unwrap()["source"].clone();
    store
        .commit_github_sync(&first, &source, &json!([]), &json!([]), &json!([]))
        .unwrap();
    store
        .confirm_ticket_source(
            &first,
            &json!({"tickets":"github","repository":"Fixture/Issues"}),
            &crewroom_core::project_source::observe(Some("git@github.com:Fixture/Issues.git")),
        )
        .unwrap();
    let kept = store.github_source(&first).unwrap().unwrap();
    assert_eq!(kept["state"], "connected");
    assert_eq!(kept["source"]["labels"], json!(["crewroom"]));
    assert_eq!(kept["source"]["repository"], "fixture/issues");

    // Switching the project to Jira removes its GitHub sync; switching to
    // another repository starts over with open issues only.
    store
        .confirm_ticket_source(
            &second,
            &json!({"tickets":"jira","site":"https://fixture.atlassian.net"}),
            &crewroom_core::project_source::observe(Some("git@bitbucket.org:fixture/other.git")),
        )
        .unwrap();
    assert_eq!(store.github_source(&second).unwrap(), None);
    support::confirm_github(&store, &first, "fixture/moved");
    let moved = store.github_source(&first).unwrap().unwrap();
    assert_eq!(moved["state"], "not_synced");
    assert_eq!(moved["source"]["repository"], "fixture/moved");
    assert_eq!(moved["source"]["labels"], json!([]));
    // A sync of the replaced source is refused.
    assert!(
        store
            .commit_github_sync(&first, &source, &json!([]), &json!([]), &json!([]))
            .is_err()
    );
}

/// Rewrites a current store into the schema-33 shape: the workspace-level
/// `github_sync` record and no per-project table.
fn as_schema_33(path: &std::path::Path, legacy: &Value) {
    let db = Connection::open(path).unwrap();
    db.execute_batch("DROP TABLE github_project_sync; PRAGMA user_version=33;")
        .unwrap();
    db.execute("INSERT INTO github_sync VALUES(1,?)", [legacy.to_string()])
        .unwrap();
}

#[test]
fn schema_34_moves_the_workspace_source_into_the_projects_that_use_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let using = support::github_project(&store, root.path(), "Fixture/Issues");
    let elsewhere = support::github_project(&store, root.path(), "fixture/elsewhere");
    let unconfirmed = support::project(&store, root.path(), "unconfirmed");
    drop(store);
    let legacy = json!({
        "source":{"repository":"fixture/issues","labels":["crewroom"],"assignee":null,"milestone":"3","state":"all"},
        "state":"connected","taskIds":[],"issueCount":0,"lastSyncedAt":"2026-10-18T08:00:00.000Z",
        "statusLabels":{"inProgress":"crewroom: in progress","inReview":null},
    });
    as_schema_33(&path, &legacy);

    let store = Store::open(&path).unwrap();
    let adopted = store.github_source(&using).unwrap().unwrap();
    assert_eq!(adopted["source"]["repository"], "Fixture/Issues");
    assert_eq!(adopted["source"]["labels"], json!(["crewroom"]));
    assert_eq!(adopted["source"]["milestone"], "3");
    assert_eq!(
        adopted["statusLabels"]["inProgress"],
        "crewroom: in progress"
    );
    assert_eq!(adopted["lastSyncedAt"], legacy["lastSyncedAt"]);
    assert_eq!(adopted["adoptedFrom"], "workspace");
    // A project confirming another repository starts with open issues only;
    // one without a confirmed source has no GitHub sync.
    let other = store.github_source(&elsewhere).unwrap().unwrap();
    assert_eq!(other["source"]["state"], "open");
    assert_eq!(other["state"], "not_synced");
    assert_eq!(store.github_source(&unconfirmed).unwrap(), None);
    assert_eq!(store.legacy_github_source().unwrap(), None);
    drop(store);
    let version: u32 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, crewroom_core::store::SCHEMA_VERSION);
}

#[test]
fn an_unused_workspace_source_is_kept_until_a_project_confirms_its_repository() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let project = support::project(&store, root.path(), "later");
    drop(store);
    let legacy = json!({
        "source":{"repository":"fixture/issues","labels":[],"assignee":"octo-fixture","milestone":null,"state":"open"},
        "state":"not_synced","taskIds":[],
    });
    as_schema_33(&path, &legacy);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.legacy_github_source().unwrap(), Some(legacy.clone()));
    assert!(store.github_sources().unwrap().is_empty());
    support::confirm_github(&store, &project, "fixture/issues");
    let adopted = store.github_source(&project).unwrap().unwrap();
    assert_eq!(adopted["source"]["assignee"], "octo-fixture");
    assert_eq!(adopted["adoptedFrom"], "workspace");
    assert_eq!(store.legacy_github_source().unwrap(), None);
}
