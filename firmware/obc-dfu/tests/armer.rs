//! Host tests for the app-side armer's decision core (S4, #619) — the scan validation matrix,
//! the arm sequencing (snapshot **before** the boot-state page write, asserted on a mock's call
//! log), the generation bump, the first-install no-rollback path, and the trial confirm. The
//! same mock-IO bar as `tests/engine.rs`.

use obc_dfu::armer::{
    arm, confirm_trial, scan, ArmError, ArmIo, ArmTicket, ExtentsError, Rollback, ScanError, StageIo,
};
use obc_dfu::engine::IoError;
use obc_dfu::sig::test_key;
use obc_dfu::{
    crc32, BootState, Extent, ImageHeader, LastOutcome, OutcomeKind, StagedRef, HEADER_LEN, MAX_EXTENTS, MAX_IMAGE_LEN,
    SIG_LEN, SIG_SCHEME_ED25519,
};

// ==================== The staged-file fake ====================

/// An in-memory `UPDATE.BIN`: the file bytes plus a scripted extent resolve and optional
/// injected read failures.
struct FakeStage {
    /// The staged file, or `None` for the missing-file case.
    file: Option<Vec<u8>>,
    /// What `stage_extents` answers.
    extents: Result<Vec<Extent>, ExtentsError>,
    /// Fail every `read_stage` at or past this offset (`u32::MAX` = never).
    fail_reads_from: u32,
}

impl FakeStage {
    /// A happy stage: `image` wrapped in a valid **signed** (OBCU v2) container under the committed
    /// test key, one whole-file extent at block 100. Since #997 an unsigned container is not a happy
    /// stage — `scan` rejects it (see `tests/signature.rs`).
    fn happy(image: &[u8], version: &str) -> (FakeStage, ImageHeader) {
        let header = ImageHeader::new(image, version).signed();
        let mut file = header.encode().to_vec();
        file.extend_from_slice(image);
        file.extend_from_slice(&obc_dfu::sign_image(&test_key::SEED, &header, image));
        let blocks = (file.len() as u32).div_ceil(512);
        (
            FakeStage {
                file: Some(file),
                extents: Ok(vec![Extent { start_block: 100, blocks }]),
                fail_reads_from: u32::MAX,
            },
            header,
        )
    }
}

impl StageIo for FakeStage {
    fn stage_len(&mut self) -> Option<u32> {
        self.file.as_ref().map(|f| f.len() as u32)
    }
    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        if offset >= self.fail_reads_from {
            return Err(IoError);
        }
        let f = self.file.as_ref().ok_or(IoError)?;
        let start = offset as usize;
        let end = start + buf.len();
        if end > f.len() {
            return Err(IoError);
        }
        buf.copy_from_slice(&f[start..end]);
        Ok(())
    }
    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, ExtentsError> {
        let ext = self.extents.clone()?;
        // An over-long scripted chain reports its true count (the resolver contract).
        if ext.len() > MAX_EXTENTS {
            return Err(ExtentsError::TooFragmented { extents: ext.len() as u32 });
        }
        out[..ext.len()].copy_from_slice(&ext);
        Ok(ext.len())
    }
}

fn scan_with(stage: &mut FakeStage) -> Result<StagedRef, ScanError> {
    // A deliberately awkward chunk size so the CRC + signature pass exercises partial-chunk tails.
    let mut chunk = [0u8; 96];
    scan(stage, &mut chunk, &test_key::PUBLIC)
}

// ==================== Scan matrix ====================

#[test]
fn scan_happy_returns_a_coherent_staged_ref() {
    let image: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    let (mut stage, header) = FakeStage::happy(&image, "v1.2.3-7-gabc1234");
    let staged = scan_with(&mut stage).expect("happy stage scans");
    assert_eq!(staged.header, header);
    assert_eq!(staged.len, image.len() as u32);
    assert_eq!(staged.crc32, crc32(&image));
    assert_eq!(staged.extent_count(), 1);
    assert_eq!(staged.extents()[0], Extent { start_block: 100, blocks: (64 + image.len() as u32).div_ceil(512) });
}

#[test]
fn scan_missing_file() {
    let (mut stage, _) = FakeStage::happy(b"img", "v1");
    stage.file = None;
    assert_eq!(scan_with(&mut stage), Err(ScanError::Missing));
}

