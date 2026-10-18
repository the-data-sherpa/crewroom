//! Per-project Jira refresh records (FOR-53) and the schema-35 migration of
//! the former workspace-level Jira refresh setting. Transport is tested in the
//! service against a simulated Jira API.
mod support;
use crewroom_core::store::Store;
use rusqlite::Connection;
use serde_json::{Value, json};

const SITE: &str = "https://fixture.atlassian.net";
const OTHER: &str = "https://other-fixture.atlassian.net";

fn issue(id: &str, title: &str, status: &str) -> Value {
    json!({"id":id,"key":format!("SIM-{id}"),"fields":{"summary":title,"description":"Source requirement","status":{"name":status,"id":"1"},"updated":"2026-10-18T08:00:00.000+0000"}})
}

fn setting() -> Value {
    json!({"siteUrl":SITE,"cloudId":"00000000-0000-4000-8000-000000000022","filterId":"12345","assigneeAccountId":"operator-1","name":"Assigned work"})
}

#[test]
fn each_project_refreshes_its_confirmed_site_with_its_own_filters() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let alpha = support::jira_project(&store, root.path(), "alpha", SITE);
    let beta = support::jira_project(&store, root.path(), "beta", SITE);
    // A confirmed site starts with no filters, so it never imports a whole site.
    let started = store.jira_project_source(&alpha).unwrap().unwrap();
    assert_eq!(started["state"], "not_synced");
    assert_eq!(
        started["source"],
        json!({"siteUrl":SITE,"projectKey":null,"filterId":null,"assignee":null,"jql":null})
    );
    assert!(store.save_jira_filters(&alpha, &json!({})).is_err());
    // The site comes from the confirmed source, never from the filters.
    assert!(
        store
            .save_jira_filters(&alpha, &json!({"projectKey":"CR","siteUrl":OTHER}))
            .is_err()
    );
    let a = store
        .save_jira_filters(&alpha, &json!({"projectKey":"cr","assignee":"operator-1"}))
        .unwrap();
    let b = store
        .save_jira_filters(&beta, &json!({"jql":"labels = beta"}))
        .unwrap();
    // Projects never share filters: saving one leaves the other unchanged.
    assert_eq!(
        crewroom_core::jira_projects::jql(&a["source"]).unwrap(),
        r#"project = "CR" AND assignee = "operator-1" ORDER BY Rank ASC"#
    );
    assert_eq!(
        store.jira_project_source(&beta).unwrap().unwrap()["source"],
        b["source"]
    );
    assert_eq!(
        store
            .save_jira_filters(&alpha, &json!({"projectKey":"CR","assignee":"operator-1"}))
            .unwrap(),
        a,
        "saving the same filters keeps the record"
    );

    let committed = store
        .commit_jira_project_sync(
            &alpha,
            &a["source"],
            &json!([
                issue("2", "Second ranked first", "To Do"),
                issue("1", "First", "In Progress")
            ]),
            &json!([]),
        )
        .unwrap();
    assert_eq!(committed["state"], "connected");
    assert_eq!(committed["issueCount"], 2);
    let snapshot = store.snapshot().unwrap();
    let keys: Vec<_> = snapshot["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys, ["SIM-2", "SIM-1"], "Jira rank order is kept");
    for task in snapshot["tasks"].as_array().unwrap() {
        // Tickets first imported by a project's refresh target that project.
        assert_eq!(task["repositoryId"], alpha.as_str());
        assert_eq!(task["source"], "jira");
        assert_eq!(task["sourceSite"], SITE);
    }
    // Jira status is the source status only; importing never queues work.
    assert_eq!(snapshot["tasks"][1]["sourceStatus"], "In Progress");
    assert!(snapshot["runs"].as_array().unwrap().is_empty());
    // A commit for a source the operator replaced meanwhile is refused.
    assert!(
        store
            .commit_jira_project_sync(&beta, &a["source"], &json!([]), &json!([]))
            .is_err()
    );

    // A failure keeps the last complete refresh and records why.
    store
        .fail_jira_project_sync(
            &alpha,
            &a["source"],
            "rejected",
            "Jira rejected the credentials.",
        )
        .unwrap();
    let failed = store.jira_project_source(&alpha).unwrap().unwrap();
    assert_eq!(failed["state"], "unavailable");
    assert_eq!(failed["reason"], "rejected");
    assert_eq!(failed["lastSyncedAt"], committed["lastSyncedAt"]);
    assert_eq!(failed["taskIds"], committed["taskIds"]);
    // A stale failure for an older source is dropped.
    store
        .fail_jira_project_sync(&beta, &a["source"], "unreachable", "x")
        .unwrap();
    assert_eq!(
        store.jira_project_source(&beta).unwrap().unwrap()["state"],
        "not_synced"
    );
}

