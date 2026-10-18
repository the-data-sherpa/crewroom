//! Direct GPUI composition contracts: the same handler methods called by Cocoa's
//! NSTextInputClient bridge. These tests do not simulate an OS IME candidate UI.
use super::*;
use gpui_kit::{EntityInputHandler as _, Focusable as _, test::TestWindowExt};

#[gpui_kit::test]
fn composition_replacement_uses_utf16_and_keeps_drafts_through_focus_changes(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = Workbench::new(None, None, window, cx);
                view.connected = true;
                view.panel = Panel::New;
                view
            })
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let input = view.read(cx).title.clone();
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.replace_text_in_range(None, "A🦀", window, cx);
            input.replace_and_mark_text_in_range(Some(3..3), "に", Some(1..1), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(3..4));
            input.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx);
            assert_eq!(input.marked_text_range(window, cx), Some(3..5));
            input.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(input.marked_text_range(window, cx), None);
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                6..6
            );
            assert_eq!(input.value(), "A🦀日本語");
        });
        window.render_frame(cx);
        window.press("tab", cx);
        assert!(
            view.read(cx)
                .description
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        window.press("shift-tab", cx);
        assert!(
            input.read(cx).focus_handle(cx).is_focused(window),
            "Reverse traversal must reach the preceding field"
        );
        window.click("settings", cx);
        window.render_frame(cx);
        window.click("roles", cx);
        window.render_frame(cx);
        assert!(view.read(cx).focus.contains_focused(window, cx));
        window.press("tab", cx);
        assert!(view.read(cx).unsaved(cx));
        assert_eq!(input.read(cx).value(), "A🦀日本語");
    })
    .unwrap();
}

#[gpui_kit::test]
fn readonly_observation_rejects_marked_text_and_composition_commit(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (handle, state) = cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| TextareaState::new(window, cx))
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        state.update(cx, |input, cx| {
            input.set_value("Saved 日本語 🦀", window, cx);
            input.set_readonly(true, cx);
            input.replace_and_mark_text_in_range(Some(0..5), "消去", Some(2..2), window, cx);
            assert_eq!(input.marked_text_range(window, cx), None);
            input.replace_text_in_range(Some(0..5), "Changed", window, cx);
            input.unmark_text(window, cx);
            assert_eq!(input.value(), "Saved 日本語 🦀");
        });
    })
    .unwrap();
}
