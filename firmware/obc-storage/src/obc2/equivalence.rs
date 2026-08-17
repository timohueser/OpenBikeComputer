//! The kernel-backed transaction, held to `obc-link`'s own scenario suite.
//!
//! `obc-link` proves its engine against an in-memory transaction. That proof is only worth what the
//! *real* store is worth, so this runs the identical scenarios — every one of them, by the list the
//! engine crate publishes — against a [`KernelTransaction`] over a faulting card. Identical wire
//! behaviour on both backends is the acceptance: a scenario asserts exact bytes, exact revisions and
//! exact identities, so a store that assigned a different LogicalObjectId, reported a different
//! admission revision, retained a result the other did not, or answered a query differently fails
//! here rather than on a device.
//!
//! It is host-only. Nothing in it is reachable from the device image, and the transaction it drives
//! is the same one #1359 will wire to the board's FAT adapter — only [`Card`] is replaced.

use std::boxed::Box;
use std::panic::{self, AssertUnwindSafe};
use std::string::String;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use obc_link::engine::{FailureCause, PrincipalScope};
use obc_link::harness::scenarios::{self, Fault, Fixture, Store};
use obc_link::ids::{GenerationId, LogicalObjectId, OperationId, Revision, StoreId};
use obc_link::registry::ObjectKind;

// The crash matrix below drives a whole connection; nothing outside it needs a link or a request.
#[cfg(test)]
use obc_link::engine::{LinkChannel, LinkContext};
#[cfg(test)]
use obc_link::harness::{Driver, FakeBleLink};
#[cfg(test)]
use obc_link::hello::LinkKind;
#[cfg(test)]
use obc_link::query::{OperationStatus, QueryOperation};
#[cfg(test)]
use obc_link::upload::{FinishUpload, Target};
#[cfg(test)]
use obc_link::{Request, Response};

use super::card::Card;
use super::model::CatalogModel;
use super::transaction::{Hooks, KernelTransaction, Validator};

/// The validator and the fault points, in one value that is installed twice.
///
/// [`Validator`] and [`Hooks`] are separate seams for a good reason — one is a domain's, the other
/// is a harness's — and a test needs both. Implementing both on one type keeps the fixture from
/// having to name two.
#[derive(Debug, Default, Clone, Copy)]
pub struct TestPolicy {
    /// The semantic detail the typed validator refuses with, in the kind's own namespace.
    pub refuse_validation: Option<u16>,
    /// Refuse the next claim in preflight. One-shot, as the fake's is.
    pub refuse_claim: Option<FailureCause>,
    /// Fail the seal.
    pub fail_seal: bool,
    /// Fail the publication.
    pub fail_publication: bool,
    /// Publish a competing revision just before the commit lock. One-shot.
    pub race_publication: bool,
    /// Fail the terminal record an abort writes.
    pub fail_abort: bool,
}

impl Validator for TestPolicy {
    fn validate(&mut self, _kind: ObjectKind, _generation: GenerationId, _length: u64, _crc: u32) -> Result<(), u16> {
        match self.refuse_validation {
            Some(detail) => Err(detail),
            None => Ok(()),
        }
    }
}

impl Hooks for TestPolicy {
    fn admit_claim(&mut self) -> Option<FailureCause> {
        self.refuse_claim.take()
    }

    fn sealing(&mut self) -> Option<FailureCause> {
        self.fail_seal.then_some(FailureCause::Checksum { detail: obc_link::error::detail::checksum::WHOLE_PAYLOAD })
    }

    fn publishing(&mut self) -> Option<FailureCause> {
        self.fail_publication.then_some(FailureCause::MediaIo { detail: obc_link::error::detail::media_io::WRITE })
    }

    fn aborting(&mut self) -> Option<FailureCause> {
        self.fail_abort.then_some(FailureCause::MediaIo { detail: obc_link::error::detail::media_io::WRITE })
    }

