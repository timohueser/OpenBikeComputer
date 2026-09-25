//! The pairing code: the link of BLE spec §9 as a QR code. A phone camera opens the companion app
//! from it, and the app finds the one OBC that shows it. The link carries only the serial, which
//! the open DIS serves to every central, so the code is no secret. Setup's pairing step draws it,
//! and so does the page that Connections opens.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::chrome::{title_frame, TITLE_BAR_H};
use super::{palette, Ctx, Render, Transition};

/// Version 4 holds the 53 link bytes at level M (BLE spec §9.2).
const VERSION: Version = Version::new(4);
/// The encoder's buffer for one version-4 symbol.
const BUF: usize = VERSION.buffer_len();
/// The module side in pixels, and the quiet zone in modules (BLE spec §9.2).
const MODULE: i32 = 5;
const QUIET: i32 = 4;
/// The side of the code with its quiet zone: 33 modules and the quiet zone either side.
pub(crate) const CODE_PX: i32 = (4 * 4 + 17 + 2 * QUIET) * MODULE;

/// The pairing link of `serial` (BLE spec §9.1).
pub(crate) fn link(serial: u64) -> heapless::String<53> {
    let mut s = heapless::String::new();
    let _ = write!(s, "https://openbikecomputer.com/pair/?s={serial:016X}");
    s
}

/// The link of `serial` encoded as §9.2 specifies. The caller lends the two buffers.
fn encode<'a>(serial: u64, data: &mut [u8; BUF], out: &'a mut [u8; BUF]) -> Option<QrCode<'a>> {
    let link = link(serial);
    data[..link.len()].copy_from_slice(link.as_bytes());
    QrCode::encode_binary(data, link.len(), out, QrCodeEcc::Medium, VERSION, VERSION, None, false).ok()
}

/// Draw the code of `serial` with its quiet zone, its top-left at `(x, y)`. The modules are dark on
/// a light square in either theme: the two colours are ones the theme mapping keeps.
fn draw_code(cv: &mut impl Surface, x: i32, y: i32, serial: u64) {
    use palette::*;
    let (mut data, mut out) = ([0; BUF], [0; BUF]);
    let Some(qr) = encode(serial, &mut data, &mut out) else { return };
    cv.round(rect(x, y, CODE_PX, CODE_PX), 4, ART_WHITE);
    let (x0, y0, n) = (x + QUIET * MODULE, y + QUIET * MODULE, qr.size());
    // One fill for each run of dark modules in a row.
    for my in 0..n {
        let mut mx = 0;
        while mx < n {
            let start = mx;
            while mx < n && qr.get_module(mx, my) {
                mx += 1;
            }
            if mx > start {
                cv.fill(rect(x0 + start * MODULE, y0 + my * MODULE, (mx - start) * MODULE, MODULE), ON_ACCENT);
            }
            mx += 1;
        }
    }
}

/// The code under the title bar, and the line under it that says to scan it.
pub(crate) fn code_page(cv: &mut impl Surface, rx: &Render) {
    let top = TITLE_BAR_H + 6;
    draw_code(cv, (rx.w - CODE_PX) / 2, top, rx.state.serial);
    let caption = Point::new(rx.w / 2, top + CODE_PX + 3);
    cv.text(rx.t(Msg::PairScan), caption, Font::Caption, TextAlign::Center, palette::INK);
}

/// The pairing code outside setup, opened from Connections. Back returns there, and a bond closes
/// the page (see [`App::set_ble_status`](crate::App::set_ble_status)).
#[derive(Debug)]
pub struct PairPhoneScreen;

impl PairPhoneScreen {
    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        title_frame(cv, rx.w, rx.h, rx.t(Msg::PasskeyTitle), "");
        code_page(cv, rx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serial is the Serial Number String: 16 uppercase hex digits, zero-padded.
    #[test]
    fn the_link_spells_the_serial_as_sixteen_uppercase_hex_digits() {
        assert_eq!(link(0x0123_4567_89AB_CDEF), "https://openbikecomputer.com/pair/?s=0123456789ABCDEF");
        assert_eq!(link(0xAB), "https://openbikecomputer.com/pair/?s=00000000000000AB");
        assert_eq!(link(u64::MAX).len(), 53, "the link is 53 bytes");
    }

    /// Every serial encodes as version 4 at level M, so every code has the side the page lays out.
    #[test]
    fn every_serial_encodes_as_a_version_4_level_m_symbol() {
        for serial in [0, 0x0123_4567_89AB_CDEF, u64::MAX] {
            let (mut data, mut out) = ([0; BUF], [0; BUF]);
            let qr = encode(serial, &mut data, &mut out).expect("the link fits version 4 at level M");
            assert_eq!(qr.version(), VERSION);
            // The level is the format information's first two bits, unmasked (ISO/IEC 18004
            // §7.9): M is 00. The crate's own accessor skips the unmasking.
            let level = (u8::from(qr.get_module(0, 8)) << 1 | u8::from(qr.get_module(1, 8))) ^ 0b10;
            assert_eq!(level, 0b00, "level M");
            assert_eq!(qr.size() * MODULE + 2 * QUIET * MODULE, CODE_PX);
        }
    }
}
