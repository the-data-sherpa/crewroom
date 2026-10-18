use super::super::test_support::{Fixture, activate_by_keyboard, click, navigate, open, wait};
use super::*;
use gpui::test::TestWindowExt;
use gpui::{AnyWindowHandle, size};
use gpui_kit as gpui;
fn role_text(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<Workbench>,
    key: &str,
    text: &str,
) {
    cx.update_window(handle, |_, window, cx| {
        let form = &view.read(cx).settings.roles[&view.read(cx).settings.role];
        if let Some(input) = form.fields.inputs.get(key).cloned() {
            input.update(cx, |i, cx| i.focus(window, cx));
        } else {
            let input = form.fields.areas[key].clone();
            input.update(cx, |i, cx| i.focus(window, cx));
        }
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        if text.is_empty() {
            window.press("backspace", cx);
        } else {
            window.input(text, cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
}
#[gpui::test]
fn native_role_editor_persists_preserves_drafts_and_rejects_stale_revisions(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    navigate(cx, handle, "roles");
    click(cx, handle, "new-role");
    for (key, value) in [
        ("id", "performance-checker"),
        ("name", "Performance checker"),
        ("category", "Quality"),
        ("purpose", "Inspect regressions"),
        ("instructions", "Measure 日本語 before changing code."),
        ("model", "fixture/model"),
    ] {
        role_text(cx, handle, &view, key, value);
    }
    cx.update_window(handle, |_, window, cx| {
        let input = view.read(cx).settings.roles[&view.read(cx).settings.role]
            .fields
            .areas["instructions"]
            .clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.press("tab", cx);
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.input("fixture/keyboard", cx);
        let form = &view.read(cx).settings.roles[&view.read(cx).settings.role];
        assert_eq!(
            form.fields.text("instructions", cx),
            "Measure 日本語 before changing code."
        );
        assert_eq!(form.fields.text("model", cx), "fixture/keyboard");
        window.press("shift-tab", cx);
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.input("Measure 日本語 before changing code.", cx);
    })
    .unwrap();
    click(cx, handle, "effort-high");
    click(cx, handle, "save-role");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && v.settings.role == "performance-checker"
    });
    cx.read(|cx| {
        let d = &view.read(cx).settings.roles["performance-checker"];
        assert_eq!(d.revision, Some(1));
        assert!(!d.dirty(cx));
    });
    role_text(cx, handle, &view, "model", "fixture/draft");
    activate_by_keyboard(cx, handle, "edit-role-coordinator");
    activate_by_keyboard(cx, handle, "edit-role-performance-checker");
    navigate(cx, handle, "repositories");
    navigate(cx, handle, "roles");
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).settings.roles["performance-checker"]
                .fields
                .text("model", cx),
            "fixture/draft"
        )
    });
    let mut competing = cx.read(|cx| {
        view.read(cx).settings.roles["performance-checker"]
            .original
            .clone()
    });
    competing["model"] = json!("fixture/other");
    assert_eq!(fixture.post("/roles/performance-checker", competing).0, 200);
    wait(cx, handle, &view, &events, |v, _| {
        records(&v.workspace, "roles")
            .iter()
            .any(|r| r["id"] == "performance-checker" && r["revision"] == 2)
    });
    click(cx, handle, "save-role");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && v.error.contains("current revision is 2")
    });
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).settings.roles["performance-checker"]
                .fields
                .text("model", cx),
            "fixture/draft"
        )
    });
    click(cx, handle, "reload-role");
    click(cx, handle, "cancel");
    cx.read(|cx| assert!(view.read(cx).settings.roles["performance-checker"].dirty(cx)));
    click(cx, handle, "reload-role");
    click(cx, handle, "ok");
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).settings.roles["performance-checker"]
                .fields
                .text("model", cx),
            "fixture/other"
        )
    });
    click(cx, handle, "role-enabled");
    click(cx, handle, "save-role");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && v.settings.roles["performance-checker"].revision == Some(3)
    });
    let catalog = crewroom_core::roles::Roles::new(fixture.root.path().join("roles.json"))
        .read()
        .unwrap();
    let role = catalog
        .iter()
        .find(|r| r["id"] == "performance-checker")
        .unwrap();
    assert_eq!(role["enabled"], false);
    assert_eq!(role["model"], "fixture/other");
    assert_eq!(role["effort"], "high");
    // A fresh native app reads the role through a new authenticated HTTP/WS client.
    let mut reopened = cx.new_app();
    let (other_handle, other_view, other_events) = open(&mut reopened, &fixture);
    wait(
        &mut reopened,
        other_handle,
        &other_view,
        &other_events,
        |v, _| v.connected,
    );
    navigate(&mut reopened, other_handle, "roles");
    activate_by_keyboard(&mut reopened, other_handle, "edit-role-performance-checker");
    reopened.read(|cx| {
        let role = &other_view.read(cx).settings.roles["performance-checker"];
        assert_eq!(role.revision, Some(3));
        assert!(!role.enabled);
        assert_eq!(
            role.fields.text("instructions", cx),
            "Measure 日本語 before changing code."
        );
    });
}

