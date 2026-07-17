//! Boot-state page codec + `decide()` contract tests (host).
//!
//! The byte layout these exercise is the bootloader↔app handoff, documented in `OBCU_Spec.md` §2 —
//! keep the two in lockstep.

use obc_dfu::{
    decide, verdict, BootDecision, BootState, Extent, ImageHeader, LastOutcome, OutcomeKind, StagedRef, Verdict,
    MAX_ENCODED_LEN, MAX_EXTENTS, PAGE_LEN,
};

fn header(tag: &str, len: u32) -> ImageHeader {
    // Build a real, self-consistent OBCU header (its CRC must survive the nested decode).
    let mut h = ImageHeader::new(&[], tag);
    h.image_len = len;
    h.image_crc32 = 0xA5A5_1234 ^ len;
    h
}

fn extents(n: usize) -> Vec<Extent> {
    (0..n).map(|i| Extent { start_block: 1000 + i as u32 * 8, blocks: (i as u32 % 5) + 1 }).collect()
}

fn staged(tag: &str, n_extents: usize) -> StagedRef {
    let h = header(tag, 123_456);
    // len/crc32 must match the header's own fields (the redundancy consistency check).
    StagedRef::new(h, h.image_len, h.image_crc32, &extents(n_extents)).expect("consistent + within MAX_EXTENTS")
}

/// Round-trip every variant, including full extent lists, through a page-sized buffer.
#[test]
fn roundtrip_all_variants() {
    let cases = vec![
        BootState::Idle { installed: None, last_outcome: None },
        BootState::Idle { installed: Some(header("v1.0.0", 800_000)), last_outcome: None },
        BootState::Idle {
            installed: Some(header("v1.0.0", 800_000)),
            last_outcome: Some(LastOutcome { kind: OutcomeKind::RolledBack, generation: 12 }),
        },
        BootState::Idle {
            installed: None,
            last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 0xFFFF_FFFF }),
        },
        BootState::Armed { generation: 1, update: staged("update", 0), rollback: None },
        BootState::Armed { generation: 7, update: staged("update", 3), rollback: Some(staged("rollback", 2)) },
        // Both refs full to MAX_EXTENTS — the largest possible blob.
        BootState::Armed {
            generation: 0xFFFF_FFFF,
            update: staged("update", MAX_EXTENTS),
            rollback: Some(staged("rollback", MAX_EXTENTS)),
        },
        BootState::Trial { generation: 2, installed: header("v2.0.0", 900_000), rollback: None },
        BootState::Trial {
            generation: 9,
            installed: header("v2.0.0", 900_000),
            rollback: Some(staged("rollback", MAX_EXTENTS)),
        },
    ];
    for state in cases {
        let page = state.encode();
        assert!(page.len() <= MAX_ENCODED_LEN, "blob within max");
        assert_eq!(page.len() % 16, 0, "16-byte-line aligned");
        // Decode straight from the encoded bytes...
        assert_eq!(BootState::decode(page.as_bytes()), state);
        // ...and from a full 4 KB page (the real read is the whole RRAM page, zero-padded).
        let mut whole = vec![0u8; PAGE_LEN];
        whole[..page.len()].copy_from_slice(page.as_bytes());
        assert_eq!(BootState::decode(&whole), state, "decodes within a full page read");
    }
}

/// `generation` is preserved and exposed for the stateful variants (S3/S4 replay guard).
#[test]
fn generation_visible() {
    let armed = BootState::Armed { generation: 42, update: staged("u", 4), rollback: None };
    assert_eq!(armed.generation(), 42);
    assert_eq!(BootState::decode(armed.encode().as_bytes()).generation(), 42);

    let trial = BootState::Trial { generation: 4_000_000_000, installed: header("v", 1), rollback: None };
    assert_eq!(BootState::decode(trial.encode().as_bytes()).generation(), 4_000_000_000);

    assert_eq!(BootState::Idle { installed: None, last_outcome: None }.generation(), 0);
}

