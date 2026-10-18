//! A run-scoped observation window. Closing it drops observer clients only.
mod transport;
use gpui::base::{Disableable, Selectable};
use gpui::component::{
    button::{Button, ButtonVariants},
    input::{Textarea, TextareaState},
};
use gpui::{prelude::*, *};
use gpui_kit as gpui;
use gpui_libghostty::{TerminalColor, TerminalConfiguration, TerminalOptions, TerminalTheme};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
pub(crate) use transport::observe;
gpui_libghostty::bind_gpui!(gpui);
gpui::actions!(crewroom_wall, [Next, Previous]);
thread_local! {static WINDOWS:RefCell<HashMap<String,AnyWindowHandle>>=RefCell::new(HashMap::new());}
/// The label of every control that opens this window.
pub(crate) const OPEN_LABEL: &str = "Open agent wall";
/// What the wall's run works on: a ticket key and title, or a comparison case.
#[derive(Clone, Default)]
pub(crate) struct Subject {
    pub(crate) key: String,
    pub(crate) title: String,
}
/// Names the ticket so concurrent walls are distinguishable in the window switcher.
pub(crate) fn window_title(subject: &Subject) -> String {
    if subject.key.is_empty() {
        "Crewroom · Agent wall".into()
    } else {
        format!("{} · Agent wall", subject.key)
    }
}
/// Points steering to the workbench and names the copy key the read-only terminal accepts.
pub(crate) fn status_hint(macos: bool) -> String {
    format!(
        "Read-only. To direct agents, message the coordinator in the workbench Conversation. Drag to select, {} to copy. Terminals show the live screen only; scroll back in Read transcript.",
        if macos { "Cmd+C" } else { "Ctrl+Shift+C" }
    )
}
/// The text a tile shows in place of its terminal; `None` when the live terminal is shown.
fn pane_notice(
    has_pane: bool,
    connected: bool,
    connecting: bool,
    attached: bool,
) -> Option<String> {
    Some(
        if attached && connected {
            return None;
        } else if attached {
            "Terminal hidden while disconnected. Use Read transcript for saved output."
        } else if !has_pane {
            "Assignment has no admitted terminal process."
        } else if connecting {
            "Attaching terminal…"
        } else {
            "Terminal not attached yet. Select Connect terminals to watch this agent."
        }
        .into(),
    )
}
/// Names an event kind in words, e.g. a tool call by its tool.
fn event_summary(event: &Value) -> String {
    let kind = text(event, "type");
    let tool = text(event, "toolName");
    match kind.as_str() {
        "tool_execution_start" if !tool.is_empty() => format!("Tool call: {tool}"),
        "tool_execution_end" if !tool.is_empty() => format!(
            "Tool result: {tool}{}",
            if event["isError"] == true {
                " · error"
            } else {
                ""
            }
        ),
        _ if !tool.is_empty() => format!("{} · {tool}", kind.replace('_', " ")),
        _ => kind.replace('_', " "),
    }
}

