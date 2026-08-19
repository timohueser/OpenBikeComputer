//! Decode + classify a §4.2 `transferControl` descriptor against the store — the half of the
//! transfer handshake that has no transport in it.
//!
//! The caller supplies the bytes and owns the consequences: arming its own data plane, delivering
//! an immediate typed reject, or forwarding an abort. What a descriptor *means* — which ops and
//! object types are legal, which ids exist, when a second open is `busy` — is identical on every
//! wire and lives here.

use core::cell::RefCell;

use defmt::{info, warn};
use obc_ble::{ObjectType, Op, TransferControl, TransferStatus};

use crate::object_store::ObjectStore;
use crate::SharedStore;

use super::{transfer_result, Armed, StatusBytes, Transport, TRANSFER_ACTIVE};

/// How a decoded `transferControl` proceeds.
pub(crate) enum TransferDisposition {
    /// Validated — hand to this transport's data plane, which answers when the transfer ends.
    Arm(Armed),
    /// Answer immediately (a reject, or an abort the store had nothing to do about).
    Answer(StatusBytes),
    /// An abort that found **nothing in flight**: quiesce the byte pipe, run the cleanup the
    /// descriptor asked for, then answer.
    ///
    /// Split out of [`Answer`](Self::Answer) because it is the one disposition with work still owed
    /// on both sides, and the caller has to sequence it. See [`IdleAbort`].
    AnswerIdleAbort(IdleAbort),
    /// An abort aimed at the in-flight transfer — signal the data plane; *it* answers.
    AbortActive,
}

/// An `op = 3` that arrived with nothing in flight, and what the transport still owes it.
///
/// # The two meanings of an idle abort, and why they are told apart by type
///
/// A peer sends an abort with nothing armed for two unrelated reasons, and conflating them deletes
/// a rider's map:
///
///   * **Quiesce** — it gave up on a transfer the device had *already* closed (a reject it noticed
///     late, a cancel that raced the device's verdict, a shard the device refused on CRC) and it is
///     about to retry. Its already-queued bulk bytes are still arriving and cannot be recalled; if
///     the retry's descriptor arms while they land, they become that object's opening payload and
///     fail its whole-object CRC. This abort must touch **no stored state** — the whole point is
///     that a retry follows.
///   * **Abandon** — it is giving up on the staged **volume set** itself (`OBCA_Spec.md` §5.4). The
///     session closes and every file of the set is deleted, exactly as a dropped link would.
///
/// Both used to be "an idle abort on USB", and the set was destroyed either way. That was survivable
/// only because the host never sent the first kind; the moment it did — quiescing after a shard's
/// CRC refusal, so the retry would start clean — a single refused shard deleted the whole set and
/// the retry sealed a manifest over nothing.
///
/// So the descriptor's **type** carries the difference, and the host names what it means:
/// `mapSet` abandons, everything else quiesces. `mapShard` deliberately does **not** abandon —
/// a failed shard drops only itself (`ObjectStore::set_shard_finish`), which is the property the
/// per-shard retry depends on.
pub(crate) struct IdleAbort {
    /// The `aborted` result to send once the transport has done its part.
    pub(crate) bytes: StatusBytes,
    /// Whether this abort also abandons the staged volume set.
    pub(crate) abandon_set: bool,
}

/// The store-side half of an idle abort, run by the transport **after** it has quiesced its pipe.
///
/// Split out of [`classify_transfer`] for ordering: on the cable, abandoning a set is up to 32 file
/// deletions, and running them first would spend the peer's abort-ack budget before the endpoint had
/// even started emptying — the sequencing `usb::data_plane::DRAIN_BUDGET_MS` documents.
pub(crate) fn finish_idle_abort(store: &RefCell<ObjectStore>, shared: &mut SharedStore, abort: &IdleAbort) {
    // Any stray temp from a transfer that never reached its commit. A no-op when there is none.
    store.borrow_mut().upload_discard(shared);
    if abort.abandon_set {
        info!("link: [ctl] idle abort names the set — abandoning every staged file of it");
        store.borrow_mut().set_upload_abort(shared);
    }
}

