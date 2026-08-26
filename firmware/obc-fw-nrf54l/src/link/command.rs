//! The §4.4 command handler — the imperatives every transport carries, in one place.
//!
//! A `command` message is a small typed verb answered with a `commandResult`; nothing about it
//! depends on whether it arrived as a GATT write or a USB control frame, so the whole dispatch
//! lives here and each transport only supplies the bytes and delivers the reply.

use core::cell::RefCell;

use defmt::{info, warn};
use obc_ble::{CommandResult, CommandStatus, SetClock, StatusMessage, WeatherUnchanged};

use crate::object_store::ObjectStore;
use crate::SharedStore;

use super::StatusBytes;

/// What a control-plane `command` did.
pub(crate) struct CommandOutcome {
    pub(crate) result: StatusBytes,
    /// `forgetBond` (§4.4 cmd 4): the peer asked the device to dissolve its own BLE bond. Deferred,
    /// not done inline — the caller rings [`crate::ble::request_forget_bond`] **after** the
    /// `commandResult` ack has gone out, so the ack reaches the peer before the forget machinery
    /// clears the bond and drops the radio link.
    pub(crate) forget_bond: bool,
}

/// Execute a legacy control-plane command. Route/trip/ride mutation moved to the flat-store object
/// protocol. Ride sync/retention returns with its ObjectId-keyed metadata boundary in #1398; the
/// former FAT `ackRides` sidecar command is intentionally no longer accepted here. `setClock`
/// (cmd 5: `utc u32 · offset_min i16`, epic #638 S2) validates the peer's clock and crosses it to
/// the ride loop to stamp — no store movement.
/// Any other command byte is `unknownCommand`.
pub(crate) fn run_command(data: &[u8], store: &RefCell<ObjectStore>, shared: &mut SharedStore) -> CommandOutcome {
    let cmd = data.first().copied().unwrap_or(0);
    let mut forget_bond = false;
    let (status, detail) = match (cmd, data) {
        (obc_ble::CMD_INSTALL_FW, _) => {
            // installFw (epic #615 S6, #621): request the on-glass-confirmed install of the staged
            // /UPDATE.BIN. Answer from cheaply-knowable edge state only — `busy` (a ride recording or an
            // install already pending) and `noStaged` (a card-root existence check); the multi-second
            // OBCU CRC scan is NOT run here (it belongs to the on-device flow), so `invalid` is never
            // produced — the handler accepts and the scan surfaces a bad image on glass. On `ok` it
            // posts a request the ride loop drains into `App::open_remote_dfu_check` — push the
            // "Checking card..." wait + post `DfuAction::Scan`, the System menu's press arriving over
            // the link — and nothing more. It never posts `DfuAction::Install` (that stays the confirm
            // screen's press and the physical debug link's): the command never waits for the human and
            // never arms/reboots on its own (spec §4.4 security posture — no silent installs, ever).
            let has_staged = store.borrow().update_staged(shared);
            let busy = super::recording() || crate::object_store::dfu_install_pending();
            let status = obc_ble::install_fw_reply(has_staged, busy, false);
            if matches!(status, CommandStatus::Ok) {
                crate::object_store::request_dfu_install_ble();
                info!("link: [cmd] installFw accepted — install request posted (awaits on-glass confirm)");
            } else {
                info!("link: [cmd] installFw rejected: {}", status.as_u8());
            }
            (status, 0)
        }
        (obc_ble::CMD_FORGET_BOND, _) => {
            // forgetBond (#756): the app's "Forget device" asks the device to dissolve its side of
            // the bond too, so a one-sided app forget doesn't leave the pair wedged (the device would
            // otherwise keep rejecting new pairings under the #455 reject-when-bonded posture until the
            // rider ran Forget phone on the device). Over BLE this is only reachable on the
            // authenticated, encrypted link — the gated `command` characteristic requires it (§8) — so
            // the bonded phone clearing its own bond is fully consistent with reject-when-bonded; a
            // stranger can never issue it. We DON'T forget here: answer `commandResult(ok)` and defer
            // the forget to *after* the ack has been sent (see the caller), so the peer gets its ack
            // before the radio link drops. The forget itself reuses the on-device Forget-phone
            // machinery (`ble::link_control` → `forget_bond`): clears the RRAM bond slot + host table,
            // lowers `paired`, drops the link, and re-opens pairing on the next connection.
            forget_bond = true;
            info!("link: [cmd] forgetBond — ack first, then clear bond + drop link");
            (CommandStatus::Ok, 0)
        }
        (obc_ble::CMD_SET_CLOCK, _) => {
            // setClock (auto-expiry epic #638 S2, #642): the peer stamps the device's UTC clock +
            // local offset on every connect. `SetClock::decode` owns the whole §4.4 validation (exact
            // 7-byte length, `utc` ≥ 2020-01-01, `|offset|` ≤ 14 h) so a bad peer clock never seeds a
            // trusted-but-stale set-point the retention sweep would honour: any `Err` → `error`. On
            // success the validated pair crosses to the ride loop (`post_ble_clock`), which stamps it
            // through `App::stamp_clock_ble` (sets + persists the offset, marks trust `Ble`). The clock
            // is not a listed object and produces no flat-catalog movement.
            match SetClock::decode(data) {
                Ok(sc) => {
                    crate::object_store::post_ble_clock(sc.utc, sc.offset_min);
                    info!("link: [cmd] setClock: utc {} offset {} min — posted to ride loop", sc.utc, sc.offset_min);
                    (CommandStatus::Ok, 0)
                }
                Err(_) => {
                    warn!("link: [cmd] setClock rejected: malformed / out-of-range ({} B)", data.len());
                    (CommandStatus::Error, 0)
                }
            }
        }
        (obc_ble::CMD_WEATHER_UNCHANGED, _) => match WeatherUnchanged::decode(data) {
            Ok(ack) => {
                let accepted = crate::ble::weather_unchanged(ack.request_id, ack.retry_after_s);
                if accepted {
                    info!("link: [cmd] weatherUnchanged: request {} checked", ack.request_id);
                    (CommandStatus::Ok, 0)
                } else {
                    (CommandStatus::NotFound, 0)
                }
            }
            Err(_) => (CommandStatus::Error, 0),
        },
        _ => (CommandStatus::UnknownCommand, 0),
    };
    CommandOutcome {
        result: StatusMessage::CommandResult(CommandResult::with_detail(cmd, status, detail)).encode(),
        forget_bond,
    }
}
