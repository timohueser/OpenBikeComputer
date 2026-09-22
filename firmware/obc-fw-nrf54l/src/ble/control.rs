//! The GATT control plane: the per-connection event pump that answers the OBC Control writes and
//! arms the CoC data plane.
//!
//! [`serve_connection`] owns the link until the peer drops it, servicing GATT reads and writes and
//! the connection lifecycle events. Writes are answered with the typed `status` envelope, never a
//! hang or a bare ATT failure:
//!
//! - A `command` write ([`run_command`](crate::link::command::run_command)) answers `commandResult`.
//! - An `objectControl` write is one complete protocol-v4 control frame. It is not parsed here, but
//!   staged for the engine driver ([`super::v4::serve_objects`]), which answers it with a confirmed
//!   indication on the same characteristic. The ATT response says only that the frame was taken.
//! - A `config` write validates and persists the rename and units to the RRAM settings; the
//!   advertised name follows on the next advertise cycle.
//! - The pairing and bonding events drive the passkey card and the single stored bond.
//!
//! The decisions behind the first three bullets are transport-free and live in [`crate::link`]. What
//! this file owns is the GATT event pump, the handle routing and the reply plumbing.
//!
//! Store borrows stay inside the synchronous `with_data` closures, never held across an `await`.

use core::cell::RefCell;

use defmt::{info, warn};
use nrf_sdc::{self as sdc};
use trouble_host::prelude::*;

use crate::link::command::run_command;
use crate::link::identity::apply_config_write;
use crate::link::StatusBytes;
use crate::link_control::LinkControl;
use crate::SharedSettingsMutex;

use super::data_plane::notify_bounded;
use super::gatt::{config_blob, Server};
use super::state;
use super::state::publish;

