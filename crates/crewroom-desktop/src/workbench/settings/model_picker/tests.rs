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
                .filter(|c| c.label == "OMP · Shared label · unknown billing")
                .count(),
            3,
            "harness, model and billing route; a gateway provider is unknown"
        );
        let unavailable = p
            .choices
            .iter()
            .find(|c| c.key.ends_with(":unsupported:unknown"))
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
    let key = "offering:omp:fixture:second:unknown";
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
    // The assignment preview records the billing route of the chosen offering.
    assert_eq!(
        fixture.get("/workspace")["roleCatalog"]["implementer"]["model"]["selection"]["billingRoute"],
        "unknown"
    );
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
                let Some(first) = w.try_find("model-choice-offering:omp:fixture:first:unknown")
                else {
                    return false;
                };
                let Some(second) = w.try_find("model-choice-offering:omp:fixture:second:unknown")
                else {
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
                // An admitted catalog selection carries its billing route.
                if kind == "codex" {
                    model["selection"] = json!({"intendedModel":"offering:codex:codex:native-old:subscription",
                        "modelSource":"job","effortSource":"job_budget","reason":"Exact offering selected.",
                        "billingRoute":"subscription","offering":{"billing":"subscription","displayName":"Native old","selector":"native-old"}});
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
            let billing = labels.iter().any(|l| l == "Billing: subscription");
            assert_eq!(billing, route.contains("codex / codex"), "{labels:?}");
        })
        .unwrap();
        let runtime = fixture.get(&format!("/runs/{}/runtime", run["id"].as_str().unwrap()));
        assert_eq!(runtime["assignments"], json!([admitted["assignment"]]));
        assert_eq!(runtime["attempts"].as_array().unwrap().len(), 1);
    }
}

