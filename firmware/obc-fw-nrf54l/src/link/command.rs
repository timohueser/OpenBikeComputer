//! The command handler — the imperatives every transport carries, in one place.
//!
//! A `command` message is a small typed verb answered with a `commandResult`. Nothing about it
//! depends on whether it arrived as a GATT write or a USB control frame, so the whole dispatch lives
//! here and each transport only supplies the bytes and delivers the reply.

use defmt::{info, warn};
use obc_ble::{CommandResult, CommandStatus, SetClock, StatusMessage};

use super::StatusBytes;

pub(crate) struct CommandOutcome {
    pub(crate) result: StatusBytes,
    /// `forgetBond`: the peer asked the device to dissolve its own BLE bond. Deferred, not done
    /// inline — the caller rings [`crate::ble::request_forget_bond`] after the `commandResult` ack
    /// has gone out, so the ack reaches the peer before the bond is cleared and the link drops.
    pub(crate) forget_bond: bool,
}

/// Execute device control commands. Object mutations use the flat-store protocol.
pub(crate) fn run_command(data: &[u8]) -> CommandOutcome {
    let cmd = data.first().copied().unwrap_or(0);
    let mut forget_bond = false;
    let (status, detail) = match (cmd, data) {
        (obc_ble::CMD_INSTALL_FW, _) => {
            // installFw: request the on-glass-confirmed install of the staged update package. Answer
            // from cheaply-knowable edge state only — `busy`, a ride recording or an install already
            // pending. Whether a package is staged is the scan's own answer one second later, and a
            // catalog walk at this edge would be a second truth about it. On `ok` it posts a request
            // the ride loop drains into `App::open_remote_dfu_check`, and nothing more. It never
            // posts `DfuAction::Install`, which stays the confirm screen's press: the command never
            // waits for the human and never arms or reboots on its own. There are no silent
            // installs.
            let busy = super::recording() || crate::object_store::dfu_install_pending();
            let status = obc_ble::install_fw_reply(busy);
            if matches!(status, CommandStatus::Ok) {
                crate::object_store::request_dfu_install_ble();
                info!("link: [cmd] installFw accepted — install request posted (awaits on-glass confirm)");
            } else {
                info!("link: [cmd] installFw rejected: {}", status.as_u8());
            }
            (status, 0)
        }
        (obc_ble::CMD_FORGET_BOND, _) => {
            // forgetBond: the app's "Forget device" asks the device to dissolve its side of the bond
            // too, so a one-sided app forget does not wedge the pair — the device would otherwise
            // keep rejecting new pairings until the rider ran Forget phone on the device. Over BLE
            // this is reachable only on the authenticated, encrypted link, so a stranger can never
            // issue it. We do not forget here: answer `commandResult(ok)` and defer the forget to
            // after the ack has been sent, so the peer gets its ack before the radio link drops. The
            // forget itself reuses the on-device Forget-phone machinery: it clears the RRAM bond slot
            // and the host table, lowers `paired`, drops the link, and re-opens pairing.
            forget_bond = true;
            info!("link: [cmd] forgetBond — ack first, then clear bond + drop link");
            (CommandStatus::Ok, 0)
        }
        (obc_ble::CMD_SET_CLOCK, _) => {
            // Validate UTC and offset before publishing the clock to the ride loop.
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

        _ => (CommandStatus::UnknownCommand, 0),
    };
    CommandOutcome {
        result: StatusMessage::CommandResult(CommandResult::with_detail(cmd, status, detail)).encode(),
        forget_bond,
    }
}
