//! One reviewed Git change, separate from environment Undo and publication.

use ratatui::{crossterm::event::KeyCode, text::Line};

use super::{Action, App, Effect};
use crate::cli::config_cli::Review;

#[derive(Clone)]
pub(super) struct View {
    pub review: Option<Review>,
    pub message: String,
    pub editing: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { review: None, message: "Update bake configuration".into(), editing: false }
    }
}

impl App {
    pub(super) fn config_key(&mut self, key: KeyCode) -> Effect {
        if self.asking {
            if key == KeyCode::Esc {
                self.asking = false;
            }
            if key == KeyCode::Char('y') && !self.busy {
                if let Some(review) = &self.config.review {
                    return Effect::ConfigCommit(Box::new(review.clone()), self.config.message.clone());
                }
            }
            return Effect::None;
        }
        if self.config.editing {
            match key {
                KeyCode::Esc => self.config.editing = false,
                KeyCode::Enter if !self.busy => {
                    self.config.editing = false;
                    return self.act(Action::ConfigConfirm);
                }
                KeyCode::Char(c) => self.config.message.push(c),
                KeyCode::Backspace => {
                    self.config.message.pop();
                }
                KeyCode::Delete => self.config.message.clear(),
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

    pub(super) fn config_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(format!("This machine: {} · Git commit only · no push", self.host))];
        let Some(review) = &self.config.review else {
            lines.push(Line::from("Reading changed bake configuration…"));
            return lines;
        };
        lines.push(Line::from(format!("Reviewed HEAD: {}", review.head)));
        lines.push(Line::from(format!(
            "Message: {}{}",
            self.config.message,
            if self.config.editing { "▏" } else { "" }
        )));
        if self.asking {
            lines.push(Line::from(format!("Commit these {} configuration paths?", review.files.len())));
            lines.push(Line::from("Unrelated staged files stay staged. Git hooks and signing still run."));
        }
        if review.files.is_empty() {
            lines.push(Line::from("No changed bake configuration. Local selection is not committed."));
        }
        lines.extend(review.diff.lines().map(|line| Line::from(line.to_string())));
        lines
    }
}
