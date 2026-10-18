//! The session's agent area: coordinator above at most three specialist slots.
//! Only an admitted attempt with a terminal process gets a terminal; proposals and empty slots never do.
use super::*;
use crate::wall::{Pane, Terminal, model_label};
use std::collections::{HashMap, HashSet};

const SPECIALIST_SLOTS: usize = 3;

/// Session agent state. Runtime and events come from the workbench's own run observation;
/// this only adds read-only terminal panes for attempts that have a terminal process.
#[derive(Default)]
pub(super) struct Agents {
    run: Option<String>,
    panes: Vec<Pane>,
    requested: HashSet<String>,
    attachments: HashMap<String, Result<String, String>>,
    errors: HashMap<String, String>,
    pub(super) maximized: Option<String>,
    timeline: HashSet<String>,
    pub(super) picker: bool,
}

/// Owned view data for one admitted attempt, read before building elements.
struct Tile {
    attempt: String,
    name: String,
    model: String,
    state: crate::agent_state::StateLabel,
    observation: &'static str,
    terminal: Option<Entity<Terminal>>,
    note: String,
    timeline: Vec<String>,
}

fn event_line(item: &Value) -> Option<String> {
    let event = &item["event"];
    let kind = event["type"].as_str().unwrap_or("event");
    let content = match kind {
        "message_update" => return None,
        "message_end" => records(&event["message"], "content")
            .iter()
            .filter(|p| p["type"] == "text")
            .map(|p| string(p, "text"))
            .collect::<Vec<_>>()
            .join(" "),
        _ => {
            let mut raw = event.to_string();
            if raw.len() > 160 {
                raw.truncate(raw.floor_char_boundary(160));
                raw.push('…');
            }
            raw
        }
    };
    (!content.is_empty()).then(|| format!("#{} · {kind} · {content}", item["cursor"]))
}

impl Workbench {
    /// Current, unretired attempts of the observed run: the coordinator plus at most three specialists.
    fn current_attempts(&self) -> Vec<Value> {
        let assignments = records(&self.observation.runtime, "assignments");
        records(&self.observation.runtime, "attempts")
            .into_iter()
            .filter(|a| {
                assignments
                    .iter()
                    .any(|s| s["currentAttemptId"] == a["id"] && s["retiredAt"].is_null())
            })
            .take(1 + SPECIALIST_SLOTS)
            .collect()
    }

