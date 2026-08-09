//! The Weather Request contract (spec §11, WX3 #1188) — the codec, the append-only extension
//! points, and the two directions of the compatibility promise.
//!
//! The compatibility tests at the bottom are the ones worth reading: "old app ↔ new firmware" and
//! "new app ↔ old firmware" are the acceptance criteria of #1188, and they are written as the byte
//! sequences each side actually puts on the wire, not as a call into a shared helper that could
//! make both sides wrong in the same direction.

use obc_ble::descriptor::FEATURE_WEATHER;
use obc_ble::weather_request::{
    authenticated_context_was_served, classify_upload, BundleIdentity, UploadDisposition, WeatherRefresh,
    WeatherRequestBudget, WeatherRequestContext, REASON_NO_BUNDLE, REASON_RETRY, REASON_SCHEDULED, REASON_URGENT,
    VALID_BEARING, VALID_BUNDLE, VALID_POSITION, VALID_ROUTE, VALID_SPEED, WEATHER_BUNDLE_OBJECT_ID,
    WEATHER_REQUEST_CONTEXT_UUID, WEATHER_REQUEST_CONTEXT_VERSION, WEATHER_REQUEST_SERVICE_UUID,
    WEATHER_REQUEST_SERVICE_UUID_LE,
};
use obc_ble::{Config, DescriptorError, ObjectType, VersionRead};

/// A fully-populated context — every optional group present, so a round-trip exercises every field.
fn full_context() -> WeatherRequestContext {
    WeatherRequestContext {
        version: WEATHER_REQUEST_CONTEXT_VERSION,
        validity: VALID_POSITION | VALID_BEARING | VALID_SPEED | VALID_BUNDLE | VALID_ROUTE,
        reason: REASON_SCHEDULED | REASON_RETRY,
        refresh_raw: 2, // Every30
        request_id: 0xDEAD_BEEF,
        // Freiburg im Breisgau, in the OBCW header's microdegrees.
        lat_udeg: 47_999_008,
        lon_udeg: 7_842_104,
        fix_utc: 1_800_000_000,
        bearing_deg: 217,
        speed_deci_ms: 58,
        route_id: 4242,
        bundle_generation: 7,
        bundle_generated_at: 1_799_996_400,
        bundle_crc32: 0x1234_5678,
    }
}

// ============================ The context codec ============================

#[test]
fn context_round_trips_every_field() {
    let ctx = full_context();
    let bytes = ctx.encode();
    assert_eq!(bytes.len(), WeatherRequestContext::ENCODED_LEN);
    assert_eq!(WeatherRequestContext::decode(&bytes).unwrap(), ctx);
}

#[test]
fn context_declares_its_own_length_in_byte_one() {
    let bytes = full_context().encode();
    assert_eq!(bytes[0], WEATHER_REQUEST_CONTEXT_VERSION);
    assert_eq!(bytes[1] as usize, WeatherRequestContext::ENCODED_LEN);
}

#[test]
fn context_is_little_endian_at_the_pinned_offsets() {
    let bytes = full_context().encode();
    // Spot-check the offsets the spec table names, so a field reorder cannot pass as a round-trip.
    assert_eq!(&bytes[8..12], &0xDEAD_BEEFu32.to_le_bytes(), "request_id at 8");
    assert_eq!(&bytes[12..16], &47_999_008i32.to_le_bytes(), "lat_udeg at 12");
    assert_eq!(&bytes[20..28], &1_800_000_000i64.to_le_bytes(), "fix_utc at 20");
    assert_eq!(&bytes[36..40], &7u32.to_le_bytes(), "bundle_generation at 36");
    assert_eq!(&bytes[48..52], &0x1234_5678u32.to_le_bytes(), "bundle_crc32 at 48");
}

#[test]
fn reserved_bytes_are_written_zero() {
    let bytes = full_context().encode();
    assert_eq!(bytes[7], 0, "reserved0");
    assert_eq!(&bytes[34..36], &[0, 0], "reserved1");
}