/// A blank / all-ones page (a freshly erased or never-written RRAM region) decodes to `Idle`.
#[test]
fn blank_page_is_idle() {
    assert_eq!(BootState::decode(&[0u8; PAGE_LEN]), BootState::Idle { installed: None, last_outcome: None });
    assert_eq!(BootState::decode(&[0xFFu8; PAGE_LEN]), BootState::Idle { installed: None, last_outcome: None });
    assert_eq!(BootState::decode(&[]), BootState::Idle { installed: None, last_outcome: None });
    assert_eq!(BootState::decode(&[1, 2, 3]), BootState::Idle { installed: None, last_outcome: None });
}

/// Torn writes: truncating or flipping bytes at several offsets must fail the CRC (or a bounds check)
/// and fall back to `Idle` — never a partially-decoded install request.
#[test]
fn torn_blob_is_idle() {
    let state = BootState::Armed { generation: 3, update: staged("u", 5), rollback: Some(staged("r", 4)) };
    let page = state.encode();
    let good = page.as_bytes().to_vec();

    // Truncation at every 16-byte line boundary (a half-completed line write).
    let mut off = 16;
    while off < good.len() {
        assert_eq!(
            BootState::decode(&good[..off]),
            BootState::Idle { installed: None, last_outcome: None },
            "truncated at {off}"
        );
        off += 16;
    }

    // A single flipped byte at a spread of offsets (magic, version, tag, blob_len, generation, deep in
    // the payload, in the trailing CRC) must all fail.
    for off in [0usize, 4, 6, 8, 12, 20, 100, good.len() - 1] {
        let mut torn = good.clone();
        torn[off] ^= 0xFF;
        assert_eq!(BootState::decode(&torn), BootState::Idle { installed: None, last_outcome: None }, "flip at {off}");
    }
}

/// A wild `blob_len` field must never trip an out-of-bounds read; it just decodes to `Idle`.
#[test]
fn bogus_blob_len_is_idle() {
    let page = BootState::Idle { installed: Some(header("v", 10)), last_outcome: None }.encode();
    for len in [0u32, 1, 15, 0xFFFF_FFFF, (PAGE_LEN as u32) + 16] {
        let mut b = page.as_bytes().to_vec();
        b[8..12].copy_from_slice(&len.to_le_bytes());
        assert_eq!(BootState::decode(&b), BootState::Idle { installed: None, last_outcome: None }, "blob_len {len}");
    }
}

/// An extent count past `MAX_EXTENTS` in the payload is rejected (defends the fixed-capacity store).
#[test]
fn overlong_extent_count_is_idle() {
    let page = BootState::Armed { generation: 1, update: staged("u", 2), rollback: None }.encode();
    let mut b = page.as_bytes().to_vec();
    // The update StagedRef's extent_count sits right after the 64-byte header + len(4) + crc(4).
    let count_off = 16 + 64 + 4 + 4;
    b[count_off..count_off + 2].copy_from_slice(&((MAX_EXTENTS as u16) + 1).to_le_bytes());
    // Fix the CRC so only the count check (not the CRC) can reject it.
    let blob_len = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    let crc = obc_dfu::crc32(&b[..blob_len - 4]);
    b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(BootState::decode(&b), BootState::Idle { installed: None, last_outcome: None });
}

/// `StagedRef::new` refuses more than `MAX_EXTENTS`.
#[test]
fn staged_ref_extent_cap() {
    let h = header("v", 1);
    assert!(StagedRef::new(h, h.image_len, h.image_crc32, &extents(MAX_EXTENTS)).is_some());
    assert!(StagedRef::new(h, h.image_len, h.image_crc32, &extents(MAX_EXTENTS + 1)).is_none());
}

/// `StagedRef::new` refuses `len`/`crc32` that disagree with the embedded header — the redundant
/// fields must never diverge (spec §2.3), or the installer would silently pick one of two truths.
#[test]
fn staged_ref_rejects_inconsistent_fields() {
    let h = header("v", 500);
    assert!(StagedRef::new(h, h.image_len, h.image_crc32, &extents(1)).is_some());
    assert!(StagedRef::new(h, h.image_len + 1, h.image_crc32, &extents(1)).is_none(), "len mismatch");
    assert!(StagedRef::new(h, h.image_len, h.image_crc32 ^ 1, &extents(1)).is_none(), "crc mismatch");
}

