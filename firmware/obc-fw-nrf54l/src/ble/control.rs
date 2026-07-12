//! The GATT control plane: the per-connection event pump that answers the OBC Control writes and arms
//! the CoC data plane.
//!
//! [`serve_connection`] owns the link until the peer drops it, servicing GATT reads/writes and the
//! connection lifecycle (PHY/params/pairing) events. Writes are answered with the typed `status`
//! envelope, never a hang or a bare ATT failure:
//!
//! - A `command` write ([`run_command`]) — `deleteObject` for routes (rides are never deleted over
//!   the link; the app tombstones them locally) and `ackRides` (the phone's possession list
//!   reconciles the synced sidecar) — answers `commandResult` and, on a store movement, notifies
//!   `storeChanged` (status msg 2 — protocol v2's sole change signal).
//! - A `transfer_control` write is decoded + [`classify_transfer`]-ed. A validated transfer is
//!   **armed** — signalled to the CoC task ([`super::state::TRANSFER_ARM`]) and answered later by the
//!   data plane; everything invalid (or an abort with nothing in flight) gets an immediate typed
//!   [`obc_ble::TransferResult`] on `status`, and an abort aimed at the in-flight transfer is forwarded
//!   to the data plane, which answers it.
//! - A `config` write validates + persists the rename/units to the RRAM settings; the advertised name
//!   follows on the next advertise cycle.
//! - The pairing/bonding events drive the passkey card + the single stored bond.
//!
//! Store borrows stay inside the synchronous `with_data` closures — never held across an `await`.

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use defmt::{info, warn};
use nrf_sdc::{self as sdc};
use obc_ble::{CommandResult, CommandStatus, Config, ObjectType, Op, StatusMessage, TransferControl, TransferStatus};
use trouble_host::prelude::*;

use crate::object_store::ObjectStore;
use crate::{SharedStore, SharedStoreMutex};

use super::data_plane::{notify_bounded, publish_store_change};
use super::gatt::{config_blob, Server};
use super::state;
use super::state::{publish, transfer_result, Armed, StatusBytes, TRANSFER_ABORT, TRANSFER_ACTIVE, TRANSFER_ARM};

/// What a `command` write did: the `commandResult` to notify, plus which store (if any) it moved
/// (→ the caller also notifies `storeChanged`, typed accordingly).
struct CommandOutcome {
    result: StatusBytes,
    store_changed: Option<ObjectType>,
    /// `forgetBond` (§4.4 cmd 4): the app asked the device to dissolve its own bond. Deferred, not
    /// done inline — the caller rings [`state::request_forget_bond`] **after** the `commandResult`
    /// ack has gone out, so the ack reaches the phone before the forget machinery
    /// ([`super::link_control`]) clears the bond and drops the link.
    forget_bond: bool,
}