#[test]
fn empty_context_is_structurally_valid_and_claims_nothing() {
    let bytes = WeatherRequestContext::EMPTY.encode();
    let decoded = WeatherRequestContext::decode(&bytes).unwrap();
    assert_eq!(decoded, WeatherRequestContext::EMPTY);
    assert_eq!(decoded.validity, 0, "a read taken out of turn must not imply a fix");
    assert_eq!(decoded.reason, 0);
    assert!(!decoded.has(VALID_POSITION));
    assert!(!decoded.has(VALID_BUNDLE));
}

#[test]
fn every_truncation_is_rejected_rather_than_half_decoded() {
    let bytes = full_context().encode();
    for len in 0..WeatherRequestContext::ENCODED_LEN {
        assert_eq!(
            WeatherRequestContext::decode(&bytes[..len]),
            Err(DescriptorError::Truncated),
            "a {len}-byte read must not decode"
        );
    }
    assert!(WeatherRequestContext::decode(&bytes).is_ok());
}

#[test]
fn a_declared_length_below_v1_is_rejected() {
    // A writer that claims fewer bytes than v1 defines is not an old writer — v1 is the first
    // version — so it is malformed rather than something to decode leniently.
    let mut bytes = full_context().encode();
    bytes[1] = (WeatherRequestContext::ENCODED_LEN - 1) as u8;
    assert_eq!(WeatherRequestContext::decode(&bytes), Err(DescriptorError::Truncated));
}

#[test]
fn a_declared_length_longer_than_the_read_is_rejected() {
    // The writer said 60 bytes and 52 arrived: a short read, not a short value.
    let mut bytes = full_context().encode();
    bytes[1] = 60;
    assert_eq!(WeatherRequestContext::decode(&bytes), Err(DescriptorError::Truncated));
}

/// The read direction of §11.8. A firmware that appends a fifth interval must not be able to kill
/// weather on every shipped app — so an unrecognised refresh byte rides through verbatim and reads
/// as *unknown*, exactly like an unrecognised `reason` bit.
#[test]
fn an_unknown_refresh_byte_reads_as_unknown_rather_than_failing() {
    let mut bytes = full_context().encode();
    bytes[6] = 9;
    let decoded = WeatherRequestContext::decode(&bytes).expect("a newer interval must not be fatal");
    assert_eq!(decoded.refresh(), None, "unknown, and specifically not Off and not the default");
    assert_ne!(decoded.refresh(), Some(WeatherRefresh::Off));
    assert_ne!(decoded.refresh(), Some(WeatherRefresh::DEFAULT));
    assert_eq!(decoded.refresh_raw, 9, "the byte survives verbatim");
    assert_eq!(decoded.encode(), bytes, "and round-trips unchanged");
    // Everything else in the read still decodes — one unknown byte must not cost the whole request.
    assert_eq!(decoded.request_id, full_context().request_id);
}

#[test]
fn a_known_refresh_byte_reads_as_that_interval() {
    let bytes = full_context().encode();
    assert_eq!(WeatherRequestContext::decode(&bytes).unwrap().refresh(), Some(WeatherRefresh::Every30));
}

/// The append-only promise in the direction that matters: tomorrow's firmware appends a field, and
/// today's shipped app keeps reading the request it understands.
#[test]
fn a_future_longer_context_still_decodes_on_this_build() {
    let ctx = full_context();
    let mut future = ctx.encode().to_vec();
    future.extend_from_slice(&[0xAA; 8]); // a v2 field this build has never heard of
    future[1] = future.len() as u8; // the future writer declares its own longer length
    future[0] = 2; // ...and its own version

    let decoded = WeatherRequestContext::decode(&future).unwrap();
    assert_eq!(decoded.version, 2, "the version is reported, not normalised away");
    assert_eq!(decoded.request_id, ctx.request_id);
    assert_eq!(decoded.bundle_crc32, ctx.bundle_crc32, "the last v1 field survives the append");
}

