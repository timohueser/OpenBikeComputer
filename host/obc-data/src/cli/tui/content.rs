//! A manual source refresh, its immutable publication review, and a pinned bake input.

use super::{Effect, Frame, KeyCode, Line, Rect};
use ratatui::{
    style::Stylize,
    widgets::{Paragraph, Wrap},
};
use serde_json::Value;

#[derive(Default, Clone)]
pub(super) struct View {
    pub status: Option<Value>,
    pub draft: Option<String>,
    pub review: Option<Value>,
    pub row: usize,
}

impl View {
    pub fn versions(&self) -> &[Value] {
        self.status.as_ref().and_then(|status| status["versions"].as_array()).map(Vec::as_slice).unwrap_or_default()
    }
    pub fn digest(&self) -> Option<String> {
        self.versions().get(self.row)?.get("snapshot")?.as_str().map(str::to_owned)
    }
    pub fn key(&mut self, key: KeyCode, busy: bool) -> Effect {
        match key {
            KeyCode::Esc => self.draft = None,
            KeyCode::Enter if !busy => {
                let path = self.draft.take().unwrap_or_default();
                if !path.trim().is_empty() {
                    return Effect::ContentConfigure(path);
                }
            }
            KeyCode::Char(c) => self.draft.as_mut().unwrap().push(c),
            KeyCode::Backspace => {
                self.draft.as_mut().unwrap().pop();
            }
            KeyCode::Delete => self.draft.as_mut().unwrap().clear(),
            _ => {}
        }
        Effect::None
    }
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![
            Line::from("Manual Wikimedia content refresh").bold(),
            Line::default(),
            Line::from("Import snapshots on this machine. Publish prepared records and device photos."),
            Line::from("Scheduled bakes use a selected version without Wikimedia API calls."),
            Line::default(),
        ];
        if let Some(draft) = &self.draft {
            lines.extend([
                Line::from("Configuration JSON path · Enter saves · Esc cancels").bold(),
                Line::from(format!("> {draft}")),
            ]);
        }
        if let Some(status) = &self.status {
            lines.push(Line::from(format!("Inputs configured: {}", status["configured"])));
            lines.push(Line::from(format!("Selected: {}", status["selected"]["sha256"].as_str().unwrap_or("none"))));
        }
        lines.push(Line::default());
        for (index, version) in self.versions().iter().enumerate() {
            let prefix = if index == self.row { ">" } else { " " };
            lines.push(Line::from(format!(
                "{prefix} {} · {} records · {}",
                version["snapshot"].as_str().unwrap_or_default(),
                version["records"],
                super::bytes(version["bytes"].as_u64().unwrap_or_default())
            )));
        }
        if let Some(review) = &self.review {
            lines.extend([
                Line::default(),
                Line::from("PUBLICATION REVIEW · y publishes this exact version").bold(),
                Line::from(review["snapshot"].as_str().unwrap_or_default().to_owned()),
                Line::from(format!(
                    "{} objects · {} · no removals",
                    review["objects"].as_array().map_or(0, Vec::len) + 1,
                    super::bytes(review["bytes"].as_u64().unwrap_or_default())
                )),
            ]);
            for origin in review["origins"].as_array().into_iter().flatten() {
                lines.push(Line::from(format!(
                    "{} · {} · {}",
                    origin["source"].as_str().unwrap_or_default(),
                    origin["date"].as_str().unwrap_or_default(),
                    origin["sha256"].as_str().unwrap_or_default()
                )));
            }
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }
}
