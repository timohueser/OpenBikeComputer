//! `obc-ble`'s **production** codecs against the shared `specs/vectors/` fixtures — the same
//! files the app's `swift test` pins and `obc-vectors` builds from the spec. `obc-vectors` proves
//! the *bytes* match spec-derived builders; this proves the shipped codecs decode and re-encode
//! those bytes exactly. A drift fails here, there, and on the Swift side.

use obc_ble::descriptor::{ObjectType, Op, StatusMessage, TransferStatus};
use obc_ble::{Config, StoreChanged, TransferControl, TransferResult, VersionRead};
use obc_ble::{Crc32, StatusMessage as Msg};

fn fixture(name: &str) -> Vec<u8> {
    let path = obc_vectors::dir().join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo test -p obc-vectors regenerate -- --ignored`")
    })
}

/// The production CRC agrees with `obc-vectors`' independent spec reference on a real object, and
/// both agree with the descriptor's announced CRC — one number tying the three implementations.
#[test]
fn production_crc_matches_reference_and_descriptor() {
    let route = fixture("route-waypoints.obcr");
    assert_eq!(Crc32::checksum(&route), obc_vectors::crc32(&route));

    let start = TransferControl::decode(&fixture("transfer-upload-start.bin")).unwrap();
    assert_eq!(start.crc32, Crc32::checksum(&route));
    assert_eq!(start.total_len as usize, route.len());
}

#[test]
fn transfer_control_vectors_round_trip() {
    for (name, op, ty, id) in [
        ("transfer-upload-start.bin", Op::Upload, ObjectType::Route, 0xFFFF),
        ("transfer-download-request.bin", Op::Download, ObjectType::RideList, 0),
        ("transfer-abort.bin", Op::Abort, ObjectType::Route, 0xFFFF),
    ] {
        let bytes = fixture(name);
        assert_eq!(bytes.len(), TransferControl::ENCODED_LEN, "{name} is a 12-byte v2 descriptor");
        let desc = TransferControl::decode(&bytes).unwrap();
        assert_eq!(desc.op, op, "{name} op");
        assert_eq!(desc.ty, ty, "{name} type");
        assert_eq!(desc.object_id, id, "{name} id");
        // Re-encoding the decoded descriptor reproduces the fixture byte-for-byte.
        assert_eq!(&desc.encode()[..], &bytes[..], "{name} re-encode");
    }
}

/// The `protocolVersion` read of a device **without** the weather contract (spec §1): `version u16 ·
/// store_epoch u32 · obcm_version u8`. The production codec decodes the fixture, sees protocol
/// version 2 and the OBCM version this build's reader actually reads, and re-encodes it
/// byte-for-byte. WX3 (#1188) appended a capability word after this; the fixture deliberately stays
/// seven bytes, because that is still what a device without weather serves —
/// `version-read-features.bin` is the widened read.
#[test]
fn version_read_vector() {
    let bytes = fixture("version-read.bin");
    assert_eq!(bytes.len(), VersionRead::ENCODED_LEN_NO_FEATURES);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION, "the fixture pins protocol version 2");
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4);
    assert_eq!(
        vr.obcm_version,
        Some(obc_formats::obcm::VERSION),
        "the fixture is what a current device serves — the map version its reader reads, not a literal"
    );
    assert_eq!(vr.feature_bits, None, "no capability word in a 7-byte read — absent, never Some(0)");
    assert!(!vr.has_weather(), "…so this device is not offered weather");
    let (enc, len) = vr.encode();
    assert_eq!(&enc[..len], &bytes[..], "re-encode");
}

/// The **pre-E1** read (spec §1): `version u16 · store_epoch u32`, no `obcm_version` — an older
/// firmware talking to a newer host. It decodes cleanly (the epoch is there, so the ack gate is
/// open) with `obcm_version = None`, and re-encodes to the same 6 bytes. `None`, not `Some(0)`: a
/// fabricated `0` would read as "this device supports OBCM v0" and refuse every real map, the same
/// class of mistake as fabricating store epoch `0`.
#[test]
fn version_read_noobcm_vector() {
    let bytes = fixture("version-read-noobcm.bin");
    assert_eq!(bytes.len(), VersionRead::ENCODED_LEN_NO_OBCM);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION);
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4, "the epoch is present — this read is not a failed one");
    assert_eq!(vr.obcm_version, None, "an absent trailing field is unknown, never a fabricated default");
    let (enc, len) = vr.encode();
    assert_eq!(&enc[..len], &bytes[..], "re-encode stays 6 bytes — the encoder does not invent the byte either");
}

/// The version-only `protocolVersion` read (spec §1, card-resident epoch #776): a device with no
/// mounted store serves just the 2-byte version. It carries the protocol version, but the full
/// [`VersionRead`] decode **rejects** it as truncated — exactly the app's "short read ⇒ storeEpoch
/// nil ⇒ ack fail-closed" gate. Unchanged by E1: the no-store read never grew a byte.
#[test]
fn version_read_nostore_vector() {
    let bytes = fixture("version-read-nostore.bin");
    assert_eq!(bytes.len(), 2, "version-only: just the u16 version, no epoch");
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), obc_ble::PROTOCOL_VERSION, "pins protocol version 2");
    assert!(VersionRead::decode(&bytes).is_err(), "a short read is not a full VersionRead — the app fail-closes");
}

/// The append-only rule this field rode in on (§1): a decoder takes the fields it knows and
/// **ignores** bytes past them, so a future firmware appending another trailing field does not
/// break this build — which is exactly why `obcm_version` needs no `PROTOCOL_VERSION` bump.
#[test]
fn version_read_ignores_unknown_trailing_bytes() {
    let mut bytes = fixture("version-read.bin");
    bytes.extend_from_slice(&[0xEE, 0xEE, 0xEE]);
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION);
    assert_eq!(vr.store_epoch, 0xA1B2_C3D4);
    assert_eq!(vr.obcm_version, Some(obc_formats::obcm::VERSION));
}