#[test]
fn scan_rejects_bad_magic_and_torn_header() {
    let (mut stage, _) = FakeStage::happy(b"img", "v1");
    stage.file.as_mut().unwrap()[0] = b'X'; // bad magic
    assert_eq!(scan_with(&mut stage), Err(ScanError::BadHeader));

    let (mut stage, _) = FakeStage::happy(b"img", "v1");
    stage.file.as_mut().unwrap()[8] ^= 0xFF; // payload flip without fixing the header CRC
    assert_eq!(scan_with(&mut stage), Err(ScanError::BadHeader));

    // A file shorter than the 64-byte header can't even be decoded.
    let (mut stage, _) = FakeStage::happy(b"img", "v1");
    stage.file.as_mut().unwrap().truncate(HEADER_LEN - 1);
    assert_eq!(scan_with(&mut stage), Err(ScanError::Truncated));
}

#[test]
fn scan_rejects_bad_image_crc() {
    let image = vec![7u8; 5000];
    let (mut stage, _) = FakeStage::happy(&image, "v1");
    stage.file.as_mut().unwrap()[HEADER_LEN + 4321] ^= 0x01; // flip one body byte
    assert_eq!(scan_with(&mut stage), Err(ScanError::BadCrc));
}

#[test]
fn scan_rejects_oversize_before_any_bulk_read() {
    // Hand-build a header whose CRC is valid but whose image_len is over the slot cap; the file
    // carries no body at all — the scan must reject on the length gate, not try to read.
    let header = ImageHeader {
        image_len: MAX_IMAGE_LEN + 1,
        image_crc32: 0,
        fw_version: [0; 32],
        sig_scheme: SIG_SCHEME_ED25519,
        sig_len: SIG_LEN as u16,
    };
    let mut stage = FakeStage {
        file: Some(header.encode().to_vec()),
        extents: Ok(vec![]),
        fail_reads_from: HEADER_LEN as u32, // any body read would fail loudly
    };
    assert_eq!(scan_with(&mut stage), Err(ScanError::Oversize));
}

#[test]
fn scan_rejects_a_torn_copy() {
    let image = vec![3u8; 10_000];
    let (mut stage, _) = FakeStage::happy(&image, "v1");
    stage.file.as_mut().unwrap().truncate(64 + 9_000); // body shorter than image_len
    assert_eq!(scan_with(&mut stage), Err(ScanError::Truncated));
}

#[test]
fn scan_rejects_too_fragmented_with_the_true_count() {
    let (mut stage, _) = FakeStage::happy(b"image bytes", "v1");
    stage.extents = Err(ExtentsError::TooFragmented { extents: 130 });
    assert_eq!(scan_with(&mut stage), Err(ScanError::TooFragmented { extents: 130 }));

    // A resolver that *returns* an over-long chain (rather than erroring) is caught too.
    let (mut stage, _) = FakeStage::happy(b"image bytes", "v1");
    stage.extents = Ok(vec![Extent { start_block: 1, blocks: 1 }; MAX_EXTENTS + 1]);
    assert_eq!(scan_with(&mut stage), Err(ScanError::TooFragmented { extents: (MAX_EXTENTS + 1) as u32 }));
}

#[test]
fn scan_maps_read_failures_to_io() {
    let image = vec![9u8; 4000];
    let (mut stage, _) = FakeStage::happy(&image, "v1");
    stage.fail_reads_from = 2000; // mid-CRC-pass failure
    assert_eq!(scan_with(&mut stage), Err(ScanError::Io));

    let (mut stage, _) = FakeStage::happy(&image, "v1");
    stage.extents = Err(ExtentsError::Io);
    assert_eq!(scan_with(&mut stage), Err(ScanError::Io));
}

// ==================== Arm sequencing ====================

/// What the mock observed, in order — the sequencing assertion's evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Snapshot(ImageHeader),
    StageBlob,
    WriteState(Box<BootState>),
}

struct FakeArmIo {
    calls: Vec<Call>,
    /// What `snapshot` answers.
    snapshot: Result<Option<StagedRef>, ScanError>,
    /// Whether `stage_boot_blob` fails (#1158).
    stage_fails: bool,
    /// Whether `write_state` fails.
    write_fails: bool,
}

impl FakeArmIo {
    fn new(snapshot: Result<Option<StagedRef>, ScanError>) -> FakeArmIo {
        FakeArmIo { calls: Vec::new(), snapshot, stage_fails: false, write_fails: false }
    }
}

impl ArmIo for FakeArmIo {
    fn snapshot(&mut self, installed: &ImageHeader) -> Result<Option<StagedRef>, ScanError> {
        self.calls.push(Call::Snapshot(*installed));
        self.snapshot
    }
    fn stage_boot_blob(&mut self) -> Result<(), IoError> {
        self.calls.push(Call::StageBlob);
        if self.stage_fails {
            Err(IoError)
        } else {
            Ok(())
        }
    }
    fn write_state(&mut self, state: &BootState) -> Result<(), IoError> {
        self.calls.push(Call::WriteState(Box::new(state.clone())));
        if self.write_fails {
            Err(IoError)
        } else {
            Ok(())
        }
    }
}