    fn races_publication(&mut self) -> bool {
        let raced = self.race_publication;
        self.race_publication = false;
        raced
    }

    fn mint_store_id(&mut self, _previous: StoreId) -> StoreId {
        StoreId::new([0x5A; 16])
    }
}

/// The store a scenario runs against here: the real kernel over a simulated card.
///
/// Boxed because the projection alone is around 56 KiB. On the board the same value is placed once
/// in static storage; on a host it lives behind a pointer so a scenario that builds four of them
/// does not build them on the stack.
pub struct KernelStore(Box<KernelTransaction<Card, TestPolicy, TestPolicy>>);

impl KernelStore {
    /// A freshly initialized, empty store.
    pub fn new(store: StoreId) -> Self {
        KernelStore::seeded(0x0BC2, store)
    }

    /// The same, over a card whose tearing is seeded by `seed`.
    ///
    /// §12's sync model is deliberately nondeterministic — "a failed sync has an uncertain outcome
    /// and is resolved by recovery" — so a cut inside one commits a seeded subset. Sweeping the
    /// seed is how a test reaches both sides of that choice instead of whichever one seed 0x0BC2
    /// happens to produce.
    pub fn seeded(seed: u64, store: StoreId) -> Self {
        let (card, model) = Card::initialize(seed, store);
        KernelStore(Box::new(KernelTransaction::mount(card, TestPolicy::default(), TestPolicy::default(), *model)))
    }

    /// A store over an already-mounted card.
    pub fn over(card: Card, model: CatalogModel) -> Self {
        KernelStore(Box::new(KernelTransaction::mount(card, TestPolicy::default(), TestPolicy::default(), model)))
    }

    /// The transaction underneath, for a test that cuts the medium or remounts it.
    pub fn inner_mut(&mut self) -> &mut KernelTransaction<Card, TestPolicy, TestPolicy> {
        &mut self.0
    }

    /// How many media operations the card has performed. A cut plan is written against this.
    pub fn ops(&mut self) -> u32 {
        self.0.media_mut().media().ops()
    }

    /// Gives the card back, ending this store's tenancy over it.
    pub fn into_card(self) -> Card {
        self.0.into_media()
    }
}

impl obc_link::engine::Transaction for KernelStore {
    fn execute<'s>(
        &mut self,
        command: obc_link::engine::Command<'_>,
        scratch: &'s mut [u8],
    ) -> obc_link::engine::Outcome<'s> {
        self.0.execute(command, scratch)
    }
}

impl Store for KernelStore {
    fn head(&self, kind: ObjectKind, logical_object_id: LogicalObjectId) -> Option<(Revision, u64, u32)> {
        self.0.head(kind, logical_object_id)
    }

    fn payload_is(&mut self, kind: ObjectKind, logical_object_id: LogicalObjectId, expected: &[u8]) -> bool {
        // The stored length is part of the comparison, not just the prefix: a head that is longer
        // than what was uploaded holds bytes nobody declared, and reading only `expected.len()` of
        // it would call that equal.
        let Some((_, length, _)) = self.0.head(kind, logical_object_id) else { return false };
        if length != expected.len() as u64 {
            return false;
        }
        let mut buffer = std::vec![0u8; expected.len()];
        match self.0.read_head(kind, logical_object_id, &mut buffer) {
            Some(read) => read == expected.len() && buffer == expected,
            None => false,
        }
    }

    fn has_lease(&self) -> bool {
        self.0.has_lease()
    }

    fn retains(&self, operation_id: OperationId) -> bool {
        self.0.retains(operation_id)
    }

    fn retained_results(&self) -> usize {
        self.0.retained_results()
    }

    fn publish_local(&mut self, kind: ObjectKind, bytes: &[u8]) -> (LogicalObjectId, Revision) {
        self.0.publish_local(kind, bytes)
    }

    fn retain_local_result(&mut self, operation_id: OperationId) {
        self.0.retain_local_result(operation_id)
    }

