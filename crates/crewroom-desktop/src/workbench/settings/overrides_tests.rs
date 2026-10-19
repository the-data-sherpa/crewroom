//! FOR-79: per-project overrides against the real local service (GPUI test
//! platform; no window on the operator's desktop).
use super::super::super::test_support::{Fixture, click, navigate, screen_text, wait};
use super::*;
use gpui::AnyWindowHandle;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;

fn checkout(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = std::fs::canonicalize(root).unwrap().join(name);
    std::fs::create_dir_all(&path).unwrap();
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
                .arg(&path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    path
}
fn fixture() -> (Fixture, String, String) {
    let fixture = Fixture::seeded(false, |store, root| {
        for name in ["Alpha", "Beta"] {
            store
                .create_repository(&json!({"name":name,"path":checkout(root, name),"targetBranch":"main","checks":[]}))
                .unwrap();
        }
    });
    let repos = fixture.get("/workspace")["repositories"].clone();
    let id = |name: &str| {
        repos
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (alpha, beta) = (id("Alpha"), id("Beta"));
    (fixture, alpha, beta)
}
fn shows(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, text: &str) -> bool {
    screen_text(cx, h).iter().any(|t| t.contains(text))
}
fn overrides(fixture: &Fixture, project: &str) -> Value {
    fixture.get("/workspace")["projectSettings"]["projects"][project].clone()
}

#[gpui::test]
fn a_project_overrides_models_with_its_source_chain_and_reset(cx: &mut gpui::TestAppContext) {
    let (fixture, alpha, _beta) = fixture();
    let (h, v, e) = super::super::super::test_support::open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    assert!(shows(cx, h, "Showing All projects"));
    // Never saved: the budget comes from the built-in default.
    assert!(shows(cx, h, "Budget: large"));
    assert!(shows(cx, h, "Source: Built-in default"));
    assert!(shows(cx, h, "No project overrides this"));

    click(cx, h, format!("settings-scope-{alpha}"));
    assert!(shows(cx, h, "Showing Alpha"));
    click(cx, h, "scope-budget-small");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["projectSettings"]["projects"][&alpha]["models.budget"] == "small"
    });
    assert_eq!(overrides(&fixture, &alpha)["models.budget"], "small");
    assert!(shows(cx, h, "Budget: small"));
    assert!(shows(cx, h, "Source: Project override"));
    click(cx, h, "chain-models-budget");
    let text = screen_text(cx, h);
    for line in [
        "Built-in default: large",
        "Workspace: not set · inherits",
        "Project (Alpha): small · in effect",
    ] {
        assert!(text.iter().any(|t| t == line), "{line}: {text:?}");
    }
    // The workspace editor is read-only: the cover takes the click, and a
    // direct workspace mutation is refused without reaching the service.
    click(cx, h, "save-model-jobs");
    cx.run_until_parked();
    assert_eq!(fixture.get("/workspace")["modelJobs"]["revision"], 0);
    assert!(shows(
        cx,
        h,
        "Workspace settings · read-only while Alpha is selected"
    ));
    cx.update(|cx| {
        v.update(cx, |v, cx| {
            v.send("/model-jobs".into(), json!({"budget":"small"}), false, cx)
        })
    });
    cx.read(|cx| {
        assert!(
            v.read(cx)
                .error
                .contains("read-only while Alpha is selected")
        )
    });
    assert_eq!(fixture.get("/workspace")["modelJobs"]["revision"], 0);

    // All projects counts the override and links to the project.
    click(cx, h, "settings-scope-all");
    assert!(shows(cx, h, "Overridden in 1 project"));
    assert!(shows(cx, h, "Budget: large"));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("workspace-read-only-cover").is_none());
    })
    .unwrap();
    click(cx, h, format!("override-models-budget-{alpha}"));
    cx.read(|cx| assert_eq!(v.read(cx).settings.scope.as_deref(), Some(alpha.as_str())));
    click(cx, h, "reset-models-budget");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["projectSettings"]["projects"][&alpha]
                .get("models.budget")
                .is_none()
    });
    assert!(shows(cx, h, "Source: Built-in default"));
}

