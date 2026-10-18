use super::super::test_support::{activate_by_keyboard, click};
use super::*;
use gpui::test::TestWindowExt;
use gpui::{Bounds, WindowBounds, WindowOptions, point, size};
use gpui_kit as gpui;

fn comparison(kind: &str) -> Value {
    let current = match kind {
        "principle" => json!({"id":"fixture","name":"Guidance","text":"Current","revision":3}),
        "playbook" => {
            json!({"id":"fixture","name":"Checklist","description":"Current","steps":[{"id":"step-1","title":"Check"}],"revision":3})
        }
        _ => {
            json!({"id":"fixture","name":"Implementer","instructions":"Current","model":"provider/model","effort":"low","revision":3})
        }
    };
    let mut proposed = current.clone();
    proposed[match kind {
        "principle" => "text",
        "playbook" => "description",
        _ => "instructions",
    }] = json!("Proposed");
    json!({"id":"comparison-fixture","state":"planned","desiredState":"planned","manifest":{"mode":"scripted","target":{"kind":kind,"id":"fixture","baseRevision":3,"current":current,"proposed":proposed,"proposedHash":"a".repeat(64)},"execution":{"model":{"selector":"provider/model","effort":"low"}},"design":{"perCellActiveMs":180000,"totalActiveMs":2880000}},"cells":[],"activeMs":0})
}
#[test]
fn adoption_association_requires_exact_snapshot_and_next_saved_revision_for_every_kind() {
    for kind in ["role", "principle", "playbook"] {
        let value = comparison(kind);
        let mut draft = target_of(&value)["proposed"].clone();
        assert!(exact_draft(&value, kind, "fixture", &draft));
        assert!(
            adoption_request(&value, &draft).is_none(),
            "Unsaved draft is not adoption"
        );
        draft["revision"] = json!(4);
        draft["history"] = json!([{"revision":4}]);
        assert!(adoption_request(&value, &draft).is_some());
        assert!(
            !exact_draft(&value, kind, "fixture", &draft),
            "Changed base is historical"
        );
        draft["name"] = json!("Changed after comparison");
        assert!(adoption_request(&value, &draft).is_none());
        draft = target_of(&value)["proposed"].clone();
        draft["revision"] = json!(5);
        assert!(
            adoption_request(&value, &draft).is_none(),
            "A later matching revision is not this adoption"
        );
        let mut adopted = value.clone();
        adopted["adoption"] = json!({"adoptedRevision":4});
        draft["revision"] = json!(4);
        assert!(
            adoption_request(&adopted, &draft).is_none(),
            "Confirmed association is not repeated"
        );
    }
}
#[test]
fn missing_cells_and_unavailable_usage_never_become_positive_results() {
    let mut value = comparison("role");
    value["cells"] = json!([
        {"state":"completed","result":{"observed":true,"correct":false}},
        {"state":"unavailable","result":{"observed":false,"usage":{"available":false}}},
        {"state":"planned"}, {"state":"running"}
    ]);
    assert!(coverage(&value).contains("1/4"));
    assert!(coverage(&value).contains("Inconclusive"));
}

#[test]
fn fully_observed_cells_keep_inconclusive_assessments_visible() {
    let mut value = comparison("role");
    value["cells"] = json!(vec![
        json!({"result":{"observed":true,"scores":{
            "classification":"unsuccessful","sourceCorrectness":false,
            "completionClaim":false,"falseCompletion":false,
            "requiredChecksFresh":false,"surfaceExercised":false
        }}});
        16
    ]);
    assert!(!coverage(&value).contains("Inconclusive assessment"));
    value["cells"][0]["result"]["scores"]["classification"] = json!("inconclusive");
    let text = coverage(&value);
    assert!(text.contains("16/16 evaluated cells"));
    assert!(text.contains("Inconclusive assessment: 1 observed cells"));
    value["cells"][0]["result"]["scores"]["classification"] = json!("unsuccessful");
    value["cells"][0]["result"]["scores"]["sourceCorrectness"] = Value::Null;
    assert!(coverage(&value).contains("Inconclusive assessment: 1 observed cells"));
}

