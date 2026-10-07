//! Live automation is a host capability; its failure does not block manual laptop publication.

use ratatui::text::Line;
use std::sync::Arc;

use super::{Action, App, Effect};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Change {
    Install { calendar: String, zone: String },
    Disable,
    Budget,
}

#[derive(Clone, Default)]
pub(super) struct View {
    pub state: Option<Result<Arc<crate::schedule::State>, String>>,
    pub calendar: String,
    pub zone: String,
    pub editing: bool,
    pub field: usize,
    pub pending: Option<Change>,
}

impl View {
    pub fn summary(&self) -> String {
        match &self.state {
            None => "checking host state".into(),
            Some(Err(_)) if !cfg!(target_os = "linux") => "unsupported on this host".into(),
            Some(Err(_)) => "state unavailable · inspect schedule".into(),
            Some(Ok(state)) => format!(
                "{} · {} · {}",
                if state.enabled { "enabled" } else { "disabled" },
                if state.active { "active" } else { "inactive" },
                if state.runnable { "runnable" } else { "not runnable" }
            ),
        }
    }
}

impl App {
    pub(super) fn schedule_key(&mut self, key: ratatui::crossterm::event::KeyCode) -> Effect {
        use ratatui::crossterm::event::KeyCode;
        if self.asking {
            if key == KeyCode::Esc {
                self.asking = false;
                self.schedule.pending = None;
            }
            if key == KeyCode::Char('y') && !self.busy {
                if let Some(change) = self.schedule.pending.clone() {
                    return Effect::ScheduleChange(change);
                }
            }
            return Effect::None;
        }
        if self.schedule.editing {
            let value = if self.schedule.field == 0 { &mut self.schedule.calendar } else { &mut self.schedule.zone };
            match key {
                KeyCode::Char(c) => value.push(c),
                KeyCode::Backspace => {
                    value.pop();
                }
                KeyCode::Delete => value.clear(),
                KeyCode::Tab => self.schedule.field = 1 - self.schedule.field,
                KeyCode::Esc => self.schedule.editing = false,
                KeyCode::Enter
                    if !self.busy && !self.schedule.calendar.is_empty() && !self.schedule.zone.is_empty() =>
                {
                    self.schedule.editing = false;
                    self.schedule.pending = Some(Change::Install {
                        calendar: self.schedule.calendar.clone(),
                        zone: self.schedule.zone.clone(),
                    });
                    self.asking = true;
                }
                _ => {}
            }
            return Effect::None;
        }
        self.bindings()
            .iter()
            .find(|binding| binding.key == key)
            .map(|binding| binding.action)
            .map_or(Effect::None, |action| self.act(action))
    }

    pub(super) fn schedule_action(&mut self, action: Action) -> Effect {
        if self.busy {
            return Effect::None;
        }
        match action {
            Action::ScheduleRead => return Effect::ScheduleRead,
            Action::ScheduleEdit => {
                self.schedule.editing = true;
                self.schedule.field = 0;
            }
            Action::ScheduleDisable | Action::ScheduleBudget => {
                self.schedule.pending =
                    Some(if action == Action::ScheduleDisable { Change::Disable } else { Change::Budget });
                self.asking = true;
            }
            _ => {}
        }
        Effect::None
    }

    pub(super) fn schedule_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(format!("This machine: {} · environment: live", self.host))];
        if self.asking {
            lines.push(Line::from(match &self.schedule.pending {
                Some(Change::Install { calendar, zone }) => format!("Enable Live automation at {calendar} {zone}?"),
                Some(Change::Disable) => "Disable future Live automation admissions?".into(),
                Some(Change::Budget) => "Install and verify this host's configured bake limits?".into(),
                None => String::new(),
            }));
            lines.push(Line::from(match self.schedule.pending {
                Some(Change::Install { .. }) => {
                    "Checked stale inputs may build, verify and publish to Live under the original approval."
                }
                Some(Change::Disable) => {
                    "An admitted run still builds and verifies. Disable refuses its next publication handoff."
                }
                _ => "This does not enable a timer or start a bake.",
            }));
        } else {
            lines.push(Line::from(self.schedule.summary()));
            match &self.schedule.state {
                Some(Err(reason)) => lines.push(Line::from(reason.clone())),
                Some(Ok(state)) => {
                    for (label, value) in [
                        ("Calendar", &state.calendar),
                        ("Time zone", &state.time_zone),
                        ("Next", &state.next),
                        ("Last trigger", &state.last_trigger),
                        ("Blocked", &state.blocked),
                    ] {
                        if let Some(value) = value {
                            lines.push(Line::from(format!("{label}: {value}")));
                        }
                    }
                    if let Some(run) = &state.last_run {
                        lines.push(Line::from(format!(
                            "Last run: {} · {:?}",
                            run.run.summary.id, run.run.summary.outcome
                        )));
                    }
                }
                None => {}
            }
            if self.schedule.editing {
                lines.extend([
                    Line::from(format!(
                        "Calendar: {}{}",
                        self.schedule.calendar,
                        if self.schedule.field == 0 { "▏" } else { "" }
                    )),
                    Line::from(format!(
                        "Time zone: {}{}",
                        self.schedule.zone,
                        if self.schedule.field == 1 { "▏" } else { "" }
                    )),
                ]);
            }
            lines.push(Line::from("Timer setup does not change manual publication from this machine."));
        }
        lines
    }
}