    /// Requests attachments for attempts with a terminal pane and starts their read-only surfaces.
    fn sync_terminals(&mut self, run_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.agents.run.as_deref() != Some(run_id) {
            self.agents = Agents {
                run: Some(run_id.to_owned()),
                picker: self.agents.picker,
                ..Agents::default()
            };
        }
        let attempts = self.current_attempts();
        self.agents
            .panes
            .retain(|p| attempts.iter().any(|a| a["id"] == p.id));
        let Some(connection) = self.client.as_ref().map(Client::descriptor_path) else {
            return;
        };
        for attempt in attempts.iter().filter(|a| a["pane"].is_string()) {
            let id = string(attempt, "id");
            if !self.agents.requested.insert(id.clone()) {
                continue;
            }
            let (path, run) = (connection.clone(), run_id.to_owned());
            cx.spawn(async move |this, cx| {
                let attempt = id.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { crate::wall::attachment(&path, &run, &attempt) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.agents.attachments.insert(id, result);
                    cx.notify();
                });
            })
            .detach();
        }
        let ready: Vec<_> = self.agents.attachments.drain().collect();
        for (id, result) in ready {
            let spawned = result.map_err(anyhow::Error::msg).and_then(|attachment| {
                crate::wall::spawn_terminal(&connection, run_id, &id, &attachment, window, cx)
            });
            match spawned {
                Ok(terminal) => {
                    self.agents.errors.remove(&id);
                    self.agents.panes.push(Pane { id, terminal });
                }
                Err(error) => {
                    self.agents.errors.insert(id, error.to_string());
                }
            }
        }
    }

    fn tiles(&self) -> (Option<Tile>, Vec<Tile>) {
        let runtime = &self.observation.runtime;
        let connected = self.connected && self.observation.runtime_fresh;
        let assignments = records(runtime, "assignments");
        let observed = records(runtime, "observedAttemptIds");
        let degraded = records(runtime, "degradedAttemptIds");
        let (mut coordinator, mut specialists) = (None, vec![]);
        for attempt in self.current_attempts() {
            let id = string(&attempt, "id");
            let assignment = assignments
                .iter()
                .find(|s| s["id"] == attempt["assignmentId"])
                .cloned()
                .unwrap_or(Value::Null);
            let tile = Tile {
                name: if assignment["choreography"].is_object() {
                    format!("{} · {} · {}",string(&assignment,"name"),string(&assignment["choreography"]["responsibility"],"responsibility"),records(&assignment["choreography"]["responsibility"],"packages").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                } else {string(&assignment,"name")},
                model: model_label(&attempt)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .replace("Model: ", ""),
                state: crate::agent_state::of(&attempt),
                observation: if !connected {
                    "retained observation"
                } else if degraded.contains(&attempt["id"]) {
                    "observation degraded"
                } else if observed.contains(&attempt["id"]) {
                    "observed"
                } else {
                    "ownership unconfirmed"
                },
                terminal: self
                    .agents
                    .panes
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| p.terminal.clone()),
                note: self.agents.errors.get(&id).cloned().unwrap_or_else(|| {
                    if attempt["pane"].is_string() {
                        "Connecting the read-only terminal…".into()
                    } else {
                        "No terminal process yet. A terminal appears once this agent's process starts.".into()
                    }
                }),
                timeline: self
                    .observation
                    .events
                    .iter()
                    .filter(|e| e["attemptId"] == attempt["id"])
                    .filter_map(event_line)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .take(120)
                    .rev()
                    .collect(),
                attempt: id,
            };
            if assignment["coordinator"] == true {
                coordinator = Some(tile);
            } else {
                specialists.push(tile);
            }
        }
        (coordinator, specialists)
    }

    fn agent_tile(&self, tile: &Tile, height: f32, cx: &mut Context<Self>) -> Div {
        let id = tile.attempt.clone();
        let maximized = self.agents.maximized.as_deref() == Some(id.as_str());
        let timeline = self.agents.timeline.contains(&id);
        let (tone, label) = (tile.state.tone, &tile.state.text);
        let (mode, toggle, max, inspect) = (id.clone(), id.clone(), id.clone(), id.clone());
        // A tile without a terminal or timeline only needs room for its explanation.
        let height = if timeline || tile.terminal.is_some() {
            height
        } else {
            88.
        };
        let mut body = div()
            .h(px(height))
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0x0b1218));
        if timeline {
            let mut list = div()
                .id(SharedString::from(format!("timeline-{id}")))
                .size_full()
                .overflow_y_scroll()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .text_xs();
            if tile.timeline.is_empty() {
                list = list.child(hint("No recorded events for this attempt yet."));
            }
            for (i, line) in tile.timeline.iter().enumerate() {
                list = list.child(crate::accessibility::named(
                    format!("timeline-{id}-{i}"),
                    line.clone(),
                ));
            }
            body = body.child(list);
        } else if let Some(terminal) = &tile.terminal {
            body = body.child(terminal.clone());
        } else {
            body = body.p_3().child(crate::accessibility::named(
                format!("agent-note-{id}"),
                tile.note.clone(),
            ));
        }
        div()
            .flex_1()
            .min_w(px(280.))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x293642))
            .bg(rgb(0x17212a))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(0x293642))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_baseline()
                            .gap_2()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(
                                crate::accessibility::named(
                                    format!("agent-name-{id}"),
                                    tile.name.clone(),
                                ),
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w_full()
                                    .text_xs()
                                    .text_color(rgb(0x9eacb9))
                                    .child(crate::accessibility::named(
                                        format!("agent-model-{id}"),
                                        tile.model.clone(),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(140.))
                                    .text_xs()
                                    .text_color(rgb(tone))
                                    .child(crate::accessibility::named(
                                        format!("agent-state-{id}"),
                                        format!("{label} · {}", tile.observation),
                                    )),
                            )
                            .child(
                                Button::new(SharedString::from(format!("agent-details-{id}")))
                                    .ghost()
                                    .label("Details")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.observation.selected_attempt = Some(inspect.clone());
                                        this.tab = "History";
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("agent-view-{id}")))
                                    .ghost()
                                    .label(if timeline { "Terminal" } else { "Timeline" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.agents.timeline.remove(&mode) {
                                            this.agents.timeline.insert(mode.clone());
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("agent-maximize-{toggle}")))
                                    .ghost()
                                    .label(if maximized { "Restore" } else { "Maximize" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.agents.maximized =
                                            if maximized { None } else { Some(max.clone()) };
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(body)
    }

    fn proposed_tile(index: usize, specialist: &Value) -> Div {
        div()
            .flex_1()
            .min_w(px(280.))
            .min_h(px(160.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_dashed()
            .border_color(rgb(0x5a4826))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(crate::accessibility::named(
                        format!("proposed-name-{index}"),
                        format!("{} · {}", string(&specialist["role"], "name"), string(specialist, "name")),
                    )))
                    .child(div().text_xs().text_color(rgb(0xf1bf73)).child(crate::accessibility::named(format!("proposed-state-{index}"), "Proposed · not started"))),
            )
            .child(div().text_xs().text_color(rgb(0x9eacb9)).child(crate::accessibility::named(
                format!("proposed-note-{index}"),
                "No process exists yet. The coordinator decides at the next safe coordination point.",
            )))
    }

    /// Coordinator pane with steering, specialist slots, the role picker and the latest checks.
    pub(super) fn agent_session(
        &mut self,
        run: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let run_id = string(run, "id");
        let refinement =
            ["explore", "grill"].contains(&run["purpose"].as_str().unwrap_or("coding"));
        let wide = window.viewport_size().width >= px(1500.);
        let observed = self.observation.id.as_deref() == Some(run_id.as_str());
        if observed {
            self.sync_terminals(&run_id, window, cx);
        }
        let (coordinator, specialists) = if observed {
            self.tiles()
        } else {
            (None, vec![])
        };
        // Only rendered panes stay visible; a hidden Ghostty surface must not keep drawing.
        let shown: HashSet<String> = match &self.agents.maximized {
            Some(id) => HashSet::from([id.clone()]),
            None => coordinator
                .iter()
                .chain(&specialists)
                .map(|t| t.attempt.clone())
                .collect(),
        };
        let connected = self.connected && self.observation.runtime_fresh;
        for pane in &self.agents.panes {
            let visible =
                connected && shown.contains(&pane.id) && !self.agents.timeline.contains(&pane.id);
            pane.terminal.update(cx, |t, _| t.set_visible(visible));
        }
        if let Some(id) = self.agents.maximized.clone() {
            if let Some(tile) = coordinator
                .iter()
                .chain(&specialists)
                .find(|t| t.attempt == id)
            {
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(self.agent_tile(tile, 620., cx));
            }
            self.agents.maximized = None;
        }
        let conversation = self.conversation(run, window, cx);
        let coordinator_pane = match &coordinator {
            Some(tile) => self.agent_tile(tile, 300., cx),
            None => section("Coordinator").child(hint(if !observed || !self.observation.runtime_fresh {
                "Loading the run observation · agent state may be stale."
            } else {
                "No coordinator is admitted for this run. Queued plans are not executing agents."
            })),
        };
        // Wide: agents on the left, steering on the right. Narrow: one column.
        let place = |agents: Div, conversation: Div| {
            if wide {
                div()
                    .flex()
                    .gap_4()
                    .items_start()
                    .child(agents.flex_1().min_w_0())
                    .child(div().w(px(460.)).flex_shrink_0().child(conversation))
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(agents)
                    .child(conversation)
            }
        };
        let mut area = div().flex().flex_col().gap_4().child(coordinator_pane);
        if refinement {
            return place(area, conversation);
        }
        let proposals: Vec<Value> = records(&self.observation.runtime, "teamProposals")
            .into_iter()
            .filter(|p| p["status"] == "pending")
            .flat_map(|p| records(&p, "specialists"))
            .collect();
        let used = specialists.len() + proposals.len();
        let mut grid = div().flex().flex_wrap().gap_3();
        for tile in &specialists {
            grid = grid.child(self.agent_tile(tile, 240., cx));
        }
        for (index, specialist) in proposals.iter().enumerate() {
            grid = grid.child(Self::proposed_tile(index, specialist));
        }
        for slot in used..SPECIALIST_SLOTS {
            grid = grid.child(
                div()
                    .flex_1()
                    .min_w(px(280.))
                    .min_h(px(160.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_dashed()
                    .border_color(rgb(0x34495a))
                    .child(
                        Button::new(SharedString::from(format!("add-specialist-slot-{slot}")))
                            .label("+ Add a specialist")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.agents.picker = true;
                                cx.notify();
                            })),
                    )
                    .child(div().text_xs().text_color(rgb(0x9eacb9)).child(
                        crate::accessibility::named(
                            format!("specialist-slot-note-{slot}"),
                            format!(
                                "{} of {SPECIALIST_SLOTS} specialist slots free",
                                SPECIALIST_SLOTS - used
                            ),
                        ),
                    )),
            );
        }
        let picker_open = self.agents.picker;
        area = area
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(crate::accessibility::text("Specialists")))
                    .child(hint(format!("{} of {SPECIALIST_SLOTS} assigned or proposed · terminals are read-only; steer through the coordinator", used.min(SPECIALIST_SLOTS))).min_w_0().max_w_full())
                    .child(div().flex_1())
                    .child(
                        Button::new("open-specialist-picker")
                            .label(if picker_open { "Close specialist picker" } else { "Add a specialist" })
                            .selected(picker_open)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.agents.picker = !picker_open;
                                cx.notify();
                            })),
                    ),
            )
            .child(grid);
        if picker_open {
            area = area.child(self.insertion(run, window, cx));
        }
        place(area.child(self.check_strip()), conversation)
    }

    /// The latest candidate's required checks, each naming the revision it covers.
    fn check_strip(&self) -> Div {
        let result = &self.observation.result;
        let definitions = records(result, "requiredChecks");
        let mut strip = div().flex().flex_wrap().items_center().gap_2().child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(crate::accessibility::text("Required checks")),
        );
        let Some(candidate) = records(result, "candidates").last().cloned() else {
            return strip.child(hint(if definitions.is_empty() {
                "No candidate revision yet."
            } else {
                "No candidate revision yet; checks run once one exists."
            }));
        };
        let head = string(&candidate["revision"], "head");
        let revision = &head[..head.len().min(8)];
        let checks = records(result, "checks");
        for definition in &definitions {
            let check = checks
                .iter()
                .rev()
                .find(|c| c["candidateId"] == candidate["id"] && c["checkId"] == definition["id"]);
            let label = results::check_label(check, definition);
            let tone = match label {
                "Passed" => 0x84d7bc,
                "Not run" | "Out of date" => 0x9eacb9,
                _ => 0xffb0a2,
            };
            strip = strip.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x293642))
                    .text_xs()
                    .text_color(rgb(tone))
                    .child(crate::accessibility::named(
                        format!("check-strip-{}", string(definition, "id")),
                        format!("{} · {label} @ {revision}", string(definition, "id")),
                    )),
            );
        }
        if definitions.is_empty() {
            strip = strip.child(hint("No required checks configured."));
        }
        strip
    }
}
