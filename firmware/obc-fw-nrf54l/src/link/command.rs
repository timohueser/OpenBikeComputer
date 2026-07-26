//! The §4.4 command handler — the imperatives every transport carries, in one place.
//!
//! A `command` message is a small typed verb answered with a `commandResult`; nothing about it
//! depends on whether it arrived as a GATT write or a USB control frame, so the whole dispatch
//! lives here and each transport only supplies the bytes and delivers the reply.

use core::cell::RefCell;

use defmt::{info, warn};
use obc_app::Retention;
use obc_ble::{CommandResult, CommandStatus, ObjectType, SetClock, SetRouteRetention, StatusMessage};

use crate::object_store::ObjectStore;
use crate::SharedStore;

use super::StatusBytes;

/// What a `command` did: the `commandResult` to send back, plus which store (if any) it moved
/// (→ the caller also sends `storeChanged`, typed accordingly).
pub(crate) struct CommandOutcome {
    pub(crate) result: StatusBytes,
    pub(crate) store_changed: Option<ObjectType>,
    /// `forgetBond` (§4.4 cmd 4): the peer asked the device to dissolve its own BLE bond. Deferred,
    /// not done inline — the caller rings [`crate::ble::request_forget_bond`] **after** the
    /// `commandResult` ack has gone out, so the ack reaches the peer before the forget machinery
    /// clears the bond and drops the radio link.
    pub(crate) forget_bond: bool,
}

