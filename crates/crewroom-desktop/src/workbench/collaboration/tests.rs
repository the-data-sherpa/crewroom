use super::super::test_support::{
    Fixture, activate_by_keyboard, click, navigate, open, screen_text, wait,
};
use super::*;
use crewroom_core::{roles::Roles, store::Store};
use gpui::AnyWindowHandle;
use gpui::test::TestWindowExt;
use gpui_kit as gpui;
use std::path::Path;

fn id(value: &Value) -> String {
    string(value, "id")
}
fn repository(store: &Store, root: &Path) -> Value {
    let path = root.join("checkout");
    std::fs::create_dir(&path).unwrap();
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
    store
        .create_repository(
            &json!({"name":"Collaboration checkout","path":path,"targetBranch":"main"}),
        )
        .unwrap()
}
fn text(
    cx: &mut gpui::TestAppContext,
    h: AnyWindowHandle,
    v: &Entity<Workbench>,
    path: &str,
    key: &str,
    value: &str,
) {
    cx.update_window(h, |_, window, cx| {
        let fields = &v.read(cx).collaboration.forms[path].fields;
        if let Some(input) = fields.inputs.get(key) {
            input
                .clone()
                .update(cx, |input, cx| input.focus(window, cx));
        } else {
            fields.areas[key]
                .clone()
                .update(cx, |input, cx| input.focus(window, cx));
        }
        window.press(crate::SELECT_ALL_SHORTCUT, cx);
        window.press("backspace", cx);
        window.input(value, cx);
    })
    .unwrap();
}
fn select(cx: &mut gpui::TestAppContext, h: AnyWindowHandle, task: &Value) {
    click(cx, h, SharedString::from(format!("ticket-{}", id(task))));
}

fn implementation_content() -> Value {
    json!({"schemaVersion":1,"goal":"Native plan","scope":["Bounded work"],"exclusions":[],"assumptions":[],"decisions":[],"workPackages":[{"id":"change","name":"Implement and verify","dependsOn":[],"outputs":["Checked patch"],"criteria":[0],"validation":["Native fixture"]}]})
}

#[gpui::test]
fn native_implementation_plan_edits_rebind_accept_and_queue_by_keyboard(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut source = Value::Null;
    let mut plan = Value::Null;
    let fixture = Fixture::seeded(false, |s, root| {
        let repo = repository(s, root);
        task=s.create_task(&json!({"title":"Review implementation plan","description":"","repositoryId":repo["id"]})).unwrap();
        task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Bounded work","criteria":["Checked patch"]})).unwrap();
        source = s
            .save_plan_source(
                &s.prepare_plan_source(&id(&task), 2)
                    .unwrap()
                    .validate()
                    .unwrap(),
            )
            .unwrap();
        plan=s.save_implementation_plan(&id(&task),&json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"content":implementation_content()})).unwrap();
    });
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    select(cx, h, &task);
    let decision = format!("/implementation-plans/{}/decision", id(&plan));
    text(cx, h, &v, &decision, "reason", "Clarify the goal");
    click(cx, h, "plan-reject");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["intake"][id(&task)]["plans"][0]["decision"]["decision"] == "reject"
    });
    click(cx, h, "edit-implementation-plan");
    let path = format!("/tasks/{}/implementation-plans", id(&task));
    text(cx, h, &v, &path, "goal", "Retained 日本語 plan edits");
    let (_,newer)=fixture.post(&path,json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"predecessorId":plan["id"],"content":implementation_content()}));
    wait(cx, h, &v, &e, |v, _| {
        v.workspace["intake"][id(&task)]["plans"]
            .as_array()
            .unwrap()
            .len()
            == 2
    });
    cx.read(|cx| {
        assert_eq!(
            v.read(cx).collaboration.forms[&path]
                .fields
                .text("goal", cx),
            "Retained 日本語 plan edits"
        )
    });
    click(cx, h, "rebind-plan-draft");
    click(cx, h, "save-implementation-plan");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["intake"][id(&task)]["plans"]
                .as_array()
                .unwrap()
                .len()
                == 3
    });
    cx.read(|cx| {
        assert_eq!(
            v.read(cx).workspace["intake"][id(&task)]["plans"][2]["predecessorId"],
            newer["id"]
        )
    });
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    activate_by_keyboard(cx, h, "plan-accept");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy
            && v.workspace["intake"][id(&task)]["currentPlan"]["content"]["goal"]
                == "Retained 日本語 plan edits"
    });
    activate_by_keyboard(cx, h, "queue-plan");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && records(&v.workspace, "runs").len() == 1
    });
    let w = fixture.get("/workspace");
    assert_eq!(w["runs"][0]["purpose"], "coding");
    assert!(records(&w, "activity").is_empty());
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let b = window.find("refresh-plan-source").bounds();
        assert!(b.origin.x >= px(0.) && b.right() <= px(390.));
    })
    .unwrap();
}

