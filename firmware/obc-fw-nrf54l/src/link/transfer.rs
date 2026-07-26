//! Decode + classify a §4.2 `transferControl` descriptor against the store — the half of the
//! transfer handshake that has no transport in it.
//!
//! The caller supplies the bytes and owns the consequences: arming its own data plane, delivering
//! an immediate typed reject, or forwarding an abort. What a descriptor *means* — which ops and
//! object types are legal, which ids exist, when a second open is `busy` — is identical on every
//! wire and lives here.

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use defmt::{info, warn};
use obc_ble::{ObjectType, Op, TransferControl, TransferStatus};

use crate::object_store::ObjectStore;
use crate::SharedStore;

use super::{transfer_result, Armed, StatusBytes, TRANSFER_ACTIVE};

/// How a decoded `transferControl` proceeds.
pub(crate) enum TransferDisposition {
    /// Validated — hand to this transport's data plane, which answers when the transfer ends.
    Arm(Armed),
    /// Answer immediately (a reject, or an abort with nothing in flight).
    Answer(StatusBytes),
    /// An abort aimed at the in-flight transfer — signal the data plane; *it* answers.
    AbortActive,
}

/// Decode + classify against the store: echo uploads, route/trip/firmware uploads, and
/// route / ride / list / diagnostics downloads. Everything invalid — malformed bytes, an unknown id
/// (`notFound`), a second open mid-transfer (`busy`), an unsupported op/type combination — is
/// answered immediately with the typed [`obc_ble::TransferResult`], never a hang or a bare
/// transport-level failure.
pub(crate) fn classify_transfer(
    data: &[u8],
    store: &RefCell<ObjectStore>,
    shared: &mut SharedStore,
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
        if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
            info!("link: [ctl] transfer_control abort active: type {} id {}", desc.ty.as_u8(), desc.object_id);
            return TransferDisposition::AbortActive;
        }
        // Nothing in flight: discard any stray temp and confirm the abort.
        store.borrow_mut().upload_discard(shared);
        info!(
            "link: [ctl] transfer_control answer: op {} type {} id {} len {} -> status {}",
            desc.op.as_u8(),
            desc.ty.as_u8(),
            desc.object_id,
            desc.total_len,
            TransferStatus::Aborted.as_u8()
        );
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Aborted));
    }
    if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
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
        // A map upload (#889): the *type* is settled — the host and the device now agree that 16 is
        // `map` — but the device deliberately does **not** accept one yet, and answers a typed,
        // logged reject rather than falling through the catch-all below, so a host that tries gets a
        // diagnosable "no" instead of a bare `error` on an unknown byte.
        //
        // What is missing is storage, not transport. Three problems, none of them small, and all of
        // them on-glass:
        //
        //  1. **The device could not find what it wrote.** `sd::Storage::open_map` matches on the
        //     **long** filename (`*.obcm`), because the 8.3 short name truncates both `.obcm` and
        //     `.obcr` to `OBC` — and embedded-sdmmc 0.9 cannot *create* long filenames. A map the
        //     firmware wrote would be an invisible `SOMETHING.OBC`.
        //  2. **There is no map catalog.** The renderer streams from "the first `*.obcm` in the card
        //     root", chosen by directory order and held open for the whole session. An upload needs
        //     a naming/collision policy and a way to become the *selected* map — which is a UI
        //     question, not just a storage one.
        //  3. **Scale.** Hundreds of megabytes at the card's proven throughput is minutes of
        //     sustained writing, against an open map handle, a cached FAT extent list and a running
        //     watchdog. A free-space guard at announce is the least of it.
        //
        // Tracked separately (see the #889 PR); until then this arm is the honest answer.
        (Op::Upload, ObjectType::Map) => {
            warn!(
                "link: [ctl] map upload rejected: the type is agreed but device-side map storage is not implemented \
                 (id {} len {})",
                desc.object_id, desc.total_len
            );
            TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Error))
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
                ObjectType::Trip => store.borrow().has_trip(desc.object_id),
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

/// Log one accepted descriptor in the same numeric vocabulary as the iOS / browser console, so an
/// on-device trace correlates both sides of the exchange.
fn log_transfer_arm(desc: &TransferControl) {
    info!(
        "link: [ctl] transfer_control arm: op {} type {} id {} len {}",
        desc.op.as_u8(),
        desc.ty.as_u8(),
        desc.object_id,
        desc.total_len
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