/// A stored `StagedRef` whose redundant `len` disagrees with its embedded header decodes to `Idle`,
/// even with the whole-blob CRC re-fixed — the consistency check itself must reject it.
#[test]
fn inconsistent_staged_len_is_idle() {
    let page = BootState::Armed { generation: 1, update: staged("u", 2), rollback: None }.encode();
    let mut b = page.as_bytes().to_vec();
    // The update StagedRef's redundant `len` sits right after its 64-byte embedded header.
    let len_off = 16 + 64;
    let stored = u32::from_le_bytes([b[len_off], b[len_off + 1], b[len_off + 2], b[len_off + 3]]);
    b[len_off..len_off + 4].copy_from_slice(&(stored + 1).to_le_bytes());
    // Fix the whole-blob CRC so only the header-consistency check can reject it.
    let blob_len = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    let crc = obc_dfu::crc32(&b[..blob_len - 4]);
    b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(BootState::decode(&b), BootState::Idle { installed: None, last_outcome: None });
}

/// The full `decide()` matrix (epic #615 invariant 3).
#[test]
fn decide_matrix() {
    // Idle ⇒ Jump (with or without an installed header).
    assert_eq!(decide(&BootState::Idle { installed: None, last_outcome: None }), BootDecision::Jump);
    assert_eq!(decide(&BootState::Idle { installed: Some(header("v", 1)), last_outcome: None }), BootDecision::Jump);

    // Armed ⇒ Install the staged update (carrying its generation + rollback snapshot).
    let up = staged("u", 3);
    assert_eq!(
        decide(&BootState::Armed { generation: 1, update: up, rollback: None }),
        BootDecision::Install { update: up, generation: 1, rollback: None }
    );
    let rb1 = staged("r", 1);
    assert_eq!(
        decide(&BootState::Armed { generation: 1, update: up, rollback: Some(rb1) }),
        BootDecision::Install { update: up, generation: 1, rollback: Some(rb1) }
    );

    // Trial with a snapshot ⇒ Rollback it (carrying the trial header + generation); without ⇒
    // AcceptAndClear (first install), carrying the running image's header + generation.
    let rb = staged("r", 2);
    let installed = header("v", 1);
    assert_eq!(
        decide(&BootState::Trial { generation: 1, installed, rollback: Some(rb) }),
        BootDecision::Rollback { snapshot: rb, installed, generation: 1 }
    );
    assert_eq!(
        decide(&BootState::Trial { generation: 1, installed, rollback: None }),
        BootDecision::AcceptAndClear { installed, generation: 1 }
    );
}

/// DR1 (#729): the WDT period is a cross-image contract, not a tunable — the running app's dog
/// must be adoptable by any bootloader (and the bootloader's trial-boot dog by any app), and
/// embassy-nrf adoption requires an exact hardware-config match. A drift here isn't unsafe on
/// its own (adoption fails closed into one unfed period), but it silently re-opens the two #729
/// gaps this constant exists to close — so pin the raw value.
#[test]
fn wdt_timeout_is_pinned() {
    assert_eq!(obc_dfu::WDT_TIMEOUT_TICKS, 786_432); // 24 s × 32768 Hz LFCLK
}

// ==================== verdict() — the boot-outcome matrix (DR2 #730) ====================

/// Build an `Idle` that records `kind` against `gen`, with `installed` set to the given version so
/// the "same-version re-stage" tests can prove the verdict ignores version strings entirely.
fn idle_outcome(installed_ver: &str, kind: OutcomeKind, gen: u32) -> BootState {
    BootState::Idle {
        installed: Some(header(installed_ver, 100)),
        last_outcome: Some(LastOutcome { kind, generation: gen }),
    }
}

/// No marker (`None`) ⇒ no arm was pending ⇒ `Verdict::None`, whatever the page holds. The recorded
/// outcome is just history until a fresh arm overwrites it.
#[test]
fn verdict_idle_without_marker_is_none() {
    for kind in [OutcomeKind::Installed, OutcomeKind::RolledBack, OutcomeKind::StageRejected, OutcomeKind::ArmAbandoned]
    {
        assert_eq!(verdict(&idle_outcome("v1", kind, 5), None), Verdict::None, "{kind:?} + no marker");
    }
    assert_eq!(verdict(&BootState::Idle { installed: None, last_outcome: None }, None), Verdict::None);
}