fn repository_text(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<Workbench>,
    key: &str,
    text: &str,
) {
    cx.update_window(handle, |_, window, cx| {
        let input = view.read(cx).settings.repositories[&view.read(cx).settings.repository]
            .fields
            .inputs[key]
            .clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        if text.is_empty() {
            window.press("backspace", cx);
        } else {
            window.input(text, cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
}
#[gpui::test]
fn native_repository_checks_conflicts_and_retirement_use_the_real_service(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let checkout = fixture.root.path().join("checkout");
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
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    navigate(cx, handle, "repositories");
    repository_text(cx, handle, &view, "name", "Native checked checkout");
    repository_text(cx, handle, &view, "path", checkout.to_str().unwrap());
    repository_text(cx, handle, &view, "branchPrefix", "native/");
    click(cx, handle, "add-check");
    cx.update_window(handle, |_, window, cx| {
        for (key, text) in [
            ("id", "required"),
            ("image", &format!("sha256:{}", "a".repeat(64))),
            ("command", "/bin/sh"),
            ("timeoutMs", "12000"),
            ("memoryMiB", "256"),
        ] {
            let input = view.read(cx).settings.repositories[NEW].checks[0]
                .fields
                .inputs[key]
                .clone();
            input.update(cx, |i, cx| i.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input(text, cx);
        }
        let input = view.read(cx).settings.repositories[NEW].checks[0]
            .fields
            .areas["args"]
            .clone();
        input.update(cx, |i, cx| i.focus(window, cx));
        window.input("-c\nprintf 'native check'", cx);
        window.render_frame(cx);
    })
    .unwrap();
    click(cx, handle, "save-repository");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && v.settings.repository != NEW
    });
    let id = cx.read(|cx| view.read(cx).settings.repository.clone());
    let original = cx.read(|cx| view.read(cx).settings.repositories[&id].original.clone());
    assert_eq!(
        original["checks"][0]["args"],
        json!(["-c", "printf 'native check'"])
    );
    assert_eq!(original["checks"][0]["memoryMiB"], 256);
    assert_eq!(original["revision"], 1);
    repository_text(cx, handle, &view, "name", "Retained conflict draft");
    let mut competing = original.clone();
    competing["name"] = json!("Updated elsewhere");
    competing["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    assert_eq!(
        fixture.post(&format!("/repositories/{id}"), competing).0,
        200
    );
    wait(cx, handle, &view, &events, |v, _| {
        records(&v.workspace, "repositories")
            .iter()
            .any(|r| r["id"] == id && r["revision"] == 2)
    });
    click(cx, handle, "save-repository");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && !v.error.is_empty()
    });
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).settings.repositories[&id]
                .fields
                .text("name", cx),
            "Retained conflict draft"
        )
    });
    click(cx, handle, "reload-repository");
    click(cx, handle, "ok");
    cx.read(|cx| assert_eq!(view.read(cx).settings.repositories[&id].revision, Some(2)));
    repository_text(cx, handle, &view, "branchPrefix", "");
    cx.read(|cx| {
        assert!(
            view.read(cx).settings.repositories[&id]
                .fields
                .text("branchPrefix", cx)
                .is_empty()
        )
    });
    click(cx, handle, "save-repository");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && v.settings.repositories[&id].revision == Some(3)
    });
    let (status, task) = fixture.post(
        "/tasks",
        json!({"title":"Queued work","description":"","repositoryId":id}),
    );
    assert_eq!(status, 200);
    let task_id = string(&task, "id");
    assert_eq!(fixture.post(&format!("/tasks/{task_id}/refinement"),json!({"revision":1,"sourceRevision":1,"brief":"Keep work","criteria":["Retain history"]})).0,200);
    fixture.accept_plan(&task_id);
    let (status, run) = fixture.post(&format!("/tasks/{task_id}/runs"), json!({"revision":2}));
    assert_eq!(status, 200);
    click(
        cx,
        handle,
        SharedString::from(format!("retire-repository-{id}")),
    );
    click(cx, handle, "cancel");
    assert!(cx.read(|cx| view.read(cx).workspace["repositories"][0]["retiredAt"].is_null()));
    click(
        cx,
        handle,
        SharedString::from(format!("retire-repository-{id}")),
    );
    click(cx, handle, "ok");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && v.error.contains(task["key"].as_str().unwrap())
    });
    assert_eq!(
        fixture
            .post(
                &format!("/queue/{}/remove", run["id"].as_str().unwrap()),
                json!({"requestId":uuid::Uuid::new_v4().to_string()})
            )
            .0,
        200
    );
    click(
        cx,
        handle,
        SharedString::from(format!("retire-repository-{id}")),
    );
    click(cx, handle, "ok");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "repositories")
                .iter()
                .any(|r| r["id"] == id && !r["retiredAt"].is_null())
    });
    cx.read(|cx| {
        let repository = &view.read(cx).workspace["repositories"][0];
        assert_eq!(repository["checks"], original["checks"]);
        assert!(repository["branchPrefix"].is_null());
        assert_eq!(repository["name"], "Updated elsewhere");
    });
}