#[gpui::test]
fn native_intake_answers_preserve_drafts_across_restart_and_record_pending_delivery(
    cx: &mut gpui::TestAppContext,
) {
    let (mut fixture, task, intake) = waiting_intake_fixture();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    select(cx, h, &task);
    let path = format!("/intakes/{}/answers/output", id(&intake));
    text(cx, h, &v, &path, "answer", "Text 日本語");
    fixture.restart();
    wait(cx, h, &v, &e, |v, _| {
        v.connected && v.workspace["intake"][id(&task)]["state"] == "Waiting for you"
    });
    cx.read(|cx| {
        assert_eq!(
            v.read(cx).collaboration.forms[&path]
                .fields
                .text("answer", cx),
            "Text 日本語"
        )
    });
    cx.simulate_window_resize(h, gpui::size(px(390.), px(844.)));
    activate_by_keyboard(cx, h, "answer-intake");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["intake"][id(&task)]["intake"]["state"] == "answer_pending"
    });
    let w = fixture.get("/workspace");
    assert_eq!(w["messages"][0]["delivery"], "pending");
    assert_eq!(
        w["intake"][id(&task)]["intake"]["questions"][0]["answer"]["body"],
        "Text 日本語"
    );
    click(cx, h, "cancel-intake");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["intake"][id(&task)]["intake"].is_null()
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find(SharedString::from(format!(
                    "intake-history-{}",
                    id(&intake)
                )))
                .bounds()
                .size
                .height
                > px(0.)
        );
    })
    .unwrap();
}