fn staged(tag: u8) -> StagedRef {
    let image = vec![tag; 1000];
    let header = ImageHeader::new(&image, "vNEXT");
    StagedRef::new(header, header.image_len, header.image_crc32, &[Extent { start_block: 8, blocks: 3 }]).unwrap()
}

fn installed_header() -> ImageHeader {
    ImageHeader::new(&[0xAAu8; 900], "vOLD")
}

#[test]
fn arm_snapshots_before_the_page_write_and_bumps_the_generation() {
    let update = staged(1);
    let rollback = staged(2);
    let old = installed_header();
    let mut io = FakeArmIo::new(Ok(Some(rollback)));
    let current = BootState::Idle { installed: Some(old), last_outcome: None };

    let ticket = arm(&mut io, &current, update).expect("arm succeeds");
    assert_eq!(ticket, ArmTicket { generation: 1, rollback: Rollback::Snapshot }, "Idle carries generation 0 → 1");

    // THE ordering assertion: the rollback snapshot lands on the card, then the blob stage lands
    // in RRAM (#1158), and only then the boot-state page write — a power cut anywhere before the
    // page write leaves nothing armed, and a valid Armed page implies a staged blob.
    assert_eq!(io.calls.len(), 3);
    assert_eq!(io.calls[0], Call::Snapshot(old));
    assert_eq!(io.calls[1], Call::StageBlob);
    match &io.calls[2] {
        Call::WriteState(s) => {
            assert_eq!(
                **s,
                BootState::Armed { generation: 1, update, rollback: Some(rollback) },
                "the written record carries the update, the snapshot, and the bumped generation"
            );
        }
        other => panic!("expected the page write last, got {other:?}"),
    }
}

#[test]
fn arm_generation_is_old_plus_one_even_from_a_stale_armed_page() {
    // Defensive totality: a non-Idle page can't be live mid-run, but arm() must stay total —
    // no snapshot (no known-installed image), generation still bumps past the stale record.
    let update = staged(1);
    let current = BootState::Armed { generation: 7, update: staged(3), rollback: None };
    let mut io = FakeArmIo::new(Err(ScanError::Io)); // would fail if snapshot were attempted
    let ticket = arm(&mut io, &current, update).expect("arm stays total");
    assert_eq!(ticket.generation, 8);
    assert_eq!(ticket.rollback, Rollback::FirstInstall);
    assert_eq!(io.calls.len(), 2, "no snapshot call for an unknown installed image");
    assert_eq!(io.calls[0], Call::StageBlob, "the blob stage still runs — the install needs the card");
}

#[test]
fn arm_first_install_skips_the_snapshot_and_records_no_rollback() {
    let update = staged(1);
    let mut io = FakeArmIo::new(Err(ScanError::Io)); // must never be consulted
    let ticket = arm(&mut io, &BootState::Idle { installed: None, last_outcome: None }, update).expect("arm succeeds");
    assert_eq!(ticket, ArmTicket { generation: 1, rollback: Rollback::FirstInstall });
    assert_eq!(io.calls.len(), 2, "snapshot skipped on a fresh device");
    assert_eq!(io.calls[0], Call::StageBlob);
    match &io.calls[1] {
        Call::WriteState(s) => {
            assert_eq!(**s, BootState::Armed { generation: 1, update, rollback: None });
        }
        other => panic!("expected the page write after the blob stage, got {other:?}"),
    }
}

#[test]
fn arm_running_mismatch_arms_without_a_rollback_and_says_so() {
    // The snapshot reported the slot no longer matches the installed header (SWD reflash):
    // the arm proceeds, rollback None, flagged for the caller's warning.
    let update = staged(1);
    let mut io = FakeArmIo::new(Ok(None));
    let ticket = arm(&mut io, &BootState::Idle { installed: Some(installed_header()), last_outcome: None }, update)
        .expect("arm succeeds");
    assert_eq!(ticket.rollback, Rollback::RunningMismatch);
    match &io.calls[2] {
        Call::WriteState(s) => assert!(matches!(**s, BootState::Armed { rollback: None, .. })),
        other => panic!("expected the page write last, got {other:?}"),
    }
}