#[test]
fn unknown_validity_and_reason_bits_are_ignored_not_rejected() {
    let mut ctx = full_context();
    ctx.validity |= 1 << 15;
    ctx.reason |= 1 << 14;
    let decoded = WeatherRequestContext::decode(&ctx.encode()).unwrap();
    assert!(decoded.has(VALID_POSITION), "a known bit still reads through an unknown neighbour");
    assert!(decoded.because(REASON_SCHEDULED));
    assert_eq!(decoded.validity, ctx.validity, "unknown bits are preserved verbatim");
}

/// Absence is expressed by a cleared flag, never by a sentinel — an unset position must not be
/// mistakable for the equator, and an unset bundle must not read as generation 0.
#[test]
fn absent_groups_are_flagged_absent_not_zeroed_into_meaning() {
    let ctx = WeatherRequestContext { validity: 0, reason: REASON_NO_BUNDLE, ..WeatherRequestContext::EMPTY };
    let decoded = WeatherRequestContext::decode(&ctx.encode()).unwrap();
    assert!(!decoded.has(VALID_POSITION));
    assert!(!decoded.has(VALID_BUNDLE));
    assert!(!decoded.has(VALID_ROUTE));
    assert!(decoded.because(REASON_NO_BUNDLE));
}

#[test]
fn a_context_with_no_fix_still_carries_a_request_the_phone_can_answer() {
    // Cold start indoors: no GPS yet, but the rider opened Weather. The phone can still fetch by
    // its own location, so this must be a well-formed request rather than a suppressed one.
    let ctx = WeatherRequestContext {
        reason: REASON_URGENT | REASON_NO_BUNDLE,
        request_id: 1,
        ..WeatherRequestContext::EMPTY
    };
    let decoded = WeatherRequestContext::decode(&ctx.encode()).unwrap();
    assert_eq!(decoded.request_id, 1);
    assert!(decoded.because(REASON_URGENT));
    assert!(!decoded.has(VALID_POSITION));
}

// ============================ The refresh enum ============================

#[test]
fn refresh_enum_round_trips_and_maps_to_the_documented_minutes() {
    let cases = [
        (WeatherRefresh::Off, 0u8, None),
        (WeatherRefresh::Every15, 1, Some(15)),
        (WeatherRefresh::Every30, 2, Some(30)),
        (WeatherRefresh::Every60, 3, Some(60)),
        (WeatherRefresh::Every120, 4, Some(120)),
    ];
    for (refresh, byte, minutes) in cases {
        assert_eq!(refresh.as_u8(), byte);
        assert_eq!(WeatherRefresh::from_u8(byte).unwrap(), refresh);
        assert_eq!(refresh.minutes(), minutes);
    }
    assert_eq!(WeatherRefresh::DEFAULT, WeatherRefresh::Every30, "epic #1185 locks 30 minutes");
    assert_eq!(WeatherRefresh::Off.minutes(), None, "Off has no interval, not a zero one");
}

#[test]
fn an_out_of_range_refresh_value_is_an_error_not_a_default() {
    for byte in 5u8..=255 {
        assert_eq!(WeatherRefresh::from_u8(byte), Err(DescriptorError::UnknownRefresh(byte)));
    }
}

// ============================ Upload disposition ============================

fn ident(generation: u32, generated_at: i64) -> BundleIdentity {
    BundleIdentity { generation, generated_at }
}

#[test]
fn a_newer_generation_commits() {
    assert_eq!(classify_upload(ident(8, 200), Some(ident(7, 100))), UploadDisposition::Commit);
}

#[test]
fn the_first_bundle_always_commits() {
    assert_eq!(classify_upload(ident(0, 0), None), UploadDisposition::Commit);
    assert_eq!(classify_upload(ident(u32::MAX, i64::MIN), None), UploadDisposition::Commit);
}

