//! The BLE passkey card: the 6-digit pairing code, drawn large for the rider to type into the
//! phone. The device is SMP DisplayOnly, so it shows the code and the phone enters it.
//!
//! The host pushes and pops this card, not a gesture: it opens when the passkey of the seam
//! becomes `Some` and closes when it clears. The card swallows every gesture, so the rider cannot
//! lose the code mid-pairing, and SMP time-boxes the window, so the app runs no timeout.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::chrome::{ble_glyph, title_frame, BLE_GLYPH_W, TITLE_BAR_H};
use super::{palette, Ctx, Render, Transition};

#[derive(Debug)]
pub struct PasskeyScreen {
    /// The passkey to display, 000000 to 999999. It is zero-padded to six digits on draw.
    passkey: u32,
}

impl PasskeyScreen {
    pub fn new(passkey: u32) -> Self {
        PasskeyScreen { passkey }
    }

    /// The code the card shows now. The card scheduler compares it with a re-fed passkey, so the
    /// card never keeps a stale value on glass.
    pub(crate) fn passkey(&self) -> u32 {
        self.passkey
    }

    /// The card swallows every gesture, so input cannot dismiss the pairing. It closes only when
    /// the host clears the passkey.
    pub fn handle(&mut self, _g: Gesture, _cx: &mut Ctx) -> Transition {
        Transition::None
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::PasskeyTitle), "");

        // Six digits in the Huge tier. The six 32 px cells always fit the 240 px panel.
        let key = self.passkey.min(999_999);
        let mut code: heapless::String<8> = heapless::String::new();
        let _ = write!(code, "{key:06}");
        let code_top = h * 42 / 100 - Font::Huge.cap_mid() as i32;
        cv.text(&code, Point::new(w / 2, code_top), Font::Huge, TextAlign::Center, INK);

        pair_glyph(cv, w / 2, (TITLE_BAR_H + code_top) / 2);

        // The caption is two lines, so it fits the 240 px panel in the Label tier.
        let cap_top = code_top + Font::Huge.line_height() as i32 + 8;
        let line = Font::Label.line_height() as i32;
        cv.text(rx.t(Msg::PasskeyEnterCode), Point::new(w / 2, cap_top), Font::Label, TextAlign::Center, SUBTEXT);
        cv.text(rx.t(Msg::PasskeyOnPhone), Point::new(w / 2, cap_top + line), Font::Label, TextAlign::Center, SUBTEXT);
    }
}

/// The device and phone pair, centred as a group on `(cx, cy)`: the Bluetooth rune, three dashes
/// for the link, and a phone outline.
fn pair_glyph(cv: &mut impl Surface, cx: i32, cy: i32) {
    use palette::*;
    // Left to right: rune (11), gap (8), dashes (21), gap (8), phone (12) — 60 px in all.
    let x0 = cx - 30;
    ble_glyph(cv, x0, cy, INK);
    let mut x = x0 + BLE_GLYPH_W + 8;
    for _ in 0..3 {
        cv.fill(rect(x, cy - 1, 5, 2), INK);
        x += 5 + 3;
    }
    // The phone outline is doubled, for a 2 px stroke.
    let px = x + 5;
    cv.round_outline(rect(px, cy - 10, 12, 20), 3, INK);
    cv.round_outline(rect(px + 1, cy - 9, 10, 18), 2, INK);
    cv.hline(px + 4, cy - 6, 4, INK);
}