/// Execute a `command` write. `deleteObject` (cmd 1: `type u8 · object_id u16`) deletes a stored route
/// through the [`ObjectStore`]. Ride deletion over the link is **deliberately not implemented**
/// (`notFound`): rides are deleted only from the device's Rides screen (#454) — the app hides synced
/// rides locally (tombstones) rather than deleting them here, so a re-sync can never resurrect them.
/// `ackRides` (cmd 2: `count u8 · count × object_id u16`) reconciles the synced sidecar from the
/// phone's possession list ([`ObjectStore::ack_rides`]); its `commandResult.detail` reports the
/// newly-flagged count. Any other command byte is `unknownCommand`.
fn run_command(data: &[u8], store: &RefCell<ObjectStore>, shared: &mut SharedStore) -> CommandOutcome {
    let cmd = data.first().copied().unwrap_or(0);
    let mut forget_bond = false;
    let (status, detail, store_changed) = match (cmd, data) {
        (obc_ble::CMD_DELETE_OBJECT, [_, ty, lo, hi, ..]) => {
            let id = u16::from_le_bytes([*lo, *hi]);
            match ObjectType::from_u8(*ty) {
                Ok(ObjectType::Route) => {
                    if store.borrow_mut().delete_route(shared, id) {
                        info!("ble: [cmd] deleted route object {}", id);
                        (CommandStatus::Ok, 0, Some(ObjectType::Route))
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
                info!("ble: [cmd] ackRides: {} acked, {} newly flagged", ack.count(), newly);
                // Only an actual flag change moved the store (and only the ride side of it).
                (CommandStatus::Ok, newly, (newly > 0).then_some(ObjectType::Ride))
            }
            Err(_) => (CommandStatus::Error, 0, None), // count promises more ids than the write carries
        },
        (obc_ble::CMD_INSTALL_FW, _) => {
            // installFw (epic #615 S6, #621): request the on-glass-confirmed install of the staged
            // /UPDATE.BIN. Answer from cheaply-knowable edge state only — `busy` (a ride recording or an
            // install already pending) and `noStaged` (a card-root existence check); the multi-second
            // OBCU CRC scan is NOT run here (it belongs to the on-device flow), so `invalid` is never
            // produced — the handler accepts and the scan surfaces a bad image on glass. On `ok` it
            // posts a request the ride loop drains into `App::open_remote_dfu_check` — push the
            // "Checking card..." wait + post `DfuAction::Scan`, the System menu's press arriving over
            // the air — and nothing more. It never posts `DfuAction::Install` (that stays the confirm
            // screen's press and the physical debug link's): the command never waits for the human and
            // never arms/reboots on its own (spec §4.4 security posture — no silent installs, ever).
            let has_staged = store.borrow().update_staged(shared);
            let busy = state::recording() || crate::object_store::dfu_install_pending();
            let status = obc_ble::install_fw_reply(has_staged, busy, false);
            if matches!(status, CommandStatus::Ok) {
                crate::object_store::request_dfu_install_ble();
                info!("ble: [cmd] installFw accepted — install request posted (awaits on-glass confirm)");
            } else {
                info!("ble: [cmd] installFw rejected: {}", status.as_u8());
            }
            (status, 0, None)
        }
        (obc_ble::CMD_FORGET_BOND, _) => {
            // forgetBond (#756): the app's "Forget device" asks the device to dissolve its side of
            // the bond too, so a one-sided app forget doesn't leave the pair wedged (the device would
            // otherwise keep rejecting new pairings under the #455 reject-when-bonded posture until the
            // rider ran Forget phone on the device). This is only reachable over the authenticated,
            // encrypted link — the gated `command` characteristic requires it (§8) — so the bonded
            // phone clearing its own bond is fully consistent with reject-when-bonded; a stranger can
            // never issue it. We DON'T forget here: answer `commandResult(ok)` and defer the forget to
            // *after* the ack has been sent (see the caller), so the phone gets its ack before the link
            // drops. The forget itself reuses the on-device Forget-phone machinery (`link_control` →
            // `forget_bond`): clears the RRAM bond slot + host table, lowers `paired`, drops the link,
            // and re-opens pairing on the next connection.
            forget_bond = true;
            info!("ble: [cmd] forgetBond — ack first, then clear bond + drop link");
            (CommandStatus::Ok, 0, None)
        }
        _ => (CommandStatus::UnknownCommand, 0, None),
    };
    CommandOutcome {
        result: StatusMessage::CommandResult(CommandResult::with_detail(cmd, status, detail)).encode(),
        store_changed,
        forget_bond,
    }
}

/// How a decoded `transfer_control` write proceeds.
enum TransferDisposition {
    /// Validated — hand to the CoC task (`serve_coc`), which answers when the transfer ends.
    Arm(Armed),
    /// Answer immediately on `status` (a reject, or an abort with nothing in flight).
    Answer(StatusBytes),
    /// An abort aimed at the in-flight transfer — signal the data plane; *it* answers.
    AbortActive,
}

/// Decode + classify a `transfer_control` write against the store: echo uploads, route uploads (fresh
/// or replace-by-id), and route / list downloads. Everything invalid — malformed bytes, an unknown id
/// (`notFound`), a non-zero upload offset or a second open mid-transfer, an unsupported op/type
/// combination — is answered immediately with the typed [`obc_ble::TransferResult`] (`error` /
/// `notFound` / `busy`), never a hang or a bare ATT failure.
fn classify_transfer(data: &[u8], store: &RefCell<ObjectStore>, shared: &mut SharedStore) -> TransferDisposition {
    let Ok(desc) = TransferControl::decode(data) else {
        // A malformed descriptor — the app can't have meant a real transfer; report `error`.
        return TransferDisposition::Answer(transfer_result(0, TransferStatus::Error));
    };
    if desc.op == Op::Abort {
        if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
            return TransferDisposition::AbortActive;
        }
        // Nothing in flight: discard any stray temp and confirm the abort.
        store.borrow_mut().upload_discard(shared);
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Aborted));
    }
    if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Busy));
    }
    match (desc.op, desc.ty) {
        (Op::Upload, ObjectType::Echo) => TransferDisposition::Arm(Armed::Echo(desc)),
        (Op::Upload, ObjectType::Route) => match store.borrow_mut().upload_open(shared, &desc) {
            Ok(rx) => TransferDisposition::Arm(Armed::Upload(desc, rx)),
            Err(status) => TransferDisposition::Answer(transfer_result(desc.object_id, status)),
        },
        // A firmware update image (epic #615 S6, #621): the size guard rejects an oversize object at
        // announce, before any byte streams; a committed transfer promotes to /UPDATE.BIN (staging,
        // not installing — see `fwimage_finish` + the `installFw` command). Same `Armed::Upload` arm as
        // a route — the CoC streaming is identical; only the commit target differs (`desc.ty`).
        (Op::Upload, ObjectType::FwImage) => match store.borrow_mut().fwimage_open(shared, &desc) {
            Ok(rx) => TransferDisposition::Arm(Armed::Upload(desc, rx)),
            Err(status) => TransferDisposition::Answer(transfer_result(desc.object_id, status)),
        },
        (
            Op::Download,
            ObjectType::Route
            | ObjectType::Ride
            | ObjectType::RouteList
            | ObjectType::RideList
            | ObjectType::Diagnostics,
        ) => {
            // Cheap existence check here for the immediate `notFound`; the source itself (and
            // its CRC pre-pass) opens on the data plane, off the GATT reply path.
            let known = match desc.ty {
                ObjectType::Route => store.borrow().has_route(desc.object_id),
                ObjectType::Ride => store.borrow().has_ride(desc.object_id),
                _ => true,
            };
            if !known {
                return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::NotFound));
            }
            TransferDisposition::Arm(Armed::Download(desc))
        }
        // Uploads of ride/list/config/diagnostics types are nonsensical.
        _ => TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Error)),
    }
}