fn waiting_intake_fixture() -> (Fixture, Value, Value) {
    let mut task = Value::Null;
    let mut intake = Value::Null;
    let fixture = Fixture::seeded(false, |s, root| {
        let repo = repository(s, root);
        task = s
            .create_task(
                &json!({"title":"Choose intake output","description":"","repositoryId":repo["id"]}),
            )
            .unwrap();
        let source = s
            .save_plan_source(
                &s.prepare_plan_source(&id(&task), 1)
                    .unwrap()
                    .validate()
                    .unwrap(),
            )
            .unwrap();
        let run = s.queue(&id(&task), &json!({"revision":1})).unwrap();
        let role = Roles::new(root.join("roles.json"))
            .read()
            .unwrap()
            .remove(0);
        let admitted=s.admit_assignment(&id(&run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Recorded native wait fixture"})).unwrap();
        let attempt = id(&admitted["attempt"]);
        s.claim_attempt(&attempt).unwrap();
        // Synthetic stopped identity; actual OS ownership is tested by the daemon fixture.
        s.record_attempt_identity(&attempt,&json!({"hostPid":123,"ompPid":124,"sessionId":"native-wait-fixture","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
        s.set_desired_state(&id(&run), "running").unwrap();
        s.start_execution(&id(&run)).unwrap();
        s.record_intake_inspection(&attempt, &id(&source)).unwrap();
        intake=s.ask_intake_questions(&attempt,"question",&json!({"sourceId":source["id"],"questions":[{"id":"output","prompt":"Which output?","rationale":"Sets the implementation scope","choices":["Text","JSON"]}]})).unwrap();
        s.record_ownership(&attempt, true, Some("Synthetic stopped fixture"))
            .unwrap();
        s.finish_pause(&id(&run)).unwrap();
    });
    (fixture, task, intake)
}

#[gpui::test]
fn native_cancelled_intake_keeps_unsent_answer_visible_and_discardable(
    cx: &mut gpui::TestAppContext,
) {
    let (fixture, task, intake) = waiting_intake_fixture();
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    select(cx, h, &task);
    let path = format!("/intakes/{}/answers/output", id(&intake));
    text(cx, h, &v, &path, "answer", "Unsent 日本語 answer");
    click(cx, h, "cancel-intake");
    wait(cx, h, &v, &e, |v, _| {
        !v.busy && v.workspace["intake"][id(&task)]["intake"].is_null()
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window
                .find(SharedString::from(format!(
                    "unsent-answer-{}-output",
                    id(&intake)
                )))
                .label(),
            Some("Unsent 日本語 answer")
        );
    })
    .unwrap();
    click(
        cx,
        h,
        SharedString::from(format!("discard-answer-{}-output", id(&intake))),
    );
    cx.read(|cx| assert!(!v.read(cx).collaboration.forms.contains_key(&path)));
}

#[gpui::test]
fn native_plan_editor_refreshes_pristine_fields_after_async_proposals(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut source = Value::Null;
    let fixture = Fixture::seeded(false, |s, root| {
        let repo = repository(s, root);
        task = s
            .create_task(
                &json!({"title":"Async plan", "description":"", "repositoryId":repo["id"]}),
            )
            .unwrap();
        task = s.refine(&id(&task), &json!({"revision":1,"sourceRevision":1,"brief":"Initial brief","criteria":["Checked patch"]})).unwrap();
        source = s
            .save_plan_source(
                &s.prepare_plan_source(&id(&task), 2)
                    .unwrap()
                    .validate()
                    .unwrap(),
            )
            .unwrap();
    });
    let (h, v, e) = open(cx, &fixture);
    wait(cx, h, &v, &e, |v, _| v.connected);
    select(cx, h, &task);
    let path = format!("/tasks/{}/implementation-plans", id(&task));
    let mut predecessor = Value::Null;
    for (index, goal) in ["Arrived proposal", "Updated proposal"].iter().enumerate() {
        let mut content = implementation_content();
        content["goal"] = json!(goal);
        content["workPackages"][0]["outputs"] = json!([goal]);
        let (status, plan) = fixture.post(&path, json!({"requestId":uuid::Uuid::new_v4().to_string(),"revision":2,"sourceId":source["id"],"predecessorId":predecessor,"content":content}));
        assert_eq!(status, 200);
        predecessor = plan["id"].clone();
        wait(cx, h, &v, &e, |v, _| {
            records(&v.workspace["intake"][id(&task)], "plans").len() == index + 1
        });
        click(cx, h, "edit-implementation-plan");
        cx.read(|cx| {
            let forms = &v.read(cx).collaboration.forms;
            assert_eq!(forms[&path].fields.text("goal", cx), *goal);
            assert_eq!(
                forms[&format!("{path}/packages/0")]
                    .fields
                    .text("outputs", cx),
                *goal
            );
            assert_eq!(forms[&path].basis["predecessorId"], predecessor);
        });
        click(cx, h, "edit-implementation-plan");
    }
}
fn proposal(store: &Store, run: &Value, target: Value, base: u64, change: &str) -> Value {
    // Synthetic recorded evidence isolates the native decision/persistence flow.
    // No agent, model, provider or process delivery is claimed by this fixture.
    store.propose_learning(&json!({"runId":run["id"],"attemptId":"record-fixture","toolCallId":uuid::Uuid::new_v4().to_string(),"target":target,"baseVersion":base,"change":change,"rationale":"Retain the recorded correction","evidence":[{"kind":"steering","id":"recorded-fixture-evidence"}]})).unwrap()
}
#[gpui::test]
fn native_learning_versions_roles_and_projects_without_changing_existing_decisions(
    cx: &mut gpui::TestAppContext,
) {
    let mut items = vec![];
    let mut repo = Value::Null;
    let mut fixture = Fixture::seeded(false, |s, root| {
        repo = repository(s, root);
        let task = s
            .create_task(&json!({"title":"Learning decisions","description":""}))
            .unwrap();
        let run = s
            .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
        items.push(proposal(
            s,
            &run,
            json!({"kind":"role","roleId":"reviewer"}),
            1,
            "Cite the exact candidate and check evidence.",
        ));
        items.push(proposal(
            s,
            &run,
            json!({"kind":"project","repositoryId":repo["id"]}),
            0,
            "Document the local validation commands.",
        ));
        items.push(proposal(
            s,
            &run,
            json!({"kind":"role","roleId":"reviewer"}),
            1,
            "Do not silently replace newer guidance.",
        ));
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "learning").len() == 3
    });
    navigate(cx, h, "learning");
    let key = format!("/learning/{}/decision", id(&items[0]));
    text(
        cx,
        h,
        &v,
        &key,
        "reason",
        "The recorded evidence supports this change.",
    );
    click(
        cx,
        h,
        SharedString::from(format!("approve-learning-{}", id(&items[0]))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "learning")
                .iter()
                .any(|p| p["id"] == items[0]["id"] && p["decision"]["appliedVersion"] == 2)
    });
    let role = fixture.get("/workspace")["roles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "reviewer")
        .unwrap()
        .clone();
    assert_eq!(role["instructions"], items[0]["change"]);
    let key = format!("/learning/{}/decision", id(&items[1]));
    text(
        cx,
        h,
        &v,
        &key,
        "reason",
        "Use the project-specific correction.",
    );
    click(
        cx,
        h,
        SharedString::from(format!("approve-learning-{}", id(&items[1]))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "learning")
                .iter()
                .any(|p| p["id"] == items[1]["id"] && p["decision"]["appliedVersion"] == 1)
    });
    let key = format!("/learning/{}/decision", id(&items[2]));
    text(cx, h, &v, &key, "reason", "Try the older proposal.");
    click(
        cx,
        h,
        SharedString::from(format!("approve-learning-{}", id(&items[2]))),
    );
    // The stale proposal is flagged before any click, so Approve sends nothing.
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            gpui::base::test_support::snapshots(window)
                .iter()
                .any(|node| node.label()
                    == Some("Stale: role is now revision 2. This proposal can't be applied."))
        );
    })
    .unwrap();
    cx.read(|cx| assert!(!v.read(cx).busy && v.read(cx).error.is_empty()));
    assert_eq!(
        cx.read(|cx| v.read(cx).collaboration.forms[&key]
            .fields
            .text("reason", cx)),
        "Try the older proposal."
    );
    text(
        cx,
        h,
        &v,
        &key,
        "reason",
        "A newer role version is already approved.",
    );
    click(
        cx,
        h,
        SharedString::from(format!("reject-learning-{}", id(&items[2]))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "learning")
                .iter()
                .any(|p| p["id"] == items[2]["id"] && p["status"] == "rejected")
    });
    fixture.restart();
    wait(cx, h, &v, &events, |v, _| {
        v.connected
            && records(&v.workspace, "learning")
                .iter()
                .filter(|p| p["status"] == "approved")
                .count()
                == 2
    });
    let snapshot = fixture.get("/workspace");
    assert_eq!(
        snapshot["repositories"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == repo["id"])
            .unwrap()["guidance"][0]["text"],
        items[1]["change"]
    );
    assert_eq!(snapshot["learning"].as_array().unwrap().len(), 3);
    assert!(snapshot["activity"].as_array().unwrap().is_empty());
}

