//! Keyboard editing of saved coverage, through the same verified region API as the CLI.

use super::{App, Effect};
use crate::cli::regions_cli::{Create, Deletion, Suggestions};
use ratatui::{
    crossterm::event::KeyCode,
    style::{Color, Stylize},
    text::{Line, Span},
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Areas,
    Box,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Field(usize),
    Area(usize),
}

#[derive(Debug, Clone, Default)]
pub(super) struct Editor {
    pub mode: Option<Mode>,
    pub areas: Option<Suggestions>,
    pub deletion: Option<Deletion>,
    fields: [String; 6],
    field: usize,
    choice: usize,
    selected: BTreeSet<String>,
    selected_only: bool,
    browsing: bool,
}

impl Editor {
    pub fn open(&mut self, mode: Mode) {
        self.mode = Some(mode);
        self.browsing = false;
        self.field = usize::from(mode == Mode::Box);
    }

    fn shown(&self) -> Vec<(&str, Option<&str>)> {
        let query = self.fields[0].to_lowercase();
        let areas: Vec<_> = self.areas.iter().flat_map(|areas| &areas.areas).collect();
        if self.selected_only {
            return self
                .selected
                .iter()
                .filter(|id| id.contains(&query))
                .map(|id| (id.as_str(), areas.iter().find(|area| &area.id == id).map(|area| area.name.as_str())))
                .collect();
        }
        areas
            .into_iter()
            .filter(|area| area.id.contains(&query) || area.name.to_lowercase().contains(&query))
            .map(|area| (area.id.as_str(), Some(area.name.as_str())))
            .collect()
    }

    pub fn targets(&self) -> Vec<(usize, Target)> {
        if self.mode == Some(Mode::Delete) {
            return Vec::new();
        }
        let first = usize::from(self.mode == Some(Mode::Box));
        let mut targets: Vec<_> = (0..5).map(|row| (row, Target::Field(first + row))).collect();
        if self.mode == Some(Mode::Areas) && self.areas.is_some() {
            targets.extend(
                self.shown()
                    .iter()
                    .enumerate()
                    .skip(self.choice.saturating_sub(1))
                    .take(3)
                    .enumerate()
                    .map(|(row, (index, _))| (9 + row, Target::Area(index))),
            );
        }
        targets
    }

    pub fn focus(&self) -> Option<usize> {
        let target = if self.browsing { Target::Area(self.choice) } else { Target::Field(self.field) };
        self.targets().into_iter().find(|(_, item)| *item == target).map(|(row, _)| row)
    }

    pub fn click(&mut self, target: Target) {
        match target {
            Target::Field(field) => {
                self.field = field;
                self.browsing = false;
            }
            Target::Area(choice) => {
                self.field = 0;
                self.choice = choice;
                self.browsing = true;
                self.toggle();
            }
        }
    }

    fn toggle(&mut self) {
        if let Some(area) = self.shown().get(self.choice) {
            let id = area.0.to_string();
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
        }
    }

    pub fn draft(&self) -> Option<Create> {
        let mode = self.mode.filter(|mode| *mode != Mode::Delete)?;
        Some(Create {
            id: self.fields[1].trim().into(),
            name: self.fields[2].trim().into(),
            time_zone: self.fields[3].trim().into(),
            countries: self.fields[4].split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect(),
            bbox: (mode == Mode::Box).then(|| self.fields[5].trim().into()),
            areas: if mode == Mode::Areas { self.selected.iter().cloned().collect() } else { Vec::new() },
        })
    }

    pub fn key(&mut self, key: KeyCode, busy: bool) -> Option<Effect> {
        let mode = self.mode?;
        if key == KeyCode::Esc {
            self.mode = None;
            return Some(Effect::None);
        }
        if mode == Mode::Delete {
            return match key {
                KeyCode::Char('y') if !busy => Some(
                    self.deletion
                        .as_ref()
                        .filter(|plan| plan.used_by.is_empty())
                        .map_or(Effect::None, |plan| Effect::DeleteRegion(plan.clone())),
                ),
                KeyCode::Up | KeyCode::Down => None,
                _ => Some(Effect::None),
            };
        }
        let first = usize::from(mode == Mode::Box);
        let last = if mode == Mode::Box { 5 } else { 4 };
        match key {
            KeyCode::Tab => {
                self.field = if self.field == last { first } else { self.field + 1 };
                self.browsing = false;
            }
            KeyCode::BackTab => {
                self.field = if self.field == first { last } else { self.field - 1 };
                self.browsing = false;
            }
            KeyCode::F(5) if mode == Mode::Areas && !busy => return Some(Effect::LoadAreas),
            KeyCode::F(2) if !busy => return Some(Effect::CreateRegion(self.draft().unwrap())),
            KeyCode::F(3) if mode == Mode::Areas => {
                self.browsing = false;
                self.selected_only = !self.selected_only;
                self.field = 0;
                self.choice = 0;
                self.fields[0].clear();
            }
            KeyCode::Up if self.field == 0 => {
                self.browsing = true;
                self.choice = self.choice.saturating_sub(1);
            }
            KeyCode::Down if self.field == 0 => {
                self.browsing = true;
                self.choice = (self.choice + 1).min(self.shown().len().saturating_sub(1))
            }
            KeyCode::Char(' ') | KeyCode::Enter if self.field == 0 => {
                self.browsing = true;
                self.toggle();
            }
            KeyCode::Char(c) => {
                self.browsing = false;
                self.fields[self.field].push(c);
                if self.field == 0 {
                    self.choice = 0;
                }
            }
            KeyCode::Backspace => {
                self.browsing = false;
                self.fields[self.field].pop();
                if self.field == 0 {
                    self.choice = 0;
                }
            }
            KeyCode::Delete => {
                self.browsing = false;
                self.fields[self.field].clear();
                if self.field == 0 {
                    self.choice = 0;
                }
            }
            _ => {}
        }
        Some(Effect::None)
    }