pub(crate) fn open(
    connection: PathBuf,
    run: String,
    subject: Subject,
    cx: &mut App,
) -> anyhow::Result<()> {
    let key = format!("{}:{run}", connection.display());
    if WINDOWS
        .with(|w| w.borrow().get(&key).copied())
        .is_some_and(|w| w.update(cx, |_, w, _| w.activate_window()).is_ok())
    {
        return Ok(());
    }
    cx.bind_keys([
        KeyBinding::new("tab", Next, Some("CrewroomWall")),
        KeyBinding::new("shift-tab", Previous, Some("CrewroomWall")),
    ]);
    let (handle, _) = gpui::open_window(
        WindowOptions {
            // Activate only after the native accessibility adapter is installed.
            focus: false,
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1240.), px(850.)),
                cx,
            ))),
            window_min_size: Some(size(px(390.), px(600.))),
            titlebar: Some(TitlebarOptions {
                title: Some(window_title(&subject).into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        cx,
        move |w, cx| cx.new(|cx| Wall::new(connection, run, subject, w, cx)),
    )?;
    handle.update(cx, |_, window, _| window.activate_window())?;
    WINDOWS.with(|w| w.borrow_mut().insert(key, handle));
    Ok(())
}
enum Update {
    Snapshot(Value, Value),
    Attachments(Vec<(String, Result<Value, String>)>),
    Error(String),
}
struct Observation {
    alive: Arc<AtomicBool>,
    connect: mpsc::Sender<Vec<String>>,
}
impl Drop for Observation {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}
fn observation(path: PathBuf, run: String) -> (Observation, async_channel::Receiver<Update>) {
    let (send, recv) = async_channel::bounded(2);
    let (connect, requests) = mpsc::channel::<Vec<String>>();
    let alive = Arc::new(AtomicBool::new(true));
    let live = alive.clone();
    std::thread::spawn(move || {
        let mut cursor = 0;
        while live.load(Ordering::Relaxed) {
            while let Ok(ids) = requests.try_recv() {
                let rows = ids
                    .into_iter()
                    .map(|id| {
                        let result =
                            transport::get(&path, &format!("/runs/{run}/attempts/{id}/terminal"))
                                .map_err(|e| e.to_string());
                        (id, result)
                    })
                    .collect();
                if send.send_blocking(Update::Attachments(rows)).is_err() {
                    return;
                }
            }
            let result = (|| -> anyhow::Result<_> {
                let runtime = transport::get(&path, &format!("/runs/{run}/runtime"))?;
                let events = transport::get(
                    &path,
                    &format!("/runs/{run}/events?after={cursor}&limit=200"),
                )?;
                Ok((runtime, events))
            })();
            let mut caught_up = true;
            let event = match result {
                Ok((runtime, events)) => {
                    let next = events["after"].as_u64().unwrap_or(cursor);
                    caught_up = events["hasMore"] != true;
                    cursor = next;
                    Update::Snapshot(runtime, events)
                }
                Err(e) => Update::Error(e.to_string()),
            };
            if send.send_blocking(event).is_err() {
                break;
            }
            if caught_up {
                for _ in 0..10 {
                    if !live.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    });
    (Observation { alive, connect }, recv)
}
pub(crate) struct Pane {
    pub(crate) id: String,
    pub(crate) terminal: Entity<Terminal>,
}
/// Observes one run's agents and owns their read-only terminal panes. The agent wall window
/// renders it directly; the workbench session holds its own instance as a feed.
pub(crate) struct Wall {
    connection: PathBuf,
    run: String,
    subject: Subject,
    focus: FocusHandle,
    pub(crate) runtime: Value,
    pub(crate) events: VecDeque<Value>,
    event_bytes: usize,
    truncated: bool,
    pub(crate) panes: Vec<Pane>,
    pub(crate) errors: HashMap<String, String>,
    pub(crate) error: String,
    pub(crate) connected: bool,
    /// Set by the first successful snapshot; until then no agent list has been read.
    loaded: bool,
    pub(crate) connecting: bool,
    transcript: Entity<TextareaState>,
    transcript_mode: bool,
    transcript_dirty: bool,
    filter: Option<String>,
    observer: Observation,
    _updates: Task<()>,
}
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").into()
}
fn rows(v: &Value, key: &str) -> Vec<Value> {
    v[key].as_array().cloned().unwrap_or_default()
}
fn label(id: impl Into<SharedString>, text: impl Into<SharedString>) -> impl IntoElement {
    let text = text.into();
    let label = div()
        .id(id.into())
        .role(Role::Label)
        .aria_label(text.clone())
        .child(text);
    #[cfg(test)]
    {
        use gpui_kit::test::TestSupportExt as _;
        label.test_support()
    }
    #[cfg(not(test))]
    label
}
pub(crate) fn model_label(attempt: &Value) -> String {
    let model = &attempt["model"];
    let known = |value: &Value| value.as_str().unwrap_or("unavailable").to_owned();
    let mut label = format!(
        "Model: {} · {} · Attempt {}\nRequested: {} · {}",
        if let Some(observed) = attempt["observedModel"].as_str() {
            format!("{} → {observed}", known(&model["resolved"]["selector"]))
        } else {
            known(&model["resolved"]["selector"])
        },
        known(&model["resolved"]["effort"]),
        attempt["generation"],
        known(&model["requested"]["model"]),
        known(&model["requested"]["effort"])
    );
    if let Some(fallback) = model["fallbackFrom"].as_str() {
        label.push_str(&format!("\nConfigured fallback from {fallback}"));
    }
    label
}
/// Fetches the service's read-only attachment for one attempt's terminal pane.
pub(crate) fn attachment(
    connection: &std::path::Path,
    run: &str,
    attempt: &str,
) -> Result<String, String> {
    transport::get(
        connection,
        &format!("/runs/{run}/attempts/{attempt}/terminal"),
    )
    .map(|v| v.to_string())
    .map_err(|e| e.to_string())
}
/// Starts a read-only Ghostty surface that observes the attempt through this executable.
pub(crate) fn spawn_terminal<T: 'static>(
    connection: &std::path::Path,
    run: &str,
    id: &str,
    attachment: &str,
    window: &mut Window,
    cx: &mut Context<T>,
) -> anyhow::Result<Entity<Terminal>> {
    let a: transport::Attachment = serde_json::from_str(attachment)?;
    a.validate(run, id)?;
    let executable = std::env::current_exe()?;
    let command = format!(
        "{} --observe-terminal {} {} {}",
        transport::quote(&executable.to_string_lossy()),
        transport::quote(&connection.to_string_lossy()),
        transport::quote(run),
        transport::quote(id)
    );
    let mut options = TerminalOptions::new(command, a.directory);
    options.read_only = true;
    options.quiet_login = true;
    options.focus_on_spawn = false;
    let colors = [
        0x17232c, 0xf0787e, 0x7ad8ad, 0xf0c674, 0x86b7f5, 0xc7a4ed, 0x6fd4d4, 0xd9e5ed, 0x607582,
        0xff9398, 0x99ecc5, 0xffdda0, 0xadcfff, 0xdbc0ff, 0x9eeeee, 0xf2f7fa,
    ]
    .map(|v| TerminalColor::new((v >> 16) as u8, (v >> 8) as u8, v as u8));
    options.configuration = TerminalConfiguration::Custom(TerminalTheme::new(
        TerminalColor::new(11, 18, 24),
        TerminalColor::new(218, 230, 239),
        colors,
    ));
    Terminal::spawn(options, window, cx).map_err(anyhow::Error::msg)
}
impl Wall {
    fn new(
        connection: PathBuf,
        run: String,
        subject: Subject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut wall = Self::feed(connection, run, window, cx);
        wall.subject = subject;
        window.focus(&wall.focus, cx);
        wall
    }
    /// An observer for embedding: it does not take keyboard focus from its host view.
    pub(crate) fn feed(
        connection: PathBuf,
        run: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (observer, updates) = observation(connection.clone(), run.clone());
        let focus = cx.focus_handle();
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = updates.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.accept(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            connection,
            run,
            subject: Subject::default(),
            focus,
            runtime: Value::Null,
            events: VecDeque::new(),
            event_bytes: 0,
            truncated: false,
            panes: vec![],
            errors: HashMap::new(),
            error: String::new(),
            connected: false,
            loaded: false,
            connecting: false,
            transcript: cx.new(|cx| TextareaState::new(window, cx)),
            transcript_mode: false,
            transcript_dirty: true,
            filter: None,
            observer,
            _updates: task,
        }
    }
    pub(crate) fn attempts(&self) -> Vec<Value> {
        let assignments = rows(&self.runtime, "assignments");
        rows(&self.runtime, "attempts")
            .into_iter()
            .filter(|a| {
                assignments
                    .iter()
                    .any(|s| s["currentAttemptId"] == a["id"] && s["retiredAt"].is_null())
            })
            .take(4)
            .collect()
    }
    pub(crate) fn connect(&mut self) {
        self.panes.clear();
        self.errors.clear();
        self.connecting = true;
        let ids = self
            .attempts()
            .iter()
            .filter(|a| a["pane"].is_string())
            .map(|a| text(a, "id"))
            .collect();
        let _ = self.observer.connect.send(ids);
    }
    fn accept(&mut self, event: Update, cx: &mut Context<Self>) {
        match event {
            Update::Error(error) => {
                self.connected = false;
                self.transcript_dirty = true;
                self.error = error;
                for p in &self.panes {
                    p.terminal.update(cx, |t, _| t.set_visible(false));
                }
            }
            Update::Snapshot(runtime, page) => {
                if self.runtime != runtime || !self.connected {
                    self.transcript_dirty = true;
                }
                self.runtime = runtime;
                self.connected = true;
                self.loaded = true;
                self.error.clear();
                let current = self.attempts();
                self.panes
                    .retain(|p| current.iter().any(|a| a["id"] == p.id));
                for item in rows(&page, "events") {
                    if self
                        .events
                        .back()
                        .is_some_and(|v| v["cursor"].as_u64() >= item["cursor"].as_u64())
                    {
                        continue;
                    }
                    self.event_bytes += item.to_string().len();
                    self.events.push_back(item);
                    self.transcript_dirty = true;
                    while self.events.len() > 2000 || self.event_bytes > 8 * 1024 * 1024 {
                        if let Some(v) = self.events.pop_front() {
                            self.event_bytes = self.event_bytes.saturating_sub(v.to_string().len());
                            self.truncated = true;
                        }
                    }
                }
            }
            Update::Attachments(values) => {
                self.connecting = false;
                for (id, result) in values {
                    match result {
                        Ok(value) => {
                            self.errors.insert(id, format!("pending:{}", value));
                        }
                        Err(error) => {
                            self.errors.insert(id, error);
                        }
                    }
                }
            }
        }
    }
    pub(crate) fn spawn_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending: Vec<_> = self
            .errors
            .iter()
            .filter_map(|(id, v)| {
                v.strip_prefix("pending:")
                    .map(|v| (id.clone(), v.to_owned()))
            })
            .collect();
        for (id, value) in pending {
            let result = spawn_terminal(&self.connection, &self.run, &id, &value, window, cx);
            self.errors.remove(&id);
            match result {
                Ok(terminal) => self.panes.push(Pane { id, terminal }),
                Err(error) => {
                    self.errors.insert(id, error.to_string());
                }
            }
        }
    }
    /// "{KEY} {title} · Run {short id}", or the run alone when its subject is unknown.
    fn heading(&self) -> String {
        let run = format!("Run {}", &self.run[..8.min(self.run.len())]);
        let subject = format!("{} {}", self.subject.key, self.subject.title);
        match subject.trim() {
            "" => format!("Agent wall · {run}"),
            subject => format!("{subject} · {run}"),
        }
    }
    fn transcript_text(&self) -> String {
        let mut lines = vec![format!(
            "Run {} · {}\nStored observations; terminal text is not check or completion evidence.",
            self.run,
            if self.connected {
                "connected"
            } else {
                "disconnected; retained state"
            }
        )];
        if self.truncated {
            lines.push("Showing the most recent bounded observations. Full event history remains in the service.".into());
        }
        for message in rows(&self.runtime, "messages") {
            if self
                .filter
                .as_ref()
                .is_some_and(|id| message["fromAttemptId"] != *id && message["toAttemptId"] != *id)
            {
                continue;
            }
            let delivery = match message["delivery"].as_str().unwrap_or("") {
                "pending" => "Pending mailbox storage",
                "stored" => "Stored in mailbox; receipt unconfirmed",
                "accepted" => "Accepted for delivery",
                "dispatched" => "Dispatching; receipt unconfirmed",
                "received" => "Received",
                "uncertain" => "Mailbox storage uncertain",
                _ => "Delivery state unavailable",
            };
            lines.push(format!(
                "Peer message · {} → {} · {}\n{}",
                text(&message, "from"),
                text(&message, "to"),
                delivery,
                text(&message, "body")
            ));
        }
        // Attribute output to the agent's assignment name, including retired attempts.
        let assignments = rows(&self.runtime, "assignments");
        let attempts = rows(&self.runtime, "attempts");
        let agent = |attempt: &Value| {
            attempts
                .iter()
                .find(|a| a["id"] == *attempt)
                .and_then(|a| assignments.iter().find(|s| s["id"] == a["assignmentId"]))
                .and_then(|s| s["name"].as_str())
                .unwrap_or("Unidentified agent")
                .to_owned()
        };
        for item in &self.events {
            if self
                .filter
                .as_ref()
                .is_some_and(|id| item["attemptId"] != *id)
            {
                continue;
            }
            let e = &item["event"];
            let kind = text(e, "type");
            let (summary, content) = if kind == "message_end" {
                let body = rows(&e["message"], "content")
                    .iter()
                    .filter(|p| p["type"] == "text")
                    .map(|p| text(p, "text"))
                    .collect::<Vec<_>>()
                    .join("\n");
                ("message".to_owned(), body)
            } else if kind == "message_update" {
                continue;
            } else {
                // The summary leads; the recorded event stays beneath it for detail.
                (event_summary(e), e.to_string())
            };
            if kind != "message_end" || !content.is_empty() {
                lines.push(format!(
                    "{} · event {} · {}\n{}",
                    agent(&item["attemptId"]),
                    item["cursor"],
                    summary,
                    content
                ));
            }
        }
        lines.join("\n\n")
    }
}
impl Render for Wall {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.spawn_pending(window, cx);
        let narrow = window.viewport_size().width < px(650.);
        let attempts = self.attempts();
        let focused = self.filter.clone().or_else(|| {
            narrow
                .then(|| attempts.first().map(|a| text(a, "id")))
                .flatten()
        });
        for pane in &self.panes {
            pane.terminal.update(cx, |t, _| {
                t.set_visible(
                    self.connected
                        && !self.transcript_mode
                        && focused.as_ref().is_none_or(|id| *id == pane.id),
                )
            });
        }
        if self.transcript_dirty {
            let value = self.transcript_text();
            self.transcript
                .update(cx, |s, cx| s.set_value(value, window, cx));
            self.transcript_dirty = false;
        }
        let mut toolbar = div()
            .flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("wall-connect")
                    .label(if self.panes.is_empty() {
                        "Connect terminals"
                    } else {
                        "Reconnect terminals"
                    })
                    .disabled(!self.connected || self.connecting)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.connect();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("wall-transcript")
                    .label(if self.transcript_mode {
                        "Show terminals"
                    } else {
                        "Read transcript"
                    })
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.transcript_mode = !this.transcript_mode;
                        this.transcript_dirty = true;
                        if this.transcript_mode {
                            this.transcript.update(cx, |input, cx| input.focus(w, cx));
                        } else {
                            window_focus(&this.focus, w, cx);
                        }
                        cx.notify();
                    })),
            )
            .child(
                Button::new("wall-close")
                    .ghost()
                    .label("Close wall")
                    .on_click(|_, w, _| w.remove_window()),
            );
        if self.transcript_mode || !narrow {
            toolbar = toolbar.child(
                Button::new("wall-all")
                    .label("All agents")
                    .selected(self.filter.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter = None;
                        this.transcript_dirty = true;
                        cx.notify();
                    })),
            );
        }
        let mut body = div().flex().flex_col().gap_3().flex_1().min_h_0();
        let assignments = rows(&self.runtime, "assignments");
        for a in &attempts {
            let id = text(a, "id");
            let assignment = assignments
                .iter()
                .find(|s| s["id"] == a["assignmentId"])
                .cloned()
                .unwrap_or(Value::Null);
            {
                let selected = focused.as_deref() == Some(&id);
                toolbar = toolbar.child(
                    Button::new(SharedString::from(format!("transcript-{id}")))
                        .label(text(&assignment, "name"))
                        .selected(selected)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter = Some(id.clone());
                            this.transcript_dirty = true;
                            cx.notify();
                        })),
                );
            }
        }
        if self.transcript_mode {
            body = body.child(
                Textarea::new(&self.transcript)
                    .readonly(true)
                    .accessibility_id("agent-transcript")
                    .aria_label("Read-only agent event transcript")
                    .h(relative(1.)),
            );
        } else {
            let attempts: Vec<_> = attempts
                .iter()
                .filter(|a| focused.as_ref().is_none_or(|id| a["id"] == *id))
                .cloned()
                .collect();
            let columns = if window.viewport_size().width > px(850.) {
                2
            } else {
                1
            };
            for chunk in attempts.chunks(columns) {
                let mut row = div().flex().gap_3().flex_1().min_h_0();
                for a in chunk {
                    let id = text(a, "id");
                    let assignment = assignments
                        .iter()
                        .find(|s| s["id"] == a["assignmentId"])
                        .cloned()
                        .unwrap_or(Value::Null);
                    let observed = rows(&self.runtime, "observedAttemptIds").contains(&a["id"]);
                    let degraded = rows(&self.runtime, "degradedAttemptIds").contains(&a["id"]);
                    let state = crate::agent_state::of(a);
                    let summary = format!(
                        "{} · {} · {} · host {} / {} {}",
                        text(&assignment, "name"),
                        state.text,
                        if !self.connected {
                            "retained observation"
                        } else if degraded {
                            "observation degraded"
                        } else if observed {
                            "observed"
                        } else {
                            "ownership unconfirmed"
                        },
                        a["hostPid"],
                        a["runnerKind"].as_str().unwrap_or("omp"),
                        a.get("runnerPid").unwrap_or(&a["ompPid"])
                    );
                    let mut tile = div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_2()
                        .flex_1()
                        .min_w_0()
                        .border_1()
                        .border_color(rgb(0x34495a))
                        .child(
                            // Only states that need the operator change the summary's tone.
                            div()
                                .when(
                                    [crate::agent_state::WARNING, crate::agent_state::FAILED]
                                        .contains(&state.tone),
                                    |d| d.text_color(rgb(state.tone)),
                                )
                                .child(label(format!("agent-{id}"), summary)),
                        )
                        .child(label(format!("agent-model-{id}"), model_label(a)));
                    let pane = self.panes.iter().find(|p| p.id == id);
                    let notice = self.errors.get(&id).cloned().or_else(|| {
                        pane_notice(
                            a["pane"].is_string(),
                            self.connected,
                            self.connecting,
                            pane.is_some(),
                        )
                    });
                    if let (Some(p), None) = (pane, &notice) {
                        tile = tile.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .child(p.terminal.clone()),
                        );
                    } else if let Some(notice) = notice {
                        tile = tile.child(label(format!("pane-{id}"), notice));
                    }
                    row = row.child(tile);
                }
                body = body.child(row);
            }
            // Loading and unavailable are not empty: only a read snapshot can show no agents.
            if attempts.is_empty() && !self.loaded && self.error.is_empty() {
                body = body.child(label("wall-loading", "Loading agents for this run…"));
            } else if attempts.is_empty() && !self.loaded {
                body = body.child(label(
                    "wall-unavailable",
                    format!(
                        "Agent observation unavailable: {}. This does not show whether agents are running.",
                        self.error
                    ),
                ));
            } else if attempts.is_empty() {
                body = body.child(label(
                    "wall-empty",
                    "No current agent processes. Queued plans are not executing agents.",
                ));
            }
        }
        div()
            .id("agent-wall")
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x111a22))
            .text_color(rgb(0xe2e8ed))
            .font_family(".SystemUIFont")
            .text_sm()
            .track_focus(&self.focus)
            .key_context("CrewroomWall")
            .on_action(|_: &Next, w, cx| w.focus_next(cx))
            .on_action(|_: &Previous, w, cx| w.focus_prev(cx))
            .child(label("wall-run", self.heading()))
            .child(label(
                "wall-status",
                if self.error.is_empty() {
                    status_hint(cfg!(target_os = "macos"))
                } else {
                    format!(
                        "Disconnected · {} · retained observations may be stale",
                        self.error
                    )
                },
            ))
            .child(toolbar)
            .child(body)
    }
}
fn window_focus(focus: &FocusHandle, window: &mut Window, cx: &mut App) {
    window.focus(focus, cx);
}