#[gpui::test]
fn native_queued_specialist_replay_adoption_and_restart_preserve_proposal_semantics(
    cx: &mut gpui::TestAppContext,
) {
    let mut task = Value::Null;
    let mut run = Value::Null;
    let mut fixture = Fixture::seeded(true, |s, root| {
        let repo = repository(s, root);
        task = s
            .create_task(
                &json!({"title":"Insert specialist","description":"","repositoryId":repo["id"]}),
            )
            .unwrap();
        task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Explain the accepted work","criteria":["Explain the result"]})).unwrap();
        crate::workbench::test_support::accept_plan(s, &id(&task));
        run = s.queue(&id(&task), &json!({"revision":2})).unwrap();
        let mut roles = Roles::new(root.join("roles.json")).read().unwrap();
        for role in &mut roles {
            role["model"] = json!("fixture/model");
            role["effort"] = json!("low");
            if role["id"] == "sre" {
                role["enabled"] = json!(false);
            }
        }
        std::fs::write(root.join("roles.json"), serde_json::to_vec(&roles).unwrap()).unwrap();
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "runs").len() == 1
    });
    select(cx, h, &task);
    navigate(cx, h, "Conversation");
    wait(cx, h, &v, &events, |v, _| v.observation.runtime_fresh);
    navigate(cx, h, "specialist-picker");
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("specialist-role-sre").is_none());
    })
    .unwrap();
    click(cx, h, "specialist-role-tech-writer");
    let path = format!("/runs/{}/specialists", id(&run));
    text(
        cx,
        h,
        &v,
        &path,
        "input",
        "Explain the current result to the coordinator.",
    );
    click(cx, h, "specialist-dependency:Coordinator");
    click(cx, h, "specialist-criterion:0");
    // Configuring reusable roles must preserve the ticket's unsent assignment.
    click(cx, h, "specialist-manage-agent-roles");
    activate_by_keyboard(cx, h, "edit-role-tech-writer");
    click(cx, h, "roles-back-to-ticket");
    click(cx, h, "propose-specialist");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.observation.runtime, "teamProposals").len() == 1
    });
    let runtime = fixture.get(&format!("/runs/{}/runtime", id(&run)));
    let first = runtime["teamProposals"][0].clone();
    assert_eq!(first["status"], "pending");
    assert_eq!(first["specialists"][0]["role"]["id"], "tech-writer");
    assert_eq!(
        first["specialists"][0]["input"],
        "Explain the current result to the coordinator."
    );
    assert_eq!(
        first["specialists"][0]["brief"]["dependsOn"],
        json!(["Coordinator"])
    );
    assert!(runtime["assignments"].as_array().unwrap().is_empty());
    assert!(runtime["attempts"].as_array().unwrap().is_empty());
    let (status, replay) = fixture.post(&path, first["request"].clone());
    assert_eq!(status, 200, "{replay}");
    assert_eq!(replay["id"], first["id"]);
    let(status,value)=fixture.post(&format!("/tasks/{}/refinement",id(&task)),json!({"revision":2,"sourceRevision":1,"brief":"Clarify the accepted work","criteria":["Explain the result"]}));
    assert_eq!(status, 200, "{value}");
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "tasks")
            .iter()
            .any(|t| t["id"] == task["id"] && t["revision"] == 3)
    });
    navigate(cx, h, "Brief");
    click(cx, h, "adopt-requirements");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.observation.runtime, "teamProposals")
                .iter()
                .any(|p| p["status"] == "rejected")
    });
    navigate(cx, h, "Conversation");
    navigate(cx, h, "specialist-picker");
    click(cx, h, "specialist-role-tech-writer");
    text(
        cx,
        h,
        &v,
        &path,
        "input",
        "Explain the current result to the coordinator.",
    );
    click(cx, h, "specialist-dependency:Coordinator");
    click(cx, h, "specialist-criterion:0");
    click(cx, h, "propose-specialist");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.observation.runtime, "teamProposals").len() == 2
    });
    let runtime = fixture.get(&format!("/runs/{}/runtime", id(&run)));
    let second = runtime["teamProposals"][1].clone();
    assert_ne!(
        second["request"]["requestId"],
        first["request"]["requestId"]
    );
    assert_eq!(second["request"]["input"], first["request"]["input"]);
    fixture.restart();
    wait(cx, h, &v, &events, |v, _| {
        v.connected
            && v.observation.runtime_fresh
            && records(&v.observation.runtime, "teamProposals").len() == 2
    });
    let runtime = fixture.get(&format!("/runs/{}/runtime", id(&run)));
    assert!(runtime["assignments"].as_array().unwrap().is_empty());
    assert_eq!(runtime["teamProposals"][1]["status"], "pending");
}