#[gpui::test]
fn narrow_configuration_navigation_keeps_drafts_and_reports_connection_truthfully(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    cx.simulate_window_resize(handle, size(px(390.), px(844.)));
    click(cx, handle, "open-navigation");
    navigate(cx, handle, "roles");
    click(cx, handle, "new-role");
    role_text(cx, handle, &view, "name", "A narrow draft");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for id in [
            "role-name",
            "save-role",
            "reload-role",
            "role-enabled",
            "close-role-editor",
        ] {
            let bounds = window.find(id).bounds();
            assert!(
                bounds.origin.x >= px(0.) && bounds.right() <= px(390.),
                "{id}: {bounds:?}"
            );
        }
    })
    .unwrap();
    click(cx, handle, "open-navigation");
    navigate(cx, handle, "repositories");
    repository_text(cx, handle, &view, "name", "Another retained draft");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for id in ["repository-name", "save-repository", "reload-repository"] {
            let bounds = window.find(id).bounds();
            assert!(
                bounds.origin.x >= px(0.) && bounds.right() <= px(390.),
                "{id}: {bounds:?}"
            );
        }
    })
    .unwrap();
    click(cx, handle, "open-navigation");
    navigate(cx, handle, "import-jira");
    cx.read(|cx| {
        let v = view.read(cx);
        assert_eq!(
            v.connection_summary(),
            "Jira not configured  ·  Runner not configured  ·  Service connected"
        );
        assert_eq!(
            v.settings.roles[&v.settings.role].fields.text("name", cx),
            "A narrow draft"
        );
        assert_eq!(
            v.settings.repositories[NEW].fields.text("name", cx),
            "Another retained draft"
        );
    });
    cx.update(|cx| {
        view.update(cx, |v, cx| {
            v.workspace["capabilities"] = json!({"liveJira":true,"execution":true});
            v.workspace["connections"] = json!({"jira":false});
            v.accept(Event::Connected(false), cx);
            assert!(v.blocked());
            assert_eq!(
                v.connection_summary(),
                "Jira sync incomplete  ·  Runner configured  ·  Retained state · reconnecting"
            );
        })
    });
    click(cx, handle, "open-navigation");
    navigate(cx, handle, "roles");
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).settings.roles[&view.read(cx).settings.role]
                .fields
                .text("name", cx),
            "A narrow draft"
        )
    });
}

#[gpui::test]
fn native_role_catalog_duplicates_and_previews_without_changing_saved_roles(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    let (_, task) = fixture.post(
        "/tasks",
        json!({"title":"Role preview ticket","description":"Saved input"}),
    );
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "roles");
    let rendered = |cx: &mut gpui::TestAppContext| {
        cx.update_window(h, |_, w, cx| {
            w.render_frame(cx);
            gpui::base::test_support::snapshots(w)
                .iter()
                .filter_map(|n| n.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap()
    };
    assert!(
        rendered(cx)
            .iter()
            .any(|s| s.contains("Required by the coding workflow"))
    );
    assert!(
        rendered(cx)
            .iter()
            .any(|s| s.contains("model resolution is not verified"))
    );
    click(cx, h, "duplicate-role-reviewer");
    let generated = cx.read(|cx| {
        v.read(cx).settings.roles[&v.read(cx).settings.role]
            .fields
            .text("id", cx)
    });
    assert!(generated.starts_with("specialist-"));
    cx.read(|cx| assert!(!v.read(cx).settings.unsaved(cx)));
    click(cx, h, "preview-role");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.settings.preview.is_some()
    });
    cx.read(|cx| {
        let p = v.read(cx).settings.preview.as_ref().unwrap();
        assert_eq!(p["task"]["id"], task["id"]);
        assert!(
            p["prompt"]
                .as_str()
                .unwrap()
                .contains("Role preview ticket")
        );
    });
    let before = fixture.get("/workspace");
    assert!(
        records(&before, "roles")
            .iter()
            .all(|r| r["id"] != generated)
    );
    role_text(
        cx,
        h,
        &v,
        "instructions",
        "A changed draft requires a refreshed preview.",
    );
    assert!(
        rendered(cx)
            .iter()
            .any(|s| s.contains("Preview is out of date"))
    );
    click(cx, h, "save-role");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.settings.role == generated
    });
    let after = fixture.get("/workspace");
    assert_eq!(
        records(&before, "roles")
            .iter()
            .find(|r| r["id"] == "reviewer"),
        records(&after, "roles")
            .iter()
            .find(|r| r["id"] == "reviewer")
    );
    assert_eq!(
        records(&after, "roles")
            .iter()
            .find(|r| r["id"] == generated)
            .unwrap()["revision"],
        1
    );
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let bounds = window
            .find(SharedString::from(format!(
                "preview-ticket-{}",
                task["id"].as_str().unwrap()
            )))
            .bounds();
        assert!(
            bounds.origin.x >= px(0.) && bounds.right() <= px(390.),
            "{bounds:?}"
        );
    })
    .unwrap();
    click(cx, h, "close-role-editor");
    activate_by_keyboard(cx, h, "edit-role-reviewer");
    assert!(
        rendered(cx)
            .iter()
            .any(|s| s.contains("Required by the coding workflow"))
    );
}

