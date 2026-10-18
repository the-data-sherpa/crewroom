//! Opt-in native selection against a separately running production daemon.
//! The caller owns the disposable daemon, Mail server and subsequent live work.
use super::*;
use crate::workbench::test_support::*;
use gpui_kit::{self as gpui, test::TestWindowExt};
use std::{path::PathBuf, time::Duration};

#[gpui::test]
#[ignore = "Requires CREWROOM_MH4_PICKER pointing to a disposable production daemon fixture"]
fn native_catalog_external_live_selection(cx: &mut gpui::TestAppContext) {
    let path = PathBuf::from(
        std::env::var_os("CREWROOM_MH4_PICKER").expect("Explicit disposable fixture required"),
    );
    let setup: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let key = setup["selection"].as_str().unwrap();
    let (h, v, e) = open_connection(cx, PathBuf::from(setup["descriptor"].as_str().unwrap()));
    wait_for(cx, h, &v, &e, Duration::from_secs(30), |v, _| v.connected);
    navigate(cx, h, "models");
    click(cx, h, "refresh-model-catalog");
    wait_for(cx, h, &v, &e, Duration::from_secs(120), |v, _| {
        v.workspace["modelDiscovery"]["generation"]
            .as_u64()
            .unwrap_or(0)
            > 0
            && v.workspace["modelDiscovery"]["state"] != "refreshing"
    });
    cx.read(|cx| {
        let workspace = &v.read(cx).workspace;
        let choice = workspace["modelDiscovery"]["choices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key)
            .expect("Requested live offering was not discovered");
        assert_eq!(
            choice["eligibility"]["low"]["eligible"], true,
            "{}",
            choice["eligibility"]["low"]
        );
    });
    click(cx, h, "budget-small");
    activate_by_keyboard(cx, h, "job-code");
    cx.update_window(h, |_, w, cx| w.input(key, cx)).unwrap();
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
        !v.busy && v.workspace["modelJobs"]["jobs"]["code"] == key
    });
    navigate(cx, h, "roles");
    for role in ["coordinator", "tech-writer"] {
        click(cx, h, SharedString::from(format!("edit-role-{role}")));
        click(cx, h, "effort-low");
        activate_by_keyboard(cx, h, "role-model");
        cx.update_window(h, |_, w, cx| w.input(key, cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(h, |_, w, cx| {
            w.press("down", cx);
            w.press("enter", cx);
        })
        .unwrap();
        wait(cx, h, &v, &e, |v, cx| {
            v.settings.roles[role].fields.text("model", cx) == key
        });
        click(cx, h, "save-role");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && v.workspace["roleCatalog"][role]["model"]["selection"]["intendedModel"] == key
        });
        activate_by_keyboard(cx, h, "close-role-editor");
    }
    let receipt=cx.read(|cx| {
        let workspace=&v.read(cx).workspace;
        json!({"scope":"Native GPUI catalog selection against production Rust daemon; no inference initiated by this UI test",
            "selection":key,"catalogRevision":workspace["modelDiscovery"]["revision"],"modelJobs":workspace["modelJobs"],
            "roles":{"coordinator":workspace["roleCatalog"]["coordinator"]["model"],"tech-writer":workspace["roleCatalog"]["tech-writer"]["model"]}})
    });
    crewroom_core::authority::write_private(
        std::path::Path::new(setup["receipt"].as_str().unwrap()),
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
}