/// Decode + classify against the store: echo uploads, route/trip/firmware/**map** uploads, and
/// route / ride / list / diagnostics downloads. Everything invalid — malformed bytes, an unknown id
/// (`notFound`), a second open mid-transfer (`busy`), an unsupported op/type combination, a map on
/// the radio — is answered immediately with the typed [`obc_ble::TransferResult`], never a hang or a
/// bare transport-level failure.
///
/// `transport` is the one transport fact this classifier needs, and it buys exactly one rule: maps
/// are USB-only (spec §10). Everything else here is wire-blind.
pub(crate) fn classify_transfer(
    data: &[u8],
    store: &RefCell<ObjectStore>,
    shared: &mut SharedStore,
    transport: Transport,
) -> TransferDisposition {
    let Ok(desc) = TransferControl::decode(data) else {
        // A malformed descriptor — the peer can't have meant a real transfer; report `error`.
        warn!(
            "link: [ctl] transfer_control reject: malformed {} B descriptor -> status {}",
            data.len(),
            TransferStatus::Error.as_u8()
        );
        return TransferDisposition::Answer(transfer_result(0, TransferStatus::Error));
    };
    if desc.op == Op::Abort {
        match TRANSFER_ACTIVE.holder() {
            Some(owner) if owner == super::gate_owner(transport) => {
                info!("link: [ctl] transfer_control abort active: type {} id {}", desc.ty.as_u8(), desc.object_id);
                return TransferDisposition::AbortActive;
            }
            // A transfer is in flight on the *other* wire. Forwarding the abort would signal a data
            // plane that is not in a transfer, so nothing would ever answer it and the peer would
            // wait forever; the honest answer is that this link is not the one transferring
            // (issue #1039).
            Some(_) => {
                warn!("link: [ctl] transfer_control abort ignored: the transfer belongs to the other transport");
                return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Busy));
            }
            None => {}
        }
        // Nothing in flight. **The store work does not happen here** — the transport runs it through
        // `finish_idle_abort` once it has quiesced its pipe, because on the cable it can be a whole
        // set's worth of deletions and those must not run in front of the drain.
        //
        // A **volume set** is abandoned only when the descriptor names one (issue #1039 for the
        // abandonment; see `IdleAbort` for why the type is what tells the two meanings apart).
        // Scoped to USB because a set is only ever received there (spec §10), and because this
        // classifier runs on the radio too: an abort from the phone must not delete the set the
        // cable is between shards of.
        let abandon_set = transport == Transport::Usb && desc.ty == ObjectType::MapSet;
        info!(
            "link: [ctl] transfer_control answer: op {} type {} id {} len {} -> status {}",
            desc.op.as_u8(),
            desc.ty.as_u8(),
            desc.object_id,
            desc.total_len,
            TransferStatus::Aborted.as_u8()
        );
        return TransferDisposition::AnswerIdleAbort(IdleAbort {
            bytes: transfer_result(desc.object_id, TransferStatus::Aborted),
            abandon_set,
        });
    }
    // `busy()`, not `in_flight()` (#1146 P2): the gate arbitrates a second resource now — the
    // scratch arena's `nav ⊥ usb` rule — so a live **route search** must answer `busy` here too.
    // `claim` refuses either way, but a control plane that only tested the narrow predicate armed a
    // transfer that then could not take the gate, leaving the host waiting on a `transferResult`
    // nothing would send. `in_flight()` keeps its narrow meaning for the abort routing above, where
    // "which wire is streaming" is exactly the question.
    if TRANSFER_ACTIVE.busy() {
        warn!(
            "link: [ctl] transfer_control reject: op {} type {} id {} len {} -> status {} (active)",
            desc.op.as_u8(),
            desc.ty.as_u8(),
            desc.object_id,
            desc.total_len,
            TransferStatus::Busy.as_u8()
        );
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Busy));
    }
    match (desc.op, desc.ty) {
        (Op::Upload, ObjectType::Echo) => {
            log_transfer_arm(&desc);
            TransferDisposition::Arm(Armed::Echo(desc))
        }
        (Op::Upload, ObjectType::Route) => match store.borrow_mut().upload_open(shared, &desc) {
            Ok(rx) => {
                log_transfer_arm(&desc);
                TransferDisposition::Arm(Armed::Upload(desc, rx))
            }
            Err(status) => {
                log_transfer_reject(&desc, status);
                TransferDisposition::Answer(transfer_result(desc.object_id, status))
            }
        },
        // A trip upload (epic #526 TR4): the same streaming + commit-then-swap as a route, but the
        // storage-full guard is against the *trip* catalog and the commit target is `TP{id}.OBT`. The
        // finish routes on `desc.ty` in the data plane, exactly like the route/fwImage split.
        (Op::Upload, ObjectType::Trip) => match store.borrow_mut().upload_open_trip(shared, &desc) {
            Ok(rx) => {
                log_transfer_arm(&desc);
                TransferDisposition::Arm(Armed::Upload(desc, rx))
            }
            Err(status) => {
                log_transfer_reject(&desc, status);
                TransferDisposition::Answer(transfer_result(desc.object_id, status))
            }
        },
        // A firmware update image (epic #615 S6, #621): the size guard rejects an oversize object at
        // announce, before any byte is consumed; a committed transfer promotes to /UPDATE.BIN (staging,
        // not installing — see `fwimage_finish` + the `installFw` command). Same `Armed::Upload` arm as
        // a route — the streaming is identical; only the commit target differs (`desc.ty`).
        (Op::Upload, ObjectType::FwImage) => match store.borrow_mut().fwimage_open(shared, &desc) {
            Ok(rx) => {
                log_transfer_arm(&desc);
                TransferDisposition::Arm(Armed::Upload(desc, rx))
            }
            Err(status) => {
                log_transfer_reject(&desc, status);
                TransferDisposition::Answer(transfer_result(desc.object_id, status))
            }
        },
        // A map upload (#889 for the type, #927 for the storage): **USB only** (spec §10). A map is
        // hundreds of megabytes — over BLE that is days, which is why the type did not exist before a
        // cable did — so a map descriptor on the radio is refused here rather than being handed to a
        // data plane that would treat it as a route. The reject is typed and logged, not a silent
        // fall-through to the catch-all, so a host that tries gets a diagnosable "no".
        //
        // On USB the store's own announce guard runs: new-only (the device never rewrites a stored
        // map in place — see the map section of `sd.rs`), long enough to be an OBCM, and a card with
        // room to spare. All three refuse **before any byte streams**, because a several-hundred-
        // megabyte transfer that fails at the end has cost the rider minutes.
        (Op::Upload, ObjectType::Map) if transport == Transport::Usb => {
            match store.borrow_mut().map_upload_open(shared, &desc) {
                Ok(rx) => {
                    log_transfer_arm(&desc);
                    TransferDisposition::Arm(Armed::Upload(desc, rx))
                }
                Err(status) => {
                    log_transfer_reject(&desc, status);
                    TransferDisposition::Answer(transfer_result(desc.object_id, status))
                }
            }
        }
        // One **shard** of a volume set (#1039, `OBCA_Spec.md` §5.1): the same streaming shape as a
        // map, so it arms the same `Armed::Upload`; what the store checks here is the set session —
        // a part field that names a real file, a shard count inside this board's ceiling, and
        // agreement with the set already in flight. The part travels with the arm because the data
        // plane needs it to name the file at the first byte.
        (Op::Upload, ObjectType::MapShard) if transport == Transport::Usb => {
            match store.borrow().set_shard_open(shared, &desc) {
                Ok((rx, part)) => {
                    log_transfer_arm(&desc);
                    TransferDisposition::Arm(Armed::SetShard(desc, rx, part))
                }
                Err(status) => {
                    log_transfer_reject(&desc, status);
                    set_refusal_to_glass(store, &desc);
                    TransferDisposition::Answer(transfer_result(desc.object_id, status))
                }
            }
        }
        // The set's **terrain shard** (#1044, `OBCA_Spec.md` §5.1's `terrain` role): an OBCT raster
        // rather than an OBCM file, so it needs its own type — a shard's `object_id` is a
        // `(shard_count, index)` pair naming one of the files the manifest's *leading* records
        // describe, and a raster is none of those. Same streaming shape as everything else in this
        // band: straight into `MS{id}.OBD` with the OBCT magic held back.
        //
        // Why the type exists at all rather than the host simply skipping the raster: §5.2's
        // `Shard Count` counts **every** record, terrain included, so a set whose host holds a
        // terrain-bearing manifest announces 56 more bytes than a device that never saw the raster
        // expects — and the manifest is refused at the very last transfer of a multi-gigabyte
        // upload.
        (Op::Upload, ObjectType::TerrainShard) if transport == Transport::Usb => {
            match store.borrow().set_terrain_open(shared, &desc) {
                Ok(rx) => {
                    log_transfer_arm(&desc);
                    TransferDisposition::Arm(Armed::SetTerrain(desc, rx))
                }
                Err(status) => {
                    log_transfer_reject(&desc, status);
                    set_refusal_to_glass(store, &desc);
                    TransferDisposition::Answer(transfer_result(desc.object_id, status))
                }
            }
        }
        // The set **manifest** — `OBCA_Spec.md` §5.4's manifest-last rule, enforced here rather
        // than trusted: a manifest announced before every shard it will name has committed is
        // refused *before a byte streams*, so a host that gets the order wrong learns in
        // milliseconds instead of after gigabytes.
        (Op::Upload, ObjectType::MapSet) if transport == Transport::Usb => {
            match store.borrow().set_manifest_open(shared, &desc) {
                Ok(rx) => {
                    log_transfer_arm(&desc);
                    TransferDisposition::Arm(Armed::SetManifest(desc, rx))
                }
                Err(status) => {
                    log_transfer_reject(&desc, status);
                    set_refusal_to_glass(store, &desc);
                    TransferDisposition::Answer(transfer_result(desc.object_id, status))
                }
            }
        }
        (
            Op::Download,
            ObjectType::Route
            | ObjectType::Ride
            | ObjectType::Trip
            | ObjectType::RouteList
            | ObjectType::RideList
            | ObjectType::TripList
            | ObjectType::Diagnostics,
        ) => {
            // Cheap existence check here for the immediate `notFound`; the source itself (and
            // its CRC pre-pass) opens on the data plane, off the control-reply path.
            let known = match desc.ty {
                ObjectType::Route => store.borrow().has_route(desc.object_id),
                ObjectType::Ride => store.borrow().has_ride(desc.object_id),
                ObjectType::Trip => store.borrow().has_trip(shared, desc.object_id),
                _ => true,
            };
            if !known {
                log_transfer_reject(&desc, TransferStatus::NotFound);
                return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::NotFound));
            }
            log_transfer_arm(&desc);
            TransferDisposition::Arm(Armed::Download(desc))
        }
        // Uploads of ride/list/config/diagnostics types are nonsensical.
        _ => {
            log_transfer_reject(&desc, TransferStatus::Error);
            TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Error))
        }
    }
}