    fn claim_install_update(&mut self, operation_id: OperationId, principal: PrincipalScope) {
        self.0.claim_install_update(operation_id, principal)
    }

    fn arm(&mut self, fault: Fault) {
        match fault {
            Fault::FailValidation(detail) => self.0.validator_mut().refuse_validation = Some(detail),
            Fault::FailPublication => self.0.hooks_mut().fail_publication = true,
            Fault::FailSeal => self.0.hooks_mut().fail_seal = true,
            Fault::RacePublication => self.0.hooks_mut().race_publication = true,
            Fault::RefuseClaim(cause) => self.0.hooks_mut().refuse_claim = Some(cause),
            Fault::FailAbort => self.0.hooks_mut().fail_abort = true,
        }
    }

    fn disarm(&mut self) {
        *self.0.validator_mut() = TestPolicy::default();
        *self.0.hooks_mut() = TestPolicy::default();
    }
}

/// The fixture whose store is the kernel-backed transaction.
pub struct Kernel;

impl Fixture for Kernel {
    type Store = KernelStore;

    fn store(&mut self) -> KernelStore {
        KernelStore::new(scenarios::STORE)
    }
}

#[cfg(test)]
impl Kernel {
    /// A BLE driver over a store that already exists, so a crash test can keep one card across
    /// several connections.
    fn driver_over(&mut self, store: KernelStore) -> Driver<FakeBleLink, KernelStore> {
        let context = LinkContext::new(LinkKind::Ble, scenarios::principal(), 1);
        Driver::new(FakeBleLink::new(context), scenarios::profile(), store)
    }
}

