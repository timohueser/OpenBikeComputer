//! The BLE **passkey card** (epic #447, P2) — the 6-digit LESC pairing code, rendered huge for the
//! rider to type into the phone. The device is SMP **DisplayOnly**: it shows the code, the phone
//! enters it.
//!
//! Unlike every other screen this one is **host-pushed**, not gesture-pushed: [`App::set_ble_status`]
//! opens it the instant the seam's passkey goes `Some` and closes it when it clears (pairing
//! complete/failed, or disconnect — all cleared BLE-side; SMP time-boxes the window, so the app runs
//! no timeout). Because pairing is modal and time-boxed, the card is **not dismissible by input** —
//! `Back` and `press` do nothing, so the rider can't lose the code mid-pairing.
//!
//! It's an opaque full-screen card (`Nav`), so when the host pops it the map plane repaints the
//! screen underneath exactly once.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::{palette, Ctx, Render, Transition};

/// The passkey card. Carries the 6-digit code it displays; the app pushes it with the live passkey
/// and pops it when pairing ends, so the value is never edited in place.
#[derive(Debug)]
pub struct PasskeyScreen {
    /// The LESC passkey to display (000000–999999). Zero-padded to six digits on draw.
    passkey: u32,
}

impl PasskeyScreen {
    /// A card showing `passkey` (an LESC code, always 000000–999999).
    pub fn new(passkey: u32) -> Self {
        PasskeyScreen { passkey }
    }

    /// Modal + time-boxed: the card swallows every gesture so pairing can't be dismissed by input.
    /// It closes only when the host clears the seam's passkey (see [`App::set_ble_status`]).
    pub fn handle(&mut self, _g: Gesture, _cx: &mut Ctx) -> Transition {
        Transition::None
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // Opaque full-screen card on the wood frame — the shared chrome, but titled as the pairing
        // prompt rather than a menu (no title-bar readout, no BLE glyph: the whole screen is the cue).
        super::title_frame(cv, w, h, rx.t(Msg::PasskeyTitle), "");

        // The six digits, zero-padded, in the Huge tier — the one oversized readout besides the Home
        // clock. LESC passkeys are 000000–999999, so `{:06}` always fits the six 32 px cells (192 px)
        // inside the 240 px panel. Centred a touch above mid so the caption below it balances the card.
        let mut code: heapless::String<8> = heapless::String::new();
        let _ = write!(code, "{:06}", self.passkey.min(999_999));
        let code_top = h * 42 / 100 - Font::Huge.cap_height() as i32 / 2;
        cv.text(&code, Point::new(w / 2, code_top), Font::Huge, TextAlign::Center, INK);

        // The caption: the phone types this code (the device is DisplayOnly). Plain, functional, and
        // split across two lines so it fits the 240 px panel in the Label tier (≈ 20 chars/line).
        let cap_top = code_top + Font::Huge.line_height() as i32 + 8;
        let line = Font::Label.line_height() as i32;
        cv.text(rx.t(Msg::PasskeyEnterCode), Point::new(w / 2, cap_top), Font::Label, TextAlign::Center, SUBTEXT);
        cv.text(rx.t(Msg::PasskeyOnPhone), Point::new(w / 2, cap_top + line), Font::Label, TextAlign::Center, SUBTEXT);
    }
}