#[gpui::test]
fn native_preflight_requires_explicit_start_and_keeps_save_optional(cx: &mut gpui::TestAppContext) {
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(1200.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.comparisons.open = true;
                    view
                })
            },
        )
        .unwrap()
    });
    let value = comparison("role");
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.accept(
                Event::Result {
                    path: "/comparisons/preflight".into(),
                    result: Ok(
                        json!({"manifest":value["manifest"],"canStart":true,"summary":{"cases":4}}),
                    ),
                },
                cx,
            )
        });
        window.render_frame(cx);
        assert!(window.try_find("start-comparison").is_some());
        assert!(!view.read(cx).comparisons.start_after_create);
        assert!(!view.read(cx).pending_keys.contains_key("/comparisons"));
    })
    .unwrap();
    activate_by_keyboard(cx, handle, "start-comparison");
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(view.comparisons.start_after_create);
        assert_eq!(
            view.pending_keys["/comparisons"].1["manifest"],
            value["manifest"]
        );
        assert!(
            view.workspace["roles"].is_null(),
            "Start does not save a role"
        );
    });
    click(cx, handle, "close-comparison");
    cx.read(|cx| assert!(!view.read(cx).comparisons.open));
}
#[gpui::test]
fn native_unavailable_preflight_history_and_cell_details_remain_inspectable(
    cx: &mut gpui::TestAppContext,
) {
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(780.), px(1200.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.comparisons.open = true;
                    view
                })
            },
        )
        .unwrap()
    });
    let mut value = comparison("principle");
    value["state"] = json!("paused");
    value["desiredState"] = json!("paused");
    value["cells"] = json!([{"id":"cell","caseId":"task-01","repetition":1,"variant":"current","state":"unavailable","result":{"observed":false,"usage":{"available":false,"reason":"Provider unavailable"}}}]);
    cx.update_window(handle,|_,window,cx|{
        view.update(cx,|view,cx| {
            view.comparisons.history=vec![value.clone()];
            view.accept(Event::Result{path:"/comparisons/preflight".into(),result:Ok(json!({"manifest":value["manifest"],"canStart":false,"reasons":["Frozen runner unavailable"]}))},cx);
        });
        window.render_frame(cx);
    }).unwrap();
    click(cx, handle, "start-comparison");
    cx.read(|cx| assert!(!view.read(cx).comparisons.start_after_create));
    activate_by_keyboard(cx, handle, "open-comparison-comparison-fixture");
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("comparison-resume").is_some());
        assert!(window.try_find("comparison-cancel").is_some());
        assert!(
            gpui::base::test_support::snapshots(window)
                .iter()
                .any(|node| node
                    .label()
                    .is_some_and(|label| label.contains("task-01: current: 0/2 observed")))
        );
    })
    .unwrap();
    activate_by_keyboard(cx, handle, "comparison-cell-cell");
    cx.read(|cx| assert!(view.read(cx).comparisons.expanded.contains("cell")));
}

#[gpui::test]
fn native_held_comparison_exposes_aggregate_and_collapsed_cell_attention(
    cx: &mut gpui::TestAppContext,
) {
    let reason = "Malformed tool response; operator attention is required.";
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(1800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| {
                let mut view = Workbench::new(None, None, window, cx);
                view.connected = true;
                view.comparisons.open = true;
                let mut value = comparison("role");
                value["state"] = json!("held");
                value["desiredState"] = json!("running");
                value["attentionReason"] = json!(reason);
                value["cells"] = json!([{"id":"held-cell","caseId":"task-01","repetition":1,"variant":"current","state":"held","attentionReason":reason}]);
                view.comparisons.selected = Some(value);
                view
            }),
        ).unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let labels = gpui::base::test_support::snapshots(window)
            .into_iter()
            .filter_map(|node| node.label().map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(
            labels
                .iter()
                .filter(|label| **label == format!("Attention required: {reason}"))
                .count(),
            2,
            "Aggregate and collapsed cell both expose the hold reason"
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("Revision 3 → proposed · held"))
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("Requested state: running"))
        );
        assert!(view.read(cx).comparisons.expanded.is_empty());
        assert!(window.try_find("comparison-pause").is_some());
        assert!(
            window.try_find("comparison-resume").is_none(),
            "A held run must record pause intent before resuming"
        );
        view.update(cx, |view, cx| {
            view.comparisons.selected.as_mut().unwrap()["desiredState"] = json!("paused");
            cx.notify();
        });
        window.render_frame(cx);
        assert!(
            window.try_find("comparison-resume").is_some(),
            "Held work with recorded pause intent can request Resume"
        );
        view.update(cx, |view, cx| {
            view.comparisons.selected.as_mut().unwrap()["state"] = json!("incomplete");
            cx.notify();
        });
        window.render_frame(cx);
        assert!(
            window.try_find("comparison-resume").is_none(),
            "A historical incomplete comparison needs a new manifest"
        );
        assert!(window.try_find("comparison-pause").is_none());
    })
    .unwrap();
}

