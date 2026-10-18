//! Cross-process native UI action driver for the disposable workflow fixture.
//! The Node gate owns the service/processes and verifies cleanup after this exits.
use super::*;
use crate::workbench::test_support::open_connection;

#[gpui::test]
#[ignore = "Requires the private disposable plan created by test-native-resume.sh."]
fn native_external_feedback_resume(cx: &mut gpui::TestAppContext) {
    let path = std::env::var_os("CREWROOM_NATIVE_JOURNEY_PLAN")
        .expect("Run scripts/test-native-resume.sh with its isolated service fixture");
    let plan: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let (h, v, e) = open_connection(cx, PathBuf::from(plan["descriptor"].as_str().unwrap()));
    select(cx, h, &v, &e, &json!({"id":plan["taskId"]}));
    navigate(cx, h, "Results");
    wait(cx, h, &v, &e, |v, _| {
        v.observation.result["publication"]["record"]["feedback"]["comments"][0]["content"]["raw"]
            == "Cover the retained feedback case."
    });
    let before = cx.read(|cx| v.read(cx).observation.result.clone());
    let run = cx.read(|cx| v.read(cx).observation.runtime["run"].clone());
    assert_eq!(run["id"], plan["runId"]);
    assert_eq!(before["publication"]["record"]["pr"]["id"], 1);
    assert_eq!(before["publication"]["record"]["ready"], true);
    if plan["action"] == "resume" {
        assert_eq!(run["status"], "paused");
        click(cx, h, "resume-run");
        wait(cx, h, &v, &e, |v, _| {
            !v.busy
                && v.error.is_empty()
                && (!v.observation.runtime["run"]["resumeRequest"].is_null()
                    || v.observation.runtime["run"]["desiredState"] == "running")
        });
    } else {
        assert_eq!(plan["action"], "verify");
        let candidates = records(&before, "candidates");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0]["id"], plan["previousCandidateId"]);
        assert_eq!(candidates[1]["id"], plan["candidateId"]);
        assert_eq!(
            before["publication"]["record"]["candidateId"],
            plan["candidateId"]
        );
        assert_eq!(
            before["publication"]["record"]["history"][0]["candidateId"],
            plan["previousCandidateId"]
        );
        for field in ["checks", "reviews"] {
            let items = records(&before, field);
            assert!(
                items
                    .iter()
                    .any(|r| r["candidateId"] == plan["candidateId"])
            );
            assert!(
                items
                    .iter()
                    .any(|r| r["candidateId"] == plan["previousCandidateId"])
            );
        }
        click(cx, h, "view-candidate-diff");
        wait(cx, h, &v, &e, |v, _| {
            v.observation
                .detail
                .as_ref()
                .is_some_and(|d| d.content.is_some())
        });
        let content = cx.read(|cx| {
            v.read(cx)
                .observation
                .detail
                .as_ref()
                .unwrap()
                .content
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .to_string()
        });
        assert!(content.contains("feedback case"));
        navigate(cx, h, "History");
        assert!(
            cx.read(|cx| records(&v.read(cx).observation.runtime["history"], "attempts").len()) > 4
        );
    }
    let result = cx.read(|cx| {
        let v = v.read(cx);
        json!({"action":plan["action"],"taskId":plan["taskId"],"runId":plan["runId"],"error":v.error,
            "run":v.observation.runtime["run"],"publication":before["publication"]["record"],
            "candidateIds":records(&before,"candidates").iter().map(|c|c["id"].clone()).collect::<Vec<_>>()})
    });
    assert_eq!(result["error"], "");
    std::fs::write(
        plan["resultPath"].as_str().unwrap(),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
}
