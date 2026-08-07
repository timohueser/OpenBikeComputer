//! The **map-transfer card** (issue #927) — what the glass shows while a map is written to the
//! card, and what it says when the write ends.
//!
//! Every other upload the device accepts is small enough to land between two frames. A map is not:
//! hundreds of megabytes at the card's proven throughput is **minutes** of sustained writing, during
//! which the SD bus is saturated and the map plane's own reads queue behind the transfer. Without
//! this card the rider watches a device that has simply gone sluggish for several minutes with no
//! explanation — the exact failure the DFU flow's "Installing update" card exists to prevent.
//!
//! So it is **host-pushed**, like the passkey card ([`PasskeyScreen`](super::PasskeyScreen)) and for
//! the same reason: the event that opens it is a link event, not a gesture.
//! [`App::set_map_transfer`](crate::App::set_map_transfer) is fed each pass with the board's live
//! transfer state and reconciles the card to it — pushing it when a transfer starts, updating the
//! bar as bytes land, swapping to the terminal sentence at the end, and popping it when the state
//! clears. Fed an unchanged state it does nothing, so the steady state never re-dirties.
//!
//! Two modal grades in one screen, deliberately:
//!
//! - While [`Receiving`](MapTransfer::Receiving) it swallows every gesture. There is nothing useful
//!   a press could do (the rider cannot cancel a transfer the *host* owns), and a dismissable card
//!   would just hide the one explanation for the sluggishness.
//! - Once terminal ([`Installed`](MapTransfer::Installed) / [`Failed`](MapTransfer::Failed)) any
//!   press or Back dismisses it, like the DFU outcome toasts.
//!
//! The installed copy says **restart**, and means it: the map's parsed tables (`MapTables`) are
//! read once at boot into a `.bss` slot that the whole ride loop borrows for the session, so the
//! device cannot swap the map it is streaming from without going back through boot. The new map is
//! already recorded as the selected one — the restart is what makes it the one on screen.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::{palette, title_frame, wrapped, Ctx, Render, Transition, TITLE_BAR_H};

/// Inset from the panel edge for the card's body, matching the DFU cards.
const INSET: i32 = 12;
/// Height of the progress bar (px).
const BAR_H: i32 = 14;

/// Why a map transfer ended without a stored map. Only the outcomes the rider can *act* on get a
/// card: an announce-time refusal (no room, not new, not long enough to be a map) never starts a
/// transfer, so it never reaches the glass — the host that asked for it is told instead. An abort
/// or an unplug clears the card rather than raising one: the rider caused it, and a red card
/// explaining what they just did is noise.
///
/// **[`Refused`](Self::Refused) is the one exception to that first sentence, and it was bought with
/// a real lie** (#1044). A volume set is several transfers, and the last of them — the manifest —
/// is what turns the files already on the card into a map. When *that* announce is refused, every
/// preceding shard has already ended in [`MapTransfer::Installed`], so the glass sat on "Map
/// installed / Restart" while the host was told `error` and the set was swept away at the next
/// boot. An announce-time refusal reaches the glass exactly when there is a stale success on it to
/// correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTransferError {
    /// The card refused the write, or the commit could not finish — nothing durable landed.
    Storage,
    /// The bytes arrived, the whole-object CRC did not match. Re-send.
    Damaged,
    /// The bytes arrived intact and are not an OBCM this firmware reads (wrong format, or a map
    /// built for a different OBCM version).
    NotAMap,
    /// A file of a **volume set** was refused before it streamed, mid-set: the set is incomplete
    /// and nothing of it will mount. The rider's action is the same either way — send it again from
    /// a builder this device agrees with.
    Refused,
}

impl MapTransferError {
    /// The plain sentence for this failure.
    fn msg(self) -> Msg {
        match self {
            MapTransferError::Storage => Msg::MapTransferFailedStorage,
            MapTransferError::Damaged => Msg::MapTransferFailedDamaged,
            MapTransferError::NotAMap => Msg::MapTransferFailedFormat,
            MapTransferError::Refused => Msg::MapTransferFailedRefused,
        }
    }
}