#[gpui::test]
fn model_job_settings_keep_drafts_conflicts_and_role_overrides(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    cx.update_window(h, |_, w, cx| {
        let input = v.read(cx).settings.models.as_ref().unwrap().fields.inputs["code"].clone();
        input.update(cx, |i, cx| i.focus(w, cx));
        w.input("fixture/code", cx);
    })
    .unwrap();
    click(cx, h, "budget-small");
    navigate(cx, h, "roles");
    click(cx, h, "edit-role-implementer");
    click(cx, h, "role-job-review");
    role_text(cx, h, &v, "model", "fixture/override");
    click(cx, h, "effort-off");
    click(cx, h, "save-role");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.settings.roles["implementer"].revision == Some(2)
    });
    navigate(cx, h, "models");
    cx.read(|cx| {
        let d = v.read(cx).settings.models.as_ref().unwrap();
        assert_eq!(d.budget, "small");
        assert_eq!(d.fields.text("code", cx), "fixture/code");
    });
    click(cx, h, "save-model-jobs");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["modelJobs"]["revision"] == 1
    });
    let current = fixture.get("/workspace");
    assert_eq!(current["modelJobs"]["budget"], "small");
    let role = records(&current, "roles")
        .into_iter()
        .find(|r| r["id"] == "implementer")
        .unwrap();
    assert_eq!(role["job"], "review");
    assert_eq!(role["model"], "fixture/override");
    assert_eq!(role["effort"], "off");
    let mut changed = current["modelJobs"].clone();
    changed["budget"] = json!("large");
    assert_eq!(fixture.post("/model-jobs", changed).0, 200);
    click(cx, h, "budget-medium");
    click(cx, h, "save-model-jobs");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.error.contains("Reload before saving")
    });
    cx.read(|cx| {
        assert_eq!(
            v.read(cx).settings.models.as_ref().unwrap().budget,
            "medium"
        )
    });
    click(cx, h, "reload-model-jobs");
    click(cx, h, "ok");
    wait(cx, h, &v, &e, |v, _| {
        v.settings
            .models
            .as_ref()
            .is_some_and(|d| d.budget == "large")
    });
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        for id in [
            "job-code",
            "budget-small",
            "save-model-jobs",
            "reload-model-jobs",
        ] {
            let b = w.find(id).bounds();
            assert!(b.origin.x >= px(0.) && b.right() <= px(390.), "{id}: {b:?}");
        }
    })
    .unwrap();
}

#[gpui::test]
fn principle_catalog_attachment_preview_and_referenced_delete(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (_, task) = fixture.post(
        "/tasks",
        json!({"title":"Principle preview","description":"Saved"}),
    );
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "principles");
    click(cx, h, "new-principle");
    for (key, value) in [
        ("name", "Evidence first"),
        ("text", "Cite the exact tested revision."),
    ] {
        cx.update_window(h, |_, w, cx| {
            let d = &v.read(cx).settings.principles[&v.read(cx).settings.principle];
            if let Some(input) = d.fields.inputs.get(key).cloned() {
                input.update(cx, |i, cx| i.focus(w, cx));
            } else {
                let input = d.fields.areas[key].clone();
                input.update(cx, |i, cx| i.focus(w, cx));
            }
            w.input(value, cx);
        })
        .unwrap();
    }
    click(cx, h, "save-principle");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "principles").len() == 1
    });
    let pid = cx.read(|cx| v.read(cx).settings.principle.clone());
    navigate(cx, h, "roles");
    click(cx, h, "edit-role-reviewer");
    click(cx, h, SharedString::from(format!("role-principle-{pid}")));
    click(cx, h, "save-role");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.settings.roles["reviewer"].revision == Some(2)
    });
    click(cx, h, "preview-role");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.settings.preview.is_some()
    });
    cx.read(|cx| {
        let p = v.read(cx).settings.preview.as_ref().unwrap();
        assert_eq!(p["task"]["id"], task["id"]);
        assert!(
            p["prompt"]
                .as_str()
                .unwrap()
                .contains("Cite the exact tested revision.")
        );
    });
    let mut principle = fixture.get("/workspace")["principles"][0].clone();
    principle["text"] = json!("Updated principle text.");
    assert_eq!(fixture.post("/principles", principle).0, 200);
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["principles"][0]["revision"] == 2
    });
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        assert!(gpui::base::test_support::snapshots(w).iter().any(|n| {
            n.label()
                .is_some_and(|s| s.contains("Preview is out of date"))
        }));
    })
    .unwrap();
    navigate(cx, h, "principles");
    click(cx, h, "delete-principle");
    click(cx, h, "ok");
    wait(cx, h, &v, &e, |v, _| !v.busy && v.error.contains("Detach"));
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        for id in ["principle-name", "save-principle", "delete-principle"] {
            let b = w.find(id).bounds();
            assert!(b.origin.x >= px(0.) && b.right() <= px(390.), "{id}: {b:?}");
        }
    })
    .unwrap();
}

