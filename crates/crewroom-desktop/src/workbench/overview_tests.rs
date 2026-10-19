//! The Overview home screen (FOR-77) over an injected workspace projection: the four sections and
//! their empty states, Needs you order and actions, the no-progress flag and its thresholds,
//! Landed revisions and Next reasons. No service, provider or agent process is involved.
use super::acceptance_tests::labels;
use super::*;
use gpui_kit::{self as gpui, AnyWindowHandle, test::TestWindowExt};
use gpui_kit::{Bounds, WindowBounds, WindowOptions, point, size};

fn open(cx: &mut gpui::TestAppContext, workspace: Value) -> (AnyWindowHandle, Entity<Workbench>) {
    cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(4000.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.workspace = workspace;
                    view
                })
            },
        )
        .unwrap()
    })
}

fn render(cx: &mut gpui::TestAppContext, h: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(h, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn present(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> bool {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(SharedString::from(id.to_owned())).is_some()
    })
    .unwrap()
}

fn press(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) {
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        window.click(SharedString::from(id.to_owned()), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn one(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, id: &str) -> Vec<String> {
    labels(cx, h, Some(id))
}

/// An RFC 3339 time `minutes` (and ten seconds) before now.
fn ago(minutes: i64) -> String {
    let ms = chrono::Utc::now().timestamp_millis() - minutes * 60_000 - 10_000;
    crewroom_core::timestamp(ms)
}

fn task(id: &str, project: &str) -> Value {
    json!({"id":id,"key":format!("SIM-{id}"),"source":"local","title":format!("Ticket {id}"),"brief":"","criteria":[],"revision":1,"sourceRevision":1,"repositoryId":project})
}

fn projects() -> Value {
    json!([
        {"id":"alpha","name":"Alpha app","path":"/synthetic/alpha","targetBranch":"main","retiredAt":null,
         "projectSource":{"state":"confirmed","saved":{"tickets":"github"}}},
        {"id":"beta","name":"Beta","path":"/synthetic/beta","targetBranch":"main","retiredAt":null,
         "projectSource":{"state":"mismatch"}}
    ])
}

#[gpui::test]
fn the_home_screen_renders_every_section_with_an_empty_state(cx: &mut gpui::TestAppContext) {
    let workspace = json!({
        "repositories":[projects()[0].clone()],
        "tasks":[task("one","alpha")],
        "runs":[],"roles":[],"activity":[],
        "needsYou":[],"landed":[],"runProgress":{},
        "queue":{"plans":[]},"capabilities":{"execution":true}
    });
    let (h, view) = open(cx, workspace);
    render(cx, h);
    // With no ticket chosen, the home screen is the Overview and its sidebar entry is current.
    assert_eq!(one(cx, h, "overview-title"), ["Overview"]);
    assert_eq!(
        view.read_with(cx, |v, _| v.sidebar_current()).as_deref(),
        Some("overview")
    );
    for (slug, title) in [
        ("needs", "Needs you · 0"),
        ("running", "Running · 0"),
        ("landed", "Landed · 0"),
        ("next", "Next · 0"),
    ] {
        assert_eq!(one(cx, h, &format!("overview-{slug}-heading")), [title]);
        assert!(present(cx, h, &format!("overview-{slug}-empty")), "{slug}");
    }
    assert_eq!(
        one(cx, h, "overview-running-empty"),
        ["No run is executing. Queued plans are not executing agents; they are listed under Next."]
    );
    // Opening a ticket and returning through the sidebar entry shows the same screen.
    view.update(cx, |v, cx| {
        v.selected = Some("one".into());
        cx.notify();
    });
    render(cx, h);
    assert!(!present(cx, h, "overview-title"));
    press(cx, h, "overview");
    assert!(view.read_with(cx, |v, _| v.panel == Panel::Overview));
    assert_eq!(one(cx, h, "overview-title"), ["Overview"]);
}

#[gpui::test]
fn without_the_execution_layer_the_overview_says_what_it_cannot_list(
    cx: &mut gpui::TestAppContext,
) {
    let workspace = json!({
        "repositories":[projects()[0].clone()],
        "tasks":[task("one","alpha")],
        "runs":[],"roles":[]
    });
    let (h, view) = open(cx, workspace);
    view.update(cx, |v, cx| {
        v.panel = Panel::Overview;
        cx.notify();
    });
    render(cx, h);
    assert!(present(cx, h, "overview-needs-unavailable"));
    assert!(present(cx, h, "overview-needs-empty"));
    assert!(present(cx, h, "overview-landed-unavailable"));
    assert_eq!(one(cx, h, "overview-landed-heading"), ["Landed"]);
    assert!(present(cx, h, "overview-running-empty"));
    assert!(present(cx, h, "overview-next-empty"));
}

#[gpui::test]
fn needs_you_lists_only_actionable_items_highest_impact_first(cx: &mut gpui::TestAppContext) {
    let workspace = json!({
        "repositories":projects(),
        "tasks":[task("one","alpha"),task("two","beta"),task("three","alpha"),task("quiet","alpha")],
        "runs":[
            {"id":"run-one","taskId":"one","purpose":"coding","status":"running"},
            {"id":"run-two","taskId":"two","purpose":"coding","status":"attention_required","attentionReason":"Source conflict: the ticket changed."},
            {"id":"run-quiet","taskId":"quiet","purpose":"coding","status":"running"}
        ],
        "runProgress":{"run-one":{"lastAt":ago(2),"source":"agent output"},"run-quiet":{"lastAt":ago(1),"source":"commit"}},
        "roles":[],"activity":[],"landed":[],
        "needsYou":[
            {"id":"learning-l","kind":"learning","title":"Coordinator proposes a change to project guidance","taskId":"one","runId":"run-one","learningId":"l"},
            {"id":"feedback-run-one","kind":"feedback","title":"2 review comments on PR #7","taskId":"one","runId":"run-one"},
            {"id":"publication-pr-run-one","kind":"publication","title":"PR outcome is uncertain","taskId":"one","runId":"run-one","available":false}
        ],
        "intake":{"three":{"state":"Plan review"},"one":{"state":"Scoping"}},
        "github":{"state":"rejected"},
        "queue":{"plans":[]},"capabilities":{"execution":true}
    });
    let (h, view) = open(cx, workspace);
    render(cx, h);
    let order: Vec<_> = view.read_with(cx, |v, _| {
        v.overview_needs().into_iter().map(|i| i.id).collect()
    });
    // A run that needs attention first, then unconfirmed delivery and connections, then
    // conflicts, plan decisions, PR feedback and proposals. Running work with recent progress
    // and a scoping intake are not items.
    assert_eq!(
        order,
        [
            "attention-run-two",
            "publication-pr-run-one",
            "credentials-github",
            "conflict-beta",
            "plan-three",
            "feedback-run-one",
            "learning-l",
        ]
    );
    assert_eq!(one(cx, h, "overview-needs-heading"), ["Needs you · 7"]);
    // Every item is labelled with its project and has its action.
    assert_eq!(
        one(cx, h, "overview-needs-place-attention-run-two"),
        ["Beta · SIM-two"]
    );
    assert_eq!(
        one(cx, h, "overview-needs-place-plan-three"),
        ["Alpha app · SIM-three"]
    );
    assert_eq!(
        one(cx, h, "overview-needs-place-credentials-github"),
        ["All projects · GitHub"]
    );
    for (id, action) in [
        ("attention-run-two", "Open session"),
        ("publication-pr-run-one", "Reconcile publication"),
        ("credentials-github", "Reconnect GitHub"),
        ("conflict-beta", "Open project settings"),
        ("plan-three", "Review plan"),
        ("feedback-run-one", "Resume run"),
        ("learning-l", "Review proposal"),
    ] {
        let button = format!("overview-needs-action-{id}");
        let label = cx
            .update_window(h, |_, window, _| {
                window
                    .find(SharedString::from(button.clone()))
                    .label()
                    .map(str::to_owned)
            })
            .unwrap();
        assert_eq!(label.as_deref(), Some(action), "{id}");
    }
    // An unavailable reconciliation says why instead of pretending it can run.
    assert!(
        one(cx, h, "overview-needs-publication-pr-run-one")
            .iter()
            .any(|l| l.starts_with("PR publication is unavailable"))
    );
    // Actions lead where the decision is made.
    press(cx, h, "overview-needs-action-plan-three");
    view.read_with(cx, |v, _| {
        assert!(v.panel == Panel::Ticket);
        assert_eq!(v.selected.as_deref(), Some("three"));
        assert_eq!(v.tab, "Brief");
    });
    view.update(cx, |v, cx| {
        v.panel = Panel::Overview;
        cx.notify();
    });
    press(cx, h, "overview-needs-action-conflict-beta");
    view.read_with(cx, |v, _| {
        assert!(v.panel == Panel::Repositories);
        assert_eq!(v.settings.repository, "beta");
    });
}

#[gpui::test]
fn a_quiet_run_is_flagged_then_promoted_without_claiming_failure(cx: &mut gpui::TestAppContext) {
    let workspace = json!({
        "repositories":projects(),
        "tasks":[task("fresh","alpha"),task("quiet","alpha"),task("silent","beta"),task("paused","alpha")],
        "runs":[
            {"id":"run-fresh","taskId":"fresh","purpose":"coding","status":"running",
             "launch":{"team":[{"name":"Lead","coordinator":true},{"name":"Reviewer","coordinator":false},{"name":"Tester","coordinator":false}]}},
            {"id":"run-quiet","taskId":"quiet","purpose":"coding","status":"running"},
            {"id":"run-silent","taskId":"silent","purpose":"coding","status":"running"},
            {"id":"run-paused","taskId":"paused","purpose":"coding","status":"paused"}
        ],
        "runProgress":{
            "run-fresh":{"lastAt":ago(3),"source":"agent output"},
            "run-quiet":{"lastAt":ago(30),"source":"commit"},
            "run-silent":{"lastAt":ago(90),"source":"run start"},
            "run-paused":{"lastAt":ago(300),"source":"agent output"}
        },
        "activity":[{"runId":"run-fresh","assignmentName":"Lead","state":"executing"}],
        "roles":[],"needsYou":[],"landed":[],
        "queue":{"plans":[]},"capabilities":{"execution":true}
    });
    let (h, view) = open(cx, workspace);
    render(cx, h);
    // Only executing runs are lines in Running; a paused run is not executing.
    assert_eq!(one(cx, h, "overview-running-heading"), ["Running · 3"]);
    assert!(!present(cx, h, "overview-running-run-paused"));
    assert_eq!(
        one(cx, h, "overview-running-place-run-fresh"),
        ["Alpha app · SIM-fresh"]
    );
    assert_eq!(
        one(cx, h, "overview-running-team-run-fresh"),
        ["Coordinator Lead · specialists Reviewer, Tester"]
    );
    assert_eq!(
        one(cx, h, "overview-running-last-run-fresh"),
        ["Last progress 3 min ago · agent output"]
    );
    // Workflow state, agent activity and the flag are separate facts.
    assert_eq!(one(cx, h, "overview-running-stage-run-fresh").len(), 1);
    assert_eq!(one(cx, h, "overview-running-activity-run-fresh").len(), 1);
    assert!(!present(cx, h, "overview-running-stale-run-fresh"));
    assert_eq!(
        one(cx, h, "overview-running-stale-run-quiet"),
        ["No progress for 30 min"]
    );
    assert_eq!(
        one(cx, h, "overview-running-stale-run-silent"),
        ["No progress for 90 min"]
    );
    for id in ["run-quiet", "run-silent"] {
        assert!(
            one(cx, h, &format!("overview-running-{id}"))
                .iter()
                .all(|l| !l.to_lowercase().contains("fail")),
            "{id}"
        );
    }
    // Only the longer threshold lists a run under Needs you, and it says it has not failed.
    let needs: Vec<_> = view.read_with(cx, |v, _| {
        v.overview_needs().into_iter().map(|i| i.id).collect()
    });
    assert_eq!(needs, ["conflict-beta", "no-progress-run-silent"]);
    assert_eq!(
        one(cx, h, "overview-needs-title-no-progress-run-silent"),
        ["No progress for 90 min"]
    );
    assert!(
        one(cx, h, "overview-needs-no-progress-run-silent")
            .iter()
            .any(|l| l.contains("This does not mean the run failed"))
    );
    assert_eq!(
        one(cx, h, "overview-thresholds"),
        ["Flag no progress after 20 min · list under Needs you after 60 min"]
    );
    // The thresholds are configurable: flag later than the quiet run's 30 minutes, and list
    // later than the silent run's 90 minutes.
    for _ in 0..3 {
        press(cx, h, "overview-stale-more");
    }
    for _ in 0..3 {
        press(cx, h, "overview-promote-more");
    }
    assert_eq!(
        one(cx, h, "overview-thresholds"),
        ["Flag no progress after 35 min · list under Needs you after 105 min"]
    );
    assert!(!present(cx, h, "overview-running-stale-run-quiet"));
    assert!(present(cx, h, "overview-running-stale-run-silent"));
    assert!(!present(cx, h, "overview-needs-no-progress-run-silent"));
}

#[gpui::test]
fn the_local_service_projects_progress_and_landed_work_for_the_overview(
    cx: &mut gpui::TestAppContext,
) {
    use super::test_support::{Fixture, open as connect, wait};
    let mut run = Value::Null;
    let f = Fixture::projected(|s, root| {
        let (_, r, _) = super::acceptance_tests::coding_plan(s, root);
        run = r;
    });
    let (h, v, e) = connect(cx, &f);
    // The real service projection carries the Overview's inputs: per-run progress for
    // unfinished runs and the (empty) landed list.
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["landed"].is_array() && v.workspace["runProgress"].is_object()
    });
    let id = run["id"].as_str().unwrap().to_owned();
    v.read_with(cx, |v, _| {
        // A queued plan has not started, so no progress has been observed yet.
        assert_eq!(
            v.workspace["runProgress"][&id],
            json!({"lastAt":null,"source":null})
        );
    });
    render(cx, h);
    assert_eq!(one(cx, h, "overview-landed-heading"), ["Landed · 0"]);
    assert!(present(cx, h, "overview-landed-empty"));
    assert!(present(cx, h, "overview-running-empty"));
    assert_eq!(one(cx, h, "overview-next-heading"), ["Next · 1"]);
    let key = v.read_with(cx, |v, _| string(&v.workspace["tasks"][0], "key"));
    assert_eq!(
        one(cx, h, &format!("overview-next-place-{id}")),
        [format!("Original captured repository · {key}")]
    );
    assert!(one(cx, h, &format!("overview-next-reason-{id}"))[0].starts_with("#1 in queue · "));
    drop(f);
}

#[gpui::test]
fn landed_names_the_covered_revision_and_next_names_the_reason(cx: &mut gpui::TestAppContext) {
    let workspace = json!({
        "repositories":projects(),
        "tasks":[task("merged","alpha"),task("open","beta"),task("study","alpha"),task("bare","alpha"),task("first","alpha"),task("second","beta")],
        "runs":[
            {"id":"run-first","taskId":"first","purpose":"coding","status":"awaiting_runner","position":0},
            {"id":"run-second","taskId":"second","purpose":"coding","status":"awaiting_runner","position":1}
        ],
        "landed":[
            {"kind":"pr","runId":"run-merged","taskId":"merged","pr":7,"merged":true,"providerState":"MERGED","revision":"0123456789abcdef0123"},
            {"kind":"pr","runId":"run-open","taskId":"open","pr":8,"merged":false,"providerState":"OPEN","revision":"fedcba9876543210"},
            {"kind":"research","runId":"run-study","taskId":"study","researchRevision":3},
            {"kind":"pr","runId":"run-bare","taskId":"bare","pr":9,"merged":false,"providerState":"OPEN","revision":null}
        ],
        "queue":{"plans":[
            {"id":"run-first","holdReason":"Waiting for SIM-x to reach ready for review (running)"},
            {"id":"run-second"}
        ]},
        "roles":[],"needsYou":[],"runProgress":{},"activity":[],"capabilities":{"execution":true}
    });
    let (h, _) = open(cx, workspace);
    render(cx, h);
    assert_eq!(one(cx, h, "overview-landed-heading"), ["Landed · 4"]);
    assert_eq!(
        one(cx, h, "overview-landed-place-run-merged"),
        ["Alpha app · SIM-merged"]
    );
    assert_eq!(
        one(cx, h, "overview-landed-what-run-merged"),
        ["PR #7 · merged"]
    );
    assert_eq!(
        one(cx, h, "overview-landed-revision-run-merged"),
        ["Evidence covers revision 0123456789ab"]
    );
    assert_eq!(
        one(cx, h, "overview-landed-what-run-open"),
        ["PR #8 · published · open"]
    );
    assert_eq!(
        one(cx, h, "overview-landed-revision-run-study"),
        ["Evidence covers research revision 3"]
    );
    // A missing revision is said, never guessed.
    assert_eq!(
        one(cx, h, "overview-landed-revision-run-bare"),
        ["Evidence revision not recorded"]
    );
    assert_eq!(one(cx, h, "overview-next-heading"), ["Next · 2"]);
    assert_eq!(
        one(cx, h, "overview-next-reason-run-first"),
        ["#1 in queue · Waiting for SIM-x to reach ready for review (running)"]
    );
    assert_eq!(
        one(cx, h, "overview-next-reason-run-second"),
        ["#2 in queue · Eligible · scheduling"]
    );
    assert_eq!(
        one(cx, h, "overview-next-place-run-second"),
        ["Beta · SIM-second"]
    );
}