/// Execute a `command`. `deleteObject` (cmd 1: `type u8 · object_id u16`) deletes a stored route
/// through the [`ObjectStore`]. Ride deletion over the link is **deliberately not implemented**
/// (`notFound`): rides are deleted only from the device's Rides screen (#454) — the app hides synced
/// rides locally (tombstones) rather than deleting them here, so a re-sync can never resurrect them.
/// `ackRides` (cmd 2: `count u8 · count × object_id u16`) reconciles the synced sidecar from the
/// peer's possession list ([`ObjectStore::ack_rides`]); its `commandResult.detail` reports the
/// newly-flagged count. `setClock` (cmd 5: `utc u32 · offset_min i16`, epic #638 S2) validates the
/// peer's clock and crosses it to the ride loop to stamp — no store movement. `setRouteRetention`
/// (cmd 6: `object_id u16 · retention u8`, epic #638 S4) sets a stored route's retention level
/// through the S3 sidecar (not touching `last_used`), bumping the route revision on a real change.
/// Any other command byte is `unknownCommand`.
pub(crate) fn run_command(data: &[u8], store: &RefCell<ObjectStore>, shared: &mut SharedStore) -> CommandOutcome {
    let cmd = data.first().copied().unwrap_or(0);
    let mut forget_bond = false;
    let (status, detail, store_changed) = match (cmd, data) {
        (obc_ble::CMD_DELETE_OBJECT, [_, ty, lo, hi, ..]) => {
            let id = u16::from_le_bytes([*lo, *hi]);
            match ObjectType::from_u8(*ty) {
                Ok(ObjectType::Route) => {
                    if store.borrow_mut().delete_route(shared, id) {
                        info!("link: [cmd] deleted route object {}", id);
                        (CommandStatus::Ok, 0, Some(ObjectType::Route))
                    } else {
                        (CommandStatus::NotFound, 0, None)
                    }
                }
                // A trip delete is **non-cascading** (spec §7.7): remove only the trip object — its
                // member routes become top-level routes. Bumps the *trip* store revision → the caller
                // notifies `storeChanged(trip)` (§4.3, its own counter). A cascade "delete trip &
                // routes" is the initiating UI's composition (individual route deletes + this), never
                // a wire verb.
                Ok(ObjectType::Trip) => {
                    if store.borrow_mut().delete_trip(shared, id) {
                        info!("link: [cmd] deleted trip object {}", id);
                        (CommandStatus::Ok, 0, Some(ObjectType::Trip))
                    } else {
                        (CommandStatus::NotFound, 0, None)
                    }
                }
                // Rides are never deleted over the link (see the fn doc); nothing else deletes.
                _ => (CommandStatus::NotFound, 0, None),
            }
        }
        (obc_ble::CMD_DELETE_OBJECT, _) => (CommandStatus::Error, 0, None), // truncated arg list
        (obc_ble::CMD_ACK_RIDES, _) => match obc_ble::AckRides::decode(data) {
            Ok(ack) => {
                let newly = store.borrow_mut().ack_rides(shared, &ack);
                info!("link: [cmd] ackRides: {} acked, {} newly flagged", ack.count(), newly);
                // Only an actual flag change moved the store (and only the ride side of it).
                (CommandStatus::Ok, newly, (newly > 0).then_some(ObjectType::Ride))
            }
            Err(_) => (CommandStatus::Error, 0, None), // count promises more ids than the message carries
        },
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
            (status, 0, None)
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
            (CommandStatus::Ok, 0, None)
        }
        (obc_ble::CMD_SET_CLOCK, _) => {
            // setClock (auto-expiry epic #638 S2, #642): the peer stamps the device's UTC clock +
            // local offset on every connect. `SetClock::decode` owns the whole §4.4 validation (exact
            // 7-byte length, `utc` ≥ 2020-01-01, `|offset|` ≤ 14 h) so a bad peer clock never seeds a
            // trusted-but-stale set-point the retention sweep would honour: any `Err` → `error`. On
            // success the validated pair crosses to the ride loop (`post_ble_clock`), which stamps it
            // through `App::stamp_clock_ble` (sets + persists the offset, marks trust `Ble`). The clock
            // is not a listed object — **no store revision bump**, so `store_changed` stays `None`.
            match SetClock::decode(data) {
                Ok(sc) => {
                    crate::object_store::post_ble_clock(sc.utc, sc.offset_min);
                    info!("link: [cmd] setClock: utc {} offset {} min — posted to ride loop", sc.utc, sc.offset_min);
                    (CommandStatus::Ok, 0, None)
                }
                Err(_) => {
                    warn!("link: [cmd] setClock rejected: malformed / out-of-range ({} B)", data.len());
                    (CommandStatus::Error, 0, None)
                }
            }
        }
        (obc_ble::CMD_SET_ROUTE_RETENTION, _) => {
            // setRouteRetention (auto-expiry epic #638 S4, #644): set a stored route's retention level
            // without re-uploading it. `SetRouteRetention::decode` owns the whole §4.4 validation
            // (exact 4-byte length, `retention` ≤ 5) so a bad write never mutates the store: any `Err`
            // → `error`. A known id writes the level through the S3 retention sidecar **without
            // touching `last_used`** (changing retention never resets the usage clock) and bumps the
            // **route** store revision only on a *real* change — so `storeChanged(route)` + the ride
            // loop's rescan re-feed `set_routes_with_meta`, and the peer sees the fresh expiry in the
            // next `routeList`. Setting the same value twice is `ok` with no bump (the idempotence
            // pin); an unknown `object_id` is `notFound`.
            match SetRouteRetention::decode(data) {
                Ok(srr) => {
                    use crate::object_store::SetRetentionResult;
                    match store.borrow_mut().set_route_retention(
                        shared,
                        srr.object_id,
                        Retention::from_u8(srr.retention),
                    ) {
                        SetRetentionResult::NotFound => {
                            info!("link: [cmd] setRouteRetention: unknown route {}", srr.object_id);
                            (CommandStatus::NotFound, 0, None)
                        }
                        // `ok` only after durable persistence; the store revision (→ storeChanged) moved
                        // inside `set_route_retention` on a real change, so notify Route then.
                        SetRetentionResult::Changed => {
                            info!(
                                "link: [cmd] setRouteRetention: route {} -> retention {} (changed)",
                                srr.object_id, srr.retention
                            );
                            (CommandStatus::Ok, 0, Some(ObjectType::Route))
                        }
                        SetRetentionResult::Unchanged => {
                            info!(
                                "link: [cmd] setRouteRetention: route {} already retention {}",
                                srr.object_id, srr.retention
                            );
                            (CommandStatus::Ok, 0, None)
                        }
                        // Finding #876-5: the write did not reach the card — never claim `ok`.
                        SetRetentionResult::WriteFailed => {
                            warn!(
                                "link: [cmd] setRouteRetention: route {} sidecar write failed — reporting Error",
                                srr.object_id
                            );
                            (CommandStatus::Error, 0, None)
                        }
                    }
                }
                Err(_) => {
                    warn!("link: [cmd] setRouteRetention rejected: malformed / out-of-range ({} B)", data.len());
                    (CommandStatus::Error, 0, None)
                }
            }
        }
        _ => (CommandStatus::UnknownCommand, 0, None),
    };
    CommandOutcome {
        result: StatusMessage::CommandResult(CommandResult::with_detail(cmd, status, detail)).encode(),
        store_changed,
        forget_bond,
    }
}
