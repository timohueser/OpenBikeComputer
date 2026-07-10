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
    rect,
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

        // The six digits, zero-padded and grouped `000 042` (three, space, three — leading zeros
        // kept), in the Huge tier — the one oversized readout besides the Home clock. LESC passkeys
        // are 000000–999999, so the seven 32 px cells (224 px) always fit the 240 px panel. Centred
        // a touch above mid so the caption below it balances the card.
        let key = self.passkey.min(999_999);
        let mut code: heapless::String<8> = heapless::String::new();
        let _ = write!(code, "{:03} {:03}", key / 1000, key % 1000);
        let code_top = h * 42 / 100 - Font::Huge.cap_height() as i32 / 2;
        cv.text(&code, Point::new(w / 2, code_top), Font::Huge, TextAlign::Center, INK);

        // The device↔phone pair in the glyph slot above the code (dialog anatomy, #678 T1): the
        // Bluetooth rune, three dashes for the link, a phone outline — quiet, no animation.
        pair_glyph(cv, w / 2, (super::TITLE_BAR_H + code_top) / 2);

        // The caption: the phone types this code (the device is DisplayOnly). Plain, functional, and
        // split across two lines so it fits the 240 px panel in the Label tier (≈ 20 chars/line).
        let cap_top = code_top + Font::Huge.line_height() as i32 + 8;
        let line = Font::Label.line_height() as i32;
        cv.text(rx.t(Msg::PasskeyEnterCode), Point::new(w / 2, cap_top), Font::Label, TextAlign::Center, SUBTEXT);
        cv.text(rx.t(Msg::PasskeyOnPhone), Point::new(w / 2, cap_top + line), Font::Label, TextAlign::Center, SUBTEXT);
    }
}

/// The **device↔phone pair**: the shared Bluetooth rune (this device) on the left, three
/// horizontal 2 px dashes (the link), and a phone outline (a rounded ≈12×20 px rect in a 2 px INK
/// stroke with a 1 px speaker line near the top) — centred as a group on `(cx, cy)`. All ink,
/// static: the code is the star, this just says who talks to whom.
fn pair_glyph(cv: &mut impl Surface, cx: i32, cy: i32) {
    use palette::*;
    // Group layout, left to right: rune (11) · gap (8) · dashes (5+3+5+3+5 = 21) · gap (8) ·
    // phone (12) — 60 px total, centred on `cx`.
    let x0 = cx - 30;
    super::ble_glyph(cv, x0, cy, INK);
    let mut x = x0 + super::BLE_GLYPH_W + 8;
    for _ in 0..3 {
        cv.fill(rect(x, cy - 1, 5, 2), INK);
        x += 5 + 3;
    }
    // The phone: a doubled 1 px round-rect outline for the 2 px stroke, plus the speaker line.
    let px = x + 5;
    cv.round_outline(rect(px, cy - 10, 12, 20), 3, INK);
    cv.round_outline(rect(px + 1, cy - 9, 10, 18), 2, INK);
    cv.hline(px + 4, cy - 6, 4, INK);
}
