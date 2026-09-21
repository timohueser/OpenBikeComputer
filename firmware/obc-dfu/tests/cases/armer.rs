//! Host tests for the armer: the scan validation matrix, the arm sequencing asserted on a mock's
//! call log, the generation bump, the first-install path, and the trial confirm.

use obc_dfu::armer::{
    arm, confirm_trial, scan, ArmError, ArmIo, ArmTicket, ExtentsError, Rollback, ScanError, StageIo,
};
use obc_dfu::engine::IoError;
use obc_dfu::sig::test_key;
use obc_dfu::{
    crc32, BootState, Extent, ImageHeader, LastOutcome, OutcomeKind, StagedRef, HEADER_LEN, MAX_EXTENTS, MAX_IMAGE_LEN,
    SIG_LEN, SIG_SCHEME_ED25519,
};

/// An in-memory staged package: the object's bytes, a scripted extent resolve, and injected read
/// failures.
struct FakeStage {
    file: Option<Vec<u8>>,
    extents: Result<Vec<Extent>, ExtentsError>,
    /// Fail every `read_stage` at or past this offset; `u32::MAX` never fails.
    fail_reads_from: u32,
}

impl FakeStage {
    /// `image` wrapped in a signed container under the committed test key, as one whole-object
    /// extent. An unsigned container is not a happy stage; `scan` rejects it.
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
        // An over-long scripted chain reports its true count.
        if ext.len() > MAX_EXTENTS {
            return Err(ExtentsError::TooFragmented { extents: ext.len() as u32 });
        }
        out[..ext.len()].copy_from_slice(&ext);
        Ok(ext.len())
    }
}

fn scan_with(stage: &mut FakeStage) -> Result<StagedRef, ScanError> {
    // An awkward chunk size, so the CRC and signature pass gets partial-chunk tails.
    let mut chunk = [0u8; 96];
    scan(stage, &mut chunk, &test_key::PUBLIC)
}

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
    // A valid header CRC, an `image_len` over the cap, and no body: the scan must reject on the
    // length gate instead of reading.
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

    // A resolver that returns an over-long chain, instead of erroring, is caught too.
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

/// What the mock observed, in order: the evidence for the sequencing assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    StageBlob,
    WriteState(Box<BootState>),
}

#[derive(Default)]
struct FakeArmIo {
    calls: Vec<Call>,
    stage_fails: bool,
    write_fails: bool,
}

impl FakeArmIo {
    fn new() -> FakeArmIo {
        FakeArmIo::default()
    }
}