/// The download announce (status `msg = 4`): the `msg` byte + the 12-byte descriptor. The
/// production codec decodes it back through the shared `StatusMessage` envelope and the descriptor
/// carries the download's size + CRC (matching the waypoint route).
#[test]
fn download_announce_vector() {
    let route = fixture("route-waypoints.obcr");
    let bytes = fixture("status-download-announce.bin");
    assert_eq!(bytes.len(), StatusMessage::MAX_ENCODED_LEN, "13 bytes: msg + 12-byte descriptor");

    let StatusMessage::DownloadAnnounce(desc) = StatusMessage::decode(&bytes).unwrap().expect("known msg") else {
        panic!("expected downloadAnnounce")
    };
    assert_eq!(desc.op, Op::Download);
    assert_eq!(desc.ty, ObjectType::Route);
    assert_eq!(desc.object_id, 7);
    assert_eq!(desc.total_len as usize, route.len());
    assert_eq!(desc.crc32, Crc32::checksum(&route));

    // Re-encode reproduces the fixture byte-for-byte.
    let (buf, len) = Msg::DownloadAnnounce(desc).encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// A *known* discriminator with a short body is a decode **error** (`Truncated`), while an unknown
/// discriminator stays `Ok(None)` (ignored) — the forward-compat rule cuts exactly between the two.
/// Pinned for `msg = 4`: every truncation of the announce frame is rejected, never misread.
#[test]
fn truncated_download_announce_is_rejected() {
    use obc_ble::DescriptorError;

    let full = fixture("status-download-announce.bin");
    assert_eq!(full.len(), 13);
    for cut in 1..full.len() {
        assert_eq!(
            StatusMessage::decode(&full[..cut]),
            Err(DescriptorError::Truncated),
            "a {cut}-byte prefix of the announce must be Truncated, not ignored or misdecoded"
        );
    }
}

#[test]
fn status_transfer_result_vector() {
    let bytes = fixture("status-transfer-result.bin");
    let route_len = fixture("route-waypoints.obcr").len() as u32;

    let msg = StatusMessage::decode(&bytes).unwrap().expect("known discriminator");
    let StatusMessage::TransferResult(result) = msg else { panic!("expected transferResult") };
    assert_eq!(result.object_id, 7, "assigned id");
    assert_eq!(result.status, TransferStatus::Committed);
    assert_eq!(result.committed_offset, route_len);

    // Re-encode reproduces the fixture.
    let rebuilt = Msg::TransferResult(TransferResult::new(7, TransferStatus::Committed, route_len));
    let (buf, len) = rebuilt.encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// The storage-full reject fixture: a new-route upload (id `0xFFFF`) refused at descriptor-open
/// time because the catalog is full. `status = 6` (`StorageFull`), nothing committed. Pins the
/// discriminant byte so the Swift half decodes the same value.
#[test]
fn status_transfer_storage_full_vector() {
    let bytes = fixture("status-transfer-storage-full.bin");

    // The status byte lives at offset 3 of the transferResult envelope.
    assert_eq!(bytes[3], 6, "storageFull discriminant is 6");

    let msg = StatusMessage::decode(&bytes).unwrap().expect("known discriminator");
    let StatusMessage::TransferResult(result) = msg else { panic!("expected transferResult") };
    assert_eq!(result.object_id, 0xFFFF, "the rejected new-id request");
    assert_eq!(result.status, TransferStatus::StorageFull);
    assert_eq!(result.committed_offset, 0, "nothing committed");

    // Re-encode reproduces the fixture byte-for-byte.
    let rebuilt = Msg::TransferResult(TransferResult::new(0xFFFF, TransferStatus::StorageFull, 0));
    let (buf, len) = rebuilt.encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

#[test]
fn status_store_changed_vector() {
    let bytes = fixture("status-store-changed.bin");
    let StatusMessage::StoreChanged(s) = StatusMessage::decode(&bytes).unwrap().unwrap() else {
        panic!("expected storeChanged")
    };
    assert_eq!(s.ty, ObjectType::Route);
    assert_eq!(s.revision, 42);

    let (buf, len) = Msg::StoreChanged(StoreChanged { ty: ObjectType::Route, revision: 42 }).encode();
    assert_eq!(&buf[..len], &bytes[..]);
}

/// The `ackRides` command fixture (spec §4.4, cmd 2) decodes through the production codec, its
/// answer fixture decodes as the documented `commandResult` (detail = newly-flagged count), and
/// re-encoding both reproduces the files byte-for-byte — the Swift side pins the same bytes.
#[test]
fn command_ack_rides_vector() {
    use obc_ble::{AckRides, CommandResult, CommandStatus, CMD_ACK_RIDES};

    let bytes = fixture("command-ack-rides.bin");
    let ack = AckRides::decode(&bytes).expect("valid ackRides");
    assert_eq!(ack.count(), 3);
    assert_eq!(ack.iter().collect::<Vec<_>>(), [3, 5, 9]);

    let mut out = [0u8; AckRides::encoded_len(3)];
    let len = AckRides::encode(&[3, 5, 9], &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");

    // The answer: commandResult{cmd 2, ok, detail 3} — detail is the newly-flagged count.
    let result_bytes = fixture("status-command-result-ack.bin");
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&result_bytes).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_ACK_RIDES, CommandStatus::Ok, 3));
    let (buf, len) = Msg::CommandResult(CommandResult::with_detail(CMD_ACK_RIDES, CommandStatus::Ok, 3)).encode();
    assert_eq!(&buf[..len], &result_bytes[..]);
}

/// `forgetBond` (§4.4 cmd 4, #756): the command is a bare id (no args), and its answer is a plain
/// `commandResult{cmd 4, ok}`. Pins the command byte and the round-trip through the production
/// `commandResult` codec — the Swift side writes `Data([4])` and reads the same envelope back.
#[test]
fn command_forget_bond_round_trip() {
    use obc_ble::{CommandResult, CommandStatus, CMD_FORGET_BOND};

    assert_eq!(CMD_FORGET_BOND, 4, "the wire command id is pinned by the spec (§4.4)");

    // The answer the firmware sends before it clears the bond + drops the link: commandResult(ok),
    // no detail. Round-trips byte-for-byte through the shared codec.
    let (buf, len) = Msg::CommandResult(CommandResult::new(CMD_FORGET_BOND, CommandStatus::Ok)).encode();
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_FORGET_BOND, CommandStatus::Ok, 0));
}

/// `setClock` (§4.4 cmd 5, epic #638 S2): the shared fixture decodes as the documented
/// `(utc, offset_min)` and re-encodes to the same 7 bytes — the Swift side pins the same file. Like
/// `forgetBond`, its answer is a bare `commandResult(ok)` with no store movement (the clock is not an
/// object — no `storeChanged`, no revision bump).
#[test]
fn command_set_clock_vector() {
    use obc_ble::{CommandResult, CommandStatus, SetClock, CMD_SET_CLOCK};

    assert_eq!(CMD_SET_CLOCK, 5, "the wire command id is pinned by the spec (§4.4, next-free after forgetBond)");

    let bytes = fixture("command-set-clock.bin");
    assert_eq!(bytes.len(), SetClock::ENCODED_LEN, "setClock is a fixed 7-byte write");
    let sc = SetClock::decode(&bytes).expect("valid setClock");
    assert_eq!(sc.utc, 1_783_598_400, "2026-07-09T12:00:00Z");
    assert_eq!(sc.offset_min, 120, "+02:00");

    let mut out = [0u8; SetClock::ENCODED_LEN];
    let len = SetClock::encode(sc.utc, sc.offset_min, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");

    // The device's answer (no `detail`, no companion `storeChanged`): commandResult{cmd 5, ok}.
    let (buf, len) = Msg::CommandResult(CommandResult::new(CMD_SET_CLOCK, CommandStatus::Ok)).encode();
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_SET_CLOCK, CommandStatus::Ok, 0));
}

