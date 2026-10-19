//! Snoozed and Settled shelves in the project sidebar (FOR-78).
//!
//! Snooze and settle are the operator's presentation of a ticket, saved in the private
//! per-dataset navigation file. They never pause, cancel or change a run, its workflow state
//! or the ticket's source status, and nothing here sends a command to the service. A snoozed
//! ticket returns by itself when its time passes, when a watched fact changes, or at once when
//! it needs the operator; it shows why it woke until the operator dismisses that.
use super::navigation::{Shelves, Snooze, Watch, Woke};
use super::projects::Section;
use super::shell::chip;
use super::*;
use chrono::{DateTime, Local, Utc};
use gpui_kit::Stateful;
use std::time::Duration;

/// How long Undo stays offered after a shelf action.
pub(super) const UNDO: Duration = Duration::from_secs(5);
/// A time snooze is rechecked at least this often, so a suspended machine still wakes it.
const RECHECK: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Shelf {
    Snoozed,
    Settled,
}

/// One ticket's place on the shelves before an action, restored by Undo.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Prior {
    task: String,
    snooze: Option<Snooze>,
    settled: bool,
    woke: Option<Woke>,
}
pub(super) struct Undo {
    pub label: String,
    prior: Prior,
    generation: u64,
}
#[derive(Default)]
pub(super) struct ShelfState {
    /// The ticket whose snooze choices are open.
    pub menu: Option<String>,
    pub undo: Option<Undo>,
    generation: u64,
    undo_timer: Option<gpui_kit::Task<()>>,
    wake_timer: Option<gpui_kit::Task<()>>,
}

/// The current value of a watched fact, or `None` when this ticket cannot be watched for it:
/// PR events need a confirmed PR, and source events a linked source.
pub(super) fn watched(event: Watch, task: &Value, run: Option<&Value>) -> Option<Value> {
    let publication = run
        .map(|r| &r["publicationWatch"])
        .filter(|w| w.is_object());
    match event {
        Watch::Review => publication.map(|w| w["feedback"].clone()),
        Watch::Ci => publication.map(|w| w["ci"].clone()),
        Watch::Source => ticket_source::provider(task)
            .is_some()
            .then(|| json!(ticket_source::status_label(task))),
    }
}

/// Why a watched fact wakes the ticket, comparing it with its value when snoozed.
pub(super) fn event_wake(event: Watch, baseline: &Value, current: &Value) -> Option<String> {
    match event {
        Watch::Review => {
            let before = baseline.as_array().cloned().unwrap_or_default();
            let new: Vec<_> = current
                .as_array()
                .into_iter()
                .flatten()
                .filter(|id| !before.contains(id))
                .collect();
            if new.is_empty() {
                None
            } else if new
                .iter()
                .any(|id| id.as_str().is_some_and(|s| s.starts_with("review:")))
            {
                Some("PR review submitted".into())
            } else {
                Some("PR comment added".into())
            }
        }
        Watch::Ci => {
            (current["state"] == "finished" && current != baseline).then(
                || match current["outcome"].as_str() {
                    Some(outcome) => format!("CI finished · {outcome}"),
                    None => "CI finished".into(),
                },
            )
        }
        Watch::Source => (current != baseline).then(|| {
            format!(
                "Source status changed to {}",
                current.as_str().unwrap_or("?")
            )
        }),
    }
}

/// When a time snooze ends, in the operator's local time.
fn local(until: &str) -> String {
    DateTime::parse_from_rfc3339(until)
        .map(|t| t.with_timezone(&Local).format("%a %H:%M").to_string())
        .unwrap_or_else(|_| until.to_owned())
}

/// What a snooze waits for, in a few words.
pub(super) fn waits_for(snooze: &Snooze) -> String {
    let event = snooze.event.map(|e| match e {
        Watch::Review => "a PR review",
        Watch::Ci => "CI to finish",
        Watch::Source => "a source status change",
    });
    match (&snooze.until, event) {
        (Some(until), Some(event)) => format!("{} or {event}", local(until)),
        (Some(until), None) => local(until),
        (None, Some(event)) => event.into(),
        (None, None) => "nothing".into(),
    }
}

/// 09:00 tomorrow in the operator's time zone.
fn tomorrow_morning() -> Option<DateTime<Utc>> {
    Local::now()
        .date_naive()
        .succ_opt()?
        .and_hms_opt(9, 0, 0)?
        .and_local_timezone(Local)
        .earliest()
        .map(|t| t.with_timezone(&Utc))
}

impl Workbench {
    pub(super) fn shelves(&self) -> &Shelves {
        &self.navigation.saved().shelves
    }

    pub(super) fn snoozed(&self, task: &str) -> Option<&Snooze> {
        self.shelves().snoozed.iter().find(|s| s.task == task)
    }

