//! Boot-state page codec and `decide()` contract tests. The byte layout they exercise is the
//! handoff between the bootloader and the app; keep it in step with `OBCU_Spec.md`.

use obc_dfu::{
    decide, verdict, BootDecision, BootState, Extent, ImageHeader, LastOutcome, OutcomeKind, StagedRef, Verdict,
    MAX_ENCODED_LEN, MAX_EXTENTS, PAGE_LEN,
};

fn header(tag: &str, len: u32) -> ImageHeader {
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
    StagedRef::new(h, h.image_len, h.image_crc32, &extents(n_extents)).expect("consistent + within MAX_EXTENTS")
}

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
        // Both refs full to MAX_EXTENTS: the largest possible blob.
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
        assert_eq!(BootState::decode(page.as_bytes()), state);
        // The real read is the whole zero-padded RRAM page.
        let mut whole = vec![0u8; PAGE_LEN];
        whole[..page.len()].copy_from_slice(page.as_bytes());
        assert_eq!(BootState::decode(&whole), state, "decodes within a full page read");
    }
}

#[test]
fn generation_visible() {
    let armed = BootState::Armed { generation: 42, update: staged("u", 4), rollback: None };
    assert_eq!(armed.generation(), 42);
    assert_eq!(BootState::decode(armed.encode().as_bytes()).generation(), 42);

    let trial = BootState::Trial { generation: 4_000_000_000, installed: header("v", 1), rollback: None };
    assert_eq!(BootState::decode(trial.encode().as_bytes()).generation(), 4_000_000_000);

    assert_eq!(BootState::Idle { installed: None, last_outcome: None }.generation(), 0);
}

/// A blank or all-ones page decodes to `Idle`.
#[test]
fn blank_page_is_idle() {
    assert_eq!(BootState::decode(&[0u8; PAGE_LEN]), BootState::Idle { installed: None, last_outcome: None });
    assert_eq!(BootState::decode(&[0xFFu8; PAGE_LEN]), BootState::Idle { installed: None, last_outcome: None });
    assert_eq!(BootState::decode(&[]), BootState::Idle { installed: None, last_outcome: None });
    assert_eq!(BootState::decode(&[1, 2, 3]), BootState::Idle { installed: None, last_outcome: None });
}

/// A torn write must fall back to `Idle`, never to a partly decoded install request.
#[test]
fn torn_blob_is_idle() {
    let state = BootState::Armed { generation: 3, update: staged("u", 5), rollback: Some(staged("r", 4)) };
    let page = state.encode();
    let good = page.as_bytes().to_vec();

    // Truncation at every 16-byte line boundary: a half-completed line write.
    let mut off = 16;
    while off < good.len() {
        assert_eq!(
            BootState::decode(&good[..off]),
            BootState::Idle { installed: None, last_outcome: None },
            "truncated at {off}"
        );
        off += 16;
    }

    for off in [0usize, 4, 6, 8, 12, 20, 100, good.len() - 1] {
        let mut torn = good.clone();
        torn[off] ^= 0xFF;
        assert_eq!(BootState::decode(&torn), BootState::Idle { installed: None, last_outcome: None }, "flip at {off}");
    }
}

/// A wild `blob_len` must never read out of bounds.
#[test]
fn bogus_blob_len_is_idle() {
    let page = BootState::Idle { installed: Some(header("v", 10)), last_outcome: None }.encode();
    for len in [0u32, 1, 15, 0xFFFF_FFFF, (PAGE_LEN as u32) + 16] {
        let mut b = page.as_bytes().to_vec();
        b[8..12].copy_from_slice(&len.to_le_bytes());
        assert_eq!(BootState::decode(&b), BootState::Idle { installed: None, last_outcome: None }, "blob_len {len}");
    }
}

/// An extent count past `MAX_EXTENTS` is rejected; it would overrun the fixed-capacity store.
#[test]
fn overlong_extent_count_is_idle() {
    let page = BootState::Armed { generation: 1, update: staged("u", 2), rollback: None }.encode();
    let mut b = page.as_bytes().to_vec();
    // extent_count sits after the 64-byte header, len and crc.
    let count_off = 16 + 64 + 4 + 4;
    b[count_off..count_off + 2].copy_from_slice(&((MAX_EXTENTS as u16) + 1).to_le_bytes());
    // Fix the CRC so only the count check can reject it.
    let blob_len = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    let crc = obc_dfu::crc32(&b[..blob_len - 4]);
    b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(BootState::decode(&b), BootState::Idle { installed: None, last_outcome: None });
}

