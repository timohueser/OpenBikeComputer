//! Live automation is a host capability; its failure does not block manual laptop publication.

use ratatui::text::Line;
use std::sync::Arc;

use super::{Action, App, Effect};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Preset {
    #[default]
    Daily,
    Weekly,
    Monthly,
    Custom,
}

impl Preset {
    const ALL: [Self; 4] = [Self::Daily, Self::Weekly, Self::Monthly, Self::Custom];
    fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone)]
pub(super) struct Form {
    pub preset: Preset,
    pub time: String,
    pub day: u8,
}

impl Form {
    fn read(calendar: &str) -> Self {
        let mut form = Self { preset: Preset::Custom, time: "00:00".into(), day: 1 };
        match calendar {
            "daily" => form.preset = Preset::Daily,
            "weekly" => form.preset = Preset::Weekly,
            "monthly" => form.preset = Preset::Monthly,
            _ => {
                let words: Vec<_> = calendar.split_whitespace().collect();
                let date = if words.len() == 3 { words[1] } else { words.first().copied().unwrap_or("") };
                let time = words.last().and_then(|value| value.strip_suffix(":00"));
                if let Some(time) = time.filter(|time| valid_time(time)) {
                    if words.len() == 2 && date == "*-*-*" {
                        form.preset = Preset::Daily;
                    } else if words.len() == 3 && date == "*-*-*" {
                        if let Some(day) = DAYS.iter().position(|day| *day == words[0]) {
                            form.preset = Preset::Weekly;
                            form.day = day as u8 + 1;
                        }
                    } else if words.len() == 2 {
                        if let Some(day) = date
                            .strip_prefix("*-*-")
                            .and_then(|day| day.parse::<u8>().ok())
                            .filter(|day| (1..=31).contains(day))
                        {
                            form.preset = Preset::Monthly;
                            form.day = day;
                        }
                    }
                    form.time = time.into();
                }
            }
        }
        form
    }
    fn has_day(&self) -> bool {
        matches!(self.preset, Preset::Weekly | Preset::Monthly)
    }
    fn fields(&self) -> usize {
        3 + usize::from(self.has_day())
    }
    fn calendar(&self, custom: &str) -> Result<String, String> {
        if self.preset == Preset::Custom {
            return (!custom.trim().is_empty()).then(|| custom.to_string()).ok_or("Enter a custom calendar.".into());
        }
        if self.has_day() && !(1..=if self.preset == Preset::Weekly { 7 } else { 31 }).contains(&self.day) {
            return Err("Choose a valid weekday or day of month.".into());
        }
        if !valid_time(&self.time) {
            return Err("Enter a time from 00:00 to 23:59.".into());
        }
        Ok(match self.preset {
            Preset::Daily => format!("*-*-* {}:00", self.time),
            Preset::Weekly => format!("{} *-*-* {}:00", DAYS[(self.day - 1) as usize], self.time),
            Preset::Monthly => format!("*-*-{:02} {}:00", self.day, self.time),
            Preset::Custom => unreachable!(),
        })
    }
}

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
fn valid_time(time: &str) -> bool {
    let [h1, h2, b':', m1, m2] = *time.as_bytes() else { return false };
    [h1, h2, m1, m2].iter().all(u8::is_ascii_digit)
        && (h1 - b'0') * 10 + h2 - b'0' < 24
        && (m1 - b'0') * 10 + m2 - b'0' < 60
}

fn short_time(value: &str) -> String {
    let words: Vec<_> = value.split_whitespace().collect();
    match (
        words.iter().find(|word| word.len() == 10 && word.as_bytes()[4] == b'-'),
        words.iter().find(|word| word.contains(':')),
    ) {
        (Some(date), Some(time)) => format!("{date} {}", time.chars().take(5).collect::<String>()),
        _ => value.chars().take(17).collect(),
    }
}

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
    pub form: Option<Form>,
}