/// A duplicate upload is the phone doing its job twice (a lost ack, a re-run request), not an
/// error. It must not fail, because a failure would send the phone back around the retry ladder to
/// upload the very same bytes again.
#[test]
fn an_identical_bundle_is_ignored_and_not_an_error() {
    assert_eq!(classify_upload(ident(7, 100), Some(ident(7, 100))), UploadDisposition::DuplicateIgnored);
}

/// A stale response — the phone's HTTP was slow and a newer bundle landed meanwhile — is likewise
/// accepted and dropped, never failed.
#[test]
fn an_older_generation_is_ignored_and_not_an_error() {
    assert_eq!(classify_upload(ident(6, 100), Some(ident(7, 200))), UploadDisposition::StaleIgnored);
}

/// Equal generations are not automatically duplicates: the producer may have re-baked the same
/// generation with fresher data, and `generated_at` is what separates them.
#[test]
fn an_equal_generation_falls_back_to_the_producer_timestamp() {
    assert_eq!(classify_upload(ident(7, 200), Some(ident(7, 100))), UploadDisposition::Commit);
    assert_eq!(classify_upload(ident(7, 50), Some(ident(7, 100))), UploadDisposition::StaleIgnored);
}

#[test]
fn generation_comparison_survives_the_u32_wrap() {
    // Just after the counter wraps, 1 is newer than u32::MAX — serial arithmetic, not `<`.
    assert_eq!(classify_upload(ident(1, 0), Some(ident(u32::MAX, 0))), UploadDisposition::Commit);
    assert_eq!(classify_upload(ident(u32::MAX, 0), Some(ident(1, 0))), UploadDisposition::StaleIgnored);
}

/// **The parity gate.** `classify_upload` decides what the wire reports; `candidate_is_newer`
/// decides what the card actually selects at boot. If they disagree, a device can answer "committed"
/// and then quietly boot the old bundle — so the two are asserted against each other directly,
/// across the wrap and the half-range ambiguity, rather than trusting a comment that says they match.
#[test]
fn classification_agrees_with_the_storage_layers_selector() {
    use obc_weather::{candidate_is_newer, Candidate, Slot};

    let generations = [0u32, 1, 2, 7, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, u32::MAX];
    let timestamps = [i64::MIN, -1, 0, 100, 200, i64::MAX];

    for &ig in &generations {
        for &it in &timestamps {
            for &ag in &generations {
                for &at in &timestamps {
                    let incoming = ident(ig, it);
                    let active = ident(ag, at);
                    let storage_would_replace = candidate_is_newer(
                        Candidate { slot: Slot::B, generation: ig, generated_at: it, total_len: 1, bundle_crc32: 0 },
                        Candidate { slot: Slot::A, generation: ag, generated_at: at, total_len: 1, bundle_crc32: 0 },
                    );
                    let wire_commits = classify_upload(incoming, Some(active)) == UploadDisposition::Commit;
                    assert_eq!(
                        wire_commits, storage_would_replace,
                        "incoming gen {ig} @ {it} vs active gen {ag} @ {at}"
                    );
                }
            }
        }
    }
}

/// The half-range delta is genuinely ambiguous under serial arithmetic, and both layers resolve it
/// the same way — on the producer timestamp — rather than one of them guessing.
#[test]
fn the_half_range_ambiguity_resolves_on_the_timestamp() {
    assert_eq!(classify_upload(ident(0x8000_0000, 200), Some(ident(0, 100))), UploadDisposition::Commit);
    assert_eq!(classify_upload(ident(0x8000_0000, 50), Some(ident(0, 100))), UploadDisposition::StaleIgnored);
}

#[test]
fn the_weather_bundle_is_a_singleton_at_object_id_zero() {
    assert_eq!(WEATHER_BUNDLE_OBJECT_ID, 0);
}

// ============================ Advertising policy ============================

#[test]
fn only_a_secured_and_sent_read_consumes_the_request() {
    assert!(authenticated_context_was_served(true, true));
    assert!(!authenticated_context_was_served(false, true), "an unbonded scan must not cost a forecast");
    assert!(!authenticated_context_was_served(true, false), "a response that never went out is not served");
    assert!(!authenticated_context_was_served(false, false));
}

