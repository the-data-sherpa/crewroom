//! The project sidebar (FOR-75). Grouping, persistence and the empty state run over an injected
//! workspace projection; New ticket and Start from a project run through the real local service.
//! No provider or agent process is involved.
use super::acceptance_tests::labels;
use super::test_support::{Fixture, click, open, wait};
use super::*;
use gpui_kit::{self as gpui, AnyWindowHandle, test::TestWindowExt};
use gpui_kit::{Bounds, WindowBounds, WindowOptions, point, size};

pub(super) fn open_injected(
    cx: &mut gpui::TestAppContext,
    workspace: Value,
    descriptor: Option<&std::path::Path>,
) -> (AnyWindowHandle, Entity<Workbench>) {
    let descriptor = descriptor.map(std::path::Path::to_path_buf);
    cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(2400.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    // A restart reads the same private navigation file.
                    view.navigation = navigation::Navigation::new(descriptor.as_deref());
                    view.connected = true;
                    view.workspace = workspace;
                    view
                })
            },
        )
        .unwrap()
    })
}

pub(super) fn render(cx: &mut gpui::TestAppContext, h: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

pub(super) fn present(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> bool {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(SharedString::from(id.to_owned())).is_some()
    })
    .unwrap()
}

pub(super) fn press(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.click(SharedString::from(id.to_owned()), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

pub(super) fn top(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> gpui::Pixels {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window
            .find(SharedString::from(id.to_owned()))
            .bounds()
            .origin
            .y
    })
    .unwrap()
}

pub(super) fn task(id: &str, project: Option<&str>) -> Value {
    let mut t = json!({"id":id,"key":format!("SIM-{id}"),"source":"local","title":format!("Ticket {id}"),"brief":"","criteria":[],"revision":1,"sourceRevision":1});
    if let Some(project) = project {
        t["repositoryId"] = json!(project);
    }
    t
}

pub(super) fn workspace() -> Value {
    json!({
        "repositories":[
            {"id":"alpha","name":"Alpha app","path":"/synthetic/alpha","targetBranch":"main","retiredAt":null,
             "projectSource":{"state":"confirmed","label":"Ticket source: GitHub Issues fixture/alpha.","saved":{"tickets":"github","repository":"fixture/alpha"}}},
            {"id":"beta","name":"beta","path":"/synthetic/beta","targetBranch":"main","retiredAt":null},
            {"id":"old","name":"Old tool","path":"/synthetic/old","targetBranch":"main","retiredAt":"2026-10-01T00:00:00Z"}
        ],
        "tasks":[
            task("a-other", Some("alpha")),
            task("a-running", Some("alpha")),
            task("a-queued", Some("alpha")),
            task("a-needs", Some("alpha")),
            task("b-other", Some("beta")),
            task("loose", None),
            task("retired", Some("old"))
        ],
        "runs":[
            {"id":"run-running","taskId":"a-running","status":"executing","purpose":"coding"},
            {"id":"run-queued","taskId":"a-queued","status":"awaiting_runner","position":0,"purpose":"coding"},
            {"id":"run-needs","taskId":"a-needs","status":"executing","purpose":"coding"}
        ],
        "activity":[{"runId":"run-running","state":"running","assignmentName":"Coordinator"}],
        "roles":[],
        "needsYou":[{"id":"decision","kind":"repair","title":"Repair limit","taskId":"a-needs","runId":"run-needs"}],
        "capabilities":{"execution":true}
    })
}

#[gpui::test]
fn sidebar_groups_tickets_by_project_in_four_sections_with_unassigned_last(
    cx: &mut gpui::TestAppContext,
) {
    let (h, view) = open_injected(cx, workspace(), None);
    render(cx, h);
    // Each project lists its tickets as Needs you, Running, Queued and Other, in that order;
    // tickets without an active project follow, and Unassigned comes last.
    let order = [
        "project-name-alpha",
        "project-alpha-needs-you",
        "row-key-a-needs",
        "project-alpha-running",
        "row-key-a-running",
        "project-alpha-queued",
        "row-key-a-queued",
        "project-alpha-other",
        "row-key-a-other",
        "project-name-beta",
        "row-key-b-other",
        "project-name-old",
        "row-key-retired",
        "project-name-unassigned",
        "row-key-loose",
    ];
    let tops: Vec<_> = order.iter().map(|id| top(cx, h, id)).collect();
    assert!(
        tops.windows(2).all(|w| w[0] < w[1]),
        "Sidebar order {order:?} at {tops:?}"
    );
    let one = |cx: &mut gpui::TestAppContext, id: &str| labels(cx, h, Some(id));
    assert_eq!(one(cx, "project-alpha-needs-you"), ["Needs you · 1"]);
    assert_eq!(one(cx, "project-alpha-running"), ["Running · 1"]);
    // The running indicator counts running workflow, including work that needs the operator.
    assert_eq!(one(cx, "project-running-alpha"), ["● 2 running"]);
    assert_eq!(one(cx, "project-running-beta"), ["Nothing running"]);
    assert_eq!(one(cx, "project-monogram-alpha"), ["AA"]);
    assert_eq!(one(cx, "project-monogram-beta"), ["BE"]);
    assert_eq!(one(cx, "project-source-alpha"), ["GitHub Issues"]);
    // A project the service has not described yet says so instead of claiming a source.
    assert_eq!(
        one(cx, "project-source-beta"),
        ["Ticket source unavailable"]
    );
    assert_eq!(one(cx, "project-name-old"), ["Old tool · Retired"]);
    // Source status, workflow state and agent activity stay separate on every row.
    for id in [
        "a-needs",
        "a-running",
        "a-queued",
        "a-other",
        "loose",
        "retired",
    ] {
        for kind in ["source", "workflow", "activity"] {
            assert_eq!(
                one(cx, &format!("row-{kind}-{id}")).len(),
                1,
                "{kind} for {id}"
            );
        }
    }
    assert_eq!(one(cx, "row-source-a-running"), ["Not linked"]);
    assert_eq!(one(cx, "row-workflow-a-queued"), ["Queued"]);
    assert_eq!(one(cx, "row-activity-a-queued"), ["No agent processes"]);
    // Header actions act in their project: Import only where a source is confirmed, and no
    // actions on a retired project or on Unassigned.
    for (id, shown) in [
        ("project-new-alpha", true),
        ("project-import-alpha", true),
        ("project-new-beta", true),
        ("project-import-beta", false),
        ("project-new-old", false),
        ("project-new-unassigned", false),
        ("start-a-other", true),
        ("start-b-other", true),
        ("start-a-running", false),
        ("start-a-queued", false),
        ("start-retired", false),
        ("start-loose", false),
        ("choose-project-loose", true),
    ] {
        assert_eq!(present(cx, h, id), shown, "{id}");
    }
    // An unassigned ticket opens its project picker, and selecting a project's ticket makes
    // that project current.
    press(cx, h, "choose-project-loose");
    assert!(present(cx, h, "assign-alpha") && present(cx, h, "assign-beta"));
    view.read_with(cx, |v, _| {
        assert!(v.panel == Panel::Ticket && v.selected.as_deref() == Some("loose"));
        assert_eq!(v.tab, "Brief");
        assert_eq!(v.current_project(), None);
    });
    press(cx, h, "ticket-b-other");
    view.read_with(cx, |v, _| {
        assert_eq!(v.current_project().as_deref(), Some("beta"))
    });
}

#[gpui::test]
fn collapse_order_and_current_project_survive_a_restart(cx: &mut gpui::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let descriptor = root.path().join("native-connection.json");
    let (h, view) = open_injected(cx, workspace(), Some(&descriptor));
    render(cx, h);
    assert!(top(cx, h, "project-name-alpha") < top(cx, h, "project-name-beta"));
    press(cx, h, "project-down-alpha");
    press(cx, h, "project-collapse-alpha");
    press(cx, h, "project-collapse-unassigned");
    press(cx, h, "project-name-beta");
    assert!(top(cx, h, "project-name-beta") < top(cx, h, "project-name-alpha"));
    for hidden in ["row-key-a-other", "row-key-a-running", "row-key-loose"] {
        assert!(!present(cx, h, hidden), "{hidden} shown while collapsed");
    }
    assert!(present(cx, h, "row-key-b-other"));
    // A collapsed project still shows whether anything is running.
    assert_eq!(
        labels(cx, h, Some("project-running-alpha")),
        ["● 2 running"]
    );
    view.read_with(cx, |v, _| {
        assert_eq!(v.current_project().as_deref(), Some("beta"))
    });
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();

    // Reopening reads the same private file: order, collapse and the current project return.
    let (h, view) = open_injected(cx, workspace(), Some(&descriptor));
    render(cx, h);
    assert!(top(cx, h, "project-name-beta") < top(cx, h, "project-name-alpha"));
    assert!(!present(cx, h, "row-key-a-other"));
    assert!(!present(cx, h, "row-key-loose"));
    view.read_with(cx, |v, _| {
        assert_eq!(v.current_project().as_deref(), Some("beta"));
        assert_eq!(
            v.project_prefs().collapsed,
            ["alpha".to_owned(), navigation::UNASSIGNED.to_owned()]
        );
    });
    // New ticket in the sidebar footer targets the current project.
    press(cx, h, "new-ticket");
    view.read_with(cx, |v, _| {
        assert!(v.panel == Panel::New);
        assert_eq!(v.new_project.as_deref(), Some("beta"));
    });
    assert_eq!(
        labels(cx, h, Some("new-ticket-project")),
        ["Creates the ticket in beta."]
    );
    // Expanding again is remembered too.
    press(cx, h, "project-collapse-alpha");
    assert!(present(cx, h, "row-key-a-other"));
    let saved = navigation::Navigation::new(Some(&descriptor));
    assert_eq!(
        saved.saved().projects.collapsed,
        [navigation::UNASSIGNED.to_owned()]
    );
    // A retired or removed current project is not offered as current.
    view.update(cx, |v, cx| {
        v.workspace["repositories"][1]["retiredAt"] = json!("2026-10-19T00:00:00Z");
        cx.notify();
    });
    view.read_with(cx, |v, _| assert_eq!(v.current_project(), None));
}

#[gpui::test]
fn empty_workspace_offers_add_a_project_through_the_folder_flow(cx: &mut gpui::TestAppContext) {
    let (h, view) = open_injected(
        cx,
        json!({"tasks":[],"runs":[],"roles":[],"repositories":[],"needsYou":[]}),
        None,
    );
    render(cx, h);
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let labels: Vec<String> = gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect();
        assert!(labels.iter().any(|l| l == "What should we work on?"));
        assert_eq!(
            window.find("empty-add-project").label(),
            Some("Add a project")
        );
        assert!(window.try_find("sidebar-add-project").is_some());
        assert_eq!(
            window.find("projects-empty").label(),
            Some("No projects yet.")
        );
        window.click("empty-add-project", cx);
        window.render_frame(cx);
        assert!(view.read(cx).panel == Panel::Repositories);
        // The FOR-70 flow starts from the folder picker, in Settings → Projects.
        assert!(window.try_find("choose-repository-folder").is_some());
        assert_eq!(window.find("repositories").label(), Some("Projects"));
    })
    .unwrap();
}

