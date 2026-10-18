//! Provider tokens are typed here and sent once to the local service, which
//! stores them encrypted with the install key. The workspace returns only where
//! each credential comes from, never the value, so nothing here can show it again.
use super::*;

pub(super) const INTEGRATIONS: [(&str, &str); 2] = [("jira", "Jira"), ("bitbucket", "Bitbucket")];

pub(super) struct CredentialInputs {
    pub(super) jira: Entity<InputState>,
    pub(super) bitbucket: Entity<InputState>,
    pub(super) github: Entity<InputState>,
    pub(super) github_repository: Entity<InputState>,
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
            github: input("Paste a GitHub personal access token", window, cx),
            github_repository: cx
                .new(|cx| InputState::new(window, cx).placeholder("owner/repository")),
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
/// Completion text for a GitHub command; the state line carries the result.
pub(super) fn github_notice(path: &str, value: &Value) -> Option<String> {
    let state = value["github"]["state"].as_str().unwrap_or("");
    Some(match path {
        "/github/connect" => "GitHub accepted the token. Saved encrypted on this machine; Crewroom will not show it again.".into(),
        "/github/check" => match state {
            "connected" => "GitHub check finished: connected.".into(),
            "rejected" => "GitHub check finished: the token is expired or revoked.".into(),
            _ => "GitHub check finished: connection not confirmed.".into(),
        },
        "/github/disconnect" => {
            "Saved GitHub token removed. An environment override, if set, is unchanged.".into()
        }
        _ => return None,
    })
}
/// One line describing the connected account, without any token material.
pub(super) fn github_account(github: &Value) -> Option<String> {
    let account = github.get("account")?;
    let login = account["login"].as_str()?;
    let mut line = format!("Account: @{login}");
    if let Some(name) = account["name"].as_str() {
        line.push_str(&format!(" ({name})"));
    }
    if let Some(kind) = github["tokenKind"].as_str() {
        line.push_str(&format!(" · {kind} token"));
    }
    if let Some(at) = github["checkedAt"].as_str() {
        line.push_str(&format!(" · checked {at}"));
    }
    Some(line)
}
impl Workbench {
    pub(super) fn github_section(&self, cx: &mut Context<Self>) -> Div {
        let github = &self.workspace["github"];
        let state = github["state"].as_str().unwrap_or("");
        let stored = self.workspace["credentials"]["github"]["stored"]
            .as_str()
            .unwrap_or("none");
        let reconnect = state == "unreadable" || stored == "unreadable";
        let checkable = !matches!(state, "" | "disconnected" | "unreadable");
        let mut panel = section("GitHub")
            .child(hint(
                "Connect with a personal access token. Fine-grained token: choose the repositories under Repository access and grant Metadata (read), Contents, Issues and Pull requests (read and write). Classic token: the repo scope. Crewroom checks the token with GitHub before saving it encrypted, and never shows it again.",
            ))
            .child(row_hint(
                "github-state".into(),
                github["label"]
                    .as_str()
                    .unwrap_or("GitHub status unavailable until the service connects.")
                    .to_owned(),
            ));
        if let Some(line) = github_account(github) {
            panel = panel.child(row_hint("github-account".into(), line));
        }
        for (i, note) in records(github, "notes").iter().enumerate() {
            if let Some(note) = note.as_str() {
                panel = panel.child(row_hint(format!("github-note-{i}"), note.to_owned()));
            }
        }
        if github["source"] == "environment" {
            panel = panel.child(row_hint(
                "github-override".into(),
                if stored == "none" {
                    "Using CREWROOM_GITHUB_TOKEN from the server environment."
                } else {
                    "Using CREWROOM_GITHUB_TOKEN from the server environment; the saved token is kept but unused."
                },
            ));
        }
        if let Some(label) = github["repository"]["label"].as_str() {
            panel = panel.child(row_hint(
                "github-repository-access".into(),
                label.to_owned(),
            ));
        }
        let token = self.credential_inputs.github.clone();
        let repository = self.credential_inputs.github_repository.clone();
        panel
            .child(
                Input::new(&token)
                    .id("github-token")
                    .disabled(self.blocked()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("connect-github")
                            .primary()
                            .label(if reconnect || state == "rejected" {
                                "Reconnect GitHub"
                            } else {
                                "Connect GitHub"
                            })
                            .disabled(self.blocked())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let input = this.credential_inputs.github.clone();
                                let token = input.read(cx).value().trim().to_owned();
                                if token.is_empty() {
                                    this.fail_at(
                                        "Paste a GitHub personal access token before connecting.",
                                        feedback::Place::Top,
                                    );
                                    cx.notify();
                                    return;
                                }
                                // Drop the typed value from the form as soon as it is sent.
                                input.update(cx, |state, cx| state.set_value("", window, cx));
                                this.send(
                                    "/github/connect".into(),
                                    json!({"token": token, "replaceUnreadable": reconnect}),
                                    false,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("check-github")
                            .label("Check connection")
                            .disabled(self.blocked() || !checkable)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send("/github/check".into(), json!({}), false, cx)
                            })),
                    )
                    .child(
                        Button::new("disconnect-github")
                            .ghost()
                            .label("Disconnect")
                            .disabled(self.blocked() || stored == "none")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send("/github/disconnect".into(), json!({}), false, cx)
                            })),
                    ),
            )
            .child(crate::accessibility::text("Check access to a repository"))
            .child(
                Input::new(&repository)
                    .id("github-repository")
                    .disabled(self.blocked()),
            )
            .child(
                Button::new("check-github-repository")
                    .label("Check repository access")
                    .disabled(self.blocked() || !checkable)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let name = this
                            .credential_inputs
                            .github_repository
                            .read(cx)
                            .value()
                            .trim()
                            .to_owned();
                        if name.is_empty() {
                            this.fail_at(
                                "Enter the repository as owner/name.",
                                feedback::Place::Top,
                            );
                            cx.notify();
                            return;
                        }
                        this.send(
                            "/github/check".into(),
                            json!({"repository": name}),
                            false,
                            cx,
                        )
                    })),
            )
    }
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