/// The budget is a deadline, not a duration: repeated failed connections must not extend the
/// advertising window into a permanent secondary beacon.
#[test]
fn the_advertising_budget_is_monotonic_across_connection_churn() {
    let budget = WeatherRequestBudget::new(1_000, 60_000);
    assert_eq!(budget.remaining_ticks(1_000), 60_000);
    assert_eq!(budget.remaining_ticks(31_000), 30_000);
    assert!(!budget.expired(60_999));
    assert!(budget.expired(61_000));
    assert!(budget.expired(u64::MAX), "past the deadline stays expired, it does not wrap");
}

#[test]
fn the_budget_saturates_rather_than_overflowing() {
    let budget = WeatherRequestBudget::new(u64::MAX, 60_000);
    assert!(budget.expired(u64::MAX));
}

// ============================ UUID pinning ============================

#[test]
fn the_service_uuid_display_and_advertising_forms_agree() {
    assert_eq!(WEATHER_REQUEST_SERVICE_UUID, "B3B60000-33B4-4F02-A5FF-E5954D54B5AA");
    assert_eq!(WEATHER_REQUEST_CONTEXT_UUID, "B3B60001-33B4-4F02-A5FF-E5954D54B5AA");

    // The advertising form is the display form's bytes reversed. Deriving it here rather than
    // restating the literal is the point: this is exactly the transposition that silently makes a
    // device undiscoverable, and it cannot be eyeballed in a 16-byte array.
    let hex: String = WEATHER_REQUEST_SERVICE_UUID.chars().filter(|c| *c != '-').collect();
    let mut big_endian = [0u8; 16];
    for (i, byte) in big_endian.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    let mut little_endian = big_endian;
    little_endian.reverse();
    assert_eq!(WEATHER_REQUEST_SERVICE_UUID_LE, little_endian);
}

// ============================ Compatibility: the acceptance criteria ============================

/// **Old app ↔ new firmware.** A shipped app decodes the identity read with the pre-WX3 rules
/// (`>= 6` bytes, take byte 6 if present, ignore the rest) and must survive the widened 11-byte
/// read that a weather-capable device now serves.
#[test]
fn old_app_reads_the_widened_identity_read_without_noticing() {
    let new_firmware = VersionRead {
        version: 2,
        store_epoch: 0xA1B2_C3D4,
        obcm_version: Some(12),
        feature_bits: Some(FEATURE_WEATHER),
    };
    let (buf, len) = new_firmware.encode();
    assert_eq!(len, 11, "a weather device serves the full read");
    let wire = &buf[..len];

    // The old app's decoder, transcribed — length-driven, ignoring what it does not know.
    let old_version = u16::from_le_bytes([wire[0], wire[1]]);
    let old_epoch = u32::from_le_bytes([wire[2], wire[3], wire[4], wire[5]]);
    let old_obcm = wire.get(6).copied();

    assert_eq!(old_version, 2, "no protocol bump — the old app must not take the mismatch path");
    assert_eq!(old_epoch, 0xA1B2_C3D4);
    assert_eq!(old_obcm, Some(12));
}

/// **New app ↔ old firmware.** The app must read a 7-byte (pre-WX3) and a 6-byte (pre-E1) identity
/// read as *no weather capability* — absent, not fabricated — and must not offer weather.
#[test]
fn new_app_reads_old_firmware_as_having_no_weather() {
    let pre_wx3 = VersionRead { version: 2, store_epoch: 9, obcm_version: Some(12), feature_bits: None };
    let (buf, len) = pre_wx3.encode();
    assert_eq!(len, 7);
    let decoded = VersionRead::decode(&buf[..len]).unwrap();
    assert_eq!(decoded, pre_wx3);
    assert_eq!(decoded.feature_bits, None, "absent, never Some(0)");
    assert!(!decoded.has_weather());

    let pre_e1 = VersionRead { version: 2, store_epoch: 9, obcm_version: None, feature_bits: None };
    let (buf, len) = pre_e1.encode();
    assert_eq!(len, 6);
    let decoded = VersionRead::decode(&buf[..len]).unwrap();
    assert_eq!(decoded.obcm_version, None);
    assert_eq!(decoded.feature_bits, None);
    assert!(!decoded.has_weather());
}