impl ArmIo for FakeArmIo {
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
fn arm_stages_the_blob_before_the_page_write_and_bumps_the_generation() {
    let update = staged(1);
    let rollback = staged(2);
    let old = installed_header();
    let mut io = FakeArmIo::new();
    let current = BootState::Idle { installed: Some(old), last_outcome: None };

    let ticket = arm(&mut io, &current, update, Some(rollback)).expect("arm succeeds");
    assert_eq!(ticket, ArmTicket { generation: 1, rollback: Rollback::Snapshot }, "Idle carries generation 0 → 1");

    // The ordering assertion: the blob stage, then the page write. The caller's rollback is already
    // on the card, so a power cut before the page write leaves nothing armed, and a valid Armed page
    // implies a staged blob.
    assert_eq!(io.calls.len(), 2);
    assert_eq!(io.calls[0], Call::StageBlob);
    match &io.calls[1] {
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
    // A non-Idle page cannot be live mid-run, but `arm` must stay total: no snapshot, and the
    // generation still bumps past the stale record.
    let update = staged(1);
    let current = BootState::Armed { generation: 7, update: staged(3), rollback: None };
    let mut io = FakeArmIo::new();
    let ticket = arm(&mut io, &current, update, Some(staged(2))).expect("arm stays total");
    assert_eq!(ticket.generation, 8);
    assert_eq!(ticket.rollback, Rollback::FirstInstall);
    assert_eq!(io.calls.len(), 2);
    assert_eq!(io.calls[0], Call::StageBlob, "the blob stage still runs — the install needs the card");
    match &io.calls[1] {
        Call::WriteState(s) => assert!(
            matches!(**s, BootState::Armed { rollback: None, .. }),
            "a rollback the page never implied is dropped rather than recorded"
        ),
        other => panic!("expected the page write last, got {other:?}"),
    }
}

#[test]
fn arm_first_install_records_no_rollback() {
    let update = staged(1);
    let mut io = FakeArmIo::new();
    let ticket =
        arm(&mut io, &BootState::Idle { installed: None, last_outcome: None }, update, None).expect("arm succeeds");
    assert_eq!(ticket, ArmTicket { generation: 1, rollback: Rollback::FirstInstall });
    assert_eq!(io.calls.len(), 2);
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
    // The slot no longer matches the installed header, so the arm proceeds without a rollback and
    // flags it for the caller.
    let update = staged(1);
    let mut io = FakeArmIo::new();
    let ticket =
        arm(&mut io, &BootState::Idle { installed: Some(installed_header()), last_outcome: None }, update, None)
            .expect("arm succeeds");
    assert_eq!(ticket.rollback, Rollback::RunningMismatch);
    match &io.calls[1] {
        Call::WriteState(s) => assert!(matches!(**s, BootState::Armed { rollback: None, .. })),
        other => panic!("expected the page write last, got {other:?}"),
    }
}

#[test]
fn arm_aborts_on_a_failed_blob_stage_without_touching_the_page() {
    // An Armed page whose blob carve cannot be validated would only be abandoned on the next
    // boot, so a failed stage aborts the arm here, where the app can say why.
    let update = staged(1);
    let mut io = FakeArmIo::new();
    io.stage_fails = true;
    let err = arm(
        &mut io,
        &BootState::Idle { installed: Some(installed_header()), last_outcome: None },
        update,
        Some(staged(2)),
    )
    .unwrap_err();
    assert_eq!(err, ArmError::BlobStage);
    assert_eq!(io.calls.len(), 1, "the boot-state page is untouched after a failed blob stage");
    assert_eq!(io.calls[0], Call::StageBlob);
}

#[test]
fn arm_records_the_carried_scan_ref_verbatim() {
    // The carry contract: the ref one scan returned feeds `arm` by value and lands in the Armed
    // page verbatim, so verify-before-erase checks exactly the image that scan validated. That
    // `arm` cannot re-read the stage is structural: `ArmIo` has no route back to the file.
    let image: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    let (mut stage, header) = FakeStage::happy(&image, "v2.0.0-1-gcarry01");

    let carried = scan_with(&mut stage).expect("the one scan validates the stage");
    assert_eq!(carried.header, header);

    let mut io = FakeArmIo::new();
    let current = BootState::Idle { installed: Some(installed_header()), last_outcome: None };
    let ticket = arm(&mut io, &current, carried, Some(staged(2))).expect("arm consumes the carried ref");
    assert_eq!(ticket.rollback, Rollback::Snapshot);

    match &io.calls[1] {
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
    let mut io = FakeArmIo::new();
    io.write_fails = true;
    let err = arm(
        &mut io,
        &BootState::Idle { installed: Some(installed_header()), last_outcome: None },
        update,
        Some(staged(2)),
    )
    .unwrap_err();
    assert_eq!(err, ArmError::StateWrite);
}

#[test]
fn confirm_trial_writes_idle_with_the_installed_header() {
    let installed = installed_header();
    let trial = BootState::Trial { generation: 4, installed, rollback: Some(staged(2)) };
    let (next, hdr) = confirm_trial(&trial).expect("a trial confirms");
    assert_eq!(
        next,
        BootState::Idle {
            installed: Some(installed),
            last_outcome: Some(LastOutcome { kind: OutcomeKind::Installed, generation: 4 })
        }
    );
    assert_eq!(hdr, installed);

    // What the confirm writes decodes back to the same Idle.
    let page = next.encode();
    assert_eq!(BootState::decode(page.as_bytes()), next);
}

#[test]
fn confirm_trial_is_a_noop_for_idle_and_armed() {
    assert_eq!(confirm_trial(&BootState::Idle { installed: None, last_outcome: None }), None);
    assert_eq!(confirm_trial(&BootState::Idle { installed: Some(installed_header()), last_outcome: None }), None);
    assert_eq!(confirm_trial(&BootState::Armed { generation: 1, update: staged(1), rollback: None }), None);
}