/// The core matrix: with a marker whose generation matches the recorded outcome, the *outcome* — not
/// any version string — decides. `Installed` ⇒ Confirmed; every other outcome ⇒ Reverted.
#[test]
fn verdict_outcome_governs_with_matching_marker() {
    let gen = 7;
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::Installed, gen), Some(gen)), Verdict::Confirmed);
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::RolledBack, gen), Some(gen)), Verdict::Reverted);
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::StageRejected, gen), Some(gen)), Verdict::Reverted);
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::ArmAbandoned, gen), Some(gen)), Verdict::Reverted);
}

/// The headline bug (DR2 acceptance): re-staging the *currently-running* version and having it
/// rolled back must read as **Reverted**, not the false Confirmed the old version-equality check
/// produced (there, `installed == staged` matched and showed the success toast).
#[test]
fn verdict_same_version_rollback_is_reverted() {
    // `installed` header carries the SAME version string as the staged image would.
    let state = idle_outcome("v3.0.0", OutcomeKind::RolledBack, 1);
    assert_eq!(verdict(&state, Some(1)), Verdict::Reverted, "same-version rollback is a revert, not a confirm");
}

/// The AcceptAndClear twin: a same-version first-install trial accepted ⇒ Confirmed (the outcome is
/// Installed regardless of the version match).
#[test]
fn verdict_same_version_accept_is_confirmed() {
    let state = idle_outcome("v3.0.0", OutcomeKind::Installed, 2);
    assert_eq!(verdict(&state, Some(2)), Verdict::Confirmed);
}

/// A marker present but the outcome's generation is from a different arm (or, below, absent): the
/// verdict cannot prove the staged image is running, so it reports the conservative Reverted rather
/// than a false success.
#[test]
fn verdict_stale_or_missing_outcome_with_marker_is_reverted() {
    // Generation mismatch: outcome belongs to an older arm than the marker.
    assert_eq!(verdict(&idle_outcome("v1", OutcomeKind::Installed, 4), Some(5)), Verdict::Reverted);
    // No recorded outcome at all — a v1→v2 migrated page (decodes to Idle{None,None}) with a marker.
    let migrated = BootState::Idle { installed: None, last_outcome: None };
    assert_eq!(verdict(&migrated, Some(9)), Verdict::Reverted, "migrated v1 page + marker ⇒ conservative revert");
}

/// A `Trial` page means this IS the trial boot — the confirm owns the verdict — regardless of any
/// marker.
#[test]
fn verdict_trial_is_in_progress() {
    let trial = BootState::Trial { generation: 3, installed: header("v", 1), rollback: Some(staged("r", 1)) };
    assert_eq!(verdict(&trial, Some(3)), Verdict::TrialInProgress);
    assert_eq!(verdict(&trial, None), Verdict::TrialInProgress);
}

/// An `Armed` record that survived into the app ⇒ NotStarted, marker-independent (the bootloader
/// never consumed it).
#[test]
fn verdict_armed_is_not_started() {
    let armed = BootState::Armed { generation: 2, update: staged("u", 2), rollback: Some(staged("r", 1)) };
    assert_eq!(verdict(&armed, Some(2)), Verdict::NotStarted);
    assert_eq!(verdict(&armed, None), Verdict::NotStarted);
}

// ==================== v1 read-compatibility (DR2 #730) ====================
//
// The bootloader is flashed once by probe and NOT updated by DFU, so a fielded bootloader keeps
// writing v1 pages after the app updates. The reader must accept v1 (Armed/Trial byte-identical;
// Idle without the outcome field) or the DR2-carrying update would fail its own trial confirm and
// self-revert on every device whose bootloader isn't reflashed in the same sitting.