impl View {
    pub fn summary(&self) -> String {
        match &self.state {
            None => "checking host state".into(),
            Some(Err(_)) if !cfg!(target_os = "linux") => "unsupported on this host".into(),
            Some(Err(_)) => "state unavailable · inspect schedule".into(),
            Some(Ok(state)) => {
                let last = state.last_run.as_ref().map_or("none", |run| {
                    if matches!(run.operation, Some(crate::operation::Status::Finished { ok: true })) {
                        match run.result.as_ref() {
                            Some(result) if result.get("applied").is_some_and(|applied| !applied.is_null()) => {
                                "applied"
                            }
                            Some(result) if result.get("built").is_some() => "verified",
                            _ => "finished",
                        }
                    } else {
                        match run.operation {
                            Some(crate::operation::Status::AwaitingOwner { .. }) => "awaiting owner",
                            Some(crate::operation::Status::UnknownOwner { .. }) => "owner unknown",
                            _ => super::execution::state(run).split(" ·").next().unwrap_or("unknown"),
                        }
                    }
                });
                if !state.enabled {
                    return format!("off · last {last}");
                }
                let cadence = state.calendar.as_deref().map_or("unknown".into(), |calendar| {
                    let form = Form::read(calendar);
                    match form.preset {
                        Preset::Custom => "custom".into(),
                        Preset::Daily => format!("daily {}", form.time),
                        Preset::Weekly => format!("weekly {} {}", DAYS[(form.day - 1) as usize], form.time),
                        Preset::Monthly => format!("monthly {} {}", form.day, form.time),
                    }
                });
                format!(
                    "{cadence} · next {} · last {last}",
                    state.next.as_deref().map(short_time).unwrap_or("unknown".into())
                )
            }
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
            let form = self.schedule.form.get_or_insert_with(|| Form::read(&self.schedule.calendar));
            if key == KeyCode::Esc {
                self.schedule.editing = false;
                return Effect::None;
            }
            if key == KeyCode::Tab {
                self.schedule.field = (self.schedule.field + 1) % form.fields();
                return Effect::None;
            }
            if key == KeyCode::BackTab {
                self.schedule.field = (self.schedule.field + form.fields() - 1) % form.fields();
                return Effect::None;
            }
            if key == KeyCode::Enter && !self.busy {
                match form.calendar(&self.schedule.calendar) {
                    Ok(calendar) if !self.schedule.zone.is_empty() => {
                        self.scroll = 0;
                        self.schedule.calendar = calendar.clone();
                        self.schedule.editing = false;
                        self.schedule.pending = Some(Change::Install { calendar, zone: self.schedule.zone.clone() });
                        self.asking = true;
                    }
                    result => {
                        self.notice = Some(
                            crate::cli::Code::Usage
                                .error(result.err().unwrap_or("Enter an explicit time zone.".into()))
                                .fix("Correct the schedule fields, then review again."),
                        )
                    }
                }
                return Effect::None;
            }
            let down = matches!(key, KeyCode::Down | KeyCode::Right | KeyCode::Char(' '));
            let up = matches!(key, KeyCode::Up | KeyCode::Left);
            if self.schedule.field == 0 {
                if up || down {
                    let at = Preset::ALL.iter().position(|preset| *preset == form.preset).unwrap_or(0);
                    form.preset = Preset::ALL[(at + if down { 1 } else { 3 }) % 4];
                    form.day = form.day.clamp(1, if form.preset == Preset::Weekly { 7 } else { 31 });
                }
            } else if self.schedule.field == 2 && form.has_day() {
                if up || down {
                    let max = if form.preset == Preset::Weekly { 7 } else { 31 };
                    form.day = if down { form.day % max + 1 } else { (form.day + max - 2) % max + 1 };
                }
            } else {
                let value = if self.schedule.field == 1 {
                    if form.preset == Preset::Custom {
                        &mut self.schedule.calendar
                    } else {
                        &mut form.time
                    }
                } else {
                    &mut self.schedule.zone
                };
                match key {
                    KeyCode::Char(c) => value.push(c),
                    KeyCode::Backspace => {
                        value.pop();
                    }
                    KeyCode::Delete => value.clear(),
                    _ => {}
                }
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
                self.schedule.form.get_or_insert_with(|| Form::read(&self.schedule.calendar));
                self.scroll = 0;
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
            if !self.schedule.editing {
                match &self.schedule.state {
                    Some(Err(reason)) => lines.push(Line::from(reason.clone())),
                    Some(Ok(state)) => {
                        lines.push(Line::from(format!(
                            "Enabled {} · active {} · runnable {}",
                            state.enabled, state.active, state.runnable
                        )));
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
            }
            if self.schedule.editing {
                if let Some(form) = &self.schedule.form {
                    let rows = std::iter::once(format!(
                        "Cadence: {} · arrows select · Custom keeps an advanced expression",
                        form.preset.label()
                    ))
                    .chain(std::iter::once(if form.preset == Preset::Custom {
                        format!("Calendar: {}", self.schedule.calendar)
                    } else {
                        format!("Time (HH:MM): {}", form.time)
                    }))
                    .chain(form.has_day().then(|| {
                        if form.preset == Preset::Weekly {
                            format!("Day: {} · arrows select", DAYS[(form.day - 1) as usize])
                        } else {
                            format!("Day of month: {} · arrows select", form.day)
                        }
                    }))
                    .chain(std::iter::once(format!("Time zone: {}", self.schedule.zone)));
                    for (field, text) in rows.enumerate() {
                        lines.push(Line::from(if self.schedule.field == field { format!("{text}▏") } else { text }));
                    }
                }
            }
            lines.push(Line::from("Timer setup does not change manual publication from this machine."));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_calendars_round_trip_and_advanced_calendars_keep_their_expression() {
        for (calendar, preset) in [
            ("*-*-* 07:30:00", Preset::Daily),
            ("Tue *-*-* 12:45:00", Preset::Weekly),
            ("*-*-31 23:59:00", Preset::Monthly),
            ("Mon..Fri *-*-* 01:12:13", Preset::Custom),
        ] {
            let form = Form::read(calendar);
            assert_eq!(form.preset, preset);
            assert_eq!(form.calendar(calendar).unwrap(), calendar);
        }
        let mut form = Form::read("daily");
        for invalid in ["24:00", "+1:00", "é:00", "12:60"] {
            form.time = invalid.into();
            assert!(form.calendar("").is_err(), "{invalid}");
        }
        form.time = "12:30".into();
        form.preset = Preset::Monthly;
        form.day = 0;
        assert!(form.calendar("").is_err());
        form.day = 31;
        assert_eq!(form.calendar("").unwrap(), "*-*-31 12:30:00");
    }
}