/// `setClock` decode rejects every write a bad phone clock (or a wrong-length frame) would produce —
/// each maps to `commandResult error` (§4.4). The plausibility gates live in the shared codec so the
/// firmware and the iOS mirror agree on "valid".
#[test]
fn set_clock_decode_edges() {
    use obc_ble::{SetClock, SET_CLOCK_MAX_OFFSET_MIN, SET_CLOCK_MIN_UTC};

    let valid = |utc: u32, off: i16| {
        let mut b = [0u8; 7];
        SetClock::encode(utc, off, &mut b).unwrap();
        b
    };

    // Short and long writes are both malformed — setClock has no variable tail, so exactly 7 bytes.
    assert!(SetClock::decode(&[5, 0, 0, 0, 0, 0]).is_err(), "6 bytes: short");
    assert!(SetClock::decode(&[5, 0, 0, 0, 0, 0, 0, 0]).is_err(), "8 bytes: trailing is malformed");
    // A wrong command byte is refused.
    assert!(SetClock::decode(&valid_cmd(4, SET_CLOCK_MIN_UTC, 0)).is_err(), "cmd 4 is not setClock");
    // A pre-2020 UTC is a bogus phone clock.
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC - 1, 0)).is_err(), "utc before 2020-01-01");
    assert!(SetClock::decode(&valid(0, 0)).is_err(), "utc = 0");
    // Offsets beyond ±14 h are rejected; the exact bounds (−12:00…+14:00 both hit ±840 here) pass.
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, SET_CLOCK_MAX_OFFSET_MIN + 1)).is_err(), "offset > +840");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, -SET_CLOCK_MAX_OFFSET_MIN - 1)).is_err(), "offset < −840");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, SET_CLOCK_MAX_OFFSET_MIN)).is_ok(), "+840 is in range");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, -SET_CLOCK_MAX_OFFSET_MIN)).is_ok(), "−840 is in range");
    assert!(SetClock::decode(&valid(SET_CLOCK_MIN_UTC, 0)).is_ok(), "the 2020 epoch itself is accepted");
}

/// Build a 7-byte setClock frame with an arbitrary leading command byte (for the wrong-command edge).
fn valid_cmd(cmd: u8, utc: u32, offset_min: i16) -> [u8; 7] {
    let mut b = [0u8; 7];
    b[0] = cmd;
    b[1..5].copy_from_slice(&utc.to_le_bytes());
    b[5..7].copy_from_slice(&offset_min.to_le_bytes());
    b
}

/// `setRouteRetention` (§4.4 cmd 6, epic #638 S4): the shared fixture decodes as `(object_id 7,
/// retention 3)` and re-encodes to the same 4 bytes — the Swift side pins the same file in S6. Its
/// answer is a bare `commandResult(ok)` (with a companion `storeChanged(route)` on a real change).
#[test]
fn command_set_route_retention_vector() {
    use obc_ble::{CommandResult, CommandStatus, ObjectType, SetRouteRetention, StoreChanged, CMD_SET_ROUTE_RETENTION};

    assert_eq!(
        CMD_SET_ROUTE_RETENTION, 6,
        "the wire command id is pinned by the spec (§4.4, next-free after setClock)"
    );

    let bytes = fixture("command-set-route-retention.bin");
    assert_eq!(bytes.len(), SetRouteRetention::ENCODED_LEN, "setRouteRetention is a fixed 4-byte write");
    let srr = SetRouteRetention::decode(&bytes).expect("valid setRouteRetention");
    assert_eq!(srr.object_id, 7, "route id 7 — the waypoint route in route-list.bin");
    assert_eq!(srr.retention, 3, "retention = 2 weeks");

    let mut out = [0u8; SetRouteRetention::ENCODED_LEN];
    let len = SetRouteRetention::encode(srr.object_id, srr.retention, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");

    // The device's answer: commandResult{cmd 6, ok}; a real change also notifies storeChanged(route).
    let (buf, len) = Msg::CommandResult(CommandResult::new(CMD_SET_ROUTE_RETENTION, CommandStatus::Ok)).encode();
    let StatusMessage::CommandResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected commandResult")
    };
    assert_eq!((r.command, r.status, r.detail), (CMD_SET_ROUTE_RETENTION, CommandStatus::Ok, 0));
    // The companion storeChanged names the *route* store (§4.3 msg 2), like a route delete.
    let (buf, len) = Msg::StoreChanged(StoreChanged { ty: ObjectType::Route, revision: 43 }).encode();
    let StatusMessage::StoreChanged(sc) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
        panic!("expected storeChanged")
    };
    assert_eq!(sc.ty, ObjectType::Route);
}

/// `setRouteRetention` decode rejects every malformed / out-of-range write — each maps to
/// `commandResult error` (§4.4). The range check lives in the shared codec so the firmware and the
/// iOS mirror agree on "valid".
#[test]
fn set_route_retention_decode_edges() {
    use obc_ble::{SetRouteRetention, SET_ROUTE_RETENTION_MAX};

    // Exactly 4 bytes — no variable tail, so a short or long write is malformed.
    assert!(SetRouteRetention::decode(&[6, 7, 0]).is_err(), "3 bytes: short");
    assert!(SetRouteRetention::decode(&[6, 7, 0, 3, 0]).is_err(), "5 bytes: trailing is malformed");
    // A wrong command byte is refused.
    assert!(SetRouteRetention::decode(&[5, 7, 0, 3]).is_err(), "cmd 5 is not setRouteRetention");
    // Every in-range retention decodes; one above the max is rejected.
    for r in 0..=SET_ROUTE_RETENTION_MAX {
        let d = SetRouteRetention::decode(&[6, 7, 0, r]).expect("in-range retention");
        assert_eq!((d.object_id, d.retention), (7, r));
    }
    assert!(
        SetRouteRetention::decode(&[6, 7, 0, SET_ROUTE_RETENTION_MAX + 1]).is_err(),
        "retention > 5 is out of range"
    );
    assert!(SetRouteRetention::decode(&[6, 7, 0, 0xFF]).is_err(), "0xFF is out of range");
}

/// `ackRides` decode edges: a `count` promising more ids than the write carries is truncated; a
/// wrong command byte is refused; an empty ack and ignored trailing bytes are both fine.
#[test]
fn ack_rides_decode_edges() {
    use obc_ble::{AckRides, DescriptorError};

    assert!(matches!(AckRides::decode(&[2, 3, 1, 0]), Err(DescriptorError::Truncated)), "short of its count");
    assert!(matches!(AckRides::decode(&[2]), Err(DescriptorError::Truncated)), "no count byte");
    assert!(matches!(AckRides::decode(&[1, 0]), Err(DescriptorError::UnknownOp(1))), "not ackRides");
    assert_eq!(AckRides::decode(&[2, 0]).unwrap().count(), 0, "empty ack is well-formed");
    let ack = AckRides::decode(&[2, 1, 7, 0, 0xEE]).unwrap();
    assert_eq!(ack.iter().collect::<Vec<_>>(), [7], "trailing bytes past count are ignored");
}