#[gpui::test]
fn operator_role_proposal_history_and_manual_drafting_fallback(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "roles");
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        assert!(gpui::base::test_support::snapshots(w).iter().any(|n| {
            n.label()
                .is_some_and(|s| s.contains("Runner drafting is unavailable"))
        }));
    })
    .unwrap();
    click(cx, h, "edit-role-reviewer");
    click(cx, h, "role-teach");
    for (key, value) in [
        ("change", "Name the exact tested revision."),
        ("rationale", "Operator observed ambiguous evidence."),
    ] {
        cx.update_window(h, |_, w, cx| {
            let input = v.read(cx).collaboration.forms["role-learning-reviewer-teach"]
                .fields
                .areas[key]
                .clone();
            input.update(cx, |i, cx| i.focus(w, cx));
            w.input(value, cx);
        })
        .unwrap();
    }
    click(cx, h, "propose-role-learning");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "learning").len() == 1
    });
    let proposal = fixture.get("/workspace")["learning"][0].clone();
    assert_eq!(proposal["origin"], "operator");
    assert!(proposal["runId"].is_null());
    let pid = string(&proposal, "id");
    let path = format!("/learning/{pid}/decision");
    cx.update_window(h, |_, w, cx| {
        let input = v.read(cx).collaboration.forms[&path].fields.areas["reason"].clone();
        input.update(cx, |i, cx| i.focus(w, cx));
        w.input("Reviewed the exact guidance.", cx);
    })
    .unwrap();
    click(cx, h, SharedString::from(format!("approve-learning-{pid}")));
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && records(&v.workspace, "roles")
                .iter()
                .any(|r| r["id"] == "reviewer" && r["revision"] == 2)
    });
    navigate(cx, h, "roles");
    click(cx, h, "edit-role-reviewer");
    click(cx, h, "role-history");
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        let labels: Vec<_> = gpui::base::test_support::snapshots(w)
            .iter()
            .filter_map(|n| n.label().map(str::to_owned))
            .collect();
        assert!(labels.iter().any(|s| s.contains("Revision 2 · learning")));
        assert!(labels.iter().any(|s| s.contains(&pid)));
    })
    .unwrap();
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        for id in ["role-ask", "role-teach", "role-history"] {
            let b = w.find(id).bounds();
            assert!(b.origin.x >= px(0.) && b.right() <= px(390.), "{id}: {b:?}");
        }
    })
    .unwrap();
}

#[gpui::test]
fn playbook_catalog_keeps_drafts_and_revision_conflicts(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "playbooks");
    click(cx, h, "new-playbook");
    for (key, value) in [
        ("name", "Investigate"),
        ("description", "For uncertain behavior"),
        ("steps", "Inspect evidence\nExplain limits"),
    ] {
        cx.update_window(h, |_, w, cx| {
            let f = &v.read(cx).settings.playbooks[&v.read(cx).settings.playbook].fields;
            if let Some(i) = f.inputs.get(key).cloned() {
                i.update(cx, |i, cx| i.focus(w, cx));
            } else {
                let i = f.areas[key].clone();
                i.update(cx, |i, cx| i.focus(w, cx));
            }
            w.input(value, cx);
        })
        .unwrap();
    }
    click(cx, h, "save-playbook");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "playbooks").len() == 1
    });
    let mut other = fixture.get("/workspace")["playbooks"][0].clone();
    other["name"] = json!("Concurrent edit");
    assert_eq!(fixture.post("/playbooks", other).0, 200);
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["playbooks"][0]["revision"] == 2
    });
    click(cx, h, "save-playbook");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.error.contains("Playbook changed")
    });
    click(cx, h, "reload-playbook");
    click(cx, h, "ok");
    cx.simulate_window_resize(h, size(px(390.), px(844.)));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        assert_eq!(
            v.read(cx).settings.playbooks[&v.read(cx).settings.playbook]
                .fields
                .text("name", cx),
            "Concurrent edit"
        );
        for id in ["playbook-name", "save-playbook", "reload-playbook"] {
            let b = w.find(id).bounds();
            assert!(b.origin.x >= px(0.) && b.right() <= px(390.), "{id}: {b:?}");
        }
    })
    .unwrap();
}