/// Serve GATT + connection events until the peer drops the link. Returns the disconnect reason (HCI
/// status code); answers the OBC Control writes with the typed `status` envelope, publishes the link
/// edges the status UI shows (conn interval, PHY) and logs the rest — including every disconnect
/// reason, named + numeric. Concrete SDC/pool types (like [`super::lifecycle::negotiate_link`]): the
/// `status` notify needs the `stack`, and this only runs on the one controller.
pub(crate) async fn serve_connection(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
) -> u8 {
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            GattConnectionEvent::Gatt { event } => {
                // Lock the shared store for this event's synchronous store work (an SD delete / upload
                // check / config persist), then drop the guard before the async status/store-change
                // sends below (#270). The ride loop's map render can hold the same lock, so a control
                // write may wait a frame for it — harmless against the seconds-long supervision timeout.
                let mut guard = shared.lock().await;
                // Settings coherence, device → phone (#456): if the ride loop persisted an on-device
                // settings change since the last event, its RRAM blob moved and our config cache is
                // stale. Refresh the cache from RRAM (a no-op when nothing changed) and re-seed the
                // Config attribute so a Config *read* this connection serves the fresh units/name
                // without a reboot. The read path returns the seeded attribute value, not a live
                // `config_blob`, so the re-seed is what actually makes the read fresh.
                let config_refreshed = {
                    let mut s = store.borrow_mut();
                    let before = *s.settings();
                    s.refresh_settings_if_changed(&mut guard);
                    *s.settings() != before
                };
                if config_refreshed {
                    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));
                }
                // Extract what a control-plane write needs answered *before* accepting (which consumes
                // the event), then notify the `status` message(s) — never a hang / bare ATT failure. A
                // validated transfer instead arms the CoC data plane (`serve_coc`), which answers when
                // it ends. Store borrows stay inside the sync `with_data` closures — never across an await.
                let mut status_msg: Option<StatusBytes> = None;
                let mut store_changed: Option<ObjectType> = None;
                let mut config_written = false;
                let mut forget_after_ack = false;
                let reply = match event {
                    GattEvent::Write(e) => {
                        let handle = e.handle();
                        if handle == server.obc.command.handle {
                            let outcome = e.with_data(|_off, data| run_command(data, store, &mut guard));
                            status_msg = Some(outcome.result);
                            store_changed = outcome.store_changed;
                            forget_after_ack = outcome.forget_bond;
                            info!("ble: [gatt] command write");
                            e.accept()
                        } else if handle == server.obc.transfer_control.handle {
                            match e.with_data(|_off, data| classify_transfer(data, store, &mut guard)) {
                                TransferDisposition::Arm(armed) => {
                                    info!("ble: [gatt] transfer_control: transfer armed");
                                    TRANSFER_ACTIVE.store(true, Ordering::Relaxed);
                                    TRANSFER_ARM.signal(armed);
                                }
                                TransferDisposition::AbortActive => {
                                    info!("ble: [gatt] transfer_control: abort → data plane");
                                    TRANSFER_ABORT.signal(());
                                }
                                TransferDisposition::Answer(bytes) => {
                                    info!("ble: [gatt] transfer_control: answered on status");
                                    status_msg = Some(bytes);
                                }
                            }
                            e.accept()
                        } else if handle == server.obc.config.handle {
                            // Validate + apply: units and rename persist to RRAM settings; the
                            // advertised name follows on the next adv cycle.
                            let applied = e.with_data(|_off, data| match Config::decode(data) {
                                Some(cfg) => match core::str::from_utf8(cfg.name) {
                                    Ok(name) => {
                                        store.borrow_mut().apply_config(&mut guard, name, cfg.units);
                                        true
                                    }
                                    Err(_) => false,
                                },
                                None => false,
                            });
                            if applied {
                                info!("ble: [gatt] config write applied + persisted");
                                config_written = true;
                                e.accept()
                            } else {
                                warn!("ble: [gatt] config write rejected (malformed)");
                                e.reject(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)
                            }
                        } else {
                            info!("ble: [gatt] write handle {}", handle);
                            e.accept()
                        }
                    }
                    GattEvent::Read(e) => {
                        info!("ble: [gatt] read handle {}", e.handle());
                        e.accept()
                    }
                    // Permission-violating request (e.g. a write to a read-only attribute): accepting
                    // lets the server send the proper ATT error response rather than dropping it.
                    GattEvent::NotAllowed(e) => e.accept(),
                    GattEvent::Other(e) => e.accept(),
                };
                // Store work for this event is done; release the shared lock before the async sends
                // (the RefCell borrows above already ended with `reply`). `config_blob`/storeChanged below
                // read only the catalog + the settings cache, no card.
                drop(guard);
                match reply {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("ble: [gatt] error sending response: {:?}", e),
                }
                if let Some((buf, len)) = status_msg {
                    notify_bounded(stack, server, server.obc.status.handle, &buf[..len], "status").await;
                }
                if forget_after_ack {
                    // forgetBond (#756): the `commandResult(ok)` ack has now been handed to the
                    // controller, so it's safe to trigger the forget. Ring the same request the
                    // on-device Forget-phone hold uses — `link_control` (the sibling in this link's
                    // `join4`) drains it, clears the bond via `forget_bond` (RRAM slot + host table +
                    // `paired = false`), and drops the link. Deferring to after the notify keeps the
                    // ordering the spec pins: ack first, then forget + disconnect (§4.4, race-free —
                    // we never disconnect before the ack is out).
                    state::request_forget_bond();
                }
                if let Some(ty) = store_changed {
                    publish_store_change(stack, server, store, ty).await;
                }
                if config_written {
                    // Re-seed the characteristic with the canonical blob (what a read serves).
                    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));
                }
            }
            GattConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                info!("ble: [conn] PHY updated: tx {:?} rx {:?}", tx_phy, rx_phy);
                // "2M" on the status screen only when both directions made it.
                publish(|s| s.phy_2m = matches!(tx_phy, PhyKind::Le2M) && matches!(rx_phy, PhyKind::Le2M));
            }
            GattConnectionEvent::ConnectionParamsUpdated { conn_interval, peripheral_latency, supervision_timeout } => {
                info!(
                    "ble: [conn] params: interval {} ms latency {} timeout {} ms",
                    conn_interval.as_millis(),
                    peripheral_latency,
                    supervision_timeout.as_millis()
                );
                publish(|s| s.conn_interval_ms = conn_interval.as_millis() as u32);
            }
            GattConnectionEvent::DataLengthUpdated { max_tx_octets, max_rx_octets, .. } => {
                info!("ble: [conn] data length: tx {} rx {} octets", max_tx_octets, max_rx_octets);
            }

            // ---- Pairing / bonding lifecycle ----
            // The device is DisplayOnly: the phone drives passkey *entry*, so `PassKeyDisplay` is
            // the one we expect — show the 6-digit code big on the status screen; the rider types
            // it into the iOS dialog. (Confirm/Input are handled defensively for completeness.)
            //
            // **Reject-when-bonded (#455, S0 §8 amendment):** while a bond is stored, a pairing
            // attempt can only be a stranger — or the bonded phone having lost its keys — and both
            // are refused: Forget phone is the only re-pair path. trouble-host 0.7 has no app hook
            // to answer the SMP Pairing Request itself (the SM auto-responds before any event
            // reaches us), so the reject lands at the first app-visible SMP event: suppress the
            // passkey (never show a code for a pairing we refuse) and drop the link. The stranger's
            // phone sees a generic pairing failure; the device shows nothing (locked: app-side
            // message only). The bonded phone's silent reconnect is encryption *resumption* — no
            // pairing events — so it never passes through here.
            GattConnectionEvent::PassKeyDisplay(passkey) => {
                if state::status().paired {
                    warn!("ble: [pair] pairing attempt while bonded — rejecting (forget phone to re-pair)");
                    conn.raw().disconnect();
                } else {
                    info!("ble: [pair] display passkey {=u32:06}", passkey.value());
                    publish(|s| s.passkey = Some(passkey.value()));
                }
            }
            GattConnectionEvent::PassKeyConfirm(passkey) => {
                if state::status().paired {
                    warn!("ble: [pair] pairing attempt while bonded — rejecting (forget phone to re-pair)");
                    conn.raw().disconnect();
                } else {
                    info!("ble: [pair] confirm passkey {=u32:06}", passkey.value());
                    publish(|s| s.passkey = Some(passkey.value()));
                }
            }
            GattConnectionEvent::PassKeyInput => {
                info!("ble: [pair] peer requests passkey input");
            }
            GattConnectionEvent::PairingComplete { security_level, bond } => {
                if state::status().paired {
                    // Belt-and-braces behind the passkey-stage reject: a pairing that slipped
                    // through anyway (e.g. a Just-Works attempt with no passkey stage) must not
                    // stand. The link is not bondable while a bond is stored, so `bond` is `None`
                    // here — nothing to persist — and the session's keys die with the link.
                    warn!("ble: [pair] pairing completed while bonded — dropping the link (not replacing the bond)");
                    conn.raw().disconnect();
                } else {
                    info!("ble: [pair] complete — level {:?}, bonded {}", security_level, bond.is_some());
                    // Persist the single bond (the open-pairing path: nothing was stored).
                    if let Some(bond) = bond {
                        let mut guard = shared.lock().await;
                        store.borrow_mut().save_bond(&mut guard, &bond);
                        publish(|s| s.paired = true);
                    }
                    publish(|s| {
                        s.passkey = None;
                        s.secured = true;
                    });
                }
            }
            GattConnectionEvent::PairingFailed(e) => {
                warn!("ble: [pair] failed: {:?}", defmt::Debug2Format(&e));
                // The link usually drops on failure → the app lands on D5 and we re-advertise.
                publish(|s| s.passkey = None);
            }
            GattConnectionEvent::Encrypted { security_level, bond } => {
                // Fires for a resumed bonded session too (no pairing UI) — mark the link secured.
                info!("ble: [pair] encrypted — level {:?}, from bond {}", security_level, bond.is_some());
                publish(|s| {
                    s.passkey = None;
                    s.secured = true;
                });
            }
            GattConnectionEvent::BondLost => {
                // The peer sent a pairing request that collides with our stored bond. Under A8 this
                // cleared the stored bond (auto-replace); #455 **reverses** that: the bond survives
                // — a peer merely *claiming* the bonded identity must not be able to evict the real
                // phone — and the pairing attempt itself is rejected at its passkey stage above.
                // A phone that genuinely lost its keys re-pairs via Forget phone on the device.
                warn!("ble: [pair] peer re-pairing against our stored bond — keeping it (reject-when-bonded)");
            }
            _ => {}
        }
    };
    info!("ble: [conn] disconnected, reason 0x{:02X} ({:?})", reason.into_inner(), reason);
    reason.into_inner()
}
