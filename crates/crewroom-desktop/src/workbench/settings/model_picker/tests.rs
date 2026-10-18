use super::*;
use crate::workbench::test_support::*;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;
use std::os::unix::fs::PermissionsExt;

fn configured(root: &std::path::Path) -> crewroom_service::ServiceOptions {
    let binary = root.join("models.py");
    if !binary.exists() {
        std::fs::write(&binary,r#"#!/usr/bin/python3
import sys,json,time
if '--version' in sys.argv:
 print('omp/18.4.12');sys.exit()
assert sys.argv[1:]==['models','--json','--no-extensions']
time.sleep(.1)
print(json.dumps({'models':[{'provider':'fixture','id':k,'selector':'fixture/'+k,'name':'Shared label','thinking':['low','medium','high','xhigh'],'input':['text']} for k in ['first','second','unsupported']]}))
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let target = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let runtime = root.join("runtime");
    let catalog=["first","second"].iter().map(|id|json!({"provider":"fixture","id":id,"api":"openai-responses","tools":true,"inputs":["text"],"efforts":["low","medium","high","xhigh"]})).collect::<Vec<_>>();
    crewroom_service::ServiceOptions {
        helper: target.join("crewroom-process"),
        host: target.join("crewroom-host"),
        tmux: "/usr/bin/tmux".into(),
        jira: None,
        publication: vec![],
        publication_unavailable: vec![],
        environment: json!({"PATH":"/usr/bin:/bin","HOME":root}),
        github: None,
        runtime: Some(
            json!({"root":runtime,"socket":runtime.join("tmux.sock"),"ompBinary":binary,
            "mail":{"atcDisabled":true,"retentionSeconds":600,"projectRoot":root.join("mail"),"url":"http://127.0.0.1:4123/mcp/","token":"native-picker-secret"},
            "models":{"providers":{"fixture":{"baseUrl":"http://127.0.0.1:5555","apiKey":"native-picker-secret"}}},
            "policy":{"default":{"model":"fixture/first","effort":"high"}},
            "catalog":catalog }),
        ),
    }
}

#[gpui::test]
fn native_catalog_search_disabled_routes_drafts_and_restart(cx: &mut gpui::TestAppContext) {
    let mut fixture = Fixture::configured(|_, _| {}, configured);
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    activate_by_keyboard(cx, h, "refresh-model-catalog");
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["modelDiscovery"]["state"] == "partial"
    });
    cx.read(|cx| {
        let p = &v.read(cx).settings.pickers["job-code"];
        assert_eq!(
            p.choices
                .iter()
                .filter(|c| c.label.contains("Shared label"))
                .count(),
            3
        );
        let unavailable = p
            .choices
            .iter()
            .find(|c| c.key.ends_with(":unsupported"))
            .unwrap();
        assert!(unavailable.blocked);
        assert!(unavailable.detail.contains("verified adapter"));
        assert!(
            !v.read(cx)
                .workspace
                .to_string()
                .contains("native-picker-secret")
        );
    });
    activate_by_keyboard(cx, h, "job-code");
    cx.update_window(h, |_, w, cx| {
        w.input("unsupported", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(h, |_, w, cx| {
        w.press("down", cx);
        w.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert_eq!(
            v.read(cx)
                .settings
                .models
                .as_ref()
                .unwrap()
                .fields
                .text("code", cx),
            "fixture/first"
        )
    });
    cx.update_window(h, |_, w, cx| {
        w.press("escape", cx);
    })
    .unwrap();
    activate_by_keyboard(cx, h, "job-code");
    cx.update_window(h, |_, w, cx| {
        w.press(crate::SELECT_ALL_SHORTCUT, cx);
        w.input("omp / fixture / second", cx);
    })
    .unwrap();
    cx.run_until_parked();
    // Search matches the disambiguated route; Enter confirms the only match.
    cx.update_window(h, |_, w, cx| {
        w.press("down", cx);
        w.press("enter", cx);
        w.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let key = "offering:omp:fixture:second";
    wait(cx, h, &v, &e, |v, cx| {
        v.settings.models.as_ref().unwrap().fields.text("code", cx) == key
    });
    click(cx, h, "budget-small");
    click(cx, h, "refresh-model-catalog");
    wait(cx, h, &v, &e, |v, cx| {
        !v.busy
            && v.workspace["modelDiscovery"]["state"] == "partial"
            && v.settings.models.as_ref().unwrap().fields.text("code", cx) == key
    });
    navigate(cx, h, "roles");
    click(cx, h, "edit-role-implementer");
    click(cx, h, "effort-high");
    activate_by_keyboard(cx, h, "role-model");
    cx.update_window(h, |_, w, cx| w.input("omp / fixture / second", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(h, |_, w, cx| {
        w.press("down", cx);
        w.press("enter", cx);
    })
    .unwrap();
    wait(cx, h, &v, &e, |v, cx| {
        v.settings.roles["implementer"].fields.text("model", cx) == key
    });
    click(cx, h, "save-role");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.settings.roles["implementer"].revision == Some(2)
    });
    navigate(cx, h, "models");
    cx.read(|cx| assert_eq!(v.read(cx).settings.models.as_ref().unwrap().budget, "small"));
    click(cx, h, "save-model-jobs");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["modelJobs"]["revision"] == 1
    });
    assert_eq!(fixture.get("/workspace")["modelJobs"]["jobs"]["code"], key);
    assert!(
        fixture.get("/workspace")["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fixture.restart();
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["modelJobs"]["jobs"]["code"] == key
    });
    assert_eq!(fixture.get("/model-catalog")["choices"][1]["key"], key);
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        for id in ["job-code", "refresh-model-catalog", "save-model-jobs"] {
            let bounds = w.find(id).bounds();
            assert!(
                bounds.origin.x >= px(0.) && bounds.right() <= px(390.),
                "{id}: {bounds:?}"
            );
        }
    })
    .unwrap();
    click(cx, h, "job-code");
    let mut measured = false;
    for _ in 0..100 {
        cx.run_until_parked();
        measured = cx
            .update_window(h, |_, w, cx| {
                w.render_frame(cx);
                let Some(first) = w.try_find("model-choice-offering:omp:fixture:first") else {
                    return false;
                };
                let Some(second) = w.try_find("model-choice-offering:omp:fixture:second") else {
                    return false;
                };
                let (first, second) = (first.bounds(), second.bounds());
                assert!(
                    first.bottom() <= second.origin.y,
                    "wrapped rows overlap: {first:?}, {second:?}"
                );
                assert!(
                    second.right() <= px(390.),
                    "wrapped row exceeds window: {second:?}"
                );
                true
            })
            .unwrap();
        if measured {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(measured, "narrow picker rows were not rendered");
}

#[gpui::test]
fn native_catalog_refresh_and_reopen_keep_pre_catalog_history(cx: &mut gpui::TestAppContext) {
    let mut records = Vec::new();
    let mut fixture = Fixture::configured(
        |store, root| {
            let roles = crewroom_core::roles::Roles::new(root.join("roles.json"))
                .read()
                .unwrap();
            let role = roles.iter().find(|r| r["id"] == "coordinator").unwrap();
            for (kind, provider, selector, api, observed) in [
                (
                    "omp",
                    "codex",
                    "historical",
                    "openai-responses",
                    "historical",
                ),
                (
                    "codex",
                    "codex",
                    "native-old",
                    "codex-app-server",
                    "native-old",
                ),
                (
                    "claude-code",
                    "claude-code",
                    "sonnet",
                    "claude-code-stream",
                    "claude-retained-model",
                ),
            ] {
                let task=store.create_task(&json!({"title":format!("Retained {kind}"),"description":"Seeded migration fixture"})).unwrap();
                let run = store
                    .queue_refinement(
                        task["id"].as_str().unwrap(),
                        &json!({"revision":1,"purpose":"explore"}),
                    )
                    .unwrap();
                let mut model = json!({"resolved":{"provider":provider,"id":selector,"selector":format!("{provider}/{selector}"),"api":api,"effort":"low"}});
                if kind != "omp" {
                    model["resolved"]["adapter"] = json!(kind);
                }
                if kind == "claude-code" {
                    model["resolved"]["resolvedModel"] = json!(observed);
                }
                let admitted=store.admit_assignment(run["id"].as_str().unwrap(),&json!({"coordinator":true,"name":"Retained coordinator","role":role,"model":model,"input":"Seeded, not executed"})).unwrap();
                store
                    .set_desired_state(run["id"].as_str().unwrap(), "paused")
                    .unwrap();
                store
                    .begin_pause(run["id"].as_str().unwrap(), None)
                    .unwrap();
                store.finish_pause(run["id"].as_str().unwrap()).unwrap();
                records.push((
                    task,
                    run,
                    admitted,
                    format!("Route: {kind} / {provider} / {selector}"),
                    selector.to_owned(),
                ));
            }
        },
        configured,
    );
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    click(cx, h, "refresh-model-catalog");
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["modelDiscovery"]["state"] == "partial"
    });
    let mut jobs = fixture.get("/workspace")["modelJobs"].clone();
    jobs["jobs"]["code"] = json!("offering:omp:fixture:second");
    assert_eq!(fixture.post("/model-jobs", jobs).0, 200);
    fixture.restart();
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["modelJobs"]["revision"] == 1
    });
    for (task, run, admitted, route, observed) in records {
        click(
            cx,
            h,
            SharedString::from(format!("ticket-{}", task["id"].as_str().unwrap())),
        );
        wait(cx, h, &v, &e, |v, _| {
            v.observation.runtime_fresh && v.observation.id.as_deref() == run["id"].as_str()
        });
        navigate(cx, h, "History");
        click(
            cx,
            h,
            SharedString::from(format!(
                "history-attempt-{}",
                admitted["attempt"]["id"].as_str().unwrap()
            )),
        );
        cx.update_window(h, |_, w, cx| {
            w.render_frame(cx);
            let labels = gpui::base::test_support::snapshots(w)
                .iter()
                .filter_map(|s| s.label().map(str::to_owned))
                .collect::<Vec<_>>();
            assert!(
                labels.iter().any(|l| l == &route),
                "Missing {route}: {labels:?}"
            );
            assert!(
                labels.iter().any(|l| l.contains(&observed)),
                "Missing observed identity: {labels:?}"
            );
        })
        .unwrap();
        let runtime = fixture.get(&format!("/runs/{}/runtime", run["id"].as_str().unwrap()));
        assert_eq!(runtime["assignments"], json!([admitted["assignment"]]));
        assert_eq!(runtime["attempts"].as_array().unwrap().len(), 1);
    }
}