    pub fn lines(&self) -> Vec<Line<'static>> {
        if self.mode == Some(Mode::Delete) {
            return match &self.deletion {
                None => vec![Line::from("Checking definition and references…")],
                Some(plan) => {
                    let mut lines = vec![
                        Line::from(format!("Delete saved region {}?", plan.region)).bold(),
                        Line::from("Baked data stays in the store."),
                    ];
                    if plan.used_by.is_empty() {
                        lines.push(Line::from("y delete this reviewed definition · esc back"));
                    } else {
                        lines.extend(plan.used_by.iter().map(|s| Line::from(format!("Used by {s}"))));
                    }
                    lines
                }
            };
        }
        let labels = ["Find area", "Saved id", "Name", "Time zone", "Countries", "West,south,east,north"];
        let indexes: &[usize] = if self.mode == Some(Mode::Box) { &[1, 2, 3, 4, 5] } else { &[0, 1, 2, 3, 4] };
        let mut lines: Vec<_> = indexes
            .iter()
            .map(|&i| {
                let text = format!("{}: {}{}", labels[i], self.fields[i], if self.field == i { "▏" } else { "" });
                if self.field == i {
                    Line::from(text).reversed()
                } else {
                    Line::from(text)
                }
            })
            .collect();
        lines.push(Line::from("Draft · tab / shift-tab fields · F2 save file · esc back"));
        if self.mode == Some(Mode::Areas) {
            lines.push(Line::from("Countries derive from areas; optional explicit codes use commas."));
            lines
                .push(Line::from(format!("{} selected areas · search never removes a selection", self.selected.len())));
            if let Some(areas) = &self.areas {
                lines.push(
                    Line::from(format!(
                        "Index {} · F3 {} · arrows / space select",
                        areas.version,
                        if self.selected_only { "all areas" } else { "selected areas" }
                    ))
                    .dim(),
                );
                let shown = self.shown();
                let start = self.choice.saturating_sub(1);
                lines.extend(shown.iter().enumerate().skip(start).take(3).map(|(i, area)| {
                    let line = Line::from(format!(
                        "[{}] {} · {}",
                        if self.selected.contains(area.0) { "x" } else { " " },
                        area.0,
                        area.1.unwrap_or("not in this index; deselect before saving")
                    ));
                    if i == self.choice && self.field == 0 {
                        line.reversed()
                    } else {
                        line
                    }
                }));
                if shown.is_empty() {
                    lines.push(Line::from("No matching areas. Change the search."));
                }
            } else {
                lines.push(Line::from("F5 Load area list · explicit small public index download"));
            }
        } else {
            lines.push(Line::from("Countries use ISO codes; time zone uses an IANA name."));
        }
        lines
    }
}

impl App {
    pub(super) fn region_lines(&self) -> Vec<Line<'static>> {
        if self.region_editor.mode.is_some() {
            return self.region_editor.lines();
        }
        let filter = match (self.filtering, self.filter.is_empty()) {
            (false, true) => Span::from("filter").dim(),
            (true, _) => Span::from(format!("{}▏", self.filter)),
            (false, false) => Span::from(self.filter.clone()),
        };
        if let Err(error) = &self.regions {
            return vec![Line::styled(error.clone(), Color::Red)];
        }
        let mut lines = vec![Line::from(vec![Span::from("/ ").bold(), filter])];
        let current = self.env.as_ref().map(|env| env.region.as_str());
        for (i, id) in self.shown_regions().into_iter().enumerate() {
            let line = Line::from(format!("{} {id}", if Some(id) == current { "●" } else { " " }));
            lines.push(if i == self.choice { line.reversed() } else { line });
            if i == self.choice {
                let region = self.regions.iter().flatten().find(|region| region.id == id).unwrap();
                let coverage = match &region.area {
                    crate::regions::Area::Geofabrik { areas } => format!("Areas: {}", areas.join(", ")),
                    crate::regions::Area::Box { bbox } => {
                        format!("Box W/S/E/N: {}, {}, {}, {}", bbox.west, bbox.south, bbox.east, bbox.north)
                    }
                    crate::regions::Area::Union { union } => format!("Members: {}", union.join(", ")),
                };
                lines.push(Line::from(coverage).dim());
                lines.push(
                    Line::from(format!(
                        "Countries {} · zone {}",
                        region.countries.join(", "),
                        region.time_zone.as_deref().unwrap_or("not set")
                    ))
                    .dim(),
                );
            }
        }
        lines
    }
}