/// Craft a genuine v1 page from a v2 encode: patch the version field to 1, for `Idle` zero the
/// outcome byte(s) out of the body (a v1 writer's Idle body ends after the installed option — what
/// follows up to the CRC is padding), and re-CRC. `blob_len` is unchanged: the one extra
/// `has_outcome` byte of a v2 `Idle { last_outcome: None }` never crosses a 16-byte line boundary
/// for the possible payload ends (17 or 81).
fn as_v1_page(state: &BootState) -> Vec<u8> {
    let page = state.encode();
    let mut b = page.as_bytes().to_vec();
    b[4..6].copy_from_slice(&1u16.to_le_bytes());
    let blob_len = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    if let BootState::Idle { installed, .. } = state {
        let v1_end = 16 + 1 + if installed.is_some() { 64 } else { 0 };
        for byte in &mut b[v1_end..blob_len - 4] {
            *byte = 0;
        }
    }
    let crc = obc_dfu::crc32(&b[..blob_len - 4]);
    b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
    b
}

/// v1 pages of every variant decode to the right states — `Armed`/`Trial` unchanged (the skew case
/// that matters: an old bootloader's freshly-written v1 `Trial` must be confirmable by the new app),
/// `Idle` with `last_outcome: None`.
#[test]
fn v1_pages_decode_with_v1_semantics() {
    let idle_hdr = BootState::Idle { installed: Some(header("v1.0.0", 800_000)), last_outcome: None };
    assert_eq!(BootState::decode(&as_v1_page(&idle_hdr)), idle_hdr, "v1 Idle with header");

    let idle_none = BootState::Idle { installed: None, last_outcome: None };
    assert_eq!(BootState::decode(&as_v1_page(&idle_none)), idle_none, "v1 Idle without header");

    let armed = BootState::Armed { generation: 7, update: staged("u", 3), rollback: Some(staged("r", 2)) };
    assert_eq!(BootState::decode(&as_v1_page(&armed)), armed, "v1 Armed is byte-identical");

    let trial =
        BootState::Trial { generation: 7, installed: header("v2.0.0", 900_000), rollback: Some(staged("r", 2)) };
    assert_eq!(BootState::decode(&as_v1_page(&trial)), trial, "v1 Trial is byte-identical");
}

/// The v1 Idle decode is gated on the version field, not on the padding happening to parse: bytes
/// after the installed option in a version-1 page are never read as an outcome record, even when
/// they hold a well-formed v2 outcome encoding.
#[test]
fn v1_idle_trailing_bytes_are_not_parsed_as_outcome() {
    // Encode a v2 Idle WITH an outcome, then only patch the version to 1 and re-CRC — the outcome
    // bytes stay in the body, but v1 semantics must ignore them.
    let v2 = BootState::Idle {
        installed: Some(header("v1.0.0", 800_000)),
        last_outcome: Some(LastOutcome { kind: OutcomeKind::RolledBack, generation: 9 }),
    };
    let mut b = v2.encode().as_bytes().to_vec();
    b[4..6].copy_from_slice(&1u16.to_le_bytes());
    let blob_len = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let crc = obc_dfu::crc32(&b[..blob_len - 4]);
    b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(
        BootState::decode(&b),
        BootState::Idle { installed: Some(header("v1.0.0", 800_000)), last_outcome: None },
        "a version-1 page never yields an outcome, whatever its trailing bytes hold"
    );
}

/// Writers emit v2 (regression pin) and any version other than 1 or 2 still falls back to Idle.
#[test]
fn version_field_bounds() {
    let state = BootState::Idle {
        installed: Some(header("v", 10)),
        last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 3 }),
    };
    let page = state.encode();
    assert_eq!(u16::from_le_bytes([page.as_bytes()[4], page.as_bytes()[5]]), 2, "writers always emit v2");
    assert_eq!(BootState::decode(page.as_bytes()), state, "a v2 page decodes (regression)");

    // Any other version — 0, 3, 0xFFFF — is unknown and falls back to Idle, even with a valid CRC.
    for v in [0u16, 3, 0xFFFF] {
        let mut b = page.as_bytes().to_vec();
        b[4..6].copy_from_slice(&v.to_le_bytes());
        let blob_len = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        let crc = obc_dfu::crc32(&b[..blob_len - 4]);
        b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            BootState::decode(&b),
            BootState::Idle { installed: None, last_outcome: None },
            "version {v} must fall back to Idle"
        );
    }
}