fn recorded_coordinator(s: &Store, run: &Value, root: &Path) -> Value {
    let role = Roles::new(root.join("roles.json"))
        .read()
        .unwrap()
        .into_iter()
        .find(|r| r["id"] == "coordinator")
        .unwrap();
    let admitted=s.admit_assignment(&id(run),&json!({"name":"Coordinator","coordinator":true,"role":role,"model":{"resolved":{"selector":"fixture/model"}},"input":"Read recorded evidence"})).unwrap();
    let attempt = id(&admitted["attempt"]);
    s.claim_attempt(&attempt).unwrap();
    // Recorded identity only. Intake mode never starts or inspects these PIDs.
    s.record_attempt_identity(&attempt,&json!({"hostPid":123,"ompPid":124,"sessionId":"record-fixture","hostUrl":"http://127.0.0.1:9","pane":"%1"})).unwrap();
    s.set_desired_state(&id(run), "running").unwrap();
    s.start_execution(&id(run)).unwrap();
    admitted
}
fn stop_recorded(s: &Store, run: &Value, admitted: &Value) {
    s.set_desired_state(&id(run), "paused").unwrap();
    s.begin_pause(&id(run), None).unwrap();
    s.record_ownership(&id(&admitted["attempt"]), true, None)
        .unwrap();
    s.finish_pause(&id(run)).unwrap();
}
#[gpui::test]
fn native_refinement_edit_accept_stale_discard_and_explicit_adoption(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run, mut proposals) = (Value::Null, Value::Null, vec![]);
    let fixture = Fixture::seeded(false, |s, root| {
        task = s
            .create_task(&json!({"title":"Refine a request","description":"Source request"}))
            .unwrap();
        run = s
            .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"grill"}))
            .unwrap();
        let a = recorded_coordinator(s, &run, root);
        for key in ["first", "second"] {
            proposals.push(s.propose_refinement(&id(&a["attempt"]),key,&json!({"brief":"Proposed brief","criteria":["Explain findings"],"decisions":["Keep evidence"],"unresolvedQuestions":["Any limits?"]})).unwrap());
        }
        stop_recorded(s, &run, &a);
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| {
        v.connected && records(&v.workspace, "refinementProposals").len() == 2
    });
    select(cx, h, &task);
    wait(cx, h, &v, &events, |v, _| v.observation.runtime_fresh);
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("queue-plan").is_none(),
            "Coding launch controls must not appear in a refinement run"
        );
    })
    .unwrap();
    let first = id(&proposals[0]);
    let path = format!("/refinement-proposals/{first}/decision");
    click(cx, h, SharedString::from(format!("edit-{first}")));
    text(cx, h, &v, &path, "brief", "Edited accepted brief");
    text(
        cx,
        h,
        &v,
        &path,
        "criteria",
        "Explain the revised result\nState limitations",
    );
    click(cx, h, SharedString::from(format!("accept-{first}")));
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "tasks")
                .iter()
                .any(|t| t["id"] == task["id"] && t["revision"] == 2)
    });
    assert_eq!(
        fixture.get("/workspace")["tasks"][0]["brief"],
        "Edited accepted brief"
    );
    assert_eq!(
        fixture.get(&format!("/runs/{}/runtime", id(&run)))["run"]["taskRevision"],
        1
    );
    // Acceptance does not silently move the active run to a new revision.
    let adoption = format!("/runs/{}/adopt-requirements", id(&run));
    click(cx, h, "adopt-requirements");
    assert!(!cx.read(|cx| v.read(cx).busy));
    text(
        cx,
        h,
        &v,
        &adoption,
        "reason",
        "Use the clarified acceptance criteria.",
    );
    click(cx, h, "adopt-requirements");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "runs")
                .iter()
                .any(|r| r["id"] == run["id"] && r["taskRevision"] == 2)
    });
    let second = id(&proposals[1]);
    let path = format!("/refinement-proposals/{second}/decision");
    click(cx, h, SharedString::from(format!("accept-{second}")));
    assert!(!cx.read(|cx| v.read(cx).busy));
    text(
        cx,
        h,
        &v,
        &path,
        "reason",
        "The edited proposal supersedes this stale base.",
    );
    click(cx, h, SharedString::from(format!("discard-{second}")));
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "refinementProposals")
                .iter()
                .any(|p| p["id"] == proposals[1]["id"] && p["status"] == "discarded")
    });
    // Closing cannot be undone, so it confirms first and Keep running sends nothing.
    click(cx, h, "close-refinement");
    let shown = screen_text(cx, h);
    for expected in [
        "Close this refinement run?",
        "Proposals and notes are kept.",
        "cannot reopen",
        "Keep running",
    ] {
        assert!(
            shown.iter().any(|l| l.contains(expected)),
            "Missing {expected:?} in {shown:#?}"
        );
    }
    click(cx, h, "cancel");
    cx.run_until_parked();
    assert!(!cx.read(|cx| v.read(cx).busy));
    assert!(
        records(&fixture.get("/workspace"), "runs")
            .iter()
            .any(|r| r["id"] == run["id"] && r["status"] != "closed")
    );
    click(cx, h, "close-refinement");
    click(cx, h, "ok");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "runs")
                .iter()
                .any(|r| r["id"] == run["id"] && r["status"] == "closed")
    });
    let snapshot = fixture.get("/workspace");
    assert_eq!(
        snapshot["tasks"][0]["criteria"],
        json!(["Explain the revised result", "State limitations"])
    );
    assert!(!snapshot["notes"].as_array().unwrap().is_empty());
}