#[test]
fn staged_ref_extent_cap() {
    let h = header("v", 1);
    assert!(StagedRef::new(h, h.image_len, h.image_crc32, &extents(MAX_EXTENTS)).is_some());
    assert!(StagedRef::new(h, h.image_len, h.image_crc32, &extents(MAX_EXTENTS + 1)).is_none());
}

/// The redundant fields must never diverge from the header, or the installer has two truths.
#[test]
fn staged_ref_rejects_inconsistent_fields() {
    let h = header("v", 500);
    assert!(StagedRef::new(h, h.image_len, h.image_crc32, &extents(1)).is_some());
    assert!(StagedRef::new(h, h.image_len + 1, h.image_crc32, &extents(1)).is_none(), "len mismatch");
    assert!(StagedRef::new(h, h.image_len, h.image_crc32 ^ 1, &extents(1)).is_none(), "crc mismatch");
}

/// A diverging `len` decodes to `Idle` even with the CRC re-fixed: the consistency check itself
/// must reject it.
#[test]
fn inconsistent_staged_len_is_idle() {
    let page = BootState::Armed { generation: 1, update: staged("u", 2), rollback: None }.encode();
    let mut b = page.as_bytes().to_vec();
    // The redundant `len` sits right after the embedded header.
    let len_off = 16 + 64;
    let stored = u32::from_le_bytes([b[len_off], b[len_off + 1], b[len_off + 2], b[len_off + 3]]);
    b[len_off..len_off + 4].copy_from_slice(&(stored + 1).to_le_bytes());
    // Fix the CRC so only the consistency check can reject it.
    let blob_len = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    let crc = obc_dfu::crc32(&b[..blob_len - 4]);
    b[blob_len - 4..blob_len].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(BootState::decode(&b), BootState::Idle { installed: None, last_outcome: None });
}

#[test]
fn decide_matrix() {
    assert_eq!(decide(&BootState::Idle { installed: None, last_outcome: None }), BootDecision::Jump);
    assert_eq!(decide(&BootState::Idle { installed: Some(header("v", 1)), last_outcome: None }), BootDecision::Jump);

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

/// The watchdog period is a cross-image contract, not a tunable: adoption needs an exact
/// hardware-config match, so pin the raw value.
#[test]
fn wdt_timeout_is_pinned() {
    assert_eq!(obc_dfu::WDT_TIMEOUT_TICKS, 786_432); // 24 s × 32768 Hz LFCLK
}

/// `installed` carries the given version, so the same-version tests can prove the verdict ignores
/// version strings.
fn idle_outcome(installed_ver: &str, kind: OutcomeKind, gen: u32) -> BootState {
    BootState::Idle {
        installed: Some(header(installed_ver, 100)),
        last_outcome: Some(LastOutcome { kind, generation: gen }),
    }
}

/// No marker means no arm was pending, whatever the page holds.
#[test]
fn verdict_idle_without_marker_is_none() {
    for kind in [OutcomeKind::Installed, OutcomeKind::RolledBack, OutcomeKind::StageRejected, OutcomeKind::ArmAbandoned]
    {
        assert_eq!(verdict(&idle_outcome("v1", kind, 5), None), Verdict::None, "{kind:?} + no marker");
    }
    assert_eq!(verdict(&BootState::Idle { installed: None, last_outcome: None }, None), Verdict::None);
}

/// With a marker whose generation matches, the recorded outcome decides, not a version string.
#[test]
fn verdict_outcome_governs_with_matching_marker() {
    let gen = 7;
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::Installed, gen), Some(gen)), Verdict::Confirmed);
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::RolledBack, gen), Some(gen)), Verdict::Reverted);
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::StageRejected, gen), Some(gen)), Verdict::Reverted);
    assert_eq!(verdict(&idle_outcome("v2", OutcomeKind::ArmAbandoned, gen), Some(gen)), Verdict::Reverted);
}

/// Re-staging the running version and having it rolled back must read as Reverted. Comparing
/// version strings would call it a success.
#[test]
fn verdict_same_version_rollback_is_reverted() {
    let state = idle_outcome("v3.0.0", OutcomeKind::RolledBack, 1);
    assert_eq!(verdict(&state, Some(1)), Verdict::Reverted, "same-version rollback is a revert, not a confirm");
}