/// Opt-in real Linux window for compositor screenshots. Scripted view data is
/// deliberately labeled; this proves native rendering only, never live scoring.
#[test]
#[ignore = "Opens a native window for compositor visual verification"]
fn native_comparison_visual_fixture() {
    let phase =
        std::env::var("CREWROOM_COMPARISON_VISUAL_PHASE").unwrap_or_else(|_| "preflight".into());
    gpui::application().with_assets(gpui::assets::Assets).run(move |cx| {
        gpui::init(cx);
        gpui::component::Theme::change(gpui::component::ThemeMode::Dark, None, cx);
        let (handle, _) = gpui::open_window(WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {title: Some("Crewroom comparison visual fixture".into()), ..Default::default()}),
            window_bounds: Some(WindowBounds::Windowed(Bounds {origin:point(px(0.),px(0.)),size:size(px(1280.),px(1000.))})),
            ..Default::default()
        }, cx, |window,cx| cx.new(|cx| {
            let mut view=Workbench::new(None,None,window,cx);
            view.connected=true;
            view.comparisons.open=true;
            view.notice="Scripted UI fixture · native rendering only · no workers or configuration changes".into();
            let mut value=comparison("role");
            value["manifest"]["target"]["current"]["instructions"]=json!("Make the smallest complete change. Follow project instructions and leave a concise handoff.");
            value["manifest"]["target"]["proposed"]["instructions"]=json!("Make the smallest complete change. Run required checks on the final source. Exercise the affected user surface. Identify the checked revision and report any verification limits.");
            value["manifest"]["execution"]["runner"]=json!({"kind":"scripted view"});
            value["manifest"]["execution"]["coordinatorModel"]=json!({"selector":"fixture/supporting-model","effort":"low"});
            if ["held", "inconclusive-completed", "inconclusive-partial"].contains(&phase.as_str()) {
                let reason="Owned process cleanup remains unverified after interruption. Keep this cell on hold while the coordinator reconciles its recorded process identity and retained workspace. No replacement can start until ownership is confirmed stopped.";
                let mut cells=Vec::new();
                for index in 0..16 {
                    let mut cell=json!({"id":format!("cell-{index}"),"caseId":format!("task-{:02}",index / 4 + 1),"repetition":index / 2 % 2 + 1,"variant":if index % 2 == 0 {"current"} else {"proposed"},"state":"planned"});
                    if phase=="inconclusive-completed" || phase=="inconclusive-partial" && index < 5 {
                        cell["state"]=json!("completed");
                        cell["result"]=json!({"observed":true,"scores":{"classification":"unsuccessful","sourceCorrectness":false,"completionClaim":false,"falseCompletion":false,"requiredChecksFresh":false,"surfaceExercised":false}});
                        if index==0 {
                            cell["result"]["scores"]["classification"]=json!("inconclusive");
                            cell["result"]["scores"]["sourceCorrectness"]=Value::Null;
                        }
                    } else if phase=="held" && index==0 {
                        cell["state"]=json!("held");
                        cell["attentionReason"]=json!(reason);
                    } else if phase=="inconclusive-partial" && index==5 {
                        cell["state"]=json!("unavailable");
                        cell["result"]=json!({"observed":false,"usage":{"available":false,"reason":"Scripted provider unavailable state"}});
                    }
                    cells.push(cell);
                }
                value["cells"]=json!(cells);
                value["state"]=json!(match phase.as_str(){"held"=>"held","inconclusive-completed"=>"completed",_=>"paused"});
                value["desiredState"]=json!(if phase=="held"{"running"}else{"paused"});
                if phase=="held" {value["attentionReason"]=json!(reason);}
                view.comparisons.selected=Some(value.clone());
                view.comparisons.history=vec![value];
            } else if phase=="results" {
                value["state"]=json!("paused");value["desiredState"]=json!("paused");
                value["cells"]=json!([
                    {"id":"cell-1","caseId":"task-01","repetition":1,"variant":"current","state":"completed","result":{"observed":true,"scores":{"sourceCorrectness":true,"falseCompletion":false}}},
                    {"id":"cell-2","caseId":"task-01","repetition":1,"variant":"proposed","state":"unavailable","result":{"observed":false,"scores":{},"usage":{"available":false,"reason":"Scripted provider unavailable state"}}},
                    {"id":"cell-3","caseId":"task-02","repetition":1,"variant":"proposed","state":"planned"}
                ]);
                view.comparisons.selected=Some(value.clone());
                view.comparisons.history=vec![value];
            } else {
                view.comparisons.preflight=Some(json!({"canStart":true,"manifest":value["manifest"],"summary":{"cases":4,"repetitions":2,"variants":2,"workers":16,"mode":"scripted"}}));
            }
            view
        })).unwrap();
        handle.update(cx, |_,window,cx| window.on_window_should_close(cx,|_,cx|{cx.quit();true})).unwrap();
        cx.activate(true);
    });
}

