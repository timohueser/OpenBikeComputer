//! The map-transfer card: what the glass shows while a map is written to the card, and what it
//! says when the write ends. A map is minutes of sustained writing, during which the SD bus is
//! saturated and the glass is sluggish, so the card is the rider's one explanation for that.
//!
//! The card is host-pushed: the event that opens it is a link event, not a gesture.
//! [`App::set_map_transfer`](crate::App::set_map_transfer) is fed the board's live transfer state
//! each pass and reconciles the card to it. Fed an unchanged state it does nothing, so the steady
//! state never re-dirties.
//!
//! The installed copy says restart, and means it: the map's parsed tables are read once at boot
//! into a `.bss` slot the ride loop borrows for the session, so the device cannot swap the map it
//! streams from without going through boot again.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::chrome::{title_frame, wrapped, TITLE_BAR_H};
use super::{palette, Ctx, Render, Transition};

/// Inset from the panel edge for the card's body.
const INSET: i32 = 12;
const BAR_H: i32 = 14;

/// Why a map transfer ended without a stored map. Only the outcomes the rider can act on get a
/// card: an announce-time refusal is reported to the host instead, and an abort or an unplug
/// clears the card, because the rider caused it. [`Refused`](Self::Refused) is the exception,
/// because a mid-set refusal must correct a stale "Map installed" left by the shards before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTransferError {
    /// The card refused the write, the commit could not finish, or a committed map could not be
    /// read back to check it. In the last case the map is on the card and unverified.
    Storage,
    /// The bytes arrived, the whole-object CRC did not match. Re-send.
    Damaged,
    /// The bytes do not parse as an OBCM this firmware reads: a wrong format, a different OBCM
    /// version, or damage nothing on the path caught.
    NotAMap,
    /// A file of a volume set was refused before it streamed, so the set is incomplete and
    /// nothing of it mounts.
    Refused,
}

impl MapTransferError {
    fn msg(self) -> Msg {
        match self {
            MapTransferError::Storage => Msg::MapTransferFailedStorage,
            MapTransferError::Damaged => Msg::MapTransferFailedDamaged,
            MapTransferError::NotAMap => Msg::MapTransferFailedFormat,
            MapTransferError::Refused => Msg::MapTransferFailedRefused,
        }
    }
}

/// The live state of a map transfer, as the board sees it. `None` at the
/// [`App::set_map_transfer`](crate::App::set_map_transfer) seam closes the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTransfer {
    /// Bytes are landing. The counts are kibibytes, which keeps a 4 GiB map inside a `u32` and is
    /// still finer than the bar can resolve.
    Receiving { received_kib: u32, total_kib: u32 },
    /// The map committed. It is the selected map from the next boot.
    Installed,
    /// The transfer failed. Nothing durable landed, except a map that committed and then failed
    /// its structure check: that one stays on the card and is what the next boot mounts.
    Failed(MapTransferError),
}

impl MapTransfer {
    pub fn is_receiving(self) -> bool {
        matches!(self, MapTransfer::Receiving { .. })
    }
}

/// The host-pushed map-transfer card. The reconcile replaces the state in place as progress
/// arrives, rather than pushing a second card.
#[derive(Debug)]
pub struct MapTransferScreen {
    state: MapTransfer,
}

impl MapTransferScreen {
    pub fn new(state: MapTransfer) -> Self {
        MapTransferScreen { state }
    }

    pub fn state(&self) -> MapTransfer {
        self.state
    }

    pub fn set_state(&mut self, state: MapTransfer) {
        self.state = state;
    }

