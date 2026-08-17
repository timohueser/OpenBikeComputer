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
        let (card, model) = Card::initialize(0x0BC2, store);
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

/// Runs the whole published suite and reports every scenario that failed, by name.
pub fn run_suite() -> Vec<String> {
    let mut failed = Vec::new();
    for (name, scenario) in scenarios::suite::<Kernel>() {
        let hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| scenario(&mut Kernel)));
        panic::set_hook(hook);
        if outcome.is_err() {
            failed.push(String::from(name));
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_engine_suite_passes_against_the_kernel_backed_transaction() {
        let total = scenarios::suite::<Kernel>().len();
        let failed = run_suite();
        assert!(failed.is_empty(), "{} of {total} scenarios diverged on the kernel: {failed:?}", failed.len());
        assert!(total >= 40, "the suite carries {total} scenarios");
    }
}

// ---------------------------------------------------------------------------------------------
// The crash matrix, through the whole stack
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
/// Drives one complete `StartUpload -> append -> FinishUpload` over a fake link and hands the store
/// back, so the caller can cut the medium underneath it and then ask what survived.
///
/// This is the full stack: the wire codec, the engine's session and upload machines, the effect
/// seam, the kernel-backed transaction, the journal, the generation writer and the medium. What it
/// proves is not that the flow works — the suite above proves that — but that **however it is cut**,
/// a remount and a reconnect answer `QueryOperation` with one of §11's four truths and never with a
/// mixed state.
fn drive_upload(store: KernelStore) -> KernelStore {
    let mut driver = Kernel.driver_over(store);
    scenarios::negotiate(&mut driver);
    let bytes = scenarios::payload(2_048);
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
        Response::OperationStatus(OperationStatus::InProgress(_)) => Truth::InProgress,
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

    /// Every cut point of one whole upload, through the whole stack.
    ///
    /// `OBC2_Storage_Format.md` §12: "Each recovered image must produce exactly the old state, the
    /// new state, or the explicitly listed in-progress state—never a mixed head and result, reused
    /// ID, leaked draft, released foreign lease, or automatic reformat." This enumerates the cut
    /// points of the sequence a real `StartUpload` performs and holds every one of them to that.
    #[test]
    fn every_cut_of_an_upload_leaves_a_query_one_of_the_four_truths() {
        let mut clean = drive_upload(KernelStore::new(scenarios::STORE));
        let total = clean.ops();
        assert!(total >= 18, "an upload performs {total} media operations: 2 journal commits, 3 payload writes, and the sealed WORK slot");
        let (truth, _) = truth_after_reboot(clean);
        assert_eq!(truth, Truth::Committed, "an uncut upload is committed");

        let mut seen = [0usize; 4];
        for op in 1..=total {
            for when in EVERY_WHEN {
                let mut store = KernelStore::new(scenarios::STORE);
                store.inner_mut().media_mut().media_mut().set_plan(FaultPlan::cut(op, when));
                let store = drive_upload(store);
                let (truth, store) = truth_after_reboot(store);
                match truth {
                    Truth::Unknown => seen[0] += 1,
                    Truth::InProgress => seen[1] += 1,
                    Truth::Committed => seen[2] += 1,
                    Truth::Aborted => seen[3] += 1,
                }
                // The two halves of a commit are never observed apart: a retained result exists if
                // and only if the head it published does.
                let head = store.head(ObjectKind::Route, LogicalObjectId::new(1));
                match truth {
                    Truth::Committed => assert!(head.is_some(), "op {op} {when:?}: committed with no head"),
                    _ => assert!(head.is_none(), "op {op} {when:?}: a head without a committed result"),
                }
            }
        }
        assert!(seen[0] > 0, "some cut lands before the claim is durable");
        assert!(seen[1] > 0, "some cut leaves the claim live and unfinished");
        assert!(seen[2] > 0, "some cut lands after the publication is durable");
    }

    /// A cut inside the seal never yields a sealed generation the payload cannot back.
    ///
    /// §7's reachability filter is what makes that true, and this reaches it through the wire path
    /// rather than through the writer's own tests: the client finishes, the medium dies inside the
    /// seal, and the reconnected client is told the operation is still in progress or already
    /// terminal — never that it committed bytes the card does not hold.
    #[test]
    fn a_cut_inside_the_finish_chain_never_publishes_bytes_the_card_cannot_back() {
        let mut clean = drive_upload(KernelStore::new(scenarios::STORE));
        let total = clean.ops();
        drop(clean);
        // The finish chain is the tail of the sequence: seal, then the terminal commit.
        for op in (total.saturating_sub(12))..=total {
            for when in [When::During, When::After] {
                let mut store = KernelStore::new(scenarios::STORE);
                store.inner_mut().media_mut().media_mut().set_plan(FaultPlan::cut(op, when));
                let store = drive_upload(store);
                let (truth, mut store) = truth_after_reboot(store);
                if truth != Truth::Committed {
                    continue;
                }
                let (_, length, crc) =
                    store.head(ObjectKind::Route, LogicalObjectId::new(1)).expect("a committed head");
                let expected = scenarios::payload(2_048);
                assert_eq!(length, expected.len() as u64, "op {op} {when:?}");
                assert_eq!(crc, obc_crc::crc32(&expected), "op {op} {when:?}");
                assert!(
                    store.payload_is(ObjectKind::Route, LogicalObjectId::new(1), &expected),
                    "op {op} {when:?}: the published head's bytes are the ones that were uploaded"
                );
            }
        }
    }
}