/// Correct the glass when a file of the **volume set in flight** is refused before it streams
/// (#1044).
///
/// An announce-time refusal is normally invisible on the device by design: no transfer starts, so
/// there is nothing on screen and the host is the one told. A set breaks that, because it is
/// several transfers and each of them ends its own card — so the shard before this one left "Map
/// installed / Restart" up, on a set that is now refused whole and will be swept at the next boot.
/// That is the device stating the opposite of the truth, and it is worth one atomic store to fix.
///
/// Scoped to a **session in flight**: with no set open there is no stale success to correct, and
/// raising a red card at a host that opened with a malformed descriptor would be noise the rider
/// cannot act on.
fn set_refusal_to_glass(store: &RefCell<ObjectStore>, desc: &TransferControl) {
    if store.borrow().set_upload_active() {
        warn!(
            "link: [ctl] volume-set file refused mid-set (type {} id {}) — raising the failure card",
            desc.ty.as_u8(),
            desc.object_id
        );
        super::map_transfer_refused();
    }
}

/// Log one accepted descriptor in the same numeric vocabulary as the iOS / browser console, so an
/// on-device trace correlates both sides of the exchange.
fn log_transfer_arm(desc: &TransferControl) {
    info!(
        "link: [ctl] transfer_control arm: op {} type {} id {} len {} crc {=u32:#010x}",
        desc.op.as_u8(),
        desc.ty.as_u8(),
        desc.object_id,
        desc.total_len,
        desc.crc32
    );
}

/// Instrument an immediate semantic reject with its exact wire status.
fn log_transfer_reject(desc: &TransferControl, status: TransferStatus) {
    warn!(
        "link: [ctl] transfer_control reject: op {} type {} id {} len {} -> status {}",
        desc.op.as_u8(),
        desc.ty.as_u8(),
        desc.object_id,
        desc.total_len,
        status.as_u8()
    );
}
