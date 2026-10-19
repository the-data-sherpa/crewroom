//! Settings → Projects → Ticket source (FOR-47) through the real local
//! service and a Git checkout with synthetic remotes (no remote is contacted):
//! the suggestion is prefilled, one click confirms it, a moved remote shows a
//! warning without switching, queued work blocks a change with the blocker
//! named, and the Jira site is entered because it cannot be inferred.
use super::test_support::{Fixture, click, navigate, open, screen_text, wait};
use super::*;
use gpui_kit::{self as gpui, test::TestWindowExt};

fn git(checkout: &std::path::Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
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
fn source(v: &Workbench, id: &str) -> Value {
    records(&v.workspace, "repositories")
        .into_iter()
        .find(|r| r["id"] == id)
        .map(|r| r["projectSource"].clone())
        .unwrap_or(Value::Null)
}

#[gpui::test]
fn native_ticket_source_is_suggested_confirmed_warned_and_blocked(cx: &mut gpui::TestAppContext) {
    let f = Fixture::start();
    let checkout = std::fs::canonicalize(f.root.path())
        .unwrap()
        .join("checkout");
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
            "Seed",
        ],
    );
    git(
        &checkout,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:Octo-Fixture/Sandbox.git",
        ],
    );
    let (status, repository) = f.post(
        "/repositories",
        json!({"name":"Sandbox","path":checkout,"targetBranch":"main","checks":[]}),
    );
    assert_eq!(status, 200, "{repository}");
    let id = repository["id"].as_str().unwrap().to_owned();

    let (h, v, e) = open(cx, &f);
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        v.connected && source(v, &target)["state"] == "unconfirmed"
    });
    navigate(cx, h, "repositories");
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "Suggested from github.com/Octo-Fixture/Sandbox: GitHub Issues (Octo-Fixture/Sandbox) for tickets and GitHub for pull requests. Confirm to save."),
        "{text:?}"
    );
    // owner/repo is prefilled; nothing is saved until the operator confirms.
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).source_inputs[&id].repository.clone();
        assert_eq!(input.read(cx).value().as_ref(), "Octo-Fixture/Sandbox");
    })
    .unwrap();
    assert!(
        f.get("/workspace")["repositories"][0]
            .get("ticketSource")
            .is_none()
    );
    click(cx, h, format!("confirm-github-source-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && source(v, &target)["state"] == "confirmed"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter()
            .any(|l| l
                == "Ticket source: GitHub Issues Octo-Fixture/Sandbox · pull requests: GitHub."),
        "{text:?}"
    );

    // The remote moves to Bitbucket and queued work references the project.
    git(
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            "https://bitbucket.org/fixture/repository.git",
        ],
    );
    let (status, task) = f.post(
        "/tasks",
        json!({"title":"Queued work","description":"","repositoryId":id}),
    );
    assert_eq!(status, 200, "{task}");
    let task_id = task["id"].as_str().unwrap().to_owned();
    let (status, refined) = f.post(
        &format!("/tasks/{task_id}/refinement"),
        json!({"revision":1,"sourceRevision":1,"brief":"Do it","criteria":["Done"]}),
    );
    assert_eq!(status, 200, "{refined}");
    f.accept_plan(&task_id);
    let (status, run) = f.post(&format!("/tasks/{task_id}/runs"), json!({"revision":2}));
    assert_eq!(status, 200, "{run}");
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        source(v, &target)["state"] == "mismatch" && source(v, &target)["changeBlocked"].is_string()
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "Source no longer matches remote: it was confirmed for github.com/Octo-Fixture/Sandbox, and the repository now has bitbucket.org/fixture/repository. Nothing was changed; confirm a new source to switch."),
        "{text:?}"
    );
    assert!(
        text.iter().any(|l| l.starts_with(
            "Changing the ticket source is blocked while queued or open work references this repository:"
        )),
        "{text:?}"
    );
    // The change controls are disabled while the work is queued: activating
    // one sends nothing (an enabled one would draw the service's refusal).
    click(cx, h, format!("confirm-github-source-{id}"));
    let sent = cx.read(|cx| v.read(cx).busy);
    wait(cx, h, &v, &e, |v, _| !v.busy);
    let error = cx.read(|cx| v.read(cx).error.clone());
    assert!(
        !sent && error.is_empty(),
        "a blocked change was sent: {error}"
    );
    assert_eq!(
        f.get("/workspace")["repositories"][0]["ticketSource"]["tickets"],
        "github"
    );

    // Cancel the work; then the operator enters the Jira site and confirms.
    let (status, removed) = f.post(
        &format!("/queue/{}/remove", run["id"].as_str().unwrap()),
        json!({"requestId":uuid::Uuid::new_v4().to_string()}),
    );
    assert_eq!(status, 200, "{removed}");
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        source(v, &target)["changeBlocked"].is_null()
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).source_inputs[&id].site.clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input("https://fixture.atlassian.net", cx);
    })
    .unwrap();
    click(cx, h, format!("confirm-jira-source-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy && source(v, &target)["state"] == "confirmed"
    });
    let text = screen_text(cx, h);
    assert!(
        text.iter().any(|l| l
            == "Ticket source: Jira https://fixture.atlassian.net · pull requests: Bitbucket."),
        "{text:?}"
    );
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}