    fn woke(&self, task: &str) -> Option<&Woke> {
        self.shelves().woke.iter().find(|w| w.task == task)
    }

    /// The shelf a ticket sits on. A snoozed ticket that needs the operator is never shelved.
    pub(super) fn shelf_of(&self, task: &str, section: Section) -> Option<Shelf> {
        if self.shelves().settled.iter().any(|s| s == task) {
            Some(Shelf::Settled)
        } else if self.snoozed(task).is_some() && section != Section::NeedsYou {
            Some(Shelf::Snoozed)
        } else {
            None
        }
    }

    fn prior(&self, task: &str) -> Prior {
        Prior {
            task: task.to_owned(),
            snooze: self.snoozed(task).cloned(),
            settled: self.shelves().settled.iter().any(|s| s == task),
            woke: self.woke(task).cloned(),
        }
    }

    fn save_shelves(&mut self, change: impl FnOnce(&mut Shelves)) {
        if let Err(error) = self.navigation.update_shelves(change) {
            self.notice = format!(
                "Snoozed and settled tickets could not be saved for reopening: {error}. Workflow data is unaffected."
            );
            self.feedback_origin = Some(self.origin_at(feedback::Place::Top));
        }
    }

    /// Replaces one ticket's shelf entries with `prior`'s.
    fn restore_shelf(&mut self, prior: Prior) {
        self.save_shelves(|s| {
            s.snoozed.retain(|z| z.task != prior.task);
            s.settled.retain(|t| *t != prior.task);
            s.woke.retain(|w| w.task != prior.task);
            s.snoozed.extend(prior.snooze);
            if prior.settled {
                s.settled.push(prior.task.clone());
            }
            s.woke.extend(prior.woke);
        });
    }