#[gpui::test]
fn roles_instructions_principles_and_capacity_override_per_project(cx: &mut gpui::TestAppContext) {
    let (fixture, alpha, beta) = fixture();
    assert_eq!(
        fixture
            .post(
                "/principles",
                json!({"id":"small-changes","name":"Small changes","text":"Prefer small changes.","revision":0})
            )
            .0,
        200
    );
    let (h, v, e) = super::super::super::test_support::open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);

    navigate(cx, h, "roles");
    click(cx, h, format!("settings-scope-{alpha}"));
    click(cx, h, "scope-role-reviewer");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["projectSettings"]["projects"][&alpha]["roles.reviewer"] == false
    });
    let reviewer = records(&fixture.get("/workspace"), "roles")
        .into_iter()
        .find(|r| r["id"] == "reviewer")
        .unwrap();
    assert_eq!(
        reviewer["enabled"], true,
        "The workspace role is unchanged."
    );

    // Workspace instructions apply to every project; Alpha overrides them.
    navigate(cx, h, "principles");
    click(cx, h, "settings-scope-all");
    let type_text = |cx: &mut gpui::TestAppContext, scope: Option<&str>, text: &str| {
        cx.update_window(h, |_, window, cx| {
            let key = format!("{}:instructions", scope.unwrap_or(""));
            let area = v.read(cx).settings.scope_drafts.fields[&key].areas["value"].clone();
            area.update(cx, |a, cx| a.focus(window, cx));
            window.press(crate::SELECT_ALL_SHORTCUT, cx);
            window.input(text, cx);
            window.render_frame(cx);
        })
        .unwrap();
    };
    type_text(cx, None, "Run the full suite.");
    click(cx, h, "set-instructions");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["projectSettings"]["workspace"]["instructions"] == "Run the full suite."
    });
    click(cx, h, "scope-principle-small-changes");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["projectSettings"]["workspace"]["principles"] == json!(["small-changes"])
    });
    click(cx, h, format!("settings-scope-{alpha}"));
    assert!(shows(cx, h, "Source: Workspace"));
    type_text(cx, Some(&alpha), "Use the alpha fixtures.");
    click(cx, h, "set-instructions");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["projectSettings"]["projects"][&alpha]["instructions"]
                == "Use the alpha fixtures."
    });
    // A workspace change never alters the override.
    click(cx, h, "settings-scope-all");
    type_text(cx, None, "Changed for everyone.");
    click(cx, h, "set-instructions");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["projectSettings"]["workspace"]["instructions"]
                == "Changed for everyone."
    });
    assert_eq!(
        overrides(&fixture, &alpha)["instructions"],
        "Use the alpha fixtures."
    );
    assert!(overrides(&fixture, &beta).is_null());
    // The principle catalog is workspace-only and read-only for a project.
    click(cx, h, format!("settings-scope-{alpha}"));
    click(cx, h, "new-principle");
    cx.read(|cx| assert!(v.read(cx).settings.principle.is_empty()));

    // Capacity: the project overrides; the workspace ceiling is read-only.
    navigate(cx, h, "repositories");
    click(cx, h, "scope-capacity-up");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["projectSettings"]["projects"][&alpha]["capacity"] == 2
    });
    assert!(shows(cx, h, "Run capacity: 2 parallel runs"));
    click(cx, h, "ceiling-set");
    cx.run_until_parked();
    assert!(fixture.get("/workspace")["queue"]["capacity"]["workspaceCeiling"].is_null());
    let capacity = |id: &str| {
        records(&fixture.get("/workspace")["queue"]["capacity"], "projects")
            .into_iter()
            .find(|p| p["repositoryId"] == id)
            .unwrap()["capacity"]
            .clone()
    };
    assert_eq!(capacity(&alpha), 2);
    assert_eq!(capacity(&beta), 1);
}
