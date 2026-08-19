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
    WeatherRequestBudget, WeatherRequestContext, REASON_HOURLY_ONLY, REASON_NO_BUNDLE, REASON_OUT_OF_AREA,
    REASON_RETRY, REASON_SCHEDULED, REASON_URGENT, VALID_BEARING, VALID_BUNDLE, VALID_POSITION, VALID_ROUTE,
    VALID_SPEED, WEATHER_BUNDLE_OBJECT_ID, WEATHER_REQUEST_CONTEXT_UUID, WEATHER_REQUEST_CONTEXT_VERSION,
    WEATHER_REQUEST_SERVICE_UUID, WEATHER_REQUEST_SERVICE_UUID_LE,
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
fn a_context_with_no_fix_still_carries_a_diagnostic_request() {
    // Cold start indoors: no GPS yet, but the rider opened Weather. It remains well-formed for
    // diagnostics/retry even though the companion cannot fetch until the device supplies a fix.
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
    use obc_weather::{candidate_is_newer, Candidate};

    let generations = [0u32, 1, 2, 7, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, u32::MAX];
    let timestamps = [i64::MIN, -1, 0, 100, 200, i64::MAX];

    for &ig in &generations {
        for &it in &timestamps {
            for &ag in &generations {
                for &at in &timestamps {
                    let incoming = ident(ig, it);
                    let active = ident(ag, at);
                    let storage_would_replace = candidate_is_newer(
                        Candidate { generation: ig, generated_at: it, total_len: 1, bundle_crc32: 0 },
                        Candidate { generation: ag, generated_at: at, total_len: 1, bundle_crc32: 0 },
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

// ============================ The due scheduler (WX8, #1193) ============================
//
// The whole ride/urgent/retry/commit matrix against a synthetic clock — the "simulator clock tests"
// the WX8 acceptance names, pinned here where the machine is pure. The board's task adds only the
// plumbing (context fill, advertising arm, real sleep), which is exactly what these must not need.

use obc_ble::weather_request::{BundleFacts, DueScheduler, RETRY_LADDER_S, WEATHER_REQUEST_WINDOW_S};

const MIN30: u64 = 30 * 60;

fn held(age_s: u64) -> BundleFacts {
    BundleFacts { held: true, age_s: Some(age_s), manual_reusable: false, location_changed: false, hourly_only: false }
}

#[test]
fn scheduled_requests_only_during_a_ride_and_never_when_off() {
    let mut s = DueScheduler::new();
    // Not riding: nothing is ever due, no matter how stale the card.
    assert_eq!(s.poll(0, WeatherRefresh::Every30, false, true, BundleFacts::NONE), None);
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, false, true), None);
    // Off: riding raises nothing either.
    assert_eq!(s.poll(10, WeatherRefresh::Off, true, true, BundleFacts::NONE), None);
    // Riding with a cadence and no bundle: due immediately, reason says both.
    let raise = s.poll(20, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("due at ride start");
    assert_eq!(raise.request_id, 1);
    assert_ne!(raise.reason & REASON_SCHEDULED, 0);
    assert_ne!(raise.reason & REASON_NO_BUNDLE, 0);
    assert_eq!(raise.reason & REASON_URGENT, 0);
}

#[test]
fn a_fresh_bundle_defers_the_first_scheduled_request_across_a_reboot() {
    // Reboot reconstruction: a 10-minute-old bundle on a 30-minute cadence is due 20 minutes in —
    // no countdown was ever persisted.
    let mut s = DueScheduler::new();
    let bundle = held(10 * 60);
    assert_eq!(s.poll(0, WeatherRefresh::Every30, true, true, bundle), None);
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(20 * 60));
    assert_eq!(s.poll(20 * 60 - 1, WeatherRefresh::Every30, true, true, bundle), None);
    let raise = s.poll(20 * 60, WeatherRefresh::Every30, true, true, bundle).expect("interval elapsed");
    assert_ne!(raise.reason & REASON_SCHEDULED, 0);
    assert_eq!(raise.reason & REASON_NO_BUNDLE, 0, "a fresh bundle is not 'no bundle'");
}

#[test]
fn a_bundle_of_unknown_age_anchors_at_scheduler_start() {
    // No trusted clock: the device cannot claim the interval already elapsed, so the countdown
    // starts at the first poll rather than firing immediately.
    let mut s = DueScheduler::new();
    let bundle =
        BundleFacts { held: true, age_s: None, manual_reusable: false, location_changed: false, hourly_only: false };
    assert_eq!(s.poll(100, WeatherRefresh::Every15, true, true, bundle), None);
    assert_eq!(s.next_wake_s(WeatherRefresh::Every15, true, true), Some(100 + 15 * 60));
    assert!(s.poll(100 + 15 * 60, WeatherRefresh::Every15, true, true, bundle).is_some());
}

#[test]
fn an_expired_bundle_reads_as_no_bundle_and_is_due_immediately() {
    let mut s = DueScheduler::new();
    let raise = s.poll(0, WeatherRefresh::Every120, true, true, held(25 * 3600)).expect("expired bundle is due");
    assert_ne!(raise.reason & REASON_NO_BUNDLE, 0, "expired reads as no-bundle in the advisory word");
}

#[test]
fn the_retry_ladder_keeps_one_request_id_and_steps_5_10_20_then_cadence() {
    let mut s = DueScheduler::new();
    let bundle = held(2 * 3600);
    let first = s.poll(0, WeatherRefresh::Every30, true, true, bundle).expect("stale bundle due");
    let id = first.request_id;

    // Rung 1 at +5 min.
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(RETRY_LADDER_S[0]));
    assert_eq!(s.poll(RETRY_LADDER_S[0] - 1, WeatherRefresh::Every30, true, true, bundle), None);
    let r1 = s.poll(RETRY_LADDER_S[0], WeatherRefresh::Every30, true, true, bundle).expect("rung 1");
    assert_eq!(r1.request_id, id, "retries keep the same request id (spec 11.2)");
    assert_ne!(r1.reason & REASON_RETRY, 0);

    // Rung 2 at +10 min, rung 3 at +20 min, then the cadence caps the wait.
    let t1 = RETRY_LADDER_S[0];
    let t2 = t1 + RETRY_LADDER_S[1];
    let r2 = s.poll(t2, WeatherRefresh::Every30, true, true, bundle).expect("rung 2");
    assert_eq!(r2.request_id, id);
    let t3 = t2 + RETRY_LADDER_S[2];
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(t3));
    let r3 = s.poll(t3, WeatherRefresh::Every30, true, true, bundle).expect("rung 3");
    assert_eq!(r3.request_id, id);
    let t4 = t3 + MIN30;
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(t4), "past the ladder = the cadence");
    let r4 = s.poll(t4, WeatherRefresh::Every30, true, true, bundle).expect("cadence retry");
    assert_eq!(r4.request_id, id, "still the same request until a commit finishes it");
}

#[test]
fn success_clears_the_request_and_schedules_from_the_commit() {
    let mut s = DueScheduler::new();
    let raise = s.poll(0, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("due");
    assert_eq!(s.pending_request_id(), Some(raise.request_id));

    s.commit_succeeded(60);
    assert_eq!(s.pending_request_id(), None);
    let fresh = held(0);
    // Nothing due until commit + interval; the next request is a *new* id.
    assert_eq!(s.poll(61, WeatherRefresh::Every30, true, true, fresh), None);
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(60 + MIN30));
    let next = s.poll(60 + MIN30, WeatherRefresh::Every30, true, true, fresh).expect("next interval");
    assert_ne!(next.request_id, raise.request_id, "a finished request is not re-raised");
}

#[test]
fn opening_weather_reuses_a_current_local_bundle_without_raising() {
    let mut s = DueScheduler::new();
    let local_current =
        BundleFacts { held: true, age_s: Some(60), manual_reusable: true, location_changed: false, hourly_only: false };
    s.open_weather();
    assert_eq!(s.poll(10, WeatherRefresh::Every30, false, true, local_current), None);
    assert_eq!(s.pending_request_id(), None);
}

#[test]
fn location_and_hourly_only_reasons_force_a_full_phone_build() {
    let mut s = DueScheduler::new();
    let moved =
        BundleFacts { held: true, age_s: Some(60), manual_reusable: false, location_changed: true, hourly_only: true };
    s.open_weather();
    let raise = s.poll(10, WeatherRefresh::Every30, false, true, moved).expect("location changed");
    assert_ne!(raise.reason & REASON_OUT_OF_AREA, 0);
    assert_ne!(raise.reason & REASON_HOURLY_ONLY, 0);
}

#[test]
fn unchanged_ack_matches_the_live_request_and_defers_manual_rechecks() {
    let mut s = DueScheduler::new();
    s.open_weather();
    let raise = s.poll(0, WeatherRefresh::Every30, false, true, held(20 * 60)).expect("probe due");
    assert!(!s.unchanged_succeeded(raise.request_id.wrapping_add(1), 5, 120));
    assert_eq!(s.pending_request_id(), Some(raise.request_id));
    assert!(s.unchanged_succeeded(raise.request_id, 5, 120));
    assert_eq!(s.pending_request_id(), None);

    let held = BundleFacts {
        held: true,
        age_s: Some(20 * 60),
        manual_reusable: false,
        location_changed: false,
        hourly_only: false,
    };
    s.open_weather();
    assert_eq!(s.poll(124, WeatherRefresh::Every30, false, true, held), None);
    s.open_weather();
    assert!(s.poll(125, WeatherRefresh::Every30, false, true, held).is_some());
}

#[test]
fn weather_unchanged_command_has_a_fixed_bounded_wire_shape() {
    use obc_ble::{DescriptorError, WeatherUnchanged, CMD_WEATHER_UNCHANGED};

    let command = WeatherUnchanged { request_id: 0x7856_3412, retry_after_s: 120 };
    assert_eq!(command.encode(), [CMD_WEATHER_UNCHANGED, 0x12, 0x34, 0x56, 0x78, 120, 0]);
    assert_eq!(WeatherUnchanged::decode(&command.encode()), Ok(command));
    assert_eq!(WeatherUnchanged::decode(&[CMD_WEATHER_UNCHANGED; 6]), Err(DescriptorError::Truncated));
    assert_eq!(WeatherUnchanged::decode(&[CMD_WEATHER_UNCHANGED, 0, 0, 0, 0, 1, 0]), Err(DescriptorError::Bounds));
}

#[test]
fn ride_stop_drops_a_scheduled_request_but_not_an_urgent_one() {
    let mut s = DueScheduler::new();
    s.poll(0, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("scheduled raise");
    // Ride stops: the pending scheduled request lapses, and its ladder never fires.
    assert_eq!(s.poll(1, WeatherRefresh::Every30, false, true, BundleFacts::NONE), None);
    assert_eq!(s.pending_request_id(), None);
    assert_eq!(s.poll(RETRY_LADDER_S[0] + 1, WeatherRefresh::Every30, false, true, BundleFacts::NONE), None);

    // Urgent survives a ride stop: the rider asked, the answer is still wanted.
    s.open_weather();
    let urgent = s.poll(1000, WeatherRefresh::Every30, false, true, BundleFacts::NONE).expect("urgent outside a ride");
    assert_ne!(urgent.reason & REASON_URGENT, 0);
    assert!(s.pending_request_id().is_some());
    assert_eq!(
        s.poll(1001, WeatherRefresh::Every30, false, true, BundleFacts::NONE),
        None,
        "still pending, not dropped"
    );
    let retry = s
        .poll(1000 + RETRY_LADDER_S[0], WeatherRefresh::Every30, false, true, BundleFacts::NONE)
        .expect("urgent retries too");
    assert_eq!(retry.request_id, urgent.request_id);
}

#[test]
fn off_disables_scheduling_but_an_urgent_request_still_raises_and_ladders_out() {
    let mut s = DueScheduler::new();
    s.open_weather();
    let urgent =
        s.poll(0, WeatherRefresh::Off, false, true, BundleFacts::NONE).expect("Weather-screen urgent under Off");
    assert_ne!(urgent.reason & REASON_URGENT, 0);

    // The three rungs fire; with no cadence to fall back on the request then lapses.
    let t1 = RETRY_LADDER_S[0];
    let t2 = t1 + RETRY_LADDER_S[1];
    let t3 = t2 + RETRY_LADDER_S[2];
    assert!(s.poll(t1, WeatherRefresh::Off, false, true, BundleFacts::NONE).is_some());
    assert!(s.poll(t2, WeatherRefresh::Off, false, true, BundleFacts::NONE).is_some());
    assert!(s.poll(t3, WeatherRefresh::Off, false, true, BundleFacts::NONE).is_some());
    assert!(s.pending_request_id().is_some(), "the final raise remains readable for its air window");
    assert_eq!(s.next_wake_s(WeatherRefresh::Off, false, true), Some(t3 + WEATHER_REQUEST_WINDOW_S));
    assert_eq!(s.poll(t3 + WEATHER_REQUEST_WINDOW_S, WeatherRefresh::Off, false, true, BundleFacts::NONE), None);
    assert_eq!(s.pending_request_id(), None, "Off + final air window = the request lapses");
}

#[test]
fn opening_weather_while_pending_reuses_the_id_with_a_fresh_fast_ladder() {
    let mut s = DueScheduler::new();
    let sched = s.poll(0, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("scheduled");
    // 4 minutes in (one minute before rung 1) the rider opens Weather.
    s.open_weather();
    let urgent = s.poll(4 * 60, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("urgent re-raise");
    assert_eq!(urgent.request_id, sched.request_id, "one request, not parallel jobs");
    assert_ne!(urgent.reason & REASON_URGENT, 0);
    assert_ne!(urgent.reason & REASON_SCHEDULED, 0, "the original reason survives");
    // The ladder restarted from the urgent raise.
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(4 * 60 + RETRY_LADDER_S[0]));
}

#[test]
fn an_interval_change_lands_at_the_next_poll() {
    let mut s = DueScheduler::new();
    s.commit_succeeded(0);
    let fresh = held(0);
    // 30-minute cadence: not due at +16 min.
    assert_eq!(s.poll(16 * 60, WeatherRefresh::Every30, true, true, fresh), None);
    // The rider tightens it to 15 minutes: the same instant is now past due.
    assert!(s.poll(16 * 60, WeatherRefresh::Every15, true, true, fresh).is_some());

    // And relaxing to Off drops the now-pending scheduled request.
    assert_eq!(s.poll(16 * 60 + 1, WeatherRefresh::Off, true, true, fresh), None);
    assert_eq!(s.pending_request_id(), None);
}

#[test]
fn a_served_context_read_does_not_finish_the_request() {
    // §11.3: the advertising window is the board's; the *request* is finished only by a commit.
    // The scheduler has no "context served" input at all — this test pins that the ladder keeps
    // running after the first raise regardless of what happened on the air.
    let mut s = DueScheduler::new();
    let first = s.poll(0, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("due");
    let retry =
        s.poll(RETRY_LADDER_S[0], WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("still pending");
    assert_eq!(retry.request_id, first.request_id);
}

#[test]
fn resting_context_serves_the_stored_refresh_byte() {
    // #1221 F2: the resting attribute value must report the rider's own setting from boot —
    // structurally EMPTY (nothing valid, no reason, request id 0), refresh byte theirs.
    for raw in 0u8..=4 {
        let resting = WeatherRequestContext::resting(raw);
        assert_eq!(resting.refresh_raw, raw);
        assert_eq!(resting.validity, 0);
        assert_eq!(resting.reason, 0);
        assert_eq!(resting.request_id, 0);
        // Round-trips through the wire layout like any context.
        assert_eq!(WeatherRequestContext::decode(&resting.encode()), Ok(resting));
    }
    // EMPTY keeps the compile-time default — `resting` is what tracks the stored setting.
    assert_eq!(WeatherRequestContext::EMPTY.refresh_raw, WeatherRefresh::DEFAULT.as_u8());
}

#[test]
fn an_accepted_stale_or_duplicate_answer_finishes_the_request_and_paces_at_the_cadence() {
    // #1221 F3 (the reviewer's 50-re-raise repro, inverted): the phone answers a stale/duplicate
    // upload `committed` (§11.6's ignored-but-successful rows) — that IS the answer ("nothing
    // newer exists upstream"), so the request finishes and the *still-expired* bundle must not
    // re-raise a second later. The next raise comes from the normal cadence machinery.
    let mut s = DueScheduler::new();
    let expired = held(25 * 3600);
    let first = s.poll(0, WeatherRefresh::Every30, true, true, expired).expect("expired bundle due at ride start");

    // The upload lands at t=10 and the store answers `committed` (stale-ignored) — same seam as a
    // fresh commit, deliberately.
    s.commit_succeeded(10);
    assert_eq!(s.pending_request_id(), None, "an accepted answer of any freshness class finishes the request");

    // One second later, same expired bundle on the card: NOT due again.
    assert_eq!(s.poll(11, WeatherRefresh::Every30, true, true, expired), None);
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, true), Some(10 + MIN30), "paced from the accept");
    assert_eq!(s.poll(10 + MIN30 - 1, WeatherRefresh::Every30, true, true, expired), None);

    let next = s.poll(10 + MIN30, WeatherRefresh::Every30, true, true, expired).expect("next cadence interval");
    assert_ne!(next.request_id, first.request_id, "a finished request is never re-raised");
    assert_ne!(next.reason & REASON_NO_BUNDLE, 0, "the reason word still tells the truth about the card");
}

#[test]
fn an_urgent_request_lapses_after_the_ladder_even_with_a_cadence_configured() {
    // #1221 F4 (the reviewer's 200-cycle repro, inverted): an off-ride urgent tap gets exactly the
    // three ladder retries, then goes quiet — the configured cadence belongs to *scheduled*
    // requests while riding, never to a standing urgent beacon.
    let mut s = DueScheduler::new();
    s.open_weather();
    let urgent = s.poll(0, WeatherRefresh::Every30, false, true, BundleFacts::NONE).expect("urgent raise");

    let t1 = RETRY_LADDER_S[0];
    let t2 = t1 + RETRY_LADDER_S[1];
    let t3 = t2 + RETRY_LADDER_S[2];
    for t in [t1, t2, t3] {
        let retry = s.poll(t, WeatherRefresh::Every30, false, true, BundleFacts::NONE).expect("ladder rung");
        assert_eq!(retry.request_id, urgent.request_id);
    }
    assert!(s.pending_request_id().is_some(), "the final raise remains readable for its air window");
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, false, true), Some(t3 + WEATHER_REQUEST_WINDOW_S));
    assert_eq!(s.poll(t3 + WEATHER_REQUEST_WINDOW_S, WeatherRefresh::Every30, false, true, BundleFacts::NONE), None);
    assert_eq!(s.pending_request_id(), None, "urgent + final air window = the request lapses");
    // Hours later: still quiet — no cadence fallback for urgent, ever.
    assert_eq!(s.poll(t3 + 10 * MIN30, WeatherRefresh::Every30, false, true, BundleFacts::NONE), None);

    // A scheduled request under the same cadence still falls back to it (the F4 boundary).
    let mut sched = DueScheduler::new();
    sched.poll(0, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("scheduled");
    let t4 = t3 + MIN30;
    for t in [t1, t2, t3, t4] {
        assert!(sched.poll(t, WeatherRefresh::Every30, true, true, BundleFacts::NONE).is_some(), "rung/cadence at {t}");
    }
    assert!(sched.pending_request_id().is_some(), "scheduled requests keep pacing at the cadence");
}

#[test]
fn no_storage_means_no_requests_at_all() {
    // #1221 F5: a card-less device answers every upload `error`, so raising a request would send
    // the phone round §11.7's fetch-build-upload loop at its own expense, forever. Nothing raises
    // — scheduled or urgent — and next_wake asks for no timer.
    let mut s = DueScheduler::new();
    assert_eq!(s.poll(0, WeatherRefresh::Every30, true, false, BundleFacts::NONE), None);
    s.open_weather();
    assert_eq!(s.poll(1, WeatherRefresh::Every30, true, false, BundleFacts::NONE), None, "urgent too");
    assert_eq!(s.next_wake_s(WeatherRefresh::Every30, true, false), None);

    // A pending request is dropped the moment the card goes away — its ladder never fires again.
    let mut s = DueScheduler::new();
    let raise = s.poll(0, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("due with a card");
    assert_eq!(s.poll(1, WeatherRefresh::Every30, true, false, BundleFacts::NONE), None);
    assert_eq!(s.pending_request_id(), None, "the card that left took the request with it");
    assert_eq!(s.poll(RETRY_LADDER_S[0], WeatherRefresh::Every30, true, false, BundleFacts::NONE), None);

    // Card back: the normal machinery raises a fresh request (a new id, not the dead one's).
    let fresh = s.poll(RETRY_LADDER_S[0] + 1, WeatherRefresh::Every30, true, true, BundleFacts::NONE).expect("re-arm");
    assert_ne!(fresh.request_id, raise.request_id);
}
