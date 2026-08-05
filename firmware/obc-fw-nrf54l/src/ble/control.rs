//! The GATT control plane: the per-connection event pump that answers the OBC Control writes and arms
//! the CoC data plane.
//!
//! [`serve_connection`] owns the link until the peer drops it, servicing GATT reads/writes and the
//! connection lifecycle (PHY/params/pairing) events. Writes are answered with the typed `status`
//! envelope, never a hang or a bare ATT failure:
//!
//! - A `command` write ([`run_command`](crate::link::command::run_command)) — the §4.4 imperatives —
//!   answers `commandResult` and, on a store movement, notifies `storeChanged` (status msg 2 —
//!   protocol v2's sole change signal).
//! - A `transfer_control` write is decoded +
//!   [`classify_transfer`](crate::link::transfer::classify_transfer)-ed. A validated transfer is
//!   **armed** — signalled to the CoC task ([`super::state::TRANSFER_ARM`]) and answered later by the
//!   data plane; everything invalid (or an abort with nothing in flight) gets an immediate typed
//!   [`obc_ble::TransferResult`] on `status`, and an abort aimed at the in-flight transfer is forwarded
//!   to the data plane, which answers it.
//! - A `config` write validates + persists the rename/units to the RRAM settings; the advertised name
//!   follows on the next advertise cycle.
//! - The pairing/bonding events drive the passkey card + the single stored bond.
//!
//! The *decisions* behind the first three bullets are transport-free and live in [`crate::link`];
//! what this file owns is the GATT event pump, the handle routing, and the reply plumbing.
//!
//! Store borrows stay inside the synchronous `with_data` closures — never held across an `await`.

use core::cell::RefCell;

use defmt::{info, warn};
use nrf_sdc::{self as sdc};
use obc_ble::ObjectType;
use trouble_host::prelude::*;

use crate::link::command::run_command;
use crate::link::identity::apply_config_write;
use crate::link::transfer::{classify_transfer, TransferDisposition};
use crate::link::{StatusBytes, TRANSFER_ACTIVE};
use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use super::data_plane::{notify_bounded, publish_store_change};
use super::gatt::{config_blob, Server};
use super::state;
use super::state::{publish, TRANSFER_ABORT, TRANSFER_ARM};

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
                            match e.with_data(|_off, data| {
                                classify_transfer(data, store, &mut guard, crate::link::Transport::Ble)
                            }) {
                                TransferDisposition::Arm(armed) => {
                                    // **The claim's answer is honoured** (#1146 P2) — the radio twin
                                    // of the USB control plane's site, and for the same reason: the
                                    // gate grew a second refuser (a live route search holding the
                                    // scratch arena's nav arm) without this call site noticing.
                                    // Nothing awaits between `classify_transfer` and this claim on
                                    // the one cooperative executor, so a refusal is unreachable
                                    // today; arming against it anyway would hand the CoC plane a
                                    // store someone else owns.
                                    if TRANSFER_ACTIVE.claim(crate::link::gate_owner(crate::link::Transport::Ble)) {
                                        info!("ble: [gatt] transfer_control: transfer armed");
                                        TRANSFER_ARM.signal(armed);
                                    } else {
                                        warn!("ble: [gatt] transfer_control lost the gate after classify — busy");
                                        debug_assert!(
                                            false,
                                            "the gate moved between classify and claim with no await between them"
                                        );
                                        status_msg = Some(crate::link::transfer_result(
                                            armed.object_id(),
                                            obc_ble::TransferStatus::Busy,
                                        ));
                                    }
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
                            let applied = e.with_data(|_off, data| apply_config_write(data, store, &mut guard));
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
