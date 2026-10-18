//! The agent wall's entry button uses one name wherever it opens the same window.
use super::test_support::{Fixture, click, open, wait};
use super::*;
use gpui_kit::{self as gpui, test::TestWindowExt};

#[gpui::test]
fn native_run_panel_opens_the_agent_wall_by_its_name(cx: &mut gpui::TestAppContext) {
    let mut task = Value::Null;
    let f = Fixture::projected(|s, _| {
        task = s
            .create_task(&json!({"title":"Wall entry","description":""}))
            .unwrap();
        s.queue_refinement(
            &string(&task, "id"),
            &json!({"revision":1,"purpose":"explore"}),
        )
        .unwrap();
    });
    let (h, v, e) = open(cx, &f);
    wait(cx, h, &v, &e, |v, _| {
        v.connected && !records(&v.workspace, "tasks").is_empty()
    });
    click(
        cx,
        h,
        SharedString::from(format!("ticket-{}", string(&task, "id"))),
    );
    wait(cx, h, &v, &e, |v, _| v.observation.id.is_some());
    cx.update_window(h, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("open-agent-wall").label(),
            Some("Open agent wall")
        );
    })
    .unwrap();
    assert_eq!(crate::wall::OPEN_LABEL, "Open agent wall");
}