#[test]
fn a_truncated_capability_word_never_claims_a_feature() {
    let full = VersionRead { version: 2, store_epoch: 9, obcm_version: Some(12), feature_bits: Some(FEATURE_WEATHER) };
    let (buf, _) = full.encode();
    // 8, 9 and 10 bytes are a broken read of a u32, not a smaller capability set.
    for len in 8..VersionRead::ENCODED_LEN {
        let decoded = VersionRead::decode(&buf[..len]).unwrap();
        assert_eq!(decoded.feature_bits, None, "{len} bytes must not yield a partial word");
        assert!(!decoded.has_weather(), "{len} bytes must not claim weather");
    }
}

#[test]
fn the_identity_read_still_rejects_anything_shorter_than_the_epoch() {
    let (buf, _) = VersionRead { version: 2, store_epoch: 9, obcm_version: Some(12), feature_bits: Some(1) }.encode();
    for len in 0..VersionRead::ENCODED_LEN_NO_OBCM {
        assert_eq!(VersionRead::decode(&buf[..len]), Err(DescriptorError::Truncated));
    }
}

#[test]
fn unknown_feature_bits_are_ignored() {
    let exotic = VersionRead {
        version: 2,
        store_epoch: 9,
        obcm_version: Some(12),
        feature_bits: Some(FEATURE_WEATHER | 1 << 31),
    };
    let (buf, len) = exotic.encode();
    let decoded = VersionRead::decode(&buf[..len]).unwrap();
    assert!(decoded.has_weather(), "an unknown neighbour bit does not mask a known one");
}

#[test]
fn features_without_a_map_version_encode_as_the_short_form_rather_than_fabricating_byte_six() {
    let impossible = VersionRead { version: 2, store_epoch: 9, obcm_version: None, feature_bits: Some(1) };
    let (_, len) = impossible.encode();
    assert_eq!(len, VersionRead::ENCODED_LEN_NO_OBCM, "positional fields cannot skip a hole");
}

/// **Old app ↔ new firmware, Config.** The old app writes a 3-byte-plus-name blob to rename the
/// device. That must read as "refresh unspecified" and leave the device's setting alone — not as
/// `Off`, which would silently disable weather on a rename.
#[test]
fn an_old_apps_config_write_does_not_disable_weather() {
    let mut out = [0u8; Config::MAX_ENCODED];
    let old = Config { name: b"OBC-1A2B", units: 0, weather_refresh: None };
    let len = old.encode(&mut out).unwrap();
    assert_eq!(len, 2 + 8 + 1, "byte-identical to the pre-WX3 blob");

    let decoded = Config::decode(&out[..len]).unwrap();
    assert_eq!(decoded.weather_refresh, None, "unspecified — no byte at all");
    assert_eq!(decoded.known_refresh(), None);
    assert_ne!(decoded.known_refresh(), Some(WeatherRefresh::Off), "absent is not Off");
    // And on the write side, absent is not the default either: it is "change nothing".
    assert_eq!(decoded.refresh_to_apply(), Ok(None));
}

/// **New app ↔ old firmware, Config.** The new app writes the trailing byte; a firmware that
/// predates it ignores the trailing byte under the append-only rule and stores the rest.
#[test]
fn config_round_trips_with_the_refresh_field() {
    let mut out = [0u8; Config::MAX_ENCODED];
    let new = Config { name: b"Timo's OBC", units: 1, weather_refresh: Some(WeatherRefresh::Every120.as_u8()) };
    let len = new.encode(&mut out).unwrap();
    assert_eq!(len, 2 + 10 + 1 + 1);
    assert_eq!(out[len - 1], 4, "the appended refresh byte");
    assert_eq!(Config::decode(&out[..len]).unwrap(), new);

    // The old firmware's decoder, transcribed: it never looks past `units`.
    let name_len = u16::from_le_bytes([out[0], out[1]]) as usize;
    assert_eq!(&out[2..2 + name_len], b"Timo's OBC");
    assert_eq!(out[2 + name_len], 1, "units still land where the old layout put them");
}