#[gpui::test]
fn native_instruction_editors_compare_exact_drafts_and_keep_ordinary_save_available(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::start();
    assert_eq!(fixture.post("/principles", json!({"id":"comparison-principle","name":"Verification","text":"Current guidance","revision":0})).0,200);
    assert_eq!(fixture.post("/playbooks", json!({"id":"comparison-playbook","name":"Verification","description":"Current checklist","steps":[{"id":"step-1","title":"Check"}],"revision":0})).0,200);
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    // Capture request payloads without connecting to any evaluation runner.
    cx.update_window(handle, |_, _, cx| {
        view.update(cx, |view, _| view.client = None)
    })
    .unwrap();
    for (kind, page, id, field, compare, save) in [
        (
            "role",
            "roles",
            "implementer",
            "instructions",
            "compare-role",
            "save-role",
        ),
        (
            "principle",
            "principles",
            "comparison-principle",
            "text",
            "compare-principle",
            "save-principle",
        ),
        (
            "playbook",
            "playbooks",
            "comparison-playbook",
            "description",
            "compare-playbook",
            "save-playbook",
        ),
    ] {
        navigate(cx, handle, page);
        activate_by_keyboard(cx, handle, SharedString::from(format!("edit-{kind}-{id}")));
        cx.update_window(handle, |_, window, cx| {
            let area = match kind {
                "role" => view.read(cx).settings.roles[id].fields.areas[field].clone(),
                "principle" => view.read(cx).settings.principles[id].fields.areas[field].clone(),
                _ => view.read(cx).settings.playbooks[id].fields.areas[field].clone(),
            };
            area.update(cx, |area, cx| {
                area.set_value("Exact proposed instructions 日本語", window, cx)
            });
            window.render_frame(cx);
            assert!(
                window
                    .try_find(SharedString::from(save.to_owned()))
                    .is_some()
            );
        })
        .unwrap();
        activate_by_keyboard(cx, handle, compare);
        cx.read(|cx| {
            let view = view.read(cx);
            let request = &view.pending_keys["/comparisons/preflight"].1;
            assert_eq!(request["kind"], kind);
            assert_eq!(request["targetId"], id);
            assert_eq!(
                request["draft"][field],
                "Exact proposed instructions 日本語"
            );
            assert!(
                !view.pending_keys.contains_key("/comparisons"),
                "Editing or preflight must not start a comparison"
            );
        });
        click(cx, handle, "close-comparison");
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let draft = view
                .read(cx)
                .settings
                .comparison_draft(kind, id, cx)
                .unwrap();
            assert_eq!(draft[field], "Exact proposed instructions 日本語");
            assert!(
                window
                    .try_find(SharedString::from(save.to_owned()))
                    .is_some()
            );
        })
        .unwrap();
    }
}

fn labels(cx: &mut gpui::TestAppContext, handle: AnyWindowHandle) -> Vec<String> {
    cx.update_window(handle, |_, w, cx| {
        w.render_frame(cx);
        gpui::base::test_support::snapshots(w)
            .iter()
            .filter_map(|n| n.label().map(str::to_owned))
            .collect()
    })
    .unwrap()
}

fn type_into(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    fields: impl Fn(&Workbench) -> &Fields,
    view: &Entity<Workbench>,
    key: &str,
    text: &str,
) {
    cx.update_window(handle, |_, w, cx| {
        let f = fields(view.read(cx));
        if let Some(input) = f.inputs.get(key).cloned() {
            input.update(cx, |i, cx| i.focus(w, cx));
        } else {
            let input = f.areas[key].clone();
            input.update(cx, |i, cx| i.focus(w, cx));
        }
        w.input(text, cx);
        w.render_frame(cx);
    })
    .unwrap();
}

#[gpui::test]
fn unsaved_new_role_drafts_stay_reachable_from_the_catalog(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "roles");
    click(cx, h, "new-role");
    let key = cx.read(|cx| v.read(cx).settings.role.clone());
    role_text(cx, h, &v, "name", "Draft reviewer");
    activate_by_keyboard(cx, h, "edit-role-coordinator");
    click(cx, h, "close-role-editor");
    assert!(
        labels(cx, h)
            .iter()
            .any(|s| s == "Unsaved draft · Draft reviewer"),
        "{:?}",
        labels(cx, h)
    );
    click(cx, h, SharedString::from(format!("open-role-draft-{key}")));
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert!(s.role_open);
        assert_eq!(s.role, key);
        assert_eq!(s.roles[&key].fields.text("name", cx), "Draft reviewer");
    });
    click(cx, h, "new-role");
    let blank = cx.read(|cx| v.read(cx).settings.role.clone());
    assert!(
        labels(cx, h)
            .iter()
            .any(|s| s == "Unsaved draft · Untitled")
    );
    click(
        cx,
        h,
        SharedString::from(format!("discard-role-draft-{key}")),
    );
    click(cx, h, "cancel");
    cx.read(|cx| assert!(v.read(cx).settings.roles.contains_key(&key)));
    click(
        cx,
        h,
        SharedString::from(format!("discard-role-draft-{key}")),
    );
    click(cx, h, "ok");
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert!(!s.roles.contains_key(&key));
        assert_eq!(s.role, blank, "Discarding another draft keeps the open one");
    });
    assert!(
        !labels(cx, h)
            .iter()
            .any(|s| s == "Unsaved draft · Draft reviewer")
    );
}