fn checkout(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let checkout = std::fs::canonicalize(root).unwrap().join(name);
    std::fs::create_dir(&checkout).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "Seed",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    checkout
}

#[gpui::test]
fn new_ticket_and_start_from_a_project_act_in_that_project(cx: &mut gpui::TestAppContext) {
    let (mut first, mut second) = (String::new(), String::new());
    let f = Fixture::seeded(false, |s, root| {
        for (name, slot) in [("First", &mut first), ("Second", &mut second)] {
            let path = checkout(root, &name.to_lowercase());
            *slot = s
                .create_repository(
                    &json!({"name":name,"path":path,"targetBranch":"main","checks":[]}),
                )
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
        }
    });
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && records(&v.workspace, "repositories").len() == 2
    });
    // New ticket in the second project's header creates the ticket there.
    click(cx, h, format!("project-new-{second}"));
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let input = v.read(cx).title.clone();
        input.update(cx, |i, cx| i.set_value("Project ticket", window, cx));
    })
    .unwrap();
    click(cx, h, "create-ticket");
    wait(cx, h, &v, &e, |v, _| {
        records(&v.workspace, "tasks")
            .iter()
            .any(|t| t["title"] == "Project ticket")
    });
    let task = records(&f.get("/workspace"), "tasks")
        .into_iter()
        .find(|t| t["title"] == "Project ticket")
        .unwrap();
    assert_eq!(task["repositoryId"], json!(second));
    let id = string(&task, "id");
    assert!(
        labels(cx, h, Some(&format!("project-{second}")))
            .iter()
            .any(|l| l == "Project ticket"),
        "listed under its project"
    );
    v.read_with(cx, |v, _| {
        assert_eq!(v.current_project().as_deref(), Some(second.as_str()))
    });

    // Start queues the ticket's coordinator in its project. Without an accepted plan the
    // coordinator scopes it; a queued request is not an executing agent.
    click(cx, h, format!("start-{id}"));
    let target = id.clone();
    wait(cx, h, &v, &e, move |v, _| {
        !v.busy
            && records(&v.workspace, "runs")
                .iter()
                .any(|r| r["taskId"] == target)
    });
    let run = records(&f.get("/workspace"), "runs")
        .into_iter()
        .find(|r| r["taskId"] == id)
        .unwrap();
    assert_eq!(run["taskSnapshot"]["repositoryId"], json!(second));
    assert_eq!(
        run["purpose"], "grill",
        "no accepted plan: the coordinator scopes it"
    );
    assert!(
        !present(cx, h, &format!("start-{id}")),
        "Start is gone once queued"
    );
    let workflow = labels(cx, h, Some(&format!("row-workflow-{id}")));
    assert_eq!(workflow.len(), 1);
    assert_ne!(workflow[0], "Refining");
    assert_eq!(
        labels(cx, h, Some(&format!("row-activity-{id}"))),
        ["No agent processes"]
    );
    // The first project is untouched.
    assert!(
        !records(&f.get("/workspace"), "tasks")
            .iter()
            .any(|t| t["repositoryId"] == json!(first))
    );
    cx.update_window(h, |_, window, _| window.remove_window())
        .unwrap();
}
