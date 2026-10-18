//! Provider tokens are typed here and sent once to the local service, which
//! stores them encrypted with the install key. The workspace returns only where
//! each credential comes from, never the value, so nothing here can show it again.
use super::*;

pub(super) const INTEGRATIONS: [(&str, &str); 2] = [("jira", "Jira"), ("bitbucket", "Bitbucket")];

pub(super) struct CredentialInputs {
    pub(super) jira: Entity<InputState>,
    pub(super) bitbucket: Entity<InputState>,
}
impl CredentialInputs {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let input =
            |placeholder: &'static str, window: &mut Window, cx: &mut Context<Workbench>| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder(placeholder)
                })
            };
        Self {
            jira: input("Paste a Jira access token", window, cx),
            bitbucket: input("Paste a Bitbucket access token", window, cx),
        }
    }
    pub(super) fn get(&self, integration: &str) -> &Entity<InputState> {
        if integration == "jira" {
            &self.jira
        } else {
            &self.bitbucket
        }
    }
}
/// A hint whose identity is per integration, since rows repeat.
fn row_hint(id: String, value: impl Into<SharedString>) -> Div {
    div()
        .text_sm()
        .text_color(rgb(0x9eacb9))
        .child(crate::accessibility::named(id, value))
}
/// The status line for one integration, as reported by the service.
pub(super) fn status(workspace: &Value, integration: &str) -> String {
    workspace["credentials"][integration]["label"]
        .as_str()
        .unwrap_or("Credential status unavailable until the service connects.")
        .to_owned()
}
/// Completion text for a credential command; never echoes the request.
pub(super) fn saved_notice(path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix("/credentials/")?;
    Some(if rest.ends_with("/clear") {
        "Saved token removed. Environment overrides are unchanged."
    } else {
        "Token saved encrypted on this machine. Crewroom will not show it again."
    })
}
impl Workbench {
    pub(super) fn credentials_section(&self, cx: &mut Context<Self>) -> Div {
        let mut panel = section("Credentials").child(hint(
            "Tokens are encrypted with this install's crewroom.secret and stay inside the local service. Agents and this window never receive a saved token. An environment variable, when set for the service, overrides the saved token.",
        ));
        for (integration, name) in INTEGRATIONS {
            panel = panel.child(self.credential_row(integration, name, cx));
        }
        panel
    }
    fn credential_row(&self, integration: &'static str, name: &str, cx: &mut Context<Self>) -> Div {
        let facts = &self.workspace["credentials"][integration];
        let unreadable = facts["stored"] == "unreadable";
        let stored = facts["stored"].as_str().unwrap_or("none") != "none";
        let mut row = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(crate::accessibility::named(
                format!("{integration}-token-name"),
                format!("{name} token"),
            ))
            .child(row_hint(
                format!("{integration}-token-status"),
                status(&self.workspace, integration),
            ));
        if facts["source"] == "environment" && stored {
            row = row.child(row_hint(
                format!("{integration}-token-override"),
                if unreadable {
                    "The saved token is unreadable; reconnect before removing the environment override."
                } else {
                    "A saved token is kept but unused while the override is set."
                },
            ));
        }
        let input = self.credential_inputs.get(integration).clone();
        row.child(
            Input::new(&input)
                .id(SharedString::from(format!("{integration}-token")))
                .disabled(self.blocked()),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("save-{integration}-token")))
                        .primary()
                        .label(if unreadable {
                            "Reconnect"
                        } else {
                            "Save token"
                        })
                        .disabled(self.blocked())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let input = this.credential_inputs.get(integration).clone();
                            let token = input.read(cx).value().trim().to_owned();
                            if token.is_empty() {
                                this.fail_at("Paste a token before saving.", feedback::Place::Top);
                                cx.notify();
                                return;
                            }
                            // Drop the typed value from the form as soon as it is sent.
                            input.update(cx, |state, cx| state.set_value("", window, cx));
                            this.send(
                                format!("/credentials/{integration}"),
                                json!({"token": token, "replaceUnreadable": unreadable}),
                                false,
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("clear-{integration}-token")))
                        .ghost()
                        .label("Remove saved token")
                        .disabled(self.blocked() || !stored)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                format!("/credentials/{integration}/clear"),
                                json!({}),
                                false,
                                cx,
                            )
                        })),
                ),
        )
    }
}