#[test]
fn config_vector() {
    let bytes = fixture("config-v1.bin");
    let config = Config::decode(&bytes).expect("valid config");
    assert_eq!(config.name, b"OBC Tourer");
    assert_eq!(config.units, 0);
    assert_eq!(config.weather_refresh, None, "a v1 blob says nothing about refresh — device default");

    let mut out = [0u8; Config::MAX_ENCODED];
    let len = Config::encode(&config, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..]);
}

/// The Config blob **with** the WX3 (#1188) refresh byte. The production codec reads it at the
/// offset after `units` and re-encodes the file byte-for-byte; the v1 fixture beside it is the same
/// object without that byte, and decodes to `None`.
///
/// The pair is the test. An implementation that got the offset wrong — or that filled an absent
/// field with a default — passes one of these two files and fails the other, and the failure mode
/// it is guarding against is a rename silently switching a rider's weather off.
#[test]
fn config_weather_refresh_vector() {
    use obc_ble::weather_request::WeatherRefresh;

    let bytes = fixture("config-weather-refresh.bin");
    let config = Config::decode(&bytes).expect("valid config");
    assert_eq!(config.name, b"OBC Alpine");
    assert_eq!(config.units, 1, "imperial — the refresh byte follows a *nonzero* units byte");
    assert_eq!(config.known_refresh(), Some(WeatherRefresh::Every60));
    assert_eq!(config.weather_refresh, Some(WeatherRefresh::Every60.as_u8()), "the raw byte");
    assert_eq!(bytes.len(), 2 + config.name.len() + 2, "name_len · name · units · weather_refresh");

    let mut out = [0u8; Config::MAX_ENCODED];
    let len = Config::encode(&config, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");

    // The same value with the field dropped is exactly the v1 blob's shape — one byte shorter, and
    // read back as "unspecified" rather than as `Off`.
    let dropped = Config { weather_refresh: None, ..config };
    let dropped_len = Config::encode(&dropped, &mut out).unwrap();
    assert_eq!(dropped_len, len - 1);
    assert_eq!(Config::decode(&out[..dropped_len]).unwrap().weather_refresh, None);
}

/// The **widened** `protocolVersion` read (spec §1, WX3 #1188): the seven bytes of
/// `version-read.bin`'s layout plus `feature_bits u32`. This is the only read that entitles a phone
/// to look for the Weather Request service, so the fixture pins both that the bit decodes and that
/// the protocol version underneath it did **not** move — the capability rode in as an append.
#[test]
fn version_read_features_vector() {
    use obc_ble::descriptor::FEATURE_WEATHER;

    let bytes = fixture("version-read-features.bin");
    assert_eq!(bytes.len(), VersionRead::ENCODED_LEN, "11 bytes: the full read");
    let vr = VersionRead::decode(&bytes).unwrap();
    assert_eq!(vr.version, obc_ble::PROTOCOL_VERSION, "a new capability is not a new protocol");
    assert_eq!(vr.store_epoch, 0xC0DE_F00D);
    assert_eq!(vr.obcm_version, Some(obc_formats::obcm::VERSION), "self-sourced, like the shorter reads");
    assert_eq!(vr.feature_bits, Some(FEATURE_WEATHER));
    assert!(vr.has_weather());

    let (enc, len) = vr.encode();
    assert_eq!(&enc[..len], &bytes[..], "re-encode");

    // A truncated read of this very file must not claim the feature: three bytes of a u32 are a
    // broken read, not a smaller capability set.
    for cut in VersionRead::ENCODED_LEN_NO_FEATURES..VersionRead::ENCODED_LEN {
        assert!(!VersionRead::decode(&bytes[..cut]).unwrap().has_weather(), "{cut} bytes must claim nothing");
    }
}

/// The three `weatherRequestContext` fixtures (spec §11, WX3 #1188) through the **production**
/// codec: each decodes to the documented value and re-encodes to the file byte-for-byte.
///
/// `obc-vectors` builds these from the spec's offset table by hand and `tests/weather_request.rs`
/// pins the codec's behaviour; this is the join between the two, and the only place that can catch
/// the hand-built table and the shipped encoder having drifted apart in the same direction.
#[test]
fn weather_request_context_vectors_round_trip() {
    use obc_ble::weather_request::{
        WeatherRefresh, WeatherRequestContext, REASON_NO_BUNDLE, REASON_SCHEDULED, REASON_URGENT, VALID_BEARING,
        VALID_BUNDLE, VALID_POSITION, VALID_ROUTE, VALID_SPEED, WEATHER_REQUEST_CONTEXT_VERSION,
    };

    for name in [
        "weather-request-context-full.bin",
        "weather-request-context-empty.bin",
        "weather-request-context-no-fix.bin",
        // Both #1214 additions belong in this loop too: whatever else they pin, they are ordinary
        // v1 values, and a codec that decoded them into something it could not re-encode verbatim
        // would fail here before any of their own assertions ran.
        "weather-request-context-unknown-refresh.bin",
        "weather-request-context-southern.bin",
    ] {
        let bytes = fixture(name);
        assert_eq!(bytes.len(), WeatherRequestContext::ENCODED_LEN, "{name} is a 52-byte v1 value");
        let ctx = WeatherRequestContext::decode(&bytes).unwrap_or_else(|e| panic!("{name} rejected: {e:?}"));
        assert_eq!(ctx.version, WEATHER_REQUEST_CONTEXT_VERSION, "{name} version");
        assert_eq!(&ctx.encode()[..], &bytes[..], "{name} re-encode");
    }

    // The full request: a rider on route 7 near Freiburg, holding the DWD-shaped bundle fixture and
    // due for its scheduled successor. The bundle identity is checked against the file it names —
    // the same whole-object CRC-32 an upload of it would have announced.
    let full = WeatherRequestContext::decode(&fixture("weather-request-context-full.bin")).unwrap();
    assert_eq!(full.validity, VALID_POSITION | VALID_BEARING | VALID_SPEED | VALID_BUNDLE | VALID_ROUTE);
    assert!(full.because(REASON_SCHEDULED));
    assert_eq!(full.refresh(), Some(WeatherRefresh::Every30));
    assert_eq!(full.request_id, 0x1188_0001);
    assert_eq!((full.lat_udeg, full.lon_udeg), (47_999_008, 7_842_104), "Freiburg, in OBCW microdegrees");
    assert_eq!((full.bearing_deg, full.speed_deci_ms), (342, 71));
    assert_eq!(full.route_id, 7, "the waypoint route route-list.bin catalogs");
    let held = fixture("weather-dwd-96x96-9f.obcw");
    assert_eq!(full.bundle_crc32, Crc32::checksum(&held), "the bundle group names a bundle that exists");
    assert_eq!(full.fix_utc, full.bundle_generated_at + 30 * 60, "one refresh interval past what it holds");

    // The resting value: structurally valid, claiming nothing, and still stating the default
    // interval — which is what keeps an out-of-turn read from looking like a device with weather off.
    let empty = WeatherRequestContext::decode(&fixture("weather-request-context-empty.bin")).unwrap();
    assert_eq!(empty, WeatherRequestContext::EMPTY);
    assert!(!empty.has(VALID_POSITION) && !empty.has(VALID_BUNDLE));
    assert_eq!(empty.refresh(), Some(WeatherRefresh::DEFAULT));

    // The urgent request with nothing behind it — and a *scheduled* refresh of Off, which must not
    // read as a device that never asks.
    let no_fix = WeatherRequestContext::decode(&fixture("weather-request-context-no-fix.bin")).unwrap();
    assert_eq!(no_fix.validity, 0, "absence is a cleared flag, not a sentinel coordinate");
    assert!(no_fix.because(REASON_URGENT) && no_fix.because(REASON_NO_BUNDLE));
    assert_eq!(no_fix.refresh(), Some(WeatherRefresh::Off));
    assert_eq!(no_fix.request_id, 0x1188_0002, "a different request from the full one");
    assert_ne!(no_fix.request_id, full.request_id);
}

/// The context read whose `refresh` names an interval this build does not know (#1214, §11.8) —
/// the **read** direction, where an unrecognised value is never fatal.
///
/// Three things have to hold at once, and the production codec is the only place they can be checked
/// together: the read decodes, `refresh()` reports *unknown* rather than guessing, and the raw byte
/// survives a re-encode **verbatim**. That last one is what makes the tolerance honest rather than
/// merely quiet — a codec that normalised the byte to the default on the way back out would agree
/// with every other assertion here and still misreport the rider's setting the moment the value was
/// forwarded anywhere.
///
/// The failure this guards against is the one an adversarial review of #1214 found: a direction-blind
/// reject. Under it, appending a fifth interval — an ordinary enum append — would have taken weather
/// permanently dead on every shipped app the day the firmware shipped it.
#[test]
fn an_unknown_refresh_byte_is_tolerated_on_a_context_read() {
    use obc_ble::weather_request::{WeatherRefresh, WeatherRequestContext, REASON_SCHEDULED, VALID_BUNDLE};

    let bytes = fixture("weather-request-context-unknown-refresh.bin");
    let ctx = WeatherRequestContext::decode(&bytes).expect("an unknown interval is not a malformed read");

    assert_eq!(ctx.refresh_raw, 9, "carried as it arrived");
    assert!(WeatherRefresh::from_u8(ctx.refresh_raw).is_err(), "…and this build genuinely cannot name it");
    assert_eq!(ctx.refresh(), None, "unknown — not Off, not the default, which would misreport the rider");
    assert_ne!(ctx.refresh(), Some(WeatherRefresh::Off));
    assert_ne!(ctx.refresh(), Some(WeatherRefresh::DEFAULT));
    assert_eq!(&ctx.encode()[..], &bytes[..], "the raw byte round-trips verbatim");

    // The rest of the request is untouched by the byte the phone could not name: this is still a
    // rider on route 7 holding a real bundle and due for its successor. An implementation that
    // treated the value as malformed would have thrown all of that away.
    let full = WeatherRequestContext::decode(&fixture("weather-request-context-full.bin")).unwrap();
    assert!(ctx.because(REASON_SCHEDULED) && ctx.has(VALID_BUNDLE));
    assert_eq!(
        ctx,
        WeatherRequestContext { refresh_raw: ctx.refresh_raw, request_id: ctx.request_id, ..full },
        "identical to the full context but for the refresh byte and the nonce"
    );
    assert_eq!(full.refresh(), Some(WeatherRefresh::Every30), "…and the sibling still names its interval");
}

/// The southern context (#1214): the signed fields, through the production codec.
///
/// `obc-vectors` checks the bytes; this checks that `decode` sign-extends them. Every value below is
/// one a wrong-signedness read gets visibly wrong — a latitude of 4245°, a clock 585 billion years
/// ahead — and the two `u32`s at the end run the trap the other way, so a codec that "fixed" the
/// signedness by flipping every field fails just as loudly.
#[test]
fn the_southern_context_decodes_its_signed_fields() {
    use obc_ble::weather_request::{
        WeatherRefresh, WeatherRequestContext, VALID_BEARING, VALID_BUNDLE, VALID_POSITION, VALID_ROUTE, VALID_SPEED,
    };

    let bytes = fixture("weather-request-context-southern.bin");
    let ctx = WeatherRequestContext::decode(&bytes).unwrap();

    assert_eq!((ctx.lat_udeg, ctx.lon_udeg), (-49_330_889, -72_886_121), "Patagonia — south and west");
    assert!(ctx.lat_udeg < 0 && ctx.lon_udeg < 0, "read unsigned these are ≈ 4245°N / 4222°E");
    assert_eq!(ctx.fix_utc, -1_000_000_000, "a pre-1970 fix");
    assert_eq!(ctx.bundle_generated_at, -1_000_003_600, "…and an older bundle, at its own offset");
    assert!(ctx.fix_utc < 0 && ctx.bundle_generated_at < 0);
    assert_eq!(ctx.fix_utc - ctx.bundle_generated_at, 60 * 60, "one 60-minute interval, on the far side of zero");
    assert_eq!(ctx.refresh(), Some(WeatherRefresh::Every60), "…which is the interval it says it is on");

    // The unsigned pair: a codec reading these as i32 gets -2 and -2147483647.
    assert_eq!(ctx.bundle_generation, 0xFFFF_FFFE);
    assert_eq!(ctx.bundle_crc32, 0x8000_0001);

    assert!(ctx.has(VALID_POSITION) && ctx.has(VALID_BEARING) && ctx.has(VALID_SPEED) && ctx.has(VALID_BUNDLE));
    assert!(!ctx.has(VALID_ROUTE), "the one cleared group — no route is active");
    assert_eq!(&ctx.encode()[..], &bytes[..], "re-encode");
}

/// Config's copy of the same byte (#1214, §11.8) — and the one place the rule is **not** tolerant.
///
/// One blob, read twice, two answers. `known_refresh()` is the read direction: `None`, meaning
/// *unknown*, indistinguishable in type from an absent field because neither is something this build
/// can show a rider — and, critically, not an error, or a phone facing a newer device could no longer
/// read Config even to rename it. `refresh_to_apply()` is the write direction: a device asked to
/// adopt an interval it cannot honour refuses the write whole, because storing the default, `Off`, or
/// the previous value would all tell the rider their choice was applied when it was discarded.
#[test]
fn an_unknown_config_refresh_is_read_tolerantly_and_written_strictly() {
    use obc_ble::descriptor::DescriptorError;
    use obc_ble::weather_request::WeatherRefresh;

    let bytes = fixture("config-weather-refresh-unknown.bin");
    let config = Config::decode(&bytes).expect("decode is direction-blind — the blob itself is well-formed");
    assert_eq!(config.name, b"OBC Horizon");
    assert_eq!(config.units, 0, "metric — so an off-by-one reader decodes a *known* `Off` and is caught");
    assert_eq!(config.weather_refresh, Some(200), "the raw byte, carried as it arrived");

    // Read: unknown, never a substitute.
    assert_eq!(config.known_refresh(), None, "unknown — not Off, not the default");
    // Write: refused, and the refusal names the byte so a log says which interval was asked for.
    assert!(
        matches!(config.refresh_to_apply(), Err(DescriptorError::UnknownRefresh(200))),
        "a device cannot store an interval it cannot honour"
    );

    // `known_refresh()` collapses unknown and absent, but the *raw* field does not — and the write
    // direction depends on that difference: absent means "leave the stored value alone" (`Ok(None)`),
    // unknown means "refuse". A codec that lost the distinction would reset a rider who had chosen
    // `Off` back to 30-minute wakeups on the next rename.
    let absent = Config { weather_refresh: None, ..config };
    assert_eq!(absent.known_refresh(), None, "same answer to a reader…");
    assert!(matches!(absent.refresh_to_apply(), Ok(None)), "…and a different one to a writer");
    assert_ne!(absent.weather_refresh, config.weather_refresh);

    // The byte survives a re-encode verbatim, which is what keeps the tolerance honest: a host that
    // read this Config and wrote it back must not silently swap the interval for one it happens to
    // know. And a *known* value still decodes as itself — tolerance is not blanket indifference.
    let mut out = [0u8; Config::MAX_ENCODED];
    let len = Config::encode(&config, &mut out).unwrap();
    assert_eq!(&out[..len], &bytes[..], "re-encode");
    let known_bytes = fixture("config-weather-refresh.bin");
    let known = Config::decode(&known_bytes).unwrap();
    assert_eq!(known.known_refresh(), Some(WeatherRefresh::Every60));
    assert_eq!(known.refresh_to_apply().unwrap(), Some(WeatherRefresh::Every60), "…and it applies as a write");
}

/// The weather bundle's upload identity (§4.1 / §11): type `20`, singleton object id `0`. Pinned
/// through the descriptor codec because the type byte and the id rule are what a companion has to
/// get right before a single byte of a bundle moves — a wrong id is answered `notFound`.
#[test]
fn weather_bundle_upload_descriptor_round_trips() {
    use obc_ble::weather_request::WEATHER_BUNDLE_OBJECT_ID;

    let bundle = fixture("weather-dwd-96x96-9f.obcw");
    let desc = TransferControl {
        op: Op::Upload,
        ty: ObjectType::WeatherBundle,
        object_id: WEATHER_BUNDLE_OBJECT_ID,
        total_len: bundle.len() as u32,
        crc32: Crc32::checksum(&bundle),
    };
    assert_eq!(desc.encode()[1], 20, "the type byte on the wire");
    assert_eq!(desc.object_id, 0, "there is one weather bundle, so the id selects nothing");
    assert_eq!(TransferControl::decode(&desc.encode()).unwrap(), desc);
    assert!(!ObjectType::WeatherBundle.is_map_payload(), "it stages through UPLOAD.TMP like a route");
}

/// Every `TransferStatus` variant round-trips through `as_u8`/`from_u8` and survives a full
/// `transferResult` encode→decode. Pins the discriminant values (`StorageFull == 6`) so a rename
/// or reorder can't silently shift the wire byte.
#[test]
fn transfer_status_round_trips_all_variants() {
    use TransferStatus::*;
    for (status, code) in
        [(Committed, 0u8), (CrcMismatch, 1), (Aborted, 2), (Error, 3), (NotFound, 4), (Busy, 5), (StorageFull, 6)]
    {
        assert_eq!(status.as_u8(), code, "{status:?} discriminant");
        assert_eq!(TransferStatus::from_u8(code).unwrap(), status, "{status:?} from_u8");

        let (buf, len) = Msg::TransferResult(TransferResult::new(0xFFFF, status, 0)).encode();
        let StatusMessage::TransferResult(r) = StatusMessage::decode(&buf[..len]).unwrap().unwrap() else {
            panic!("expected transferResult")
        };
        assert_eq!(r.status, status, "{status:?} survives encode→decode");
    }
}

/// An unknown `status` discriminator inside a well-formed `transferResult` envelope is an error
/// (the discriminator IS known — only the status byte is out of range). Forward-compat for the
/// status field lives on the *decoding* side (the app treats a decode failure / unknown status as
/// a generic error); the codec itself rejects a byte it can't name.
#[test]
fn unknown_transfer_status_byte_is_rejected() {
    let (mut buf, len) = Msg::TransferResult(TransferResult::new(0xFFFF, TransferStatus::Committed, 0)).encode();
    buf[3] = 0x7F; // a status code past the highest defined variant
    assert!(StatusMessage::decode(&buf[..len]).is_err());
}

/// The descriptor-open reject rule (issue #452), as a truth table. This is the exact classifier the
/// board crate's `ObjectStore::upload_open` calls; the board crate can't host-test (bare-metal, no
/// `test` crate), so the rule is pinned here.
#[test]
fn upload_open_reject_rule() {
    use obc_ble::TransferControl;
    let new = TransferControl::NEW_OBJECT_ID; // 0xFFFF
    let known = 7u16; // a route the device holds
    let unknown = 42u16; // a named id the device does NOT hold

    // Not full: new + replace both proceed; a named-but-unknown id is a genuine client error.
    assert_eq!(TransferStatus::upload_open_reject(new, false, false), None, "new, room → arm");
    assert_eq!(TransferStatus::upload_open_reject(known, true, false), None, "replace, room → arm");
    assert_eq!(
        TransferStatus::upload_open_reject(unknown, false, false),
        Some(TransferStatus::NotFound),
        "named-but-unknown id, room → notFound"
    );

    // Full: a new upload is rejected up front; a replace-by-id of an existing route is EXEMPT.
    assert_eq!(
        TransferStatus::upload_open_reject(new, false, true),
        Some(TransferStatus::StorageFull),
        "new + full → storageFull"
    );
    assert_eq!(TransferStatus::upload_open_reject(known, true, true), None, "replace at the cap still commits");
    // At the cap, even a named-but-unknown id reads as storage-full (it would grow the catalog).
    assert_eq!(
        TransferStatus::upload_open_reject(unknown, false, true),
        Some(TransferStatus::StorageFull),
        "unknown id + full → storageFull"
    );

    // The rule is object-type-agnostic (epic #526 TR4): the board's `upload_open_trip` passes the
    // *trip* catalog's `catalog_full`/`id_known` into the exact same classifier — a new trip past the
    // 16-trip cap → storageFull before any byte streams, a replace-by-id of a stored trip is exempt.
    assert_eq!(
        TransferStatus::upload_open_reject(new, false, true),
        Some(TransferStatus::StorageFull),
        "new trip + full trip catalog → storageFull"
    );
    assert_eq!(TransferStatus::upload_open_reject(known, true, true), None, "replace trip at the cap still commits");
}

/// The **map** announce-time reject rule (issue #927), as a truth table — the exact classifier the
/// board's `ObjectStore::map_upload_open` calls before a single byte of a several-hundred-megabyte
/// transfer moves. Pinned here for the same reason as `upload_open_reject_rule`: the board crate's
/// own tests never run in CI.
#[test]
fn map_announce_reject_rule() {
    use obc_ble::TransferControl;
    const HEADER: u32 = 40; // obc_formats::obcm::HEADER_LEN — the board passes it in
    const HEADROOM: u64 = 8 << 20;
    let new = TransferControl::NEW_OBJECT_ID;
    let map = |id, len, free| TransferStatus::map_announce_reject(id, len, HEADER, free, HEADROOM);

    // The happy path: a new map that fits, on a card whose free count we can read.
    assert_eq!(map(new, 300_000_000, Some(600 << 20)), None, "new map with room → arm");

    // New-only. Every named id is refused — the device never rewrites a stored map in place, so
    // there is no id an upload may target. (Contrast `upload_open_reject`, where a known id is the
    // *exempt* case.)
    assert_eq!(map(0, 1_000, Some(u64::MAX)), Some(TransferStatus::NotFound), "id 0 → notFound");
    assert_eq!(map(7, 1_000, Some(u64::MAX)), Some(TransferStatus::NotFound), "a named id → notFound");
    assert_eq!(map(0xFF00, 1_000, Some(u64::MAX)), Some(TransferStatus::NotFound), "even a session-band id → notFound");

    // Too short to be an OBCM at all — rejected before the free-space arithmetic.
    assert_eq!(map(new, 0, Some(u64::MAX)), Some(TransferStatus::Error), "an empty map → error");
    assert_eq!(map(new, HEADER - 1, Some(u64::MAX)), Some(TransferStatus::Error), "shorter than a header → error");
    assert_eq!(map(new, HEADER, Some(u64::MAX)), None, "exactly a header is structurally acceptable");

    // The free-space guard, including the reserve that keeps a map from taking the last cluster.
    let len = 100u32 << 20;
    assert_eq!(map(new, len, Some(len as u64 + HEADROOM)), None, "exactly len + headroom free → arm");
    assert_eq!(
        map(new, len, Some(len as u64 + HEADROOM - 1)),
        Some(TransferStatus::StorageFull),
        "one byte short of the reserve → storageFull"
    );
    assert_eq!(map(new, len, Some(len as u64)), Some(TransferStatus::StorageFull), "fits but eats the reserve");
    assert_eq!(map(new, len, Some(0)), Some(TransferStatus::StorageFull), "a full card → storageFull");

    // An unmeasurable free count must not become a blanket refusal.
    assert_eq!(map(new, u32::MAX, None), None, "unknown free space → arm (fail late, not never)");
}

/// The held-back magic of a direct-to-final streamed upload (issue #927): the first four payload
/// bytes are withheld from the write and replayed at commit, whatever the host's segmentation.
#[test]
fn held_magic_withholds_the_first_four_bytes() {
    use obc_ble::{HeldMagic, MAGIC_LEN};

    // One fat chunk: the magic is held, the rest is written.
    let mut h = HeldMagic::new();
    assert_eq!(h.feed(b"OBCM\x0a\x01\x02\x03"), b"\x0a\x01\x02\x03", "the tail of the first chunk is written");
    assert_eq!(h.take(), Some(*b"OBCM"));
    assert_eq!(h.feed(b"more"), b"more", "later chunks pass straight through");

    // Byte-at-a-time: the same magic, nothing written until it is complete.
    let mut h = HeldMagic::new();
    for (i, byte) in b"OBCM".iter().enumerate() {
        assert_eq!(h.feed(&[*byte]), b"", "byte {i} is held, not written");
        assert_eq!(h.take().is_some(), i + 1 == MAGIC_LEN, "complete only on the {MAGIC_LEN}th byte");
    }
    assert_eq!(h.take(), Some(*b"OBCM"));
    assert_eq!(h.feed(b"body"), b"body");

    // A split across an awkward boundary, and the total written length is always `len - MAGIC_LEN`.
    let payload = b"OBCMxxxxxxxxxxxxxxxxxxxx";
    for split in 0..payload.len() {
        let mut h = HeldMagic::new();
        let a = h.feed(&payload[..split]).len();
        let b = h.feed(&payload[split..]).len();
        assert_eq!(a + b, payload.len() - MAGIC_LEN, "split at {split}: exactly the magic is withheld");
        assert_eq!(h.take(), Some(*b"OBCM"), "split at {split}: the magic still reassembles");
    }

    // An object shorter than a magic never yields one (the announce guard rejects those first).
    let mut h = HeldMagic::new();
    assert_eq!(h.feed(b"OB"), b"");
    assert_eq!(h.take(), None, "a 2-byte object has no magic to replay");
}

/// An unknown `status` discriminator decodes to `None` (ignored), never an error — forward
/// compatibility.
#[test]
fn unknown_status_discriminator_is_ignored() {
    assert_eq!(StatusMessage::decode(&[0xEE, 0, 0, 0]), Ok(None));
}

#[test]
fn weather_request_status_is_a_one_byte_hint() {
    let (buf, len) = StatusMessage::WeatherRequest.encode();
    assert_eq!(&buf[..len], &[5]);
    assert_eq!(StatusMessage::decode(&buf[..len]), Ok(Some(StatusMessage::WeatherRequest)));
}

/// The `tripList` fixture (spec §7.4) decodes through the production list codec: a 6-byte v2 header
/// (entry_len 76) + one 76-byte entry whose totals sum the trip's two **resolvable** stages
/// (2×2207 m / 2×76 m) while `stage_count` counts all three stored stages (the third is dangling),
/// and whose trailing `crc32` is the trip file's whole-object CRC-32. Re-encoding reproduces the
/// file byte-for-byte — the Swift `TripCodecTests` pin the same bytes.
#[test]
fn trip_list_vector() {
    use obc_ble::{ListHeader, TripListEntry};

    let trip = fixture("trip-v2.bin");
    let bytes = fixture("trip-list.bin");
    let (h, entry_len) = ListHeader::decode(&bytes).unwrap();
    assert_eq!(h.count, 1);
    assert_eq!(h.total, 1, "nothing truncated");
    assert!(!h.is_truncated());
    assert_eq!(entry_len, TripListEntry::ENTRY_LEN, "v2 tripList entry is 76 bytes");
    assert_eq!(bytes.len(), ListHeader::object_len(h.count as usize, entry_len));

    let e = TripListEntry::decode(&bytes[ListHeader::ENCODED_LEN..ListHeader::ENCODED_LEN + entry_len]).unwrap();
    assert_eq!(e.object_id, 1, "the trip's own device id (separate counter, §4.1)");
    assert_eq!(e.byte_len as usize, trip.len(), "byte_len sizes the stored trip file");
    assert_eq!((e.total_distance_m, e.total_ascent_m), (2 * 2207, 2 * 76), "summed over resolvable stages");
    assert_eq!(e.stage_count, 3, "counts every stored stage, dangling ref included");
    assert_eq!(e.name, b"Alpen Traverse");
    assert_eq!(e.crc32, Crc32::checksum(&trip), "trailing crc32 = the trip file's whole-object CRC");

    let mut rebuilt = ListHeader { count: h.count, total: h.total }.encode(entry_len as u8).to_vec();
    rebuilt.extend_from_slice(&e.encode());
    assert_eq!(rebuilt, bytes, "re-encode");
}

/// The trip object type + tripList type decode from the wire `type` byte (spec §4.1: trip = 9,
/// tripList = 10). Pins the discriminants so a reorder can't silently shift the wire byte, and that
/// a download-request descriptor for the tripList round-trips.
#[test]
fn trip_object_types_and_descriptor() {
    assert_eq!(ObjectType::from_u8(9).unwrap(), ObjectType::Trip);
    assert_eq!(ObjectType::from_u8(10).unwrap(), ObjectType::TripList);
    assert_eq!(ObjectType::Trip.as_u8(), 9);
    assert_eq!(ObjectType::TripList.as_u8(), 10);

    // A tripList download request (op=2, type=10, id 0) round-trips through the production codec.
    let desc = TransferControl { op: Op::Download, ty: ObjectType::TripList, object_id: 0, total_len: 0, crc32: 0 };
    assert_eq!(TransferControl::decode(&desc.encode()).unwrap(), desc);
}

/// The `map` object type (#889): the USB transport introduces it, because a map is far too large to
/// have ever crossed BLE. Pins **16**, pins that the `11`–`15` sensor-reserved band still rejects —
/// the reason 16 was chosen over the next free number — and that an upload descriptor for it
/// round-trips through the production codec, so host and device agree on the byte before the device
/// side can accept one.
#[test]
fn map_object_type_and_reserved_band() {
    assert_eq!(ObjectType::from_u8(16).unwrap(), ObjectType::Map);
    assert_eq!(ObjectType::Map.as_u8(), 16);
    for reserved in 11..=15 {
        assert!(ObjectType::from_u8(reserved).is_err(), "type {reserved} is reserved (sensors, M4)");
    }

    let desc = TransferControl {
        op: Op::Upload,
        ty: ObjectType::Map,
        object_id: 0,
        total_len: 300_000_000,
        crc32: 0x1234_5678,
    };
    assert_eq!(TransferControl::decode(&desc.encode()).unwrap(), desc);
}

/// The object-type band the map upload lives in. Pins the bytes, that the reserved sensor band
/// still rejects, and that `map` classifies as map payload — the property the board's
/// held-back-magic streaming path keys on.
///
/// **The volume-set types are gone** (OBCM v14, #1420): `mapShard` 17, `mapSet` 18 and
/// `terrainShard` 19 named files of a multi-file map, and a map is one file now. Their values are
/// not re-issued to anything else — same no-reuse discipline as a retired GATT UUID — so `from_u8`
/// refuses them along with everything else unallocated.
#[test]
fn map_object_types() {
    assert_eq!(ObjectType::from_u8(16).unwrap(), ObjectType::Map);
    assert_eq!(ObjectType::Map.as_u8(), 16);
    for retired in 17..=19u8 {
        assert!(ObjectType::from_u8(retired).is_err(), "{retired} was a volume-set type and is not re-issued");
    }
    // 20 is the weather bundle (WX3, #1188) — the one type since #889 that is *not* USB-only, so it
    // is named here rather than left as "not a type yet"; 21 is the next unallocated value.
    assert_eq!(ObjectType::from_u8(20).unwrap(), ObjectType::WeatherBundle);
    assert!(ObjectType::from_u8(21).is_err(), "21 is not a type yet");
    for reserved in 11..=15u8 {
        assert!(ObjectType::from_u8(reserved).is_err(), "{reserved} stays reserved for the sensor work");
    }

    assert!(ObjectType::Map.is_map_payload(), "a map streams into its final file with the magic held back");
    for ty in [ObjectType::Route, ObjectType::Trip, ObjectType::FwImage, ObjectType::Echo] {
        assert!(!ty.is_map_payload(), "{ty:?} stages through UPLOAD.TMP");
    }
}

/// The `routeList` fixture decodes through the production list codec, its first two entries agree
/// with the stored route fixtures they describe, its auto-expiry tail spans the epic #638 S4 spread
/// (a live countdown, a not-yet-started clock, a Never route), and re-encoding reproduces the file
/// byte-for-byte.
#[test]
fn route_list_vector() {
    use obc_ble::{ListHeader, RouteListEntry};

    let route_wp = fixture("route-waypoints.obcr");
    let route_plain = fixture("route-plain.obcr");
    let bytes = fixture("route-list.bin");
    let (h, entry_len) = ListHeader::decode(&bytes).unwrap();
    assert_eq!(h.count, 3);
    assert_eq!(h.total, 3, "nothing truncated");
    assert!(!h.is_truncated());
    assert_eq!(entry_len, RouteListEntry::ENTRY_LEN, "v2 routeList entry is 84 bytes (76 core + expiry tail)");
    assert_eq!(entry_len, 84);
    assert_eq!(bytes.len(), ListHeader::object_len(h.count as usize, entry_len));

    // Per-entry expectations, in id order: (byte_len, waypoint_count, content_crc, expires_at,
    // retention). Ids 7/8 are the two stored `.obcr` fixtures; id 9 is a synthetic Never route
    // (reusing the plain route's size/CRC) that pins the Never state on the wire.
    let plain_crc = Crc32::checksum(&route_plain);
    let expect = [
        (7u16, route_wp.len(), 2u16, Crc32::checksum(&route_wp), obc_vectors::ROUTE_EXPIRES_AT_LIVE, 3u8),
        (8, route_plain.len(), 0, plain_crc, 0, 1),
        (9, route_plain.len(), 0, plain_crc, 0, 0),
    ];
    let mut rebuilt = ListHeader { count: h.count, total: h.total }.encode(entry_len as u8).to_vec();
    for (k, &(id, byte_len, waypoints, crc, expires_at, retention)) in expect.iter().enumerate() {
        let off = ListHeader::ENCODED_LEN + k * entry_len;
        let e = RouteListEntry::decode(&bytes[off..off + entry_len]).unwrap();
        assert_eq!(e.object_id, id, "entry {k} id");
        assert_eq!(e.byte_len as usize, byte_len, "entry {k} sizes its stored file");
        assert_eq!(e.waypoint_count, waypoints);
        assert_eq!(e.name, b"Vector Loop");
        assert_eq!(e.crc32, crc, "entry {k} carries its content CRC-32");
        assert_eq!((e.distance_m, e.ascent_m, e.point_count), (2207, 76, 9), "OBCR header stats");
        assert_eq!(e.expires_at, expires_at, "entry {k} expires_at");
        assert_eq!(e.retention, retention, "entry {k} retention");
        rebuilt.extend_from_slice(&e.encode());
    }
    assert_eq!(rebuilt, bytes, "re-encode");
}