/// Runs the whole published suite and reports every scenario that failed, **with its message**.
///
/// The suite is run in one test rather than forty-three so a divergence is reported as a set, but
/// the default panic hook would then interleave forty-three unattributed messages with the runner's
/// own output. The hook is replaced by one that records the message instead, so each failure comes
/// back as `scenario: assertion` — which is the whole reason a name-only report is not enough: the
/// point of the equivalence run is to say *how* the two backends disagreed.
pub fn run_suite() -> Vec<String> {
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&captured);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| String::from(*text))
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| String::from("panicked"));
        let where_ = info.location().map_or_else(String::new, |at| std::format!(" at {}:{}", at.file(), at.line()));
        *sink.lock().expect("the capture mutex") = Some(std::format!("{message}{where_}"));
    }));

    let mut failed = Vec::new();
    for (name, scenario) in scenarios::suite::<Kernel>() {
        *captured.lock().expect("the capture mutex") = None;
        if panic::catch_unwind(AssertUnwindSafe(|| scenario(&mut Kernel))).is_err() {
            let message = captured.lock().expect("the capture mutex").take();
            failed.push(std::format!("{name}: {}", message.unwrap_or_else(|| String::from("panicked"))));
        }
    }
    panic::set_hook(previous);
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_engine_suite_passes_against_the_kernel_backed_transaction() {
        let total = scenarios::suite::<Kernel>().len();
        let failed = run_suite();
        assert!(
            failed.is_empty(),
            "{} of {total} scenarios diverged on the kernel:\n  {}",
            failed.len(),
            failed.join("\n  ")
        );
        // The floor is a tripwire, not a target: `obc-link`'s own parity guard is what proves the
        // list is complete, and this only catches a suite that came back empty.
        assert!(total >= 40, "the suite carries {total} scenarios");
    }

    /// §11: "Failure returns without claiming."
    ///
    /// The one media act §11 and §12 put *before* the claim record is the lazy creation of the
    /// generation's shard directory, and it can fail: a full card, a directory that cannot be made.
    /// If that failure came after the claim, the client would have burned an OperationId on a
    /// request that never started — and §11 makes an identifier spent for ever, so it could not
    /// even retry with the same one.
    #[test]
    fn a_preflight_that_cannot_create_a_shard_burns_no_identifier() {
        let mut store = KernelStore::new(scenarios::STORE);
        store.inner_mut().media_mut().fail_ensure_shards = true;
        let mut driver = Kernel.driver_over(store);
        scenarios::negotiate(&mut driver);

        let bytes = scenarios::payload(512);
        let mut buffer = [0u8; 32];
        let metadata = scenarios::route_put(&mut buffer, 1);
        let request = scenarios::start_upload(scenarios::OP_A, Target::Create, &bytes, metadata);
        driver.link.deliver(LinkChannel::Control, &scenarios::record(&Request::StartUpload(request), 3));
        driver.pump().unwrap();

        let body = scenarios::error_of(driver.link.sent(LinkChannel::Control).last().unwrap());
        assert_eq!(body.category, obc_link::ErrorCategory::MEDIA_IO);
        assert_eq!(body.presence & obc_link::error::presence::DURABLE_CLAIM_EXISTS, 0, "no claim was created");
        assert!(!driver.transaction.retains(scenarios::OP_A), "and none was retained");
        assert!(driver.engine.live_session().is_none());

        // The identifier is still the client's to use: the medium recovers and the same
        // OperationId, with the same intent, is admitted as a fresh claim rather than replayed.
        driver.transaction.inner_mut().media_mut().fail_ensure_shards = false;
        let mut buffer = [0u8; 32];
        let metadata = scenarios::route_put(&mut buffer, 1);
        let request = scenarios::start_upload(scenarios::OP_A, Target::Create, &bytes, metadata);
        driver.link.deliver(LinkChannel::Control, &scenarios::record(&Request::StartUpload(request), 4));
        driver.pump().unwrap();
        match scenarios::decoded(driver.link.sent(LinkChannel::Control).last().unwrap()) {
            Response::UploadAccepted(obc_link::upload::Disposition::Accepted(acceptance)) => {
                assert_eq!(acceptance.flags.bits(), 0, "a fresh claim, not a restart of one that existed");
            }
            other => panic!("expected the acceptance, got {other:?}"),
        }
    }

    /// §5.3's principal digest is 32 bytes and the wire's scope is 16, so the mapping has a
    /// reserved half — and the device-local producer must live outside its image.
    ///
    /// A client whose scope happened to be all zeros would otherwise own every local publication:
    /// §3 decides `QueryOperation`'s authorization by comparing exactly these bytes, so an alias is
    /// not a cosmetic collision, it is a client reading another producer's operations.
    #[test]
    fn a_zero_wire_scope_does_not_alias_the_local_producer() {
        let mut store = KernelStore::new(scenarios::STORE);
        let zero = PrincipalScope::new([0; 16]);
        store.retain_local_result(OperationId::new([0x4c; 16]));
        assert_eq!(
            store.inner_mut().report_for(OperationId::new([0x4c; 16]), zero),
            obc_link::engine::OperationReport::NotAuthorized,
            "an all-zero wire scope is still not the local producer"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The crash matrix, through the whole stack
// ---------------------------------------------------------------------------------------------

/// The uploaded object every crash scenario writes.
#[cfg(test)]
const CUT_OBJECT: usize = 2_048;

#[cfg(test)]
/// Drives one complete `StartUpload -> append -> FinishUpload` over a fake link and hands the store
/// back, so the caller can cut the medium underneath it and then ask what survived.
///
/// This is the full stack: the wire codec, the engine's session and upload machines, the effect
/// seam, the kernel-backed transaction, the journal, the generation writer and the medium.
fn drive_upload(store: KernelStore) -> KernelStore {
    let mut driver = Kernel.driver_over(store);
    scenarios::negotiate(&mut driver);
    let bytes = scenarios::payload(CUT_OBJECT);
    let mut buffer = [0u8; 32];
    let metadata = scenarios::route_put(&mut buffer, 1);
    let request = scenarios::start_upload(scenarios::OP_A, Target::Create, &bytes, metadata);
    driver.link.deliver(LinkChannel::Control, &scenarios::record(&Request::StartUpload(request), 3));
    let _ = driver.pump();
    if let Some(session_id) = driver.engine.live_session() {
        for chunk in bytes.chunks(1_008) {
            let Some(upload) = driver.engine.active_upload() else { break };
            driver.link.deliver(LinkChannel::Stream, &scenarios::data_frame(session_id, upload.next_offset, chunk));
            let _ = driver.pump();
        }
        driver
            .link
            .deliver(LinkChannel::Control, &scenarios::record(&Request::FinishUpload(FinishUpload { session_id }), 4));
        let _ = driver.pump();
    }
    driver.transaction
}

#[cfg(test)]
/// What a reconnected client is told about the operation after the card came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Truth {
    Unknown,
    InProgress,
    Committed,
    Aborted,
}

#[cfg(test)]
/// Reboots, remounts, reconnects, and asks `QueryOperation` — over the wire, not by reading the
/// projection, because what §11 fixes is what the *client* is told.
fn truth_after_reboot(store: KernelStore) -> (Truth, KernelStore) {
    let mut card = store.into_card();
    card.reboot();
    let model = card.mount().expect("a cut card still mounts");
    let store = KernelStore::over(card, *model);
    let mut driver = Kernel.driver_over(store);
    scenarios::negotiate(&mut driver);
    driver.link.deliver(
        LinkChannel::Control,
        &scenarios::record(&Request::QueryOperation(QueryOperation { operation_id: scenarios::OP_A }), 9),
    );
    driver.pump().expect("the reconnected link answers");
    let record = driver.link.sent(LinkChannel::Control).last().expect("an answer").clone();
    let truth = match scenarios::decoded(&record) {
        Response::OperationStatus(OperationStatus::Unknown) => Truth::Unknown,
        Response::OperationStatus(OperationStatus::InProgress(progress)) => {
            // §8.1: the bit is set "only while that session exists". A mount has no sessions, so a
            // claim it recovered can never report one however far its durable phase had got.
            assert_eq!(
                progress.flags & obc_link::query::progress_flags::SESSION_ATTACHED,
                0,
                "a remounted claim reported a session that cannot exist"
            );
            Truth::InProgress
        }
        Response::OperationStatus(OperationStatus::Committed(_)) => Truth::Committed,
        Response::OperationStatus(OperationStatus::Aborted(_)) => Truth::Aborted,
        other => panic!("a query answered with {other:?}"),
    };
    (truth, driver.transaction)
}

#[cfg(test)]
mod crash {
    use super::*;
    use crate::obc2::media::{FaultPlan, When, EVERY_WHEN};

    /// The one-based operation indices of the two syncs that publish a journal record's gate.
    ///
    /// A journal append is body, sync, gate, **sync** (§1's exemption), so every second sync on
    /// `COMMIT.JNL` is the moment a record becomes durable. Deriving them from the medium's own log
    /// rather than counting them by hand is what keeps the oracle below honest when the commit path
    /// gains or loses a step: it would move with the code instead of quietly becoming a lie.
    fn journal_gate_syncs(store: &mut KernelStore) -> Vec<u32> {
        let media = store.inner_mut().media_mut().media();
        assert_eq!(
            media.log().len() as u32,
            media.ops(),
            "an upload performs no counted read, so a log index is an operation index"
        );
        media
            .log()
            .iter()
            .enumerate()
            .filter(|(_, op)| op.file == "COMMIT.JNL" && op.kind == "sync")
            .map(|(index, _)| index as u32 + 1)
            .skip(1)
            .step_by(2)
            .collect()
    }

    /// What a query must answer for a cut at `op`/`when`, given where the two records land.
    ///
    /// Every entry is a single value except at the two gate syncs themselves, where §12 makes the
    /// outcome genuinely uncertain — "a failed sync has an uncertain outcome and is resolved by
    /// recovery", and the medium models that by committing a seeded subset. There, and only there,
    /// the oracle names *both* admissible answers; everywhere else it names one, so a store that
    /// lost a record or invented one fails rather than landing in a permissive set.
    fn expected(op: u32, when: When, claim_sync: u32, commit_sync: u32) -> &'static [Truth] {
        if op == commit_sync {
            return match when {
                When::Before => &[Truth::InProgress],
                When::During => &[Truth::InProgress, Truth::Committed],
                When::After => &[Truth::Committed],
            };
        }
        if op == claim_sync {
            return match when {
                When::Before => &[Truth::Unknown],
                When::During => &[Truth::Unknown, Truth::InProgress],
                When::After => &[Truth::InProgress],
            };
        }
        if op > claim_sync {
            &[Truth::InProgress]
        } else {
            &[Truth::Unknown]
        }
    }

    /// Everything a recovered store must agree about, whichever truth it landed on.
    fn assert_consistent(store: &mut KernelStore, truth: Truth, where_: &str) {
        let head = store.head(ObjectKind::Route, LogicalObjectId::new(1));
        // The two halves of a commit are never observed apart: the head this operation published
        // exists exactly when its retained result does.
        //
        // §8.1 caveat for #1359: `retains` is the *window*, and the window evicts. This holds here
        // because one operation runs and 64 results cannot have displaced it; a store that ran the
        // ring round would need the head-side assertion alone.
        assert_eq!(head.is_some(), truth == Truth::Committed, "{where_}: head and result disagree");
        assert_eq!(
            store.retains(scenarios::OP_A),
            matches!(truth, Truth::Committed | Truth::Aborted),
            "{where_}: a terminal truth without a retained result, or the reverse"
        );
        if truth == Truth::Committed {
            let expected = scenarios::payload(CUT_OBJECT);
            let (_, length, crc) = head.expect("a committed head");
            assert_eq!(length, expected.len() as u64, "{where_}");
            assert_eq!(crc, obc_crc::crc32(&expected), "{where_}");
            assert!(
                store.payload_is(ObjectKind::Route, LogicalObjectId::new(1), &expected),
                "{where_}: the published head's bytes are the ones that were uploaded"
            );
        }
    }

    /// Every cut point of one whole upload, through the whole stack, against a derived oracle.
    ///
    /// `OBC2_Storage_Format.md` §12: "Each recovered image must produce exactly the old state, the
    /// new state, or the explicitly listed in-progress state—never a mixed head and result, reused
    /// ID, leaked draft, released foreign lease, or automatic reformat." A membership test against
    /// all four truths would pass on a store that answered at random, so what is asserted here is
    /// the truth each individual cut *must* produce.
    #[test]
    fn every_cut_of_an_upload_answers_exactly_what_its_cut_point_implies() {
        let mut clean = drive_upload(KernelStore::new(scenarios::STORE));
        let total = clean.ops();
        let syncs = journal_gate_syncs(&mut clean);
        assert_eq!(syncs.len(), 2, "an upload commits exactly two journal records: the claim and the terminal");
        let (claim_sync, commit_sync) = (syncs[0], syncs[1]);
        assert_eq!(commit_sync, total, "the terminal record's gate sync is the last thing an upload does");

        let (truth, mut clean) = truth_after_reboot(clean);
        assert_eq!(truth, Truth::Committed, "an uncut upload is committed");
        assert_consistent(&mut clean, truth, "uncut");

        let mut seen = [0usize; 4];
        for op in 1..=total {
            for when in EVERY_WHEN {
                let mut store = KernelStore::new(scenarios::STORE);
                store.inner_mut().media_mut().media_mut().set_plan(FaultPlan::cut(op, when));
                let store = drive_upload(store);
                let (truth, mut store) = truth_after_reboot(store);
                let admissible = expected(op, when, claim_sync, commit_sync);
                assert!(
                    admissible.contains(&truth),
                    "op {op} {when:?}: answered {truth:?}, and the cut point admits only {admissible:?}"
                );
                match truth {
                    Truth::Unknown => seen[0] += 1,
                    Truth::InProgress => seen[1] += 1,
                    Truth::Committed => seen[2] += 1,
                    Truth::Aborted => seen[3] += 1,
                }
                assert_consistent(&mut store, truth, &std::format!("op {op} {when:?}"));
            }
        }
        assert!(seen[0] > 0, "some cut lands before the claim is durable");
        assert!(seen[1] > 0, "some cut leaves the claim live and unfinished");
        assert!(seen[2] > 0, "some cut lands after the publication is durable");
        // The fourth truth is **not** reachable from a power cut, and saying so is the point: a cut
        // kills the card, so the abort the engine then attempts cannot be made durable either. A
        // durable Aborted needs a medium that fails while staying alive, which is the next test.
        assert_eq!(seen[3], 0, "a power cut cannot produce a durable abort");
    }

    /// The fourth truth, from the only thing that can produce it: a medium that fails and lives.
    ///
    /// A full card refuses one payload write and keeps answering. The engine faults the stream,
    /// durably aborts the restart-only work (§13), and a reconnecting client is told the operation
    /// is terminal — with an `Aborted` body, not a lost claim.
    #[test]
    fn a_write_failure_the_card_survives_reaches_the_fourth_truth() {
        let mut probe = drive_upload(KernelStore::new(scenarios::STORE));
        let payload_write = {
            let media = probe.inner_mut().media_mut().media();
            media
                .log()
                .iter()
                .position(|op| op.kind == "write" && op.file.starts_with("GEN."))
                .expect("an upload writes payload bytes") as u32
                + 1
        };
        drop(probe);

        let mut store = KernelStore::new(scenarios::STORE);
        store
            .inner_mut()
            .media_mut()
            .media_mut()
            .set_plan(FaultPlan { media_full: Some(payload_write), ..FaultPlan::default() });
        let store = drive_upload(store);
        // No reboot: the card never lost power, so this is a reconnect and nothing more.
        let (truth, mut store) = truth_after_reboot(store);
        assert_eq!(truth, Truth::Aborted, "restart-only work that cannot be written is durably aborted");
        assert_consistent(&mut store, truth, "media-full");
    }

    /// A commit torn inside its own gate sync still publishes all of the object or none of it.
    ///
    /// This is the case the matrix above can only sample: §12's sync commits a seeded subset, so
    /// whether the terminal gate lands is a property of the seed. Sweeping seeds until both sides
    /// have been seen is what turns "the oracle allowed either" into "both actually happen, and
    /// both are consistent" — and the committed side of it is a genuinely torn card whose head,
    /// length, CRC and bytes must still agree.
    #[test]
    fn a_terminal_record_torn_inside_its_sync_commits_wholly_or_not_at_all() {
        let mut clean = drive_upload(KernelStore::new(scenarios::STORE));
        let commit_sync = *journal_gate_syncs(&mut clean).last().expect("the terminal gate sync");
        drop(clean);

        let mut committed = 0usize;
        let mut in_progress = 0usize;
        for seed in 1..48u64 {
            let mut store = KernelStore::seeded(seed, scenarios::STORE);
            store.inner_mut().media_mut().media_mut().set_plan(FaultPlan::cut(commit_sync, When::During));
            let store = drive_upload(store);
            let (truth, mut store) = truth_after_reboot(store);
            match truth {
                Truth::Committed => committed += 1,
                Truth::InProgress => in_progress += 1,
                other => panic!("seed {seed}: a torn terminal sync answered {other:?}"),
            }
            assert_consistent(&mut store, truth, &std::format!("seed {seed}"));
        }
        assert!(committed > 0, "no seed produced a torn-but-committed terminal record");
        assert!(in_progress > 0, "no seed produced a torn-and-lost terminal record");
    }
}
