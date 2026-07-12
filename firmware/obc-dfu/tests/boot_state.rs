//! Boot-state page codec + `decide()` contract tests (host).
//!
//! The byte layout these exercise is the bootloader↔app handoff, documented in `OBCU_Spec.md` §2 —
//! keep the two in lockstep.

use obc_dfu::{
    decide, BootDecision, BootState, Extent, ImageHeader, StagedRef, MAX_ENCODED_LEN, MAX_EXTENTS, PAGE_LEN,
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
        BootState::Idle { installed: None },
        BootState::Idle { installed: Some(header("v1.0.0", 800_000)) },
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

    assert_eq!(BootState::Idle { installed: None }.generation(), 0);
}

/// A blank / all-ones page (a freshly erased or never-written RRAM region) decodes to `Idle`.
#[test]
fn blank_page_is_idle() {
    assert_eq!(BootState::decode(&[0u8; PAGE_LEN]), BootState::Idle { installed: None });
    assert_eq!(BootState::decode(&[0xFFu8; PAGE_LEN]), BootState::Idle { installed: None });
    assert_eq!(BootState::decode(&[]), BootState::Idle { installed: None });
    assert_eq!(BootState::decode(&[1, 2, 3]), BootState::Idle { installed: None });
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
        assert_eq!(BootState::decode(&good[..off]), BootState::Idle { installed: None }, "truncated at {off}");
        off += 16;
    }

    // A single flipped byte at a spread of offsets (magic, version, tag, blob_len, generation, deep in
    // the payload, in the trailing CRC) must all fail.
    for off in [0usize, 4, 6, 8, 12, 20, 100, good.len() - 1] {
        let mut torn = good.clone();
        torn[off] ^= 0xFF;
        assert_eq!(BootState::decode(&torn), BootState::Idle { installed: None }, "flip at {off}");
    }
}

/// A wild `blob_len` field must never trip an out-of-bounds read; it just decodes to `Idle`.
#[test]
fn bogus_blob_len_is_idle() {
    let page = BootState::Idle { installed: Some(header("v", 10)) }.encode();
    for len in [0u32, 1, 15, 0xFFFF_FFFF, (PAGE_LEN as u32) + 16] {
        let mut b = page.as_bytes().to_vec();
        b[8..12].copy_from_slice(&len.to_le_bytes());
        assert_eq!(BootState::decode(&b), BootState::Idle { installed: None }, "blob_len {len}");
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
    assert_eq!(BootState::decode(&b), BootState::Idle { installed: None });
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
    assert_eq!(BootState::decode(&b), BootState::Idle { installed: None });
}

/// The full `decide()` matrix (epic #615 invariant 3).
#[test]
fn decide_matrix() {
    // Idle ⇒ Jump (with or without an installed header).
    assert_eq!(decide(&BootState::Idle { installed: None }), BootDecision::Jump);
    assert_eq!(decide(&BootState::Idle { installed: Some(header("v", 1)) }), BootDecision::Jump);

    // Armed ⇒ Install the staged update (regardless of a rollback snapshot).
    let up = staged("u", 3);
    assert_eq!(decide(&BootState::Armed { generation: 1, update: up, rollback: None }), BootDecision::Install(up));
    assert_eq!(
        decide(&BootState::Armed { generation: 1, update: up, rollback: Some(staged("r", 1)) }),
        BootDecision::Install(up)
    );

    // Trial with a snapshot ⇒ Rollback it; without ⇒ AcceptAndClear (first install).
    let rb = staged("r", 2);
    assert_eq!(
        decide(&BootState::Trial { generation: 1, installed: header("v", 1), rollback: Some(rb) }),
        BootDecision::Rollback(rb)
    );
    assert_eq!(
        decide(&BootState::Trial { generation: 1, installed: header("v", 1), rollback: None }),
        BootDecision::AcceptAndClear
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
