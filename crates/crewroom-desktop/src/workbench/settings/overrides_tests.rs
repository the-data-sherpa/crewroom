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

/// FOR-88: a project's job model uses the workspace catalog picker; a value
/// saved before the picker stays selected and is saved back verbatim.
#[gpui::test]
fn a_project_job_model_uses_the_catalog_picker_and_keeps_saved_text(cx: &mut gpui::TestAppContext) {
    let (fixture, alpha, _beta) = fixture();
    let saved = "pre-picker/model text";
    assert_eq!(
        fixture
            .post(
                "/project-settings",
                json!({"repositoryId":alpha,"set":{"models.jobs.code":saved,"models.budget":"small"}})
            )
            .0,
        200
    );
    let (h, v, e) = super::super::super::test_support::open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    click(cx, h, format!("settings-scope-{alpha}"));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        assert!(w.try_find("scope-job-code").is_some());
        // The plain text field is gone.
        assert!(w.try_find("scope-models-jobs-code-value").is_none());
    })
    .unwrap();
    cx.read(|cx| {
        let pickers = &v.read(cx).settings.pickers;
        let picker = &pickers["scope-job-code"];
        assert_eq!(picker.selected, saved);
        let kept = picker.choices.iter().find(|c| c.key == saved).unwrap();
        assert_eq!(kept.label, format!("Legacy selection: {saved}"));
        assert!(kept.blocked, "kept as saved, never offered as a new choice");
        // The workspace editor keeps its own picker.
        assert!(pickers.contains_key("job-code"));
        // A job the project does not override starts from what it inherits.
        assert_eq!(pickers["scope-job-review"].selected, "");
    });
    // The project's budget sets the effort shown with its pickers.
    let effort = crewroom_core::model_jobs::effort("small", "judgment").unwrap();
    assert!(shows(cx, h, &format!("Effective effort: {effort}")));
    click(cx, h, "set-models-jobs-code");
    wait(cx, h, &v, &e, |v, _| !v.busy);
    assert_eq!(overrides(&fixture, &alpha)["models.jobs.code"], saved);
    assert_eq!(fixture.get("/workspace")["modelJobs"]["revision"], 0);
}

/// FOR-88: with a project selected, a catalog model is chosen for its job,
/// and the roles page shows what each role resolves to there, including the
/// model and effort sources, with a project-aware preview.
#[gpui::test]
fn a_project_model_choice_shows_in_its_role_facts_and_preview(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::configured(
        |store, root| {
            let repo = store
                .create_repository(&json!({"name":"Alpha","path":checkout(root, "Alpha"),"targetBranch":"main","checks":[]}))
                .unwrap();
            store
                .create_task(&json!({"title":"Alpha preview ticket","description":"","repositoryId":repo["id"]}))
                .unwrap();
        },
        super::super::model_picker::tests::configured,
    );
    let alpha = fixture.get("/workspace")["repositories"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (h, v, e) = super::super::super::test_support::open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    click(cx, h, "refresh-model-catalog");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["modelDiscovery"]["state"] == "partial"
    });
    click(cx, h, format!("settings-scope-{alpha}"));
    super::super::super::test_support::activate_by_keyboard(cx, h, "scope-job-code");
    cx.update_window(h, |_, w, cx| w.input("omp / fixture / second", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(h, |_, w, cx| {
        w.press("down", cx);
        w.press("enter", cx);
    })
    .unwrap();
    let key = "offering:omp:fixture:second:unknown";
    let draft = format!("{alpha}:models.jobs.code");
    wait(cx, h, &v, &e, |v, cx| {
        v.settings.scope_drafts.fields[&draft].text("value", cx) == key
    });
    // The chosen model's facts appear with the project's picker.
    assert!(shows(cx, h, "Eligible · effort"));
    click(cx, h, "set-models-jobs-code");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["projectSettings"]["projects"][&alpha]["models.jobs.code"] == key
    });
    assert_eq!(fixture.get("/workspace")["modelJobs"]["revision"], 0);

    navigate(cx, h, "roles");
    let view = fixture.get("/workspace");
    let scoped = &view["roleCatalog"]["implementer"]["projects"][&alpha]["model"];
    assert_eq!(scoped["projectOverrides"]["jobs"]["code"], key);
    let resolved = &scoped["resolved"];
    assert!(shows(
        cx,
        h,
        &format!(
            "Model in Alpha: {} · effort {}",
            string(resolved, "selector"),
            string(resolved, "effort")
        )
    ));
    assert!(shows(
        cx,
        h,
        "Model source: Project override · code job model"
    ));
    // The workspace resolution is unchanged.
    assert_ne!(
        view["roleCatalog"]["implementer"]["model"]["selection"]["intendedModel"],
        key
    );

    click(cx, h, "scope-preview-refresh");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.settings
                .preview
                .as_ref()
                .is_some_and(|p| p["repositoryId"] == alpha.as_str())
    });
    cx.read(|cx| {
        let preview = v.read(cx).settings.preview.clone().unwrap();
        let model = &preview["assignment"]["model"];
        assert_eq!(model["projectOverrides"]["jobs"]["code"], key);
        assert_eq!(model["selection"]["intendedModel"], key);
        assert_eq!(model["selection"]["modelSource"], "job");
    });
    assert!(shows(cx, h, "Read-only preview of the current inputs."));
    // Resetting the override makes the preview stale until refreshed.
    navigate(cx, h, "models");
    click(cx, h, "reset-models-jobs-code");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["projectSettings"]["projects"][&alpha]
                .get("models.jobs.code")
                .is_none()
    });
    navigate(cx, h, "roles");
    assert!(shows(
        cx,
        h,
        "Preview is out of date. Refresh it to include current inputs."
    ));
    assert!(!shows(
        cx,
        h,
        "Model source: Project override · code job model"
    ));
}
