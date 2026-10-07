//! One source scope and filter, with details that remain readable in an 80-column terminal.

use super::{state_style, Action, App, Hit, SourceRow};
use crate::sources::Refresh;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::Line,
    widgets::{Paragraph, Wrap},
    Frame,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Scope {
    #[default]
    Live,
    All,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Used by Live",
            Self::All => "All sources",
        }
    }
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Live => Self::All,
            Self::All => Self::Live,
        };
    }
}

#[derive(Clone, Default)]
pub(super) struct View {
    pub scope: Scope,
    pub filter: String,
    pub typing: bool,
}

impl View {
    pub fn rows<'a>(&self, sources: &'a [SourceRow]) -> Vec<(usize, &'a SourceRow)> {
        let query = self.filter.to_lowercase();
        sources
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let used = row.live.as_ref().is_some_and(|versions| !versions.is_empty()) || !row.requests.is_empty();
                (self.scope == Scope::All || used)
                    && (row.source.id.contains(&query)
                        || row.source.licence.as_ref().is_some_and(|licence| licence.to_lowercase().contains(&query)))
            })
            .collect()
    }
}

impl App {
    pub(super) fn version_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from("Move ALL active requests of this source. Nothing fetches until preparation."),
            Line::from("Pending intent enters Plan; apply needs the exact reviewed plan."),
        ];
        let Some(versions) = &self.versions else { return lines };
        let rows = std::iter::once(("Keep normal Live selection".to_string(), true))
            .chain(std::iter::once(("Newest for each request".into(), versions.newest)))
            .chain(versions.common.iter().map(|v| (format!("All requests @ {v}"), true)));
        for (index, (label, available)) in rows.enumerate() {
            let line = Line::from(format!("{} {label}", if available { " " } else { "blocked" }));
            lines.push(if self.choice == index { line.reversed() } else { line });
        }
        if versions.requests.is_empty() {
            lines.push(Line::from("No active requests. Held provenance cannot be moved."));
        }
        if let Some(version) = self.moves.get(&versions.source) {
            lines.push(Line::from(format!(
                "Pending ALL-request move: {}",
                version.as_deref().unwrap_or("newest per request")
            )));
        }
        lines.extend(versions.lines().into_iter().skip(2).map(Line::from));
        lines
    }

    pub(super) fn version_choose(&mut self) -> Effect {
        if self.busy {
            return Effect::None;
        }
        let Some(versions) = &self.versions else { return Effect::None };
        let source = versions.source.clone();
        if self.choice == 0 {
            self.moves.remove(&source);
        } else {
            let version = if self.choice == 1 { None } else { versions.common.get(self.choice - 2).cloned() };
            if let Err(error) = versions.check(version.as_deref()) {
                self.notice = Some(error);
                return Effect::None;
            }
            self.moves.insert(source, version);
        }
        self.plan = None;
        self.overlay = None;
        Effect::None
    }

    pub(super) fn move_args(&self) -> Vec<String> {
        self.moves
            .iter()
            .map(|(source, version)| version.as_ref().map_or(source.clone(), |v| format!("{source}@{v}")))
            .collect()
    }

    pub(super) fn source_visible(&self) -> bool {
        self.source_view.rows(&self.sources).iter().any(|(index, _)| *index == self.source)
    }

    pub(super) fn source_move(&mut self, down: bool) {
        let rows = self.source_view.rows(&self.sources);
        let at = rows.iter().position(|(index, _)| *index == self.source);
        let next = match (at, down) {
            (Some(at), true) => (at + 1).min(rows.len().saturating_sub(1)),
            (Some(at), false) => at.saturating_sub(1),
            (None, _) => 0,
        };
        if let Some((index, _)) = rows.get(next) {
            self.source = *index;
        }
    }

    pub(super) fn draw_sources(&mut self, frame: &mut Frame, area: Rect) {
        let [scope, list, details] =
            Layout::vertical([Constraint::Length(2), Constraint::Percentage(55), Constraint::Fill(1)]).areas(area);
        let filter = if self.source_view.filter.is_empty() { "search sources" } else { &self.source_view.filter };
        frame.render_widget(
            Paragraph::new(format!(
                "{} [f change] · / {}{}",
                self.source_view.scope.label(),
                filter,
                if self.source_view.typing { "▏" } else { "" }
            )),
            scope,
        );
        let scope_width = (self.source_view.scope.label().len() + 11) as u16;
        self.hits
            .push((Rect { width: scope_width.min(scope.width), height: 1, ..scope }, Hit::Action(Action::SourceScope)));
        self.hits.push((
            Rect { x: scope.x + scope_width, width: scope.width.saturating_sub(scope_width), height: 1, ..scope },
            Hit::Action(Action::SourceFilter),
        ));
        let rows = self.source_view.rows(&self.sources);
        if rows.is_empty() {
            frame.render_widget(
                Paragraph::new("No sources in this view. Change the scope or search. Selected source stays selected."),
                list,
            );
            return;
        }
        let width = list.width.saturating_sub(43).max(8) as usize;
        let short = |text: &str| text.chars().take(width).collect::<String>();
        let header =
            Line::from(format!("{:<width$} {:<13} {:>6} {:>8}  LIVE", "SOURCE", "STATE", "AGE", "POLICY")).dim();
        let mut lines = Vec::new();
        let mut selected = 0;
        for (at, (index, row)) in rows.iter().enumerate() {
            let live = match &row.live {
                None => "?".into(),
                Some(versions) if versions.is_empty() => "—".into(),
                Some(versions) => row.short(versions.first().map(String::as_str)),
            };
            let text = format!(
                "{:<width$} {:<13} {:>6} {:>8}  {}",
                short(&row.source.id),
                row.state.to_string(),
                row.age_days.map_or("—".into(), |days| format!("{days}d")),
                row.source.refresh.to_string(),
                live
            );
            let line = Line::from(text).style(state_style(row.state));
            if *index == self.source {
                selected = at;
                lines.push(line.reversed());
            } else {
                lines.push(line);
            }
        }
        let at: Vec<_> = (0..rows.len()).collect();
        self.draw_lines(frame, list, header, lines, &at, selected);
        frame.render_widget(Paragraph::new(self.source_lines()).wrap(Wrap { trim: false }), details);
    }

    pub(super) fn source_lines(&self) -> Vec<Line<'static>> {
        if let Some(row) = self.sources.get(self.source).filter(|_| self.source_visible()) {
            let mut lines = vec![
                Line::from(row.source.id.clone()).bold(),
                Line::from(format!(
                    "Policy {} · R2 copy {} · credential {}",
                    row.source.refresh,
                    if row.source.r2_copy { "yes" } else { "no" },
                    if row.credential_missing { "missing for a new fetch" } else { "available or not needed" }
                )),
            ];
            lines.extend(super::expansion(row));
            if let Some(version) = self.moves.get(&row.source.id) {
                lines.push(Line::from(format!(
                    "Pending ALL-request move: {} · p review plan",
                    version.as_deref().unwrap_or("newest per request")
                )));
            }
            if let Some(credential) = &row.source.credential {
                lines.push(Line::from(format!("New fetch needs: {}", credential.describe())));
            }
            for request in &row.requests {
                let params =
                    request.params.iter().map(|(key, value)| format!("{key}={value}")).collect::<Vec<_>>().join(", ");
                lines.push(Line::from(format!(
                    "Request {} · data age {}",
                    if params.is_empty() { "all" } else { &params },
                    request.age_days.map_or("not dated".into(), |days| format!("{days} days"))
                )));
                lines.push(Line::from(format!(
                    "Checked {} · last success {}",
                    request
                        .observation
                        .checked_at
                        .map(crate::date::timestamp)
                        .unwrap_or("not an upstream probe".into()),
                    request
                        .observation
                        .last_success
                        .as_ref()
                        .map(|success| format!("{} ({})", crate::date::timestamp(success.checked_at), success.version))
                        .unwrap_or("none".into())
                )));
            }
            lines
        } else {
            Vec::new()
        }
    }
}

impl App {
    pub(super) fn policy_lines(&self) -> Vec<Line<'static>> {
        let current = self.sources[self.source].source.refresh;
        if let Some(days) = &self.policy_days {
            return vec![
                Line::from(format!("Current policy: {current}")),
                Line::from(format!("Whole days: {days}▏")).reversed(),
                Line::from("1..65535 · enter save · esc keep current policy"),
            ];
        }
        let lines = Refresh::ALL.iter().enumerate().map(|(i, &refresh)| {
            let mark = if refresh == current { "●" } else { " " };
            let line = Line::from(format!("{mark} {refresh}"));
            if i == self.choice {
                line.reversed()
            } else {
                line
            }
        });
        lines.collect()
    }
}
