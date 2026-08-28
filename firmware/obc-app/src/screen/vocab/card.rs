//! Selection, input, and drawing mechanics for action rows on full-screen cards.

use obc_render::Surface;

use crate::input::Gesture;

use super::list;
use super::rows::{draw_guarded_rows, GuardedRowsGeometry, MenuItem};

/// The screen-level result of one action-row gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardEvent {
    /// The gesture did not activate or dismiss the card.
    None,
    /// The card's Back action was requested.
    Dismiss,
    /// Row `usize` was activated by its required gesture.
    Activate(usize),
}

/// The selected row and shared interaction mechanics for a card's action list.
#[derive(Debug, Default)]
pub(crate) struct ActionRows {
    selected: usize,
}

impl ActionRows {
    pub(crate) const fn new() -> Self {
        ActionRows { selected: 0 }
    }

    /// Apply one gesture against the caller's guard declaration.
    pub(crate) fn handle(&mut self, gesture: Gesture, guards: &[bool]) -> CardEvent {
        if guards.is_empty() {
            return if matches!(gesture, Gesture::Back) { CardEvent::Dismiss } else { CardEvent::None };
        }

        match gesture {
            Gesture::Step(n) => {
                self.selected = list::step_selection(self.selected, n, guards.len());
                CardEvent::None
            }
            Gesture::Back => CardEvent::Dismiss,
            Gesture::Press if !self.selection_is_guarded(guards) => CardEvent::Activate(self.selected),
            Gesture::Hold if self.selection_is_guarded(guards) => CardEvent::Activate(self.selected),
            _ => CardEvent::None,
        }
    }

    /// Whether the selected row requires a hold.
    pub(crate) fn selection_is_guarded(&self, guards: &[bool]) -> bool {
        guards.get(self.selected).copied().unwrap_or(false)
    }

    /// Draw the caller's labels, guards, color, and geometry through the shared row vocabulary.
    pub(crate) fn draw(
        &self,
        cv: &mut impl Surface,
        items: &[MenuItem],
        hold_progress: f32,
        fill: u16,
        geometry: GuardedRowsGeometry,
    ) {
        draw_guarded_rows(cv, items, self.selected, hold_progress, fill, geometry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: [bool; 3] = [false; 3];
    const GUARDED: [bool; 2] = [false, true];

    #[test]
    fn forward_and_backward_steps_wrap() {
        let mut rows = ActionRows::new();
        assert_eq!(rows.handle(Gesture::Step(-1), &OPEN), CardEvent::None);
        assert_eq!(rows.handle(Gesture::Press, &OPEN), CardEvent::Activate(2));
        assert_eq!(rows.handle(Gesture::Step(1), &OPEN), CardEvent::None);
        assert_eq!(rows.handle(Gesture::Press, &OPEN), CardEvent::Activate(0));
    }

    #[test]
    fn back_dismisses() {
        assert_eq!(ActionRows::new().handle(Gesture::Back, &OPEN), CardEvent::Dismiss);
    }

    #[test]
    fn press_activates_an_unguarded_row() {
        assert_eq!(ActionRows::new().handle(Gesture::Press, &GUARDED), CardEvent::Activate(0));
    }

    #[test]
    fn press_refuses_a_guarded_row() {
        let mut rows = ActionRows::new();
        rows.handle(Gesture::Step(1), &GUARDED);
        assert_eq!(rows.handle(Gesture::Press, &GUARDED), CardEvent::None);
    }

    #[test]
    fn hold_activates_only_a_guarded_row() {
        let mut rows = ActionRows::new();
        assert_eq!(rows.handle(Gesture::Hold, &GUARDED), CardEvent::None);
        rows.handle(Gesture::Step(1), &GUARDED);
        assert_eq!(rows.handle(Gesture::Hold, &GUARDED), CardEvent::Activate(1));
    }

    #[test]
    fn guarded_selection_is_reported() {
        let mut rows = ActionRows::new();
        assert!(!rows.selection_is_guarded(&GUARDED));
        rows.handle(Gesture::Step(1), &GUARDED);
        assert!(rows.selection_is_guarded(&GUARDED));
    }
}