#[test]
fn confirming_github_or_another_site_stops_refresh_and_keeps_imported_tickets() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("workflow.sqlite")).unwrap();
    let project = support::jira_project(&store, root.path(), "alpha", SITE);
    let saved = store
        .save_jira_filters(&project, &json!({"projectKey":"CR"}))
        .unwrap();
    store
        .commit_jira_project_sync(
            &project,
            &saved["source"],
            &json!([issue("1", "First", "To Do")]),
            &json!([]),
        )
        .unwrap();
    let task = store.snapshot().unwrap()["tasks"][0].clone();

    // Another Jira site starts over with no filters; the old site's refresh is refused.
    support::confirm_jira(&store, &project, OTHER);
    let moved = store.jira_project_source(&project).unwrap().unwrap();
    assert_eq!(moved["source"]["siteUrl"], OTHER);
    assert!(!crewroom_core::jira_projects::has_filters(&moved["source"]));
    assert!(
        store
            .commit_jira_project_sync(&project, &saved["source"], &json!([]), &json!([]))
            .is_err()
    );
    // Confirming the same site again keeps its filters.
    let filtered = store
        .save_jira_filters(&project, &json!({"filterId":"77"}))
        .unwrap();
    support::confirm_jira(&store, &project, OTHER);
    assert_eq!(
        store.jira_project_source(&project).unwrap().unwrap()["source"],
        filtered["source"]
    );

    // GitHub Issues has no Jira refresh at all.
    support::confirm_github(&store, &project, "fixture/issues");
    assert_eq!(store.jira_project_source(&project).unwrap(), None);
    assert!(
        store
            .save_jira_filters(&project, &json!({"projectKey":"CR"}))
            .is_err()
    );
    // Tickets imported earlier keep their source identity and status.
    let kept = store.get_task(task["id"].as_str().unwrap()).unwrap();
    assert_eq!(kept["source"], "jira");
    assert_eq!(kept["sourceSite"], SITE);
    assert_eq!(kept["sourceStatus"], task["sourceStatus"]);
}

/// Rewrites a current store into the schema-34 shape: the workspace-level
/// `jira_sync` record and no per-project table.
fn as_schema_34(path: &std::path::Path, legacy: &Value) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "DROP TABLE jira_project_sync; DELETE FROM jira_sync; PRAGMA user_version=34;",
    )
    .unwrap();
    db.execute("INSERT INTO jira_sync VALUES(1,?)", [legacy.to_string()])
        .unwrap();
}

#[test]
fn schema_35_moves_the_workspace_setting_into_the_projects_that_confirmed_its_site() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let using = support::jira_project(&store, root.path(), "alpha", SITE);
    let also = support::jira_project(&store, root.path(), "beta", SITE);
    let elsewhere = support::jira_project(&store, root.path(), "gamma", OTHER);
    let github = support::github_project(&store, root.path(), "fixture/issues");
    drop(store);
    let legacy = json!({"source":setting(),"state":"connected","taskIds":[],"lastSyncedAt":"2026-10-18T08:00:00.000Z","lastAttemptAt":"2026-10-18T08:00:00.000Z"});
    as_schema_34(&path, &legacy);

    let store = Store::open(&path).unwrap();
    for project in [&using, &also] {
        let adopted = store.jira_project_source(project).unwrap().unwrap();
        assert_eq!(adopted["adoptedFrom"], "workspace");
        assert_eq!(
            crewroom_core::jira_projects::jql(&adopted["source"]).unwrap(),
            r#"filter = 12345 AND assignee = "operator-1" ORDER BY Rank ASC"#
        );
        assert_eq!(adopted["lastSyncedAt"], legacy["lastSyncedAt"]);
    }
    // Each adopting project holds its own copy: editing one leaves the other.
    store
        .save_jira_filters(&using, &json!({"projectKey":"CR"}))
        .unwrap();
    assert_eq!(
        store.jira_project_source(&also).unwrap().unwrap()["source"]["filterId"],
        "12345"
    );
    let other = store.jira_project_source(&elsewhere).unwrap().unwrap();
    assert!(!crewroom_core::jira_projects::has_filters(&other["source"]));
    assert_eq!(store.jira_project_source(&github).unwrap(), None);
    assert_eq!(store.legacy_jira_setting().unwrap(), None, "adopted");
    // Offering the same setting again from the connection file changes nothing.
    assert_eq!(
        store.offer_workspace_jira_setting(&setting()).unwrap()["state"],
        "adopted"
    );
    assert_eq!(
        store.jira_project_source(&using).unwrap().unwrap()["source"]["projectKey"],
        "CR"
    );
    drop(store);
    let version: u32 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, crewroom_core::store::SCHEMA_VERSION);
}

#[test]
fn an_unused_workspace_setting_is_reported_never_refreshed_and_adopted_on_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("workflow.sqlite");
    let store = Store::open(&path).unwrap();
    let elsewhere = support::jira_project(&store, root.path(), "gamma", OTHER);
    // The connection file still carries the workspace setting; no project uses its site.
    let offered = store.offer_workspace_jira_setting(&setting()).unwrap();
    assert_eq!(offered["state"], "unused");
    assert!(store.legacy_jira_setting().unwrap().is_some());
    assert!(
        !crewroom_core::jira_projects::has_filters(
            &store.jira_project_source(&elsewhere).unwrap().unwrap()["source"]
        ),
        "a project on another site never takes it"
    );
    // A setting without a saved filter is not a refresh setting at all.
    assert_eq!(
        store
            .offer_workspace_jira_setting(
                &json!({"siteUrl":SITE,"cloudId":"00000000-0000-4000-8000-000000000022"})
            )
            .unwrap()["state"],
        "none"
    );
    // The next project that confirms its site adopts it.
    let project = support::jira_project(&store, root.path(), "alpha", SITE);
    let adopted = store.jira_project_source(&project).unwrap().unwrap();
    assert_eq!(adopted["adoptedFrom"], "workspace");
    assert_eq!(adopted["source"]["filterId"], "12345");
    assert_eq!(adopted["source"]["assignee"], "operator-1");
    assert_eq!(store.legacy_jira_setting().unwrap(), None);
    // A later project confirming the same site starts with its own, empty filters.
    let later = support::jira_project(&store, root.path(), "beta", SITE);
    assert!(!crewroom_core::jira_projects::has_filters(
        &store.jira_project_source(&later).unwrap().unwrap()["source"]
    ));
}