/// §11.8's asymmetry, both halves, on one blob. The *write* direction refuses an interval the
/// device cannot honour; the *read* direction reports it as unknown and keeps going. A single
/// direction-blind rule cannot be right for both: rejecting the read is what would let a future
/// fifth interval stop a shipped app from renaming its device, and tolerating the write is what
/// would tell a rider their choice was applied when it was discarded.
#[test]
fn an_unknown_refresh_byte_is_refused_on_a_write_and_tolerated_on_a_read() {
    let mut out = [0u8; Config::MAX_ENCODED];
    let cfg = Config { name: b"OBC", units: 0, weather_refresh: Some(WeatherRefresh::Off.as_u8()) };
    let len = cfg.encode(&mut out).unwrap();
    out[len - 1] = 200;

    let decoded = Config::decode(&out[..len]).expect("the blob itself is well-formed");
    assert_eq!(decoded.weather_refresh, Some(200), "the byte survives verbatim");
    assert_eq!(decoded.known_refresh(), None, "a reader sees unknown, not Off and not the default");
    assert_eq!(
        decoded.refresh_to_apply(),
        Err(DescriptorError::UnknownRefresh(200)),
        "a device asked to adopt it must refuse"
    );
}

/// The write-direction meaning of *absent*, which is not the read-direction meaning. WX8 stores
/// this: absent on a write means **leave the stored value alone**, so an old app's rename cannot
/// reset a rider who deliberately chose `Off`.
#[test]
fn an_absent_refresh_on_a_write_means_leave_the_stored_value_alone() {
    let mut out = [0u8; Config::MAX_ENCODED];
    let old_app_rename = Config { name: b"Alps", units: 0, weather_refresh: None };
    let len = old_app_rename.encode(&mut out).unwrap();
    let decoded = Config::decode(&out[..len]).unwrap();
    assert_eq!(decoded.refresh_to_apply(), Ok(None), "no refusal, and nothing to store");
    assert_eq!(decoded.known_refresh(), None);
}

#[test]
fn config_still_accepts_a_trailing_byte_past_the_fields_we_know() {
    // A future field appended after `weather_refresh` must not make this build refuse the blob.
    let mut out = [0u8; Config::MAX_ENCODED];
    let cfg = Config { name: b"OBC", units: 0, weather_refresh: Some(WeatherRefresh::Every15.as_u8()) };
    let len = cfg.encode(&mut out).unwrap();
    out[len] = 0x77;
    let decoded = Config::decode(&out[..len + 1]).unwrap();
    assert_eq!(decoded, cfg);
}

// ============================ The object type ============================

#[test]
fn the_weather_bundle_type_is_twenty() {
    assert_eq!(ObjectType::WeatherBundle.as_u8(), 20);
    assert_eq!(ObjectType::from_u8(20).unwrap(), ObjectType::WeatherBundle);
}

#[test]
fn the_sensor_and_unallocated_bands_still_reject() {
    for reserved in 11u8..=15 {
        assert_eq!(
            ObjectType::from_u8(reserved),
            Err(DescriptorError::UnknownType(reserved)),
            "{reserved} stays reserved for the sensor work (M4)"
        );
    }
    assert_eq!(ObjectType::from_u8(21), Err(DescriptorError::UnknownType(21)));
}

#[test]
fn a_weather_bundle_is_not_a_map_payload() {
    // It rides the ordinary temp-file upload path, not the magic-held-back streaming one, and it is
    // the one new type since #889 that is *not* USB-only.
    assert!(!ObjectType::WeatherBundle.is_map_payload());
}