#[gpui::test]
fn native_comparison_history_names_revision_state_and_date_and_adoption_in_plain_text(
    cx: &mut gpui::TestAppContext,
) {
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = Workbench::new(None, None, window, cx);
                view.connected = true;
                view.comparisons.open = true;
                view
            })
        })
        .unwrap()
    });
    let mut history = vec![];
    for (id, base, state, at) in [
        (
            "0b6c1f5e-3a51-4c35-9f7d-7d1b8d2a1001",
            1,
            "completed",
            "2026-10-01T08:15:00Z",
        ),
        (
            "0b6c1f5e-3a51-4c35-9f7d-7d1b8d2a1002",
            2,
            "cancelled",
            "2026-10-03T09:30:00Z",
        ),
        (
            "0b6c1f5e-3a51-4c35-9f7d-7d1b8d2a1003",
            3,
            "planned",
            "2026-10-05T17:45:00Z",
        ),
    ] {
        let mut value = comparison("role");
        value["id"] = json!(id);
        value["state"] = json!(state);
        value["createdAt"] = json!(at);
        value["manifest"]["target"]["baseRevision"] = json!(base);
        history.push(value);
    }
    history[0]["adoption"] = json!({"baseRevision":1,"adoptedRevision":2,"proposedHash":"a".repeat(64),"at":"2026-10-02T10:00:00Z"});
    let labels = |cx: &mut gpui::TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter_map(|node| node.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap()
    };
    cx.update_window(handle, |_, _, cx| {
        view.update(cx, |view, _| view.comparisons.history = history.clone())
    })
    .unwrap();
    let rendered = labels(cx);
    for expected in [
        "Revision 1 → proposed · completed · 2026-10-01 08:15",
        "Revision 2 → proposed · cancelled · 2026-10-03 09:30",
        "Revision 3 → proposed · planned · 2026-10-05 17:45",
    ] {
        assert!(
            rendered.iter().any(|l| l == expected),
            "Missing {expected:?} in {rendered:#?}"
        );
    }
    assert!(
        rendered
            .iter()
            .filter(|l| l.contains("0b6c1f5e"))
            .all(|l| l.starts_with("ID ")),
        "A comparison ID is secondary text only: {rendered:#?}"
    );
    activate_by_keyboard(
        cx,
        handle,
        "open-comparison-0b6c1f5e-3a51-4c35-9f7d-7d1b8d2a1001",
    );
    let rendered = labels(cx);
    assert!(
        rendered.iter().any(|l| l == "Adopted as revision 2."),
        "{rendered:#?}"
    );
    assert!(
        !rendered
            .iter()
            .any(|l| l.contains("Adoption association") || l.contains("proposedHash"))
    );
    assert!(
        rendered
            .iter()
            .any(|l| l == "Revision 1 → proposed · completed · 2026-10-01 08:15"),
        "The open result is titled like its history entry"
    );
}

#[gpui::test]
fn native_cancel_remaining_cells_confirms_and_keep_sends_nothing(cx: &mut gpui::TestAppContext) {
    let (handle, view) = cx.update(|cx| {
        gpui::init(cx);
        gpui::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(780.), px(1200.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut view = Workbench::new(None, None, window, cx);
                    view.connected = true;
                    view.comparisons.open = true;
                    view
                })
            },
        )
        .unwrap()
    });
    let mut value = comparison("principle");
    value["state"] = json!("running");
    value["desiredState"] = json!("running");
    value["cells"] = json!([
        {"id":"done","caseId":"task-01","repetition":1,"variant":"current","state":"completed","result":{"observed":true,"scores":{}}},
        {"id":"open-1","caseId":"task-01","repetition":1,"variant":"proposed","state":"running"},
        {"id":"open-2","caseId":"task-02","repetition":1,"variant":"current","state":"planned"}
    ]);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |view, _| {
            view.comparisons.selected = Some(value.clone())
        });
        window.render_frame(cx);
    })
    .unwrap();
    let control = "/comparisons/comparison-fixture/control";
    click(cx, handle, "comparison-cancel");
    let shown = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            gpui::base::test_support::snapshots(window)
                .into_iter()
                .filter_map(|node| node.label().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .unwrap();
    for expected in [
        "Cancel remaining cells?",
        "2 of 3 cells can't be resumed.",
        "Finished results are kept.",
        "Keep comparison",
    ] {
        assert!(
            shown.iter().any(|l| l.contains(expected)),
            "Missing {expected:?} in {shown:#?}"
        );
    }
    click(cx, handle, "cancel");
    cx.read(|cx| assert!(!view.read(cx).pending_keys.contains_key(control)));
    click(cx, handle, "comparison-cancel");
    click(cx, handle, "ok");
    cx.read(|cx| assert_eq!(view.read(cx).pending_keys[control].1["action"], "cancel"));
}