#[cfg(test)]
mod tests {
    use super::{Update, observation, rows};
    use std::time::Duration;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
    };
    #[test]
    fn observer_drains_full_event_pages_using_service_after_cursor() {
        let root = tempfile::tempdir().unwrap();
        let descriptor = root.path().join("connection.json");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::fs::write(&descriptor,serde_json::json!({"protocol":1,"origin":format!("http://{address}"),"token":"a".repeat(64)}).to_string()).unwrap();
        std::fs::set_permissions(&descriptor, std::fs::Permissions::from_mode(0o600)).unwrap();
        let server = std::thread::spawn(move || {
            let mut cursors = vec![];
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut b = [0];
                    assert_eq!(stream.read(&mut b).unwrap(), 1);
                    request.extend(b);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains(&format!("authorization: bearer {}", "a".repeat(64)))
                );
                let path = request.split_whitespace().nth(1).unwrap();
                let body=if let Some(query)=path.split("?after=").nth(1) {
                    let after:u64=query.split('&').next().unwrap().parse().unwrap();cursors.push(after);let end=(after+200).min(401);
                    serde_json::json!({"after":end,"hasMore":end<401,"events":((after+1)..=end).map(|n|serde_json::json!({"cursor":n,"attemptId":"fixture","event":{"type":"fixture","number":n}})).collect::<Vec<_>>()})
                } else {serde_json::json!({"assignments":[],"attempts":[]})}.to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
            assert_eq!(cursors, vec![0, 200, 400]);
        });
        let (observer, events) = observation(descriptor, "fixture".into());
        let mut total = 0;
        for expected in [200, 400, 401] {
            match events.recv_blocking().unwrap() {
                Update::Snapshot(_, page) => {
                    assert_eq!(page["after"], expected);
                    total += rows(&page, "events").len();
                }
                _ => panic!("Expected a complete event page"),
            }
        }
        assert_eq!(total, 401);
        drop(observer);
        drop(events);
        server.join().unwrap();
    }
    #[gpui_kit::test]
    fn agent_tiles_retain_each_attempt_model_and_distinguish_stale_observation(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext as _, test::TestWindowExt as _};
        let runtime = serde_json::json!({
            "assignments": [{"id":"coordinator","name":"Coordinator","currentAttemptId":"second"}],
            "attempts": [
                {"id":"first","assignmentId":"coordinator","generation":1,"state":"failed","model":{"requested":{"model":"fixture/first","effort":"low"},"resolved":{"selector":"fixture/first","effort":"low"}}},
                {"id":"second","assignmentId":"coordinator","generation":2,"state":"idle","model":{"requested":{"model":"fixture/first","effort":"low"},"resolved":{"selector":"fixture/alternate","effort":"high"},"fallbackFrom":"fixture/first"}}
            ], "observedAttemptIds":["second"]
        });
        // Reopen the view twice from the same immutable attempt snapshot. These
        // records do not start a terminal or claim a real provider execution.
        for _ in 0..2 {
            let (handle, wall) = cx.update(|cx| {
                gpui_kit::init(cx);
                gpui_kit::open_window(Default::default(), cx, |window, cx| {
                    cx.new(|cx| {
                        super::Wall::new(
                            "/missing-model-fixture".into(),
                            "fixture".into(),
                            super::Subject::default(),
                            window,
                            cx,
                        )
                    })
                })
                .unwrap()
            });
            cx.update_window(handle, |_, window, cx| {
                wall.update(cx, |wall, cx| wall.accept(Update::Snapshot(runtime.clone(), serde_json::json!({"events":[]})), cx));
                window.render_frame(cx);
                let model = window.find("agent-model-second");
                assert_eq!(model.label(), Some("Model: fixture/alternate · high · Attempt 2\nRequested: fixture/first · low\nConfigured fallback from fixture/first"));
                assert!(window.try_find("agent-model-first").is_none());
                wall.update(cx, |wall, cx| wall.accept(Update::Error("Scripted observation timeout".into()), cx));
                window.render_frame(cx);
                assert!(window.find("agent-second").label().unwrap().contains("Waiting · not a check result · retained observation"));
                assert_eq!(window.find("agent-model-second").label(), model.label());
            }).unwrap();
            cx.simulate_window_resize(
                handle,
                gpui_kit::size(gpui_kit::px(390.), gpui_kit::px(844.)),
            );
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                let bounds = window.find("agent-model-second").bounds();
                assert!(
                    bounds.origin.x >= gpui_kit::px(0.) && bounds.right() <= gpui_kit::px(390.)
                );
            })
            .unwrap();
        }
    }
    #[gpui_kit::test]
    fn agent_tiles_use_the_shared_plain_state_labels(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, test::TestWindowExt as _};
        let states = [
            ("admitted", "Not started"),
            ("starting", "Starting"),
            ("executing", "Working"),
            ("idle", "Waiting · not a check result"),
            ("exited", "Stopped"),
            (
                "attention_required",
                "Needs attention: Process exited without a stop request",
            ),
            ("failed", "Failed"),
            ("unknown", "State: unknown"),
        ];
        let (handle, wall) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    super::Wall::new(
                        "/missing-state-fixture".into(),
                        "fixture".into(),
                        super::Subject::default(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        // The wall shows at most four current attempts, so check the states four at a time.
        for chunk in states.chunks(4) {
            let attempts: Vec<_> = chunk
                .iter()
                .map(|(state, _)| {
                    let mut a = serde_json::json!({"id":state,"assignmentId":state,"generation":1,"state":state,"model":{"resolved":{"selector":"fixture/model"}}});
                    if *state == "attention_required" {
                        a["ownershipReason"] = "Process exited without a stop request".into();
                    }
                    a
                })
                .collect();
            let assignments: Vec<_> = chunk
                .iter()
                .map(|(state, _)| serde_json::json!({"id":state,"name":"Reviewer","currentAttemptId":state}))
                .collect();
            let runtime = serde_json::json!({"assignments":assignments,"attempts":attempts,"observedAttemptIds":[]});
            cx.update_window(handle, |_, window, cx| {
                wall.update(cx, |wall, cx| {
                    wall.accept(
                        Update::Snapshot(runtime, serde_json::json!({"events":[]})),
                        cx,
                    )
                });
                window.render_frame(cx);
                for (state, expected) in chunk {
                    let summary = window
                        .find(gpui_kit::SharedString::from(format!("agent-{state}")))
                        .label()
                        .unwrap()
                        .to_owned();
                    assert!(
                        summary.starts_with(&format!("Reviewer · {expected} · ")),
                        "{state}: {summary}"
                    );
                }
            })
            .unwrap();
        }
    }
    #[gpui_kit::test]
    fn wall_distinguishes_loading_and_unavailable_from_no_agents(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext as _, test::TestWindowExt as _};
        let (handle, wall) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    super::Wall::new(
                        "/missing-loading-fixture".into(),
                        "fixture".into(),
                        super::Subject::default(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("wall-empty").is_none());
            assert_eq!(
                window.find("wall-loading").label(),
                Some("Loading agents for this run…")
            );
            wall.update(cx, |wall, cx| {
                wall.accept(Update::Error("Connection refused".into()), cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("wall-empty").is_none());
            assert!(window.try_find("wall-loading").is_none());
            assert_eq!(
                window.find("wall-unavailable").label(),
                Some(
                    "Agent observation unavailable: Connection refused. This does not show whether agents are running."
                )
            );
            wall.update(cx, |wall, cx| {
                wall.accept(
                    Update::Snapshot(
                        serde_json::json!({"assignments":[],"attempts":[]}),
                        serde_json::json!({"events":[]}),
                    ),
                    cx,
                )
            });
            window.render_frame(cx);
            assert!(window.try_find("wall-unavailable").is_none());
            assert!(window.find("wall-empty").label().unwrap().starts_with("No current agent processes"));
            // A later failure keeps the last successful snapshot, flagged as stale by the status line.
            wall.update(cx, |wall, cx| {
                wall.accept(Update::Error("Connection refused".into()), cx)
            });
            window.render_frame(cx);
            assert!(window.try_find("wall-empty").is_some());
            assert!(window.find("wall-status").label().unwrap().starts_with("Disconnected"));
        })
        .unwrap();
    }
    fn open_wall(
        cx: &mut gpui_kit::TestAppContext,
        run: &str,
        subject: super::Subject,
    ) -> (gpui_kit::AnyWindowHandle, gpui_kit::Entity<super::Wall>) {
        use gpui_kit::AppContext as _;
        cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    super::Wall::new(
                        "/missing-subject-fixture".into(),
                        run.into(),
                        subject,
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        })
    }
    fn two_agents() -> serde_json::Value {
        serde_json::json!({
            "assignments": [
                {"id":"impl","name":"Implementer","currentAttemptId":"0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa"},
                {"id":"rev","name":"Reviewer","currentAttemptId":"7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb"}
            ],
            "attempts": [
                {"id":"0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa","assignmentId":"impl","generation":1,"state":"executing","pane":"%1","model":{"resolved":{"selector":"fixture/model"}}},
                {"id":"7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb","assignmentId":"rev","generation":1,"state":"idle","pane":"%2","model":{"resolved":{"selector":"fixture/model"}}}
            ],
            "observedAttemptIds":["0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa","7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb"]
        })
    }
    #[gpui_kit::test]
    fn two_walls_each_name_their_ticket(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, test::TestWindowExt as _};
        let first = super::Subject {
            key: "SIM-1".into(),
            title: "Fix login".into(),
        };
        let second = super::Subject {
            key: "SIM-2".into(),
            title: "Add export".into(),
        };
        assert_eq!(super::window_title(&first), "SIM-1 · Agent wall");
        assert_eq!(super::window_title(&second), "SIM-2 · Agent wall");
        let run = "5a1c2d3e-4f50-4617-8899-aabbccddeeff";
        for (subject, expected) in [
            (first, "SIM-1 Fix login · Run 5a1c2d3e"),
            (second, "SIM-2 Add export · Run 5a1c2d3e"),
        ] {
            let (handle, _) = open_wall(cx, run, subject);
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(window.find("wall-run").label(), Some(expected));
            })
            .unwrap();
        }
    }
    #[test]
    fn status_points_to_conversation_with_the_platform_copy_shortcut() {
        let linux = super::status_hint(false);
        assert!(linux.contains("Ctrl+Shift+C"), "{linux}");
        assert!(!linux.contains("Cmd+C"), "{linux}");
        let mac = super::status_hint(true);
        assert!(mac.contains("Cmd+C"), "{mac}");
        assert!(!mac.contains("Ctrl+Shift+C"), "{mac}");
        for hint in [linux, mac] {
            assert!(hint.contains("message the coordinator in the workbench Conversation"));
            assert!(
                hint.contains(
                    "Terminals show the live screen only; scroll back in Read transcript."
                )
            );
        }
    }
    #[gpui_kit::test]
    fn rendered_status_uses_this_platforms_copy_shortcut(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{AppContext as _, test::TestWindowExt as _};
        let (handle, _) = open_wall(cx, "fixture", super::Subject::default());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                window.find("wall-status").label(),
                Some(super::status_hint(cfg!(target_os = "macos")).as_str())
            );
        })
        .unwrap();
    }
    #[test]
    fn a_hidden_terminal_is_always_labelled() {
        // An attached pane hidden by a service disconnect keeps an explicit notice.
        assert_eq!(
            super::pane_notice(true, false, false, true),
            Some(
                "Terminal hidden while disconnected. Use Read transcript for saved output.".into()
            )
        );
        assert_eq!(super::pane_notice(true, true, false, true), None);
    }
    #[gpui_kit::test]
    fn tiles_say_not_attached_before_the_first_connect_and_stay_labelled_on_disconnect(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{AppContext as _, test::TestWindowExt as _};
        let (handle, wall) = open_wall(cx, "fixture", super::Subject::default());
        cx.update_window(handle, |_, window, cx| {
            wall.update(cx, |wall, cx| {
                wall.accept(
                    Update::Snapshot(two_agents(), serde_json::json!({"events":[]})),
                    cx,
                )
            });
            window.render_frame(cx);
            for id in [
                "0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa",
                "7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb",
            ] {
                let pane = window
                    .find(gpui_kit::SharedString::from(format!("pane-{id}")))
                    .label()
                    .unwrap()
                    .to_owned();
                assert_eq!(
                    pane,
                    "Terminal not attached yet. Select Connect terminals to watch this agent."
                );
            }
            wall.update(cx, |wall, cx| {
                wall.accept(Update::Error("Connection refused".into()), cx)
            });
            window.render_frame(cx);
            for id in [
                "0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa",
                "7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb",
            ] {
                let pane = window
                    .find(gpui_kit::SharedString::from(format!("pane-{id}")))
                    .label()
                    .unwrap()
                    .to_owned();
                assert!(!pane.is_empty());
                assert!(!pane.to_lowercase().contains("disconnected"), "{pane}");
            }
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn transcript_names_agents_and_summarises_other_events(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let (handle, wall) = open_wall(cx, "fixture", super::Subject::default());
        cx.update_window(handle, |_, _, cx| {
            wall.update(cx, |wall, cx| {
                let page = serde_json::json!({"events":[
                    {"cursor":411,"attemptId":"0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa","event":{"type":"tool_execution_start","toolName":"bash","args":{"command":"cargo test"}}},
                    {"cursor":412,"attemptId":"0b6f3c1e-1111-4a5b-9c2d-aaaaaaaaaaaa","event":{"type":"message_end","message":{"content":[{"type":"text","text":"Tests pass."}]}}},
                    {"cursor":413,"attemptId":"7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb","event":{"type":"tool_execution_end","toolName":"repository_read","isError":true}},
                    {"cursor":414,"attemptId":"7d2e9f40-2222-4b6c-8d3e-bbbbbbbbbbbb","event":{"type":"agent_start"}}
                ]});
                wall.accept(Update::Snapshot(two_agents(), page), cx);
                let text = wall.transcript_text();
                assert!(text.contains("Implementer · event 411 · Tool call: bash"), "{text}");
                assert!(text.contains("Implementer · event 412 · message\nTests pass."), "{text}");
                assert!(text.contains("Reviewer · event 413 · Tool result: repository_read · error"), "{text}");
                assert!(text.contains("Reviewer · event 414 · agent start"), "{text}");
                assert!(!text.contains("0b6f3c1e"), "{text}");
                assert!(!text.contains("7d2e9f40"), "{text}");
            });
        })
        .unwrap();
    }
    #[gpui_kit::test]
    fn retained_observation_is_bounded_and_reconnect_duplicates_do_not_grow_it(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;
        let (handle, wall) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| {
                    super::Wall::new(
                        "/missing-native-observation-fixture".into(),
                        "fixture".into(),
                        super::Subject::default(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, _, cx| {
            wall.update(cx, |wall, cx| {
                let page = serde_json::json!({"events": (1..=2005).map(|cursor| serde_json::json!({"cursor":cursor,"event":{"type":"fixture"}})).collect::<Vec<_>>()});
                wall.accept(Update::Snapshot(serde_json::json!({}), page.clone()), cx);
                assert_eq!(wall.events.len(), 2000);
                assert_eq!(wall.events.front().unwrap()["cursor"], 6);
                assert!(wall.truncated);
                let retained_bytes = wall.event_bytes;
                wall.accept(Update::Snapshot(serde_json::json!({}), page), cx);
                assert_eq!(wall.event_bytes, retained_bytes);
                assert_eq!(wall.events.len(), 2000);
                wall.accept(Update::Snapshot(serde_json::json!({}), serde_json::json!({"events":[{"cursor":2006,"event":{"type":"fixture","text":"x".repeat(8*1024*1024)}}]})), cx);
                assert!(wall.events.is_empty());
                assert_eq!(wall.event_bytes, 0);
                assert!(wall.truncated);
            });
        }).unwrap();
    }
}
