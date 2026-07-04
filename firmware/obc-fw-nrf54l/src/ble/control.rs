//! The GATT control plane: the per-connection event pump that answers the OBC Control writes and arms
//! the CoC data plane.
//!
//! [`serve_connection`] owns the link until the peer drops it, servicing GATT reads/writes and the
//! connection lifecycle (PHY/params/pairing) events. Writes are answered with the typed `status`
//! envelope, never a hang or a bare ATT failure:
//!
//! - A `command` write ([`run_command`]) — `deleteObject` for routes; rides are never deleted over the
//!   link (the app tombstones them locally) — answers `commandResult` and, on a store movement,
//!   notifies `storeChanged` + the refreshed digest.
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

use super::data_plane::{notify_bounded, publish_store_change};
use super::gatt::{config_blob, Server};
use super::state::{publish, transfer_result, Armed, StatusBytes, TRANSFER_ABORT, TRANSFER_ACTIVE, TRANSFER_ARM};

/// What a `command` write did: the `commandResult` to notify, plus whether the store changed
/// (→ the caller also notifies `storeChanged` + the digest characteristic).
struct CommandOutcome {
    result: StatusBytes,
    store_changed: bool,
}

/// Execute a `command` write. `deleteObject` (cmd 1: `type u8 · object_id u16`) deletes a stored route
/// through the [`ObjectStore`]. Ride deletion is **deliberately not implemented** (`notFound`): the
/// device retains every tracked ride until a future device-side
/// management UI — the app hides synced rides locally (tombstones) rather than deleting them
/// here, so a re-sync can never resurrect them. Any other command byte is `unknownCommand`.
fn run_command(data: &[u8], store: &RefCell<ObjectStore>) -> CommandOutcome {
    let cmd = data.first().copied().unwrap_or(0);
    let (status, store_changed) = match (cmd, data) {
        (1, [_, ty, lo, hi, ..]) => {
            let id = u16::from_le_bytes([*lo, *hi]);
            match ObjectType::from_u8(*ty) {
                Ok(ObjectType::Route) => {
                    if store.borrow_mut().delete_route(id) {
                        info!("ble: [cmd] deleted route object {}", id);
                        (CommandStatus::Ok, true)
                    } else {
                        (CommandStatus::NotFound, false)
                    }
                }
                // Rides are never deleted over the link (see the fn doc); nothing else deletes.
                _ => (CommandStatus::NotFound, false),
            }
        }
        (1, _) => (CommandStatus::Error, false), // deleteObject with a truncated arg list
        _ => (CommandStatus::UnknownCommand, false),
    };
    CommandOutcome { result: StatusMessage::CommandResult(CommandResult::new(cmd, status)).encode(), store_changed }
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
fn classify_transfer(data: &[u8], store: &RefCell<ObjectStore>) -> TransferDisposition {
    let Ok(desc) = TransferControl::decode(data) else {
        // A malformed descriptor — the app can't have meant a real transfer; report `error`.
        return TransferDisposition::Answer(transfer_result(0, TransferStatus::Error));
    };
    if desc.op == Op::Abort {
        if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
            return TransferDisposition::AbortActive;
        }
        // Nothing in flight: discard any stray temp and confirm the abort.
        store.borrow_mut().upload_discard();
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Aborted));
    }
    if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Busy));
    }
    match (desc.op, desc.ty) {
        (Op::Upload, ObjectType::Echo) => TransferDisposition::Arm(Armed::Echo(desc)),
        (Op::Upload, ObjectType::Route) => match store.borrow_mut().upload_open(&desc) {
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
) -> u8 {
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            GattConnectionEvent::Gatt { event } => {
                // Extract what a control-plane write needs answered *before* accepting (which consumes
                // the event), then notify the `status` message(s) — never a hang / bare ATT failure. A
                // validated transfer instead arms the CoC data plane (`serve_coc`), which answers when
                // it ends. Store borrows stay inside the sync `with_data` closures — never across an await.
                let mut status_msg: Option<StatusBytes> = None;
                let mut store_changed = false;
                let mut config_written = false;
                let reply = match event {
                    GattEvent::Write(e) => {
                        let handle = e.handle();
                        if handle == server.obc.command.handle {
                            let outcome = e.with_data(|_off, data| run_command(data, store));
                            status_msg = Some(outcome.result);
                            store_changed = outcome.store_changed;
                            info!("ble: [gatt] command write");
                            e.accept()
                        } else if handle == server.obc.transfer_control.handle {
                            match e.with_data(|_off, data| classify_transfer(data, store)) {
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
                                        store.borrow_mut().apply_config(name, cfg.units);
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
                match reply {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("ble: [gatt] error sending response: {:?}", e),
                }
                if let Some((buf, len)) = status_msg {
                    notify_bounded(stack, server, server.obc.status.handle, &buf[..len], "status").await;
                }
                if store_changed {
                    publish_store_change(stack, server, store).await;
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
            GattConnectionEvent::PassKeyDisplay(passkey) => {
                info!("ble: [pair] display passkey {=u32:06}", passkey.value());
                publish(|s| s.passkey = Some(passkey.value()));
            }
            GattConnectionEvent::PassKeyConfirm(passkey) => {
                info!("ble: [pair] confirm passkey {=u32:06}", passkey.value());
                publish(|s| s.passkey = Some(passkey.value()));
            }
            GattConnectionEvent::PassKeyInput => {
                info!("ble: [pair] peer requests passkey input");
            }
            GattConnectionEvent::PairingComplete { security_level, bond } => {
                info!("ble: [pair] complete — level {:?}, bonded {}", security_level, bond.is_some());
                // Persist the single bond: a fresh pairing replaces whatever was stored.
                if let Some(bond) = bond {
                    store.borrow_mut().save_bond(&bond);
                }
                publish(|s| {
                    s.passkey = None;
                    s.secured = true;
                });
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
                // The peer paired again despite our stored bond ⇒ it lost its keys (the app/OS
                // "forgot" us). Drop our stale bond so this fresh pairing is the new one.
                warn!("ble: [pair] peer lost its bond — clearing stored bond");
                store.borrow_mut().clear_bond();
            }
            _ => {}
        }
    };
    info!("ble: [conn] disconnected, reason 0x{:02X} ({:?})", reason.into_inner(), reason);
    reason.into_inner()
}