/// FOR-38: the operator's Linux service had no runner configuration. Only a
/// newer Codex is installed here; OMP and Claude Code are missing from PATH.
fn unconfigured(root: &std::path::Path) -> crewroom_service::ServiceOptions {
    let bin = root.join("bin");
    if !bin.exists() {
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(bin.join("codex"), r#"#!/usr/bin/python3
import json,sys
if '--version' in sys.argv:
 print('codex-cli 0.163.0');sys.exit(0)
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 result={'data':[{'model':'fixture-model','displayName':'Fixture model','inputModalities':['text'],'supportedReasoningEfforts':[{'reasoningEffort':e} for e in ['low','medium','high','xhigh']]}],'nextCursor':None} if v['method']=='model/list' else {}
 print(json.dumps({'id':v['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::set_permissions(bin.join("codex"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        // OMP lists no models without runner providers; this version is
        // discovery-verified and contract-checked with synthetic/scripted
        // providers (FOR-52), but not yet launch-verified live.
        std::fs::write(bin.join("omp"), "#!/usr/bin/python3\nimport sys\nif '--version' in sys.argv:\n print('omp/18.11.3');sys.exit(0)\nprint('{\"models\":[]}')\n").unwrap();
        std::fs::set_permissions(bin.join("omp"), std::fs::Permissions::from_mode(0o700)).unwrap();
        // Synthetic ChatGPT login in the service HOME: subscription billing.
        std::fs::create_dir(root.join(".codex")).unwrap();
        std::fs::write(
            root.join(".codex/auth.json"),
            r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"access_token":"picker-login-secret","account_id":"fixture"}}"#,
        )
        .unwrap();
        std::fs::set_permissions(
            root.join(".codex/auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let mut options = configured(root);
    options.runtime = None;
    options.environment = json!({"PATH":bin,"HOME":root});
    options
}
fn labels(cx: &mut gpui::TestAppContext, h: gpui::AnyWindowHandle) -> Vec<String> {
    cx.update_window(h, |_, w, cx| {
        w.render_frame(cx);
        gpui::base::test_support::snapshots(w)
            .iter()
            .filter_map(|s| s.label().map(str::to_owned))
            .collect::<Vec<_>>()
    })
    .unwrap()
}

#[gpui::test]
fn unconfigured_runner_lists_installed_models_and_explains_each_cause(
    cx: &mut gpui::TestAppContext,
) {
    let fixture = Fixture::configured(|_, _| {}, unconfigured);
    let mut jobs = fixture.get("/workspace")["modelJobs"].clone();
    jobs["jobs"]["judgment"] = json!("legacy/free-text");
    assert_eq!(fixture.post("/model-jobs", jobs).0, 200);
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    navigate(cx, h, "models");
    let before = labels(cx, h);
    for expected in [
        "Runner not configured:",
        "CREWROOM_RUNTIME_CONFIG",
        "(`claude`) was not found on the service PATH",
    ] {
        assert!(
            before.iter().any(|l| l.contains(expected)),
            "Missing {expected}: {before:?}"
        );
    }
    activate_by_keyboard(cx, h, "refresh-model-catalog");
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["modelDiscovery"]["generation"].as_u64() == Some(1)
            && v.workspace["modelDiscovery"]["state"] != "refreshing"
    });
    let key = "offering:codex:codex:fixture-model:subscription";
    cx.read(|cx| {
        let pickers = &v.read(cx).settings.pickers;
        assert!(
            !v.read(cx)
                .workspace
                .to_string()
                .contains("picker-login-secret")
        );
        for job in ["code", "judgment", "review", "fast"] {
            let choice = pickers[&format!("job-{job}")]
                .choices
                .iter()
                .find(|c| c.key == key)
                .unwrap_or_else(|| panic!("{job} lacks the installed Codex model"));
            assert!(!choice.blocked, "intent choices stay selectable");
            assert_eq!(choice.label, "Codex · Fixture model · subscription");
            assert!(
                choice
                    .detail
                    .contains("checked when the runner is configured"),
                "{}",
                choice.detail
            );
        }
        let legacy = pickers["job-judgment"]
            .choices
            .iter()
            .find(|c| c.key == "legacy/free-text")
            .unwrap();
        assert!(legacy.blocked);
        assert_eq!(legacy.label, "Legacy selection: legacy/free-text");
    });
    let after = labels(cx, h);
    for expected in [
        "OMP omp/18.11.3 lists models, but Crewroom has verified launching agents only with",
        "OMP lists models only from providers defined in the runner configuration",
        "codex · success",
        "Billing (codex): subscription · Codex auth.json has a ChatGPT login",
    ] {
        assert!(
            after.iter().any(|l| l.contains(expected)),
            "Missing {expected}: {after:?}"
        );
    }
    assert!(
        !after
            .iter()
            .any(|l| l.contains("codex-cli 0.163.0 lists models")),
        "Launch-verified Codex carries no execution caveat"
    );
    activate_by_keyboard(cx, h, "job-code");
    cx.update_window(h, |_, w, cx| w.input("fixture-model", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(h, |_, w, cx| {
        w.press("down", cx);
        w.press("enter", cx);
    })
    .unwrap();
    wait(cx, h, &v, &e, |v, cx| {
        v.settings.models.as_ref().unwrap().fields.text("code", cx) == key
    });
    click(cx, h, "save-model-jobs");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["modelJobs"]["revision"] == 2
    });
    let saved = fixture.get("/workspace")["modelJobs"]["jobs"].clone();
    assert_eq!(saved["code"], key);
    assert_eq!(saved["judgment"], "legacy/free-text");
    navigate(cx, h, "roles");
    click(cx, h, "edit-role-implementer");
    cx.read(|cx| {
        let p = &v.read(cx).settings.pickers["role-model"];
        assert_eq!(p.choices[0].label, "Use job model");
        assert!(p.choices.iter().any(|c| c.key == key && !c.blocked));
        assert_eq!(
            v.read(cx).settings.roles["implementer"]
                .fields
                .text("model", cx),
            "",
            "an empty override still inherits the job model"
        );
    });
    let role = labels(cx, h);
    assert!(
        role.iter().any(|l| l.starts_with("Runner not configured:")),
        "Role editor must name the cause: {role:?}"
    );
    assert!(
        fixture.get("/workspace")["runs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// Selections saved before billing routes name harness, provider and model
/// only. The service resolves them to the single route, so the picker shows
/// that routed choice instead of an unavailable saved selection.
#[gpui::test]
fn pre_billing_selections_show_their_only_route(cx: &mut gpui::TestAppContext) {
    let fixture = Fixture::configured(
        |_, root| {
            let roles = crewroom_core::roles::Roles::new(root.join("roles.json"));
            let mut role = roles
                .read()
                .unwrap()
                .into_iter()
                .find(|r| r["id"] == "implementer")
                .unwrap();
            role["model"] = json!("offering:omp:fixture:first");
            role["revision"] = role["revision"].as_u64().map_or(json!(1), |r| json!(r));
            roles.update("implementer", &role).unwrap();
        },
        configured,
    );
    let mut jobs = fixture.get("/workspace")["modelJobs"].clone();
    jobs["jobs"]["code"] = json!("offering:omp:fixture:second");
    assert_eq!(fixture.post("/model-jobs", jobs).0, 200);
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    assert_eq!(fixture.post("/model-catalog/refresh", json!({})).0, 200);
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["modelDiscovery"]["state"] == "partial"
            && v.workspace["modelJobs"]["revision"] == 1
    });
    navigate(cx, h, "models");
    let routed = "offering:omp:fixture:second:unknown";
    cx.read(|cx| {
        let v = v.read(cx);
        assert_eq!(
            v.settings.models.as_ref().unwrap().fields.text("code", cx),
            "offering:omp:fixture:second",
            "the saved text is retained until the operator chooses again"
        );
        let p = &v.settings.pickers["job-code"];
        assert!(
            !p.choices
                .iter()
                .any(|c| c.blocked && !c.key.ends_with(":unsupported:unknown")),
            "no unavailable saved selection: {:?}",
            p.choices.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        assert_eq!(p.selected, routed);
        assert_eq!(
            p.state.read(cx).selected_value().map(String::as_str),
            Some(routed)
        );
        let shown = p.choices.iter().find(|c| c.key == routed).unwrap();
        assert_eq!(shown.label, "OMP · Shared label · unknown billing");
    });
    let models = labels(cx, h);
    assert!(
        !models
            .iter()
            .any(|l| l.contains("Unavailable saved selection")
                || l.contains("Not in this catalog")
                || l.starts_with("Retained selection: offering:")),
        "{models:?}"
    );
    for expected in [
        "Saved before billing routes: offering:omp:fixture:second.",
        "omp / fixture / second · unknown billing · Access:",
    ] {
        assert!(
            models.iter().any(|l| l.contains(expected)),
            "Missing {expected}: {models:?}"
        );
    }
    navigate(cx, h, "roles");
    click(cx, h, "edit-role-implementer");
    cx.read(|cx| {
        let p = &v.read(cx).settings.pickers["role-model"];
        assert_eq!(p.selected, "offering:omp:fixture:first:unknown");
        assert!(!p.choices.iter().any(|c| c.label.starts_with("Unavailable")));
    });
    let role = labels(cx, h);
    assert!(
        role.iter()
            .any(|l| l.contains("Saved before billing routes: offering:omp:fixture:first.")),
        "{role:?}"
    );
    assert!(
        !role.iter().any(|l| l.contains("Not in this catalog")),
        "{role:?}"
    );
}

#[test]
fn pre_billing_selection_with_several_routes_needs_a_route_choice() {
    let rows = vec![
        json!({"key":"offering:claude-code:claude-code:sonnet:subscription","billingRoute":"subscription","displayName":"Sonnet"}),
        json!({"key":"offering:claude-code:claude-code:sonnet:api","billingRoute":"api","displayName":"Sonnet"}),
        json!({"key":"offering:codex:codex:fixture:subscription","billingRoute":"subscription","displayName":"Fixture"}),
    ];
    let several = saved_rows("offering:claude-code:claude-code:sonnet", &rows);
    assert_eq!(several.len(), 2);
    assert_eq!(
        route_choice_required(&several),
        "Sonnet is reachable through subscription and API billing. Choose one billing route explicitly; your selection is retained."
    );
    assert_eq!(
        saved_rows("offering:codex:codex:fixture", &rows),
        vec![&rows[2]]
    );
    assert_eq!(
        saved_rows("offering:codex:codex:fixture:subscription", &rows),
        vec![&rows[2]]
    );
    assert!(saved_rows("offering:codex:codex:fixture:api", &rows).is_empty());
    assert!(saved_rows("offering:codex:codex:fix", &rows).is_empty());
    assert!(saved_rows("", &rows).is_empty());
}