#[gpui::test]
fn native_conversation_pages_reconnect_and_keyboard_store_paused_steering(
    cx: &mut gpui::TestAppContext,
) {
    let (mut task, mut run) = (Value::Null, Value::Null);
    let mut fixture = Fixture::seeded(false, |s, root| {
        task = s
            .create_task(&json!({"title":"Conversation recovery","description":""}))
            .unwrap();
        run = s
            .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
        let a = recorded_coordinator(s, &run, root);
        for sequence in 1..=225 {
            let event = if sequence == 1 || sequence == 225 {
                json!({"type":"message_end","message":{"role":"assistant","timestamp":1700000000000i64+sequence,"content":[{"type":"text","text":format!("Recorded coordinator reply {sequence}")}]}})
            } else {
                json!({"type":"session_settled"})
            };
            s.record_agent_event(&id(&a["attempt"]), sequence as u64, &event)
                .unwrap();
        }
        stop_recorded(s, &run, &a);
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| v.connected);
    select(cx, h, &task);
    navigate(cx, h, "Conversation");
    wait(cx, h, &v, &events, |v, _| {
        v.observation.runtime_fresh && v.observation.events.len() == 225
    });
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        let replies: Vec<_> = gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .filter(|label| label.starts_with("Recorded coordinator reply"))
            .collect();
        assert_eq!(
            replies.len(),
            2,
            "Repeated replies need independent accessibility identities"
        );
    })
    .unwrap();
    click(cx, h, "browse-replies");
    wait(cx, h, &v, &events, |v, _| {
        v.observation
            .archive
            .as_ref()
            .is_some_and(|p| records(p, "events").len() == 200)
    });
    click(cx, h, "next-replies");
    wait(cx, h, &v, &events, |v, _| {
        v.observation
            .archive
            .as_ref()
            .is_some_and(|p| records(p, "events").len() == 25)
    });
    assert_eq!(
        cx.read(|cx| v.read(cx).observation.archive_starts.clone()),
        vec![0, 200]
    );
    click(cx, h, "previous-replies");
    wait(cx, h, &v, &events, |v, _| {
        v.observation
            .archive
            .as_ref()
            .is_some_and(|p| records(p, "events").len() == 200)
    });
    cx.update_window(h, |_, window, cx| {
        let input = v.read(cx).steering[&id(&run)].clone();
        input.update(cx, |input, cx| input.focus(window, cx));
        window.input("Please retain this paused direction.", cx);
        window.press("cmd-enter", cx);
    })
    .unwrap();
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.workspace, "messages").len() == 1
    });
    let messages = fixture.get("/workspace")["messages"].clone();
    assert_eq!(messages[0]["delivery"], "pending");
    assert_eq!(messages[0]["body"], "Please retain this paused direction.");
    fixture.restart();
    wait(cx, h, &v, &events, |v, _| {
        v.connected && v.observation.runtime_fresh && v.observation.events.len() == 225
    });
    // A reconnect must not duplicate already displayed durable events.
    assert_eq!(fixture.get("/workspace")["messages"], messages);
    navigate(cx, h, "Brief");
    click(cx, h, "close-refinement");
    click(cx, h, "ok");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "runs")
                .iter()
                .any(|r| r["status"] == "closed")
    });
    navigate(cx, h, "Conversation");
    click(cx, h, "save-steering");
    assert!(!cx.read(|cx| v.read(cx).busy));
    assert_eq!(
        fixture.get("/workspace")["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[gpui::test]
fn native_standalone_research_queues_without_claiming_execution(cx: &mut gpui::TestAppContext) {
    let mut task = Value::Null;
    let fixture = Fixture::seeded(false, |s, _| {
        task = s
            .create_task(&json!({"title":"Standalone research","description":""}))
            .unwrap();
        task=s.refine(&id(&task),&json!({"revision":1,"sourceRevision":1,"brief":"Compare approaches","criteria":["State the tradeoffs"]})).unwrap();
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| v.connected);
    select(cx, h, &task);
    for disabled in ["explore", "grill"] {
        click(cx, h, disabled);
        assert!(!cx.read(|cx| v.read(cx).busy));
    }
    click(cx, h, "queue-research");
    wait(cx, h, &v, &events, |v, _| {
        !v.busy && records(&v.workspace, "runs").len() == 1 && v.observation.runtime_fresh
    });
    let snapshot = fixture.get("/workspace");
    assert_eq!(snapshot["runs"][0]["purpose"], "research");
    assert_eq!(snapshot["runs"][0]["status"], "awaiting_runner");
    assert!(snapshot["runs"][0]["repositoryId"].is_null());
    assert!(snapshot["activity"].as_array().unwrap().is_empty());
    click(cx, h, "start-research");
    assert!(!cx.read(|cx| v.read(cx).busy));
}

#[gpui::test]
fn native_learning_retry_completes_an_approved_version_after_write_failure(
    cx: &mut gpui::TestAppContext,
) {
    use std::os::unix::fs::PermissionsExt;
    let mut item = Value::Null;
    let fixture = Fixture::seeded(true, |s, _| {
        let task = s
            .create_task(&json!({"title":"Recover role save","description":""}))
            .unwrap();
        let run = s
            .queue_refinement(&id(&task), &json!({"revision":1,"purpose":"explore"}))
            .unwrap();
        item = proposal(
            s,
            &run,
            json!({"kind":"role","roleId":"reviewer"}),
            1,
            "Preserve the exact review revision.",
        );
    });
    let (h, v, events) = open(cx, &fixture);
    wait(cx, h, &v, &events, |v, _| v.connected);
    navigate(cx, h, "learning");
    let key = format!("/learning/{}/decision", id(&item));
    text(
        cx,
        h,
        &v,
        &key,
        "reason",
        "Retain the approved correction after interruption.",
    );
    // A real filesystem denial occurs after the durable decision is recorded.
    std::fs::set_permissions(fixture.root.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    click(
        cx,
        h,
        SharedString::from(format!("approve-learning-{}", id(&item))),
    );
    wait(cx, h, &v, &events, |v, _| !v.busy && !v.error.is_empty());
    std::fs::set_permissions(fixture.root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    wait(cx, h, &v, &events, |v, _| {
        records(&v.workspace, "learning")
            .iter()
            .any(|p| p["decision"]["pendingRoleSave"] == true)
    });
    let before = fixture.get("/workspace")["learning"][0].clone();
    assert_eq!(before["status"], "approved");
    click(
        cx,
        h,
        SharedString::from(format!("retry-learning-{}", id(&item))),
    );
    wait(cx, h, &v, &events, |v, _| {
        !v.busy
            && records(&v.workspace, "learning")
                .iter()
                .any(|p| p["decision"]["appliedVersion"] == 2)
    });
    let after = fixture.get("/workspace")["learning"][0].clone();
    assert_eq!(
        after["decision"]["requestId"],
        before["decision"]["requestId"]
    );
    assert_eq!(after["decision"]["reason"], before["decision"]["reason"]);
}