#[gpui::test]
fn untouched_duplicate_role_is_not_unsaved_work(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "roles");
    click(cx, h, "duplicate-role-reviewer");
    cx.read(|cx| assert!(!v.read(cx).unsaved(cx)));
    click(cx, h, "close-role-editor");
    let key = cx.read(|cx| v.read(cx).settings.role.clone());
    let name = cx.read(|cx| v.read(cx).settings.roles[&key].fields.text("name", cx));
    assert!(
        labels(cx, h)
            .iter()
            .any(|s| *s == format!("Unsaved draft · {name}"))
    );
    click(cx, h, SharedString::from(format!("open-role-draft-{key}")));
    role_text(cx, h, &v, "purpose", "Edited copy");
    cx.read(|cx| assert!(v.read(cx).unsaved(cx)));
}

#[gpui::test]
fn discarding_a_new_role_restores_creation_defaults(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "roles");
    click(cx, h, "new-role");
    role_text(cx, h, &v, "category", "Research");
    click(cx, h, "role-job-code");
    assert!(labels(cx, h).iter().any(|s| s == "Discard new role"));
    assert!(!labels(cx, h).iter().any(|s| s == "Reload saved role"));
    click(cx, h, "reload-role");
    click(cx, h, "ok");
    cx.update_window(h, |_, w, cx| w.render_frame(cx)).unwrap();
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        let draft = &s.roles[&s.role];
        assert!(s.role.starts_with(NEW));
        assert_eq!(draft.fields.text("category", cx), "Quality");
        assert_eq!(draft.job, json!("judgment"));
        assert!(draft.fields.text("id", cx).starts_with("specialist-"));
        assert!(!draft.dirty(cx));
    });
}

#[gpui::test]
fn unsaved_new_principle_and_playbook_drafts_stay_reachable(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    for (path, body) in [
        (
            "/principles",
            json!({"id":"existing-principle","name":"Existing","text":"Saved","revision":0}),
        ),
        (
            "/playbooks",
            json!({"id":"existing-playbook","name":"Existing","description":"Saved","steps":[{"id":"step-1","title":"Check"}],"revision":0}),
        ),
    ] {
        assert_eq!(fixture.post(path, body).0, 200);
    }
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && records(&v.workspace, "playbooks").len() == 1
    });
    navigate(cx, h, "principles");
    click(cx, h, "new-principle");
    let principle = cx.read(|cx| v.read(cx).settings.principle.clone());
    type_into(
        cx,
        h,
        |w| &w.settings.principles[&w.settings.principle].fields,
        &v,
        "name",
        "Draft guidance",
    );
    click(cx, h, "edit-principle-existing-principle");
    assert!(
        labels(cx, h)
            .iter()
            .any(|s| s == "Unsaved draft · Draft guidance")
    );
    click(
        cx,
        h,
        SharedString::from(format!("open-principle-draft-{principle}")),
    );
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert_eq!(s.principle, principle);
        assert_eq!(
            s.principles[&principle].fields.text("name", cx),
            "Draft guidance"
        );
    });
    click(
        cx,
        h,
        SharedString::from(format!("discard-principle-draft-{principle}")),
    );
    click(cx, h, "ok");
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert!(!s.principles.contains_key(&principle));
        assert!(s.principle.is_empty());
    });

    navigate(cx, h, "playbooks");
    click(cx, h, "new-playbook");
    let playbook = cx.read(|cx| v.read(cx).settings.playbook.clone());
    type_into(
        cx,
        h,
        |w| &w.settings.playbooks[&w.settings.playbook].fields,
        &v,
        "name",
        "Draft checklist",
    );
    click(cx, h, "edit-playbook-existing-playbook");
    assert!(
        labels(cx, h)
            .iter()
            .any(|s| s == "Unsaved draft · Draft checklist")
    );
    click(
        cx,
        h,
        SharedString::from(format!("open-playbook-draft-{playbook}")),
    );
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert_eq!(s.playbook, playbook);
        assert_eq!(
            s.playbooks[&playbook].fields.text("name", cx),
            "Draft checklist"
        );
    });
    click(
        cx,
        h,
        SharedString::from(format!("discard-playbook-draft-{playbook}")),
    );
    click(cx, h, "ok");
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert!(!s.playbooks.contains_key(&playbook));
        assert!(s.playbook.is_empty());
    });
}

