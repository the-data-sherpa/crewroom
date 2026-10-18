//! Readable text needs a GPUI identity and role; raw string children have neither.
use gpui_kit::{Div, FocusHandle, IntoElement, Role, SharedString, Stateful, div, prelude::*};

/// Stable identities are independent of the displayed value, so reconnect/status
/// updates change the existing accessibility node rather than replacing it.
pub(crate) fn readable(
    id: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Stateful<Div> {
    let value = value.into();
    div()
        .id(id.into())
        .role(Role::Label)
        .aria_label(value.clone())
        .child(value)
}

/// A label with a caller-chosen identity, for rows and facts that repeat or change value.
/// Tests can locate it by that identity.
pub(crate) fn named(
    id: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    let label = readable(id, value);
    #[cfg(test)]
    {
        use gpui_kit::test::TestSupportExt as _;
        label.test_support()
    }
    #[cfg(not(test))]
    label
}

/// An identified container that tests can locate, such as a feedback banner.
pub(crate) fn observable(element: Stateful<Div>) -> impl IntoElement {
    #[cfg(test)]
    {
        use gpui_kit::test::TestSupportExt as _;
        element.test_support()
    }
    #[cfg(not(test))]
    element
}

/// A keyboard destination region; tests can ask whether focus is inside it.
pub(crate) fn focus_region(element: Stateful<Div>, focus: &FocusHandle) -> impl IntoElement {
    #[cfg(test)]
    {
        use gpui_kit::test::TestSupportExt as _;
        element.test_support().track_focus(focus)
    }
    #[cfg(not(test))]
    element.track_focus(focus)
}

/// A call site identifies one label. Repeated rows must have their own parent ID.
#[track_caller]
pub(crate) fn text(value: impl Into<SharedString>) -> impl IntoElement {
    let site = std::panic::Location::caller();
    let label = readable(
        format!("text:{}:{}:{}", site.file(), site.line(), site.column()),
        value,
    );
    #[cfg(test)]
    {
        use gpui_kit::test::TestSupportExt as _;
        label.test_support()
    }
    #[cfg(not(test))]
    label
}

/// GPUI 0.3.7 makes its NSWindow the first responder. Forward its native AX focus
/// query to the AccessKit content view; otherwise macOS reports the whole window.
pub(crate) fn install_native_focus_bridge() {
    #[cfg(target_os = "macos")]
    {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            // SAFETY: called after GPUI creates its pinned GPUIWindow NSWindow
            // subclass. AccessKit is statically linked and never unloaded.
            unsafe { accesskit_macos::add_focus_forwarder_to_window_class("GPUIWindow") };
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        AppContext, Context, Render, Window,
        test::{TestSupportExt, TestWindowExt},
    };
    struct Status;
    impl Render for Status {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(readable("cause", "Process ownership is uncertain").test_support())
                .child(
                    readable("status", "Paused · process state is separate from Jira")
                        .test_support(),
                )
                .child(
                    readable(
                        "readiness",
                        "Not ready · required review does not cover HEAD",
                    )
                    .test_support(),
                )
        }
    }
    #[gpui_kit::test]
    fn readable_evidence_and_status_have_accessibility_labels(cx: &mut gpui_kit::TestAppContext) {
        let (handle, _) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Status)).unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            for (id, label) in [
                ("cause", "Process ownership is uncertain"),
                ("status", "Paused · process state is separate from Jira"),
                (
                    "readiness",
                    "Not ready · required review does not cover HEAD",
                ),
            ] {
                let snapshot = window.find(id);
                assert_eq!(snapshot.role(), Some(Role::Label));
                assert_eq!(snapshot.label(), Some(label));
            }
        })
        .unwrap();
    }
}