    /// Modal while bytes are landing: the rider cannot cancel a transfer the host owns. Once the
    /// transfer is terminal, a press or Back dismisses the card.
    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        if self.state.is_receiving() {
            return Transition::None;
        }
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::MapTransferTitle), "");
        let body_w = w - 2 * INSET;

        match self.state {
            MapTransfer::Receiving { received_kib, total_kib } => {
                let after =
                    wrapped(cv, rx.t(Msg::MapTransferReceiving), w / 2, TITLE_BAR_H + 34, body_w, Font::Body, INK);

                // The fill grows inside an outline, so an empty bar still reads as a bar.
                let bar_y = after + 18;
                cv.round_outline(rect(INSET, bar_y, body_w, BAR_H), 4, WOOD_LIGHT);
                let permille = permille(received_kib, total_kib);
                let fill_w = ((body_w - 4) as i64 * permille as i64 / 1000) as i32;
                if fill_w > 0 {
                    cv.round(rect(INSET + 2, bar_y + 2, fill_w, BAR_H - 4), 2, AMBER);
                }

                let mut pct: heapless::String<8> = heapless::String::new();
                let _ = core::fmt::Write::write_fmt(&mut pct, format_args!("{} %", permille / 10));
                cv.text(&pct, Point::new(w / 2, bar_y + BAR_H + 12), Font::Body, TextAlign::Center, INK);

                let mut mb: heapless::String<24> = heapless::String::new();
                let _ = core::fmt::Write::write_fmt(
                    &mut mb,
                    format_args!("{} / {} MB", received_kib / 1024, total_kib / 1024),
                );
                cv.text(
                    &mb,
                    Point::new(w / 2, bar_y + BAR_H + 12 + Font::Body.line_height() as i32),
                    Font::Label,
                    TextAlign::Center,
                    SUBTEXT,
                );

                // Unplugging here costs the whole transfer, because an upload restarts and never
                // resumes.
                wrapped(
                    cv,
                    rx.t(Msg::MapTransferKeepCable),
                    w / 2,
                    h - 2 * Font::Label.line_height() as i32 - 14,
                    body_w,
                    Font::Label,
                    WARNING,
                );
            }
            MapTransfer::Installed => {
                let after =
                    wrapped(cv, rx.t(Msg::MapTransferInstalled), w / 2, TITLE_BAR_H + 40, body_w, Font::Body, INK);
                wrapped(cv, rx.t(Msg::MapTransferRestart), w / 2, after + 16, body_w, Font::Label, INK);
            }
            MapTransfer::Failed(why) => {
                let after =
                    wrapped(cv, rx.t(Msg::MapTransferFailed), w / 2, TITLE_BAR_H + 40, body_w, Font::Body, WARNING);
                wrapped(cv, rx.t(why.msg()), w / 2, after + 16, body_w, Font::Label, INK);
            }
        }
    }
}

/// Progress in permille. A `total` of 0 reads as 0 %, and a `received` past `total` reads as
/// 100 %, so the bar cannot overflow.
fn permille(received: u32, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    ((received.min(total) as u64 * 1000) / total as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permille_is_saturating_and_exact_at_the_ends() {
        assert_eq!(permille(0, 0), 0, "an empty announce reads 0 %, never a division by zero");
        assert_eq!(permille(5, 0), 0, "bytes against a zero total still read 0 %");
        assert_eq!(permille(0, 400_000), 0);
        assert_eq!(permille(400_000, 400_000), 1000, "a finished transfer reads exactly 100 %");
        assert_eq!(permille(500_000, 400_000), 1000, "a receiver overshoot clamps at 100 %");
        assert_eq!(permille(200_000, 400_000), 500);
        // A 4 GiB map in KiB is 4,194,304; the u64 widening keeps the multiply from wrapping.
        assert_eq!(permille(4_194_304 / 2, 4_194_304), 500, "the widest map the wire can announce");
    }

    #[test]
    fn only_a_terminal_card_can_be_dismissed() {
        assert!(MapTransfer::Receiving { received_kib: 1, total_kib: 2 }.is_receiving());
        assert!(!MapTransfer::Installed.is_receiving());
        assert!(!MapTransfer::Failed(MapTransferError::Damaged).is_receiving());
    }
}