#[test]
fn arm_aborts_on_a_failed_snapshot_without_touching_the_page() {
    let update = staged(1);
    let mut io = FakeArmIo::new(Err(ScanError::Io));
    let err =
        arm(&mut io, &BootState::Idle { installed: Some(installed_header()), last_outcome: None }, update).unwrap_err();
    assert_eq!(err, ArmError::Snapshot(ScanError::Io));
    assert_eq!(io.calls.len(), 1, "the blob stage and the boot-state page are untouched after a failed snapshot");
    assert!(matches!(io.calls[0], Call::Snapshot(_)));
}

#[test]
fn arm_aborts_on_a_failed_blob_stage_without_touching_the_page() {
    // #1158: an Armed page whose blob carve can't be validated would only ever be abandoned next
    // boot — so a failed stage aborts the arm here, page untouched, where the app can say why.
    let update = staged(1);
    let mut io = FakeArmIo::new(Ok(Some(staged(2))));
    io.stage_fails = true;
    let err =
        arm(&mut io, &BootState::Idle { installed: Some(installed_header()), last_outcome: None }, update).unwrap_err();
    assert_eq!(err, ArmError::BlobStage);
    assert_eq!(io.calls.len(), 2, "the boot-state page is untouched after a failed blob stage");
    assert!(matches!(io.calls[0], Call::Snapshot(_)));
    assert_eq!(io.calls[1], Call::StageBlob);
}

#[test]
fn arm_records_the_carried_scan_ref_verbatim() {
    // DR6 (#734): the confirm's single scan produces the StagedRef the arm consumes. Note what
    // this test does NOT claim: "arm never re-reads the stage" is structural, not asserted here —
    // `arm` takes only an `ArmIo` (snapshot + page write), which by construction has no route back
    // to the staged file, so a read-counter on the stage would be vacuously flat. The board-side
    // "one CRC pass before `armed gen=…`" rides on that seam shape plus `arm_update` skipping its
    // fallback scan, and is only observable on glass / via the sim.
    //
    // What this test pins is the carry contract itself: the ref the scan returned feeds `arm` by
    // value (StagedRef is Copy) and lands in the Armed page verbatim — the bootloader's
    // verify-before-erase then checks exactly the image the one scan validated.
    let image: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let (mut stage, header) = FakeStage::happy(&image, "v2.0.0-1-gcarry01");

    // The one scan: the full read + CRC pass, yielding the ref the confirm carries.
    let carried = scan_with(&mut stage).expect("the one scan validates the stage");
    assert_eq!(carried.header, header);

    let mut io = FakeArmIo::new(Ok(Some(staged(2))));
    let current = BootState::Idle { installed: Some(installed_header()), last_outcome: None };
    let ticket = arm(&mut io, &current, carried).expect("arm consumes the carried ref");
    assert_eq!(ticket.rollback, Rollback::Snapshot);

    // The armed record carries exactly the scanned image — same header/len/CRC/extents the one
    // CRC pass validated.
    match &io.calls[2] {
        Call::WriteState(s) => match **s {
            BootState::Armed { update, .. } => {
                assert_eq!(update, carried, "the Armed page records the carried ref verbatim")
            }
            ref other => panic!("expected an Armed page, got {other:?}"),
        },
        other => panic!("expected the page write last, got {other:?}"),
    }
}

#[test]
fn arm_reports_a_failed_page_write() {
    let update = staged(1);
    let mut io = FakeArmIo::new(Ok(Some(staged(2))));
    io.write_fails = true;
    let err =
        arm(&mut io, &BootState::Idle { installed: Some(installed_header()), last_outcome: None }, update).unwrap_err();
    assert_eq!(err, ArmError::StateWrite);
}

// ==================== Trial confirm ====================

#[test]
fn confirm_trial_writes_idle_with_the_installed_header() {
    let installed = installed_header();
    let trial = BootState::Trial { generation: 4, installed, rollback: Some(staged(2)) };
    let (next, hdr) = confirm_trial(&trial).expect("a trial confirms");
    assert_eq!(
        next,
        BootState::Idle {
            installed: Some(installed),
            // The confirm records the accept against the trial's generation (4).
            last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 4 })
        }
    );
    assert_eq!(hdr, installed);

    // Idempotent through the codec: what the confirm writes decodes back to the same Idle.
    let page = next.encode();
    assert_eq!(BootState::decode(page.as_bytes()), next);
}

#[test]
fn confirm_trial_is_a_noop_for_idle_and_armed() {
    assert_eq!(confirm_trial(&BootState::Idle { installed: None, last_outcome: None }), None);
    assert_eq!(confirm_trial(&BootState::Idle { installed: Some(installed_header()), last_outcome: None }), None);
    assert_eq!(confirm_trial(&BootState::Armed { generation: 1, update: staged(1), rollback: None }), None);
}