#[gpui::test]
fn native_repository_save_error_is_visible_at_the_save_row(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let checkout = fixture.root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .arg(&checkout)
            .output()
            .unwrap()
            .status
            .success()
    );
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    navigate(cx, handle, "repositories");
    repository_text(cx, handle, &view, "name", "Digest fixture");
    repository_text(cx, handle, &view, "path", checkout.to_str().unwrap());
    click(cx, handle, "add-check");
    cx.update_window(handle, |_, window, cx| {
        for (key, text) in [
            ("id", "required"),
            ("image", "not-a-digest"),
            ("command", "/bin/sh"),
            ("timeoutMs", "12000"),
        ] {
            let input = view.read(cx).settings.repositories[NEW].checks[0]
                .fields
                .inputs[key]
                .clone();
            input.update(cx, |i, cx| i.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input(text, cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
    click(cx, handle, "save-repository");
    wait(cx, handle, &view, &events, |v, _| {
        !v.busy && !v.error.is_empty()
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("feedback-error-top").is_none());
        let error = window.find("feedback-error-save-row").bounds();
        let save = window.find("save-repository").bounds();
        assert!(
            error.origin.y >= px(0.) && error.bottom() <= window.viewport_size().height,
            "The error must be visible without scrolling: {error:?}"
        );
        assert!(
            (save.origin.y - error.bottom()).abs() < px(160.),
            "The error must render beside Save: {error:?} / {save:?}"
        );
    })
    .unwrap();
}

fn git_checkout(path: &std::path::Path) {
    std::fs::create_dir_all(path).unwrap();
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
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}
fn check_text(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<Workbench>,
    index: usize,
    values: &[(&str, &str)],
) {
    cx.update_window(handle, |_, window, cx| {
        for (key, text) in values {
            let input = view.read(cx).settings.repositories[NEW].checks[index]
                .fields
                .inputs[*key]
                .clone();
            input.update(cx, |i, cx| i.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input(text, cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
}
fn save_error(
    cx: &mut gpui::TestAppContext,
    handle: AnyWindowHandle,
    view: &Entity<Workbench>,
    events: &async_channel::Receiver<Event>,
) -> String {
    click(cx, handle, "save-repository");
    wait(cx, handle, view, events, |v, _| {
        !v.busy && !v.error.is_empty()
    });
    cx.read(|cx| view.read(cx).error.clone())
}

#[gpui::test]
fn repository_check_errors_name_their_check_and_field(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let checkout = fixture.root.path().join("checkout");
    git_checkout(&checkout);
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    navigate(cx, handle, "repositories");
    repository_text(cx, handle, &view, "name", "Two checks");
    repository_text(cx, handle, &view, "path", checkout.to_str().unwrap());
    click(cx, handle, "add-check");
    click(cx, handle, "add-check");
    assert!(
        labels(cx, handle)
            .iter()
            .any(|s| s == "Image digest · sha256: followed by 64 lowercase hex characters")
    );
    let digest = format!("sha256:{}", "a".repeat(64));
    check_text(
        cx,
        handle,
        &view,
        0,
        &[
            ("id", "unit"),
            ("image", &digest),
            ("command", "/bin/true"),
            ("timeoutMs", "50"),
        ],
    );
    check_text(
        cx,
        handle,
        &view,
        1,
        &[
            ("id", "lint"),
            ("image", "alpine:3.20"),
            ("command", "/bin/true"),
        ],
    );
    let error = save_error(cx, handle, &view, &events);
    assert_eq!(error, "Check 1 (unit): Timeout must be 100–3,600,000 ms.");
    check_text(cx, handle, &view, 0, &[("timeoutMs", "12000")]);
    let error = save_error(cx, handle, &view, &events);
    assert_eq!(
        error,
        "Check 2 (lint): Image digest must be sha256: followed by 64 lowercase hex characters."
    );
    check_text(cx, handle, &view, 1, &[("timeoutMs", "soon")]);
    let error = save_error(cx, handle, &view, &events);
    assert_eq!(error, "Check 2 (lint): Timeout must be a whole number.");
}

#[gpui::test]
fn repository_path_and_branch_errors_are_distinct(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    let checkout = fixture.root.path().join("checkout");
    git_checkout(&checkout);
    let plain = fixture.root.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let (handle, view, events) = open(cx, &fixture);
    wait(cx, handle, &view, &events, |v, _| v.connected);
    navigate(cx, handle, "repositories");
    repository_text(cx, handle, &view, "name", "Path or branch");
    repository_text(cx, handle, &view, "path", plain.to_str().unwrap());
    let path_error = save_error(cx, handle, &view, &events);
    assert_eq!(path_error, "Checkout path is not a Git repository root.");
    repository_text(cx, handle, &view, "path", checkout.to_str().unwrap());
    repository_text(cx, handle, &view, "targetBranch", "release");
    let branch_error = save_error(cx, handle, &view, &events);
    assert_eq!(branch_error, "Target branch 'release' was not found.");
}

#[gpui::test]
fn deleting_an_unreferenced_principle_closes_its_editor(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::start();
    assert_eq!(
        fixture
            .post(
                "/principles",
                json!({"id":"unused-principle","name":"Unused","text":"Delete me","revision":0}),
            )
            .0,
        200
    );
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && records(&v.workspace, "principles").len() == 1
    });
    navigate(cx, h, "principles");
    click(cx, h, "edit-principle-unused-principle");
    click(cx, h, "delete-principle");
    click(cx, h, "ok");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "principles").is_empty()
    });
    cx.read(|cx| {
        let s = &v.read(cx).settings;
        assert!(s.principle.is_empty());
        assert!(!s.principles.contains_key("unused-principle"));
        assert!(!s.unsaved(cx));
    });
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("save-principle").is_none());
    })
    .unwrap();
}