/// Serve GATT and connection events until the peer drops the link, and return the disconnect reason
/// (an HCI status code). Answers the OBC Control writes with the typed `status` envelope, publishes
/// the link edges the status UI shows, and logs the rest. Concrete SDC and pool types, because the
/// `status` notify needs the `stack` and this runs only on the one controller.
pub(crate) async fn serve_connection(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &RefCell<LinkControl>,
    shared: &SharedSettingsMutex,
) -> u8 {
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            GattConnectionEvent::Gatt { event } => {
                // `objectControl` is served before the shared store is locked, and that ordering is
                // load-bearing. The ride loop holds this lock across a render pass, and parking here
                // would let a `PUT`'s first CoC SDU reach the engine before the control frame that
                // admits it. This arm needs no store — the engine owns the card, one queue hop away
                // — so it takes no lock, and the adapter's own admission hold covers the rest.
                let event = match event {
                    GattEvent::Write(e) if e.handle() == server.obc.object_control.handle => {
                        let staging = e.with_data(|_off, data| super::v4::stage_control(data));
                        let reply = match staging {
                            super::v4::Staging::Taken => {
                                info!("ble: [gatt] objectControl: control record staged for the engine");
                                e.accept()
                            }
                            // A frame this link cannot carry. The length is the complaint.
                            super::v4::Staging::BadLength => {
                                warn!("ble: [gatt] objectControl write refused — outside the record bound");
                                e.reject(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)
                            }
                            // The driver is busy with the previous record, or no stream channel is
                            // up yet. Neither is a complaint about these bytes, so a length error
                            // would be a lie: the client should retry, not re-encode.
                            super::v4::Staging::Unavailable => {
                                warn!("ble: [gatt] objectControl write refused — the driver cannot take it yet");
                                e.reject(AttErrorCode::PROCEDURE_ALREADY_IN_PROGRESS)
                            }
                        };
                        match reply {
                            Ok(reply) => reply.send().await,
                            Err(e) => warn!("ble: [gatt] error accepting objectControl: {:?}", e),
                        }
                        if matches!(staging, super::v4::Staging::Taken) {
                            // The ATT response is out, so the peer may write again — but the engine
                            // may not have consumed the staged record yet.
                            super::v4::control_taken().await;
                        }
                        continue;
                    }
                    other => other,
                };
                // Lock the shared store for this event's synchronous store work, then drop the
                // guard before the async sends below. The ride loop's map render can hold the same
                // lock, so a control write may wait a frame for it, which is harmless against the
                // supervision timeout.
                let mut guard = shared.lock().await;
                // Settings coherence, device to phone: if the ride loop persisted an on-device
                // settings change, our config cache is stale. Refresh it from RRAM and re-seed the
                // Config attribute, so a Config read on this connection serves the fresh units and
                // name without a reboot. The read path returns the seeded attribute value, not a live
                // `config_blob`, so the re-seed is what makes the read fresh.
                let config_refreshed = {
                    let mut s = store.borrow_mut();
                    let before = *s.settings();
                    s.refresh_settings_if_changed(&mut guard);
                    *s.settings() != before
                };
                if config_refreshed {
                    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));
                }
                // Extract what a control-plane write needs answered before accepting, which consumes
                // the event, then notify the `status` messages.
                let mut status_msg: Option<StatusBytes> = None;
                let mut config_written = false;
                let mut forget_after_ack = false;

                let reply = match event {
                    GattEvent::Write(e) => {
                        let handle = e.handle();
                        if handle == server.obc.command.handle {
                            let outcome = e.with_data(|_off, data| run_command(data));
                            status_msg = Some(outcome.result);
                            forget_after_ack = outcome.forget_bond;
                            info!("ble: [gatt] command write");
                            e.accept()
                        } else if handle == server.obc.config.handle {
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
                // Store work for this event is done; release the lock before the async sends.
                drop(guard);
                match reply {
                    Ok(reply) => {
                        // trouble-host's reply send is infallible once accept succeeded.
                        reply.send().await;
                    }
                    Err(e) => {
                        warn!("ble: [gatt] error accepting request: {:?}", e);
                    }
                };
                if let Some((buf, len)) = status_msg {
                    notify_bounded(stack, server, server.obc.status.handle, &buf[..len], "status").await;
                }
                if forget_after_ack {
                    // The `commandResult(ok)` ack is now with the controller, so the forget is safe
                    // to trigger. Ring the same request the on-device Forget-phone hold uses:
                    // `link_control`, the sibling in this link's `join4`, drains it, clears the bond
                    // and drops the link. Deferring to after the notify keeps the ordering the spec
                    // pins: ack first, then forget and disconnect.
                    state::request_forget_bond();
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

            // The device is DisplayOnly: the phone drives passkey entry, so `PassKeyDisplay` is the
            // one we expect — show the 6-digit code on the status screen and the rider types it into
            // the phone. Confirm and Input are handled defensively.
            //
            // While a bond is stored, a pairing attempt can only be a stranger, or the bonded phone
            // having lost its keys, and both are refused: Forget phone is the only re-pair path.
            // trouble-host has no app hook to answer the SMP Pairing Request itself, so the reject
            // lands at the first app-visible SMP event: suppress the passkey, because we never show a
            // code for a pairing we refuse, and drop the link. The bonded phone's silent reconnect is
            // encryption resumption, with no pairing events, so it never passes through here.
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
                    // Behind the passkey-stage reject: a pairing that slipped through anyway, such
                    // as a Just-Works attempt with no passkey stage, must not stand. The link is not
                    // bondable while a bond is stored, so `bond` is `None` here and the session's
                    // keys die with the link.
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
                // The link usually drops on failure, and the loop re-advertises.
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
                // The peer sent a pairing request that collides with our stored bond. The bond
                // survives — a peer merely claiming the bonded identity must not evict the real
                // phone — and the pairing attempt is rejected at its passkey stage above. A phone
                // that genuinely lost its keys re-pairs through Forget phone on the device.
                warn!("ble: [pair] peer re-pairing against our stored bond — keeping it (reject-when-bonded)");
            }
            _ => {}
        }
    };
    info!("ble: [conn] disconnected, reason 0x{:02X} ({:?})", reason.into_inner(), reason);
    reason.into_inner()
}