/// A same-version first-install trial that is accepted reads as Confirmed.
#[test]
fn verdict_same_version_accept_is_confirmed() {
    let state = idle_outcome("v3.0.0", OutcomeKind::Installed, 2);
    assert_eq!(verdict(&state, Some(2)), Verdict::Confirmed);
}

/// With a stale or missing outcome the verdict cannot prove the staged image runs, so it reports
/// Reverted rather than a false success.
#[test]
fn verdict_stale_or_missing_outcome_with_marker_is_reverted() {
    assert_eq!(verdict(&idle_outcome("v1", OutcomeKind::Installed, 4), Some(5)), Verdict::Reverted);
    // No recorded outcome at all, as a v1 page decodes.
    let migrated = BootState::Idle { installed: None, last_outcome: None };
    assert_eq!(verdict(&migrated, Some(9)), Verdict::Reverted, "migrated v1 page + marker ⇒ conservative revert");
}

/// A `Trial` page means this is the trial boot, so the confirm owns the verdict.
#[test]
fn verdict_trial_is_in_progress() {
    let trial = BootState::Trial { generation: 3, installed: header("v", 1), rollback: Some(staged("r", 1)) };
    assert_eq!(verdict(&trial, Some(3)), Verdict::TrialInProgress);
    assert_eq!(verdict(&trial, None), Verdict::TrialInProgress);
}

/// An `Armed` record that survived into the app reads as NotStarted, with or without a marker.
#[test]
fn verdict_armed_is_not_started() {
    let armed = BootState::Armed { generation: 2, update: staged("u", 2), rollback: Some(staged("r", 1)) };
    assert_eq!(verdict(&armed, Some(2)), Verdict::NotStarted);
    assert_eq!(verdict(&armed, None), Verdict::NotStarted);
}

/// `running_image` maps every state to the image that actually executes, which the firmware
/// revision string is built from. It is never the staged image.
#[test]
fn running_image_per_state() {
    assert_eq!(
        BootState::Idle { installed: None, last_outcome: None }.running_image(),
        None,
        "a never-installed device names no image — the caller falls back to its build-time string"
    );
    let installed = header("v1.0.0", 800_000);
    assert_eq!(
        BootState::Idle {
            installed: Some(installed),
            last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 4 }),
        }
        .running_image(),
        Some(installed),
        "Idle runs its installed record"
    );
    let trial = header("v2.0.0", 900_000);
    assert_eq!(
        BootState::Trial { generation: 2, installed: trial, rollback: Some(staged("rollback", 2)) }.running_image(),
        Some(trial),
        "a trial boot runs the freshly-installed image, confirmed or not"
    );
    let rollback = staged("v1.0.0", 3);
    assert_eq!(
        BootState::Armed { generation: 7, update: staged("v3.0.0", 2), rollback: Some(rollback) }.running_image(),
        Some(rollback.header),
        "an unconsumed arm is still running the old image the armer snapshotted — never the staged one"
    );
    assert_eq!(
        BootState::Armed { generation: 7, update: staged("v3.0.0", 2), rollback: None }.running_image(),
        None,
        "a first-install arm has no snapshot, so the page names no running image"
    );
}

// The bootloader is flashed once and is not updated by DFU, so a fielded bootloader keeps writing
// v1 pages after the app updates. The reader must accept them, or an update would fail its own
// trial confirm on every device whose bootloader is not reflashed at the same time.

/// Makes a genuine v1 page from a v2 encode: patch the version to 1, zero an `Idle`'s outcome bytes
/// out of the body, and re-CRC. `blob_len` does not change, because the extra `has_outcome` byte of
/// a v2 `Idle` never crosses a 16-byte line boundary.
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

/// v1 pages decode to the right states. The case that matters: an old bootloader's v1 `Trial` must
/// still be confirmable by the new app.
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

/// The v1 `Idle` decode is gated on the version field, so trailing bytes are never read as an
/// outcome record, even when they hold a well-formed one.
#[test]
fn v1_idle_trailing_bytes_are_not_parsed_as_outcome() {
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

/// Writers emit v2, and any version but 1 or 2 falls back to `Idle`.
#[test]
fn version_field_bounds() {
    let state = BootState::Idle {
        installed: Some(header("v", 10)),
        last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 3 }),
    };
    let page = state.encode();
    assert_eq!(u16::from_le_bytes([page.as_bytes()[4], page.as_bytes()[5]]), 2, "writers always emit v2");
    assert_eq!(BootState::decode(page.as_bytes()), state, "a v2 page decodes (regression)");

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