    /// Applies a shelf change and offers Undo for five seconds.
    fn shelve(&mut self, next: Prior, label: String, cx: &mut Context<Self>) {
        let prior = self.prior(&next.task);
        self.restore_shelf(next);
        self.shelf.menu = None;
        self.shelf.generation += 1;
        let generation = self.shelf.generation;
        self.shelf.undo = Some(Undo {
            label,
            prior,
            generation,
        });
        self.shelf.undo_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(UNDO).await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .shelf
                    .undo
                    .as_ref()
                    .is_some_and(|u| u.generation == generation)
                {
                    this.shelf.undo = None;
                    cx.notify();
                }
            });
        }));
        self.arm_wake_timer(cx);
        cx.notify();
    }

    fn shelf_ticket_key(&self, task: &str) -> String {
        records(&self.workspace, "tasks")
            .into_iter()
            .find(|t| t["id"] == task)
            .map(|t| string(&t, "key"))
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| "Ticket".into())
    }

    pub(super) fn snooze(
        &mut self,
        task: &str,
        until: Option<DateTime<Utc>>,
        event: Option<Watch>,
        cx: &mut Context<Self>,
    ) {
        let found = records(&self.workspace, "tasks")
            .into_iter()
            .find(|t| t["id"] == task);
        let run = self.current_run(task);
        let baseline = match (event, &found) {
            (Some(event), Some(t)) => match watched(event, t, run.as_ref()) {
                Some(value) => value,
                None => return,
            },
            (Some(_), None) => return,
            _ => Value::Null,
        };
        let snooze = Snooze {
            task: task.to_owned(),
            until: until.map(|t| t.to_rfc3339()),
            event,
            baseline,
        };
        let label = format!(
            "Snoozed {} until {}",
            self.shelf_ticket_key(task),
            waits_for(&snooze)
        );
        self.shelve(
            Prior {
                task: task.to_owned(),
                snooze: Some(snooze),
                settled: false,
                woke: None,
            },
            label,
            cx,
        );
    }

    pub(super) fn settle(&mut self, task: &str, settled: bool, cx: &mut Context<Self>) {
        let label = format!(
            "{} {}",
            if settled { "Settled" } else { "Un-settled" },
            self.shelf_ticket_key(task)
        );
        self.shelve(
            Prior {
                task: task.to_owned(),
                snooze: None,
                settled,
                woke: None,
            },
            label,
            cx,
        );
    }

    /// Wake now, or dismiss the reason a ticket woke.
    fn clear_snooze(&mut self, task: &str, cx: &mut Context<Self>) {
        let mut next = self.prior(task);
        let label = if next.snooze.is_some() {
            format!("Woke {}", self.shelf_ticket_key(task))
        } else {
            format!("Dismissed why {} woke", self.shelf_ticket_key(task))
        };
        next.snooze = None;
        next.woke = None;
        self.shelve(next, label, cx);
    }

    pub(super) fn undo_shelf(&mut self, cx: &mut Context<Self>) {
        if let Some(undo) = self.shelf.undo.take() {
            self.restore_shelf(undo.prior);
            self.shelf.undo_timer = None;
            self.arm_wake_timer(cx);
            cx.notify();
        }
    }

    /// Wakes every snoozed ticket whose time passed, whose watched fact changed, or that needs
    /// the operator, recording why. Returns whether any woke.
    pub(super) fn wake_at(&mut self, now: DateTime<Utc>) -> bool {
        if !self.workspace_loaded() {
            return false;
        }
        let tasks = records(&self.workspace, "tasks");
        let mut woke = vec![];
        for snooze in &self.shelves().snoozed {
            let Some(task) = tasks.iter().find(|t| t["id"] == snooze.task) else {
                continue;
            };
            let run = self.current_run(&snooze.task);
            let reason = if self.section(task, run.as_ref()) == Section::NeedsYou {
                let decision = self.needs_you().and_then(|items| {
                    items
                        .into_iter()
                        .find(|i| i["taskId"] == snooze.task)
                        .map(|i| string(&i, "title"))
                });
                Some(format!(
                    "Needs you · {}",
                    decision.unwrap_or_else(|| Self::workflow_label(task, run.as_ref()).into())
                ))
            } else if snooze.until.as_deref().is_some_and(|t| {
                DateTime::parse_from_rfc3339(t).is_ok_and(|t| t.with_timezone(&Utc) <= now)
            }) {
                Some(format!(
                    "Snooze ended {}",
                    local(snooze.until.as_deref().unwrap())
                ))
            } else {
                snooze.event.and_then(|event| {
                    let current = watched(event, task, run.as_ref())?;
                    event_wake(event, &snooze.baseline, &current)
                })
            };
            if let Some(reason) = reason {
                let reason: String = reason.chars().take(200).collect();
                woke.push(Woke {
                    task: snooze.task.clone(),
                    reason,
                });
            }
        }
        if woke.is_empty() {
            return false;
        }
        self.save_shelves(|s| {
            s.snoozed.retain(|z| !woke.iter().any(|w| w.task == z.task));
            s.woke.retain(|w| !woke.iter().any(|n| n.task == w.task));
            s.woke.extend(woke);
        });
        true
    }

    /// Checks snoozes after a workspace change, and keeps a timer for the next time snooze.
    pub(super) fn wake_snoozed(&mut self, cx: &mut Context<Self>) {
        self.wake_at(Utc::now());
        self.arm_wake_timer(cx);
    }

    fn arm_wake_timer(&mut self, cx: &mut Context<Self>) {
        let next = self
            .shelves()
            .snoozed
            .iter()
            .filter_map(|s| DateTime::parse_from_rfc3339(s.until.as_deref()?).ok())
            .min();
        self.shelf.wake_timer = next.map(|next| {
            let wait = (next.with_timezone(&Utc) - Utc::now())
                .to_std()
                .unwrap_or_default()
                .min(RECHECK);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| {
                    this.wake_snoozed(cx);
                    cx.notify();
                });
            })
        });
    }

    /// Undo for the last shelf action, while it is offered.
    pub(super) fn undo_bar(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let undo = self.shelf.undo.as_ref()?;
        Some(
            div()
                .id("shelf-undo-bar")
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .p_2()
                .rounded_md()
                .bg(rgb(0x223b46))
                .text_xs()
                .child(crate::accessibility::named(
                    "shelf-undo-label",
                    undo.label.clone(),
                ))
                .child(
                    Button::new("shelf-undo")
                        .ghost()
                        .label("Undo")
                        .accessibility_label(format!("Undo: {}", undo.label))
                        .on_click(cx.listener(|this, _, _, cx| this.undo_shelf(cx))),
                ),
        )
    }

    /// The collapsible Snoozed or Settled shelf header of a project group.
    pub(super) fn shelf_header(
        &self,
        group: &str,
        shelf: Shelf,
        count: usize,
        cx: &mut Context<Self>,
    ) -> (bool, Button) {
        let (slug, title) = match shelf {
            Shelf::Snoozed => ("snoozed", "Snoozed"),
            Shelf::Settled => ("settled", "Settled"),
        };
        let key = format!("{group}/{slug}");
        let open = self.shelves().open.contains(&key);
        let button = Button::new(SharedString::from(format!("shelf-{group}-{slug}")))
            .ghost()
            .label(format!(
                "{} {title} · {count}",
                if open { "▾" } else { "▸" }
            ))
            .accessibility_label(format!(
                "{} {title} · {count}",
                if open { "Collapse" } else { "Expand" }
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                let key = key.clone();
                this.save_shelves(|s| {
                    if let Some(i) = s.open.iter().position(|o| *o == key) {
                        s.open.remove(i);
                    } else {
                        s.open.push(key);
                    }
                });
                cx.notify();
            }));
        (open, button)
    }

    /// Snooze, settle and their reversals on a ticket row, with why it woke.
    pub(super) fn shelf_controls(
        &self,
        task: &Value,
        run: Option<&Value>,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = string(task, "id");
        let mut column = div().flex().flex_col().gap_1();
        let mut facts = div().flex().flex_wrap().gap_1();
        let mut actions = div().flex().flex_wrap().gap_1();
        let settled = self.shelves().settled.contains(&id);
        if let Some(woke) = self.woke(&id) {
            facts = facts.child(chip(
                format!("row-woke-{id}"),
                format!("Woke · {}", woke.reason),
                0x84d7bc,
            ));
            let target = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("woke-dismiss-{id}")))
                    .ghost()
                    .label("Dismiss")
                    .accessibility_label("Dismiss why this ticket woke")
                    .on_click(cx.listener(move |this, _, _, cx| this.clear_snooze(&target, cx))),
            );
        }
        if let Some(snooze) = self.snoozed(&id) {
            facts = facts.child(chip(
                format!("row-snoozed-{id}"),
                format!("Snoozed until {}", waits_for(snooze)),
                0x9eacb9,
            ));
            let target = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("wake-{id}")))
                    .ghost()
                    .label("Wake now")
                    .on_click(cx.listener(move |this, _, _, cx| this.clear_snooze(&target, cx))),
            );
        } else if settled {
            let target = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("unsettle-{id}")))
                    .ghost()
                    .label("Un-settle")
                    .on_click(cx.listener(move |this, _, _, cx| this.settle(&target, false, cx))),
            );
        } else {
            let target = id.clone();
            let open = self.shelf.menu.as_deref() == Some(id.as_str());
            actions = actions.child(
                Button::new(SharedString::from(format!("snooze-{id}")))
                    .ghost()
                    .label("Snooze…")
                    .selected(open)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.shelf.menu = if this.shelf.menu.as_deref() == Some(target.as_str()) {
                            None
                        } else {
                            Some(target.clone())
                        };
                        cx.notify();
                    })),
            );
            let target = id.clone();
            actions = actions.child(
                Button::new(SharedString::from(format!("settle-{id}")))
                    .ghost()
                    .label("Settle")
                    .on_click(cx.listener(move |this, _, _, cx| this.settle(&target, true, cx))),
            );
        }
        column = column.child(facts).child(actions);
        if self.shelf.menu.as_deref() == Some(id.as_str())
            && self.snoozed(&id).is_none()
            && !settled
        {
            column = column.child(self.snooze_menu(task, run, cx));
        }
        column
    }

    fn snooze_menu(&self, task: &Value, run: Option<&Value>, cx: &mut Context<Self>) -> Div {
        let id = string(task, "id");
        let mut choices = div().flex().flex_wrap().gap_1();
        for (slug, label, hours) in [
            ("1h", "1 hour", Some(1)),
            ("4h", "4 hours", Some(4)),
            ("tomorrow", "Tomorrow 09:00", None),
        ] {
            let target = id.clone();
            choices = choices.child(
                Button::new(SharedString::from(format!("snooze-{slug}-{id}")))
                    .ghost()
                    .label(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let until = match hours {
                            Some(h) => Some(Utc::now() + chrono::Duration::hours(h)),
                            None => tomorrow_morning(),
                        };
                        if until.is_some() {
                            this.snooze(&target, until, None, cx);
                        }
                    })),
            );
        }
        let mut unavailable = false;
        for (slug, label, event) in [
            ("review", "Until a PR review", Watch::Review),
            ("ci", "Until CI finishes", Watch::Ci),
            ("source", "Until source status changes", Watch::Source),
        ] {
            let available = watched(event, task, run).is_some();
            unavailable |= !available;
            let target = id.clone();
            choices = choices.child(
                Button::new(SharedString::from(format!("snooze-{slug}-{id}")))
                    .ghost()
                    .label(label)
                    .disabled(!available)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.snooze(&target, None, Some(event), cx)
                    })),
            );
        }
        let mut menu = div().flex().flex_col().gap_1().child(choices);
        if unavailable {
            menu = menu.child(
                named_hint(
                    format!("snooze-unavailable-{id}"),
                    "PR events need a confirmed PR; source status needs a linked ticket source.",
                )
                .text_xs(),
            );
        }
        menu.child(
            named_hint(
                format!("snooze-note-{id}"),
                "Snoozing only hides the ticket here. Runs, workflow and source status are unchanged; it returns at once if it needs you.",
            )
            .text_xs(),
        )
    }
}
