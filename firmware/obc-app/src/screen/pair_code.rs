//! The pairing code: the app link of BLE spec §9 as a QR code. A phone camera opens the companion
//! app from it, and the app lists the unpaired OBCs nearby by name. Every OBC shows the same code,
//! so the name under it tells the rider which one to pick. Setup's pairing step draws it, and so
//! does the page that Connections opens.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::{BleLink, Msg};

use super::vocab::chrome::{copy_w, title_frame, TITLE_BAR_H};
use super::vocab::marquee::fit;
use super::{palette, Ctx, Render, Transition};

/// The modules of the §9.2 symbol of the §9.1 link: version 3, level Q, mask 6. Row `y` is
/// `MODULES[y]`, and bit `SIZE - 1 - x` is the module in column `x`; 1 is dark.
const MODULES: [u32; SIZE as usize] = [
    0b11111110010001111011101111111,
    0b10000010101010001110001000001,
    0b10111010110110101010101011101,
    0b10111010001111110101001011101,
    0b10111010010000110001001011101,
    0b10000010010001010011001000001,
    0b11111110101010101010101111111,
    0b00000000000011001110100000000,
    0b01110110001011111101000000110,
    0b01001100011110000111111111101,
    0b11011111111000100010001101010,
    0b11000100000111110000000100001,
    0b10011110110110100100110100111,
    0b01110000110101001001001101101,
    0b10111011000100100111111111011,
    0b11000000110010111001100011001,
    0b01011111100100100011110011000,
    0b00111000101100001000110100100,
    0b10101011101110110110001111000,
    0b00011000110000100101011001100,
    0b01111011111001001110111110101,
    0b00000000100001101010100011011,
    0b11111110001101001101101010110,
    0b10000010111101011010100010011,
    0b10111010011010101100111111100,
    0b10111010100111011011000010101,
    0b10111010110111001000100101001,
    0b10000010101110110000110011010,
    0b11111110000011100010101101010,
];
/// The modules on one side of a version-3 symbol.
const SIZE: i32 = 29;
/// The module side in pixels, and the quiet zone in modules (BLE spec §9.2).
const MODULE: i32 = 5;
const QUIET: i32 = 4;
/// The side of the code with its quiet zone.
const CODE_PX: i32 = (SIZE + 2 * QUIET) * MODULE;

fn dark(x: i32, y: i32) -> bool {
    MODULES[y as usize] >> (SIZE - 1 - x) & 1 == 1
}

/// Draw the code with its quiet zone, its top-left at `(x, y)`. The modules are dark on a light
/// square in either theme: the two colours are ones the theme mapping keeps.
fn draw_code(cv: &mut impl Surface, x: i32, y: i32) {
    use palette::*;
    cv.round(rect(x, y, CODE_PX, CODE_PX), 4, ART_WHITE);
    let (x0, y0) = (x + QUIET * MODULE, y + QUIET * MODULE);
    // One fill for each run of dark modules in a row.
    for my in 0..SIZE {
        let mut mx = 0;
        while mx < SIZE {
            let start = mx;
            while mx < SIZE && dark(mx, my) {
                mx += 1;
            }
            if mx > start {
                cv.fill(rect(x0 + start * MODULE, y0 + my * MODULE, (mx - start) * MODULE, MODULE), ON_ACCENT);
            }
            mx += 1;
        }
    }
}

/// The code under the title bar, the name the OBC advertises under it (BLE spec §9.3), and the
/// line that says to scan it. With the rider's switch on, the radio is off only under the board's
/// USB interlock, so that line then says to unplug the cable.
pub(crate) fn code_page(cv: &mut impl Surface, rx: &Render) {
    let top = TITLE_BAR_H + 4;
    draw_code(cv, (rx.w - CODE_PX) / 2, top);
    let rename = rx.settings.device_name.as_str();
    let name = if rename.is_empty() { rx.factory_name } else { rename };
    let name_at = Point::new(rx.w / 2, top + CODE_PX + 2);
    cv.text(&fit(name, copy_w(rx.w), Font::Label), name_at, Font::Label, TextAlign::Center, palette::INK);
    let caption = name_at + Point::new(0, Font::Label.line_height() as i32);
    let cable = rx.settings.ble_enabled && rx.state.device.ble_link == BleLink::Off;
    let line = if cable { Msg::PairUnplug } else { Msg::PairScan };
    cv.text(rx.t(line), caption, Font::Caption, TextAlign::Center, palette::INK);
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
    use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

    const LINK: &str = "https://openbikecomputer.com/app";

    /// The bitmap is the symbol an encoder makes from the link as §9.2 specifies, module for module.
    #[test]
    fn the_modules_are_the_link_encoded_as_version_3_level_q() {
        const VERSION: Version = Version::new(3);
        let (mut data, mut out) = ([0; VERSION.buffer_len()], [0; VERSION.buffer_len()]);
        data[..LINK.len()].copy_from_slice(LINK.as_bytes());
        let qr =
            QrCode::encode_binary(&mut data, LINK.len(), &mut out, QrCodeEcc::Quartile, VERSION, VERSION, None, false)
                .expect("the link fits version 3 at level Q");
        assert_eq!(qr.size(), SIZE);
        for y in 0..SIZE {
            for x in 0..SIZE {
                assert_eq!(dark(x, y), qr.get_module(x, y), "module ({x}, {y})");
            }
        }
    }
}