/// The live state of a map transfer, as the board sees it — the value
/// [`App::set_map_transfer`](crate::App::set_map_transfer) is fed each pass. `None` at that seam
/// means "no transfer and nothing to report", which closes the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTransfer {
    /// Bytes are landing. `received`/`total` are **kibibytes**, not bytes: the board publishes them
    /// through atomics the ride loop polls, and KiB keeps a 4 GiB map inside a `u32` with room to
    /// spare while still being finer than any bar the 240 px panel can resolve.
    Receiving { received_kib: u32, total_kib: u32 },
    /// The map committed. It is the selected map from the next boot.
    Installed,
    /// The transfer ended with nothing stored.
    Failed(MapTransferError),
}

impl MapTransfer {
    /// Whether the card is still waiting on bytes — the modal grade, and what the reconcile uses to
    /// decide a press may dismiss.
    pub fn is_receiving(self) -> bool {
        matches!(self, MapTransfer::Receiving { .. })
    }
}

/// The host-pushed map-transfer card. Holds the state it draws; the reconcile **replaces** the
/// value in place as progress arrives rather than pushing a second card.
#[derive(Debug)]
pub struct MapTransferScreen {
    state: MapTransfer,
}

impl MapTransferScreen {
    pub fn new(state: MapTransfer) -> Self {
        MapTransferScreen { state }
    }

    /// The state this card is currently showing — how the reconcile decides whether a re-fed value
    /// is a change worth a repaint.
    pub fn state(&self) -> MapTransfer {
        self.state
    }

    /// Point the card at a new state (progress ticked, or the transfer reached its outcome).
    pub fn set_state(&mut self, state: MapTransfer) {
        self.state = state;
    }

    /// Modal while bytes are landing — the rider cannot cancel a transfer the host owns, and the
    /// card is the only explanation for the sluggish glass. Dismissable once terminal, like the DFU
    /// outcome toasts.
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

                // The bar: a wood-light outline the fill grows inside, so an empty bar still reads
                // as a bar (a bare fill at 0 % would be an invisible card).
                let bar_y = after + 18;
                cv.round_outline(rect(INSET, bar_y, body_w, BAR_H), 4, WOOD_LIGHT);
                let permille = permille(received_kib, total_kib);
                let fill_w = ((body_w - 4) as i64 * permille as i64 / 1000) as i32;
                if fill_w > 0 {
                    cv.round(rect(INSET + 2, bar_y + 2, fill_w, BAR_H - 4), 2, AMBER);
                }

                // Percent above MB, both centred: the percent is the glanceable one, the megabytes
                // are what tells a rider whether "23 %" means one more minute or fifteen.
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

                // The one imperative, in the warning colour like the DFU card's "Keep power on":
                // unplugging here costs the whole transfer (uploads restart, never resume).
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

/// Progress in permille, saturating and division-safe: a `total` of 0 (a transfer whose announce
/// somehow claimed nothing) reads as 0 %, never a divide-by-zero, and a `received` past `total`
/// (which the receiver's own clamp already prevents) reads as 100 % rather than overflowing the bar.
fn permille(received: u32, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    ((received.min(total) as u64 * 1000) / total as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bar arithmetic is total: no divide-by-zero on an empty announce, no overshoot past
    /// 100 %, and the ends are exact (a full transfer must not read 99 %).
    #[test]
    fn permille_is_saturating_and_exact_at_the_ends() {
        assert_eq!(permille(0, 0), 0, "an empty announce reads 0 %, never a division by zero");
        assert_eq!(permille(5, 0), 0, "bytes against a zero total still read 0 %");
        assert_eq!(permille(0, 400_000), 0);
        assert_eq!(permille(400_000, 400_000), 1000, "a finished transfer reads exactly 100 %");
        assert_eq!(permille(500_000, 400_000), 1000, "a receiver overshoot clamps at 100 %");
        assert_eq!(permille(200_000, 400_000), 500);
        // A 4 GiB map in KiB is 4,194,304 — the u64 widening keeps the multiply from wrapping.
        assert_eq!(permille(4_194_304 / 2, 4_194_304), 500, "the widest map the wire can announce");
    }

    /// The modal grade: receiving swallows input, terminal states dismiss.
    #[test]
    fn only_a_terminal_card_can_be_dismissed() {
        assert!(MapTransfer::Receiving { received_kib: 1, total_kib: 2 }.is_receiving());
        assert!(!MapTransfer::Installed.is_receiving());
        assert!(!MapTransfer::Failed(MapTransferError::Damaged).is_receiving());
    }
}
