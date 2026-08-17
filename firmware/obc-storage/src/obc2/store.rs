//! `CardStore`: §2's fifth owner, and the only thing that owns a mounted OBC2 volume.
//!
//! §2 of the system contract:
//!
//! > One `CardStore` owns the mounted FAT volume, catalog commit store, reader leases, recovery, and
//! > garbage collection.
//!
//! > CardStore does not grow a union of every domain method: it lends transaction, lease, and
//! > catalog capabilities to one concrete repository at a time.
//!
//! This type is exactly those two sentences. It composes what a mount produced — the media, the
//! resident index, §6.3's slot origin — into one [`KernelTransaction`] with the domain repositories
//! installed as its validator and the commit log installed as its hooks, and it hands out short
//! borrowed views ([`CardStore::routes`], [`trips`](CardStore::trips),
//! [`weather`](CardStore::weather)) for the semantic work. Everything else it exposes is a fact
//! about the store rather than a way around it.
//!
//! ## The admission-lock law
//!
//! **A store lock is never held across card I/O.**
//!
//! The reason is not storage's: it is the BLE stack's. On the nRF54L the MPSL owns hard radio
//! timeslots, and every long synchronous hold in a cooperative executor is time the radio does not
//! get. A single OBC2 operation is not small — this store's own board measurements put one journal
//! stride read at ~14.7 ms, a mount survey at ~3.3 s and §6.3's compaction pass at 715 ms over 30
//! entries — so a lock held "for the duration of the operation" is a lock held for as long as the
//! card feels like taking, and a connection drops.
//!
//! What makes this a law rather than an aspiration is the shape above it. The kernel does not expose
//! an operation; it exposes a *step* — `Claim`, `Append`, `Checkpoint`, `Seal`, `Validate`,
//! `Publish` — and returns to its caller between every one of them, which is where a lock is
//! released and the executor gets its turn. A repository view is bound to one such call by the
//! borrow checker: [`routes`](CardStore::routes) takes `&mut self`, the view holds that borrow, and
//! there is no way to stash it, send it, or carry it across an `await` in the board glue, because
//! doing so would keep `&mut CardStore` alive across the suspension point and the compiler refuses.
//!
//! **Where this is not yet true, and it must be said plainly:** the board's *v1* storage owner takes
//! the opposite posture on purpose — `SharedStoreMutex` in the board crate is documented as being
//! held across `.await` where the BLE object plane needs it, which a `RefCell` could not do. That is
//! v1's arrangement and it is not this store's; reconciling the two is the cutover slice's work, and
//! nothing in this file may be read as saying it has already happened.
//!
//! ## What this owns, and what is still owed
//!
//! Owned here now: the mounted media and its handles, the resident index, the commit journal, the
//! lease table, §6.3's compaction, §4's commit events, and the repository lending.
//!
//! Still owed, and listed rather than implied: the mount/recovery state machine as a *typed status
//! snapshot* (`fat::survey` and `fat::attach` produce the facts, and nothing yet folds them into one
//! observable value), the incremental garbage collector's schedule (`gc::Collector` exists and no
//! owner steps it), the transfer coordinator's session table (#1353), and the resource arbiter.
//! Compaction still runs inline inside a commit, with the 715 ms measurement at its call site and a
//! standing recommendation to move it to a budgeted background pass.

use obc_link::engine::{Command, Outcome, Transaction};
use obc_link::ids::StoreId;
use obc_link::registry::ObjectKind;

use super::commit::{CommitEvent, CommitLog};
use super::index::RamIndex;
use super::repositories::{Capability, DomainRepositories, Routes, StoreHooks, Trips, Weather};
use super::transaction::{KernelMedia, KernelTransaction};

/// The one owner of a mounted OBC2 volume.
///
/// `repr(transparent)` is load-bearing rather than decorative: [`mount_in_place`](Self::mount_in_place)
/// initializes the store through the kernel's own in-place constructor, and that is sound only
/// because this value *is* its transaction in memory.
#[repr(transparent)]
pub struct CardStore<M: KernelMedia> {
    transaction: KernelTransaction<M, DomainRepositories, StoreHooks>,
}

impl<M: KernelMedia> CardStore<M> {
    /// Composes a store over what a mount produced.
    ///
    /// `epoch_base` is the `through_sequence` of the checkpoint `index` was loaded from — §6.3's
    /// slot origin — and getting it wrong is not a performance question: a store that computed
    /// physical slots against the wrong origin would overwrite the record it just wrote.
    pub fn mount(media: M, index: RamIndex, epoch_base: u64) -> Self {
        CardStore {
            transaction: KernelTransaction::mount(
                media,
                DomainRepositories::default(),
                StoreHooks::default(),
                index,
                epoch_base,
            ),
        }
    }

    /// The board's constructor: writes the store into storage the caller already owns.
    ///
    /// The by-value [`mount`](Self::mount) costs 206,080 bytes of transient stack on the nRF54L
    /// against a 51,576-byte residual main stack — it does not fault later, it faults during the
    /// mount — so a device places the store once and never moves it. The projection starts empty and
    /// the caller loads it through [`media_and_index_mut`](Self::media_and_index_mut) and then calls
    /// [`rebind`](Self::rebind); the order is not optional.
    pub fn mount_in_place(slot: &mut core::mem::MaybeUninit<Self>, media: M, store: StoreId) -> &mut Self {
        // SAFETY: `CardStore` is `repr(transparent)` over its transaction, so the two have the same
        // layout and the cast between their `MaybeUninit`s is exact. `mount_in_place` then
        // initializes every field of that transaction through its own safety argument.
        let inner: &mut core::mem::MaybeUninit<KernelTransaction<M, DomainRepositories, StoreHooks>> =
            unsafe { &mut *slot.as_mut_ptr().cast() };
        KernelTransaction::mount_in_place(inner, media, DomainRepositories::default(), StoreHooks::default(), store);
        // SAFETY: the only field is initialized, so the whole value is.
        unsafe { slot.assume_init_mut() }
    }

    /// The media and the resident index together, so a mount can read one into the other.
    pub fn media_and_index_mut(&mut self) -> (&mut M, &mut RamIndex) {
        self.transaction.media_and_index_mut()
    }

    /// Derives every cursor from the index that was just loaded, against §6.3's slot origin.
    pub fn rebind(&mut self, epoch_base: u64) {
        self.transaction.rebind(epoch_base);
    }

    /// The store's identity. A card replacement mints a new one, which is what invalidates every
    /// client link without a filename scan (§3).
    pub fn store_id(&self) -> StoreId {
        self.transaction.store_id()
    }

    /// The transfer engine's effect seam.
    ///
    /// This is how the wire reaches the store, and it is deliberately the *only* way: the engine
    /// hands out one typed [`Command`] at a time and takes back one [`Outcome`], so transport code
    /// cannot select a filename, a validator, a revision or a domain policy — it can only name the
    /// next step of a lifecycle the engine owns.
    pub fn transaction_mut(&mut self) -> &mut KernelTransaction<M, DomainRepositories, StoreHooks> {
        &mut self.transaction
    }

    /// §4's commit log: the retained revision per repository and the coalescing wake.
    pub fn commits(&self) -> &CommitLog {
        &self.transaction.hooks().commits
    }

    /// The same, for a subscriber that takes wakes.
    pub fn commits_mut(&mut self) -> &mut CommitLog {
        &mut self.transaction.hooks_mut().commits
    }

    /// The next repository waiting to wake a consumer, newest revision first (§4).
    ///
    /// This is the whole subscriber API. A consumer calls it until it returns `None`, and for each
    /// event reads the repository's current state — which is why nothing here is a stream and
    /// nothing is durable: the event says *which* repository moved and to *what* revision, and the
    /// answer to "and what does it hold now" is a catalog read, not a replayed history.
    pub fn next_commit(&mut self) -> Option<CommitEvent> {
        self.commits_mut().take()
    }

    /// Whether §6.3's compaction is due before the next commit may be written.
    pub fn compaction_required(&self) -> bool {
        self.transaction.compaction_required()
    }

    /// The routes repository, holding the store for the duration of the call chain.
    pub fn routes(&mut self) -> Routes<'_, M> {
        Routes::new(Capability::new(&mut self.transaction))
    }

    /// The trips repository.
    pub fn trips(&mut self) -> Trips<'_, M> {
        Trips::new(Capability::new(&mut self.transaction))
    }

    /// The weather repository.
    pub fn weather(&mut self) -> Weather<'_, M> {
        Weather::new(Capability::new(&mut self.transaction))
    }

    /// How many heads of one kind the store holds, for a diagnostic that does not want a view.
    pub fn head_count(&self, kind: ObjectKind) -> usize {
        self.transaction.index().heads.iter().filter(|entry| entry.kind == kind.to_u16()).count()
    }

    /// Gives the media back, ending this store's tenancy over it.
    pub fn into_media(self) -> M {
        self.transaction.into_media()
    }
}

impl<M: KernelMedia> Transaction for CardStore<M> {
    fn execute<'s>(&mut self, command: Command<'_>, scratch: &'s mut [u8]) -> Outcome<'s> {
        self.transaction.execute(command, scratch)
    }
}

#[cfg(test)]
mod tests {
    use std::boxed::Box;

    use obc_link::engine::{ClaimIntent, ClaimOutcome, FailureCause, IntentMetadata, PrincipalScope};
    use obc_link::frame::Opcode;
    use obc_link::ids::{LogicalObjectId, OperationId, Revision};
    use obc_link::metadata::{
        MetadataEnvelope, MetadataWriter, Schema, SchemaClass, MAX_CATALOG_ENVELOPE, MAX_PUT_ENVELOPE,
        MAX_REGISTERED_MUTATION_ENVELOPE,
    };
    use obc_link::registry::retention;
    use obc_link::upload::Target;

    use crate::obc2::card::Card;
    use crate::obc2::commit::ChangeKind;
    use crate::obc2::index::RamIndex;
    use crate::obc2::repositories::route::PutIntent;

    use super::*;

    const STORE: StoreId = StoreId::new([0x4D; 16]);
    const PRINCIPAL: PrincipalScope = PrincipalScope::new([0x11; 16]);
    /// `specs/vectors/route-plain.obcr`: one chunk, nine points, no waypoints, named "Vector Loop".
    const ROUTE: &[u8] = include_bytes!("../../../../specs/vectors/route-plain.obcr");

    fn store() -> Box<CardStore<Card>> {
        let (card, model) = Card::initialize(0x0BC2, STORE);
        Box::new(CardStore::mount(card, *RamIndex::project(&model), 0))
    }

    /// A route Put envelope (§4.1: one field, retention).
    fn route_put(buffer: &mut [u8], keep: u8) -> IntentMetadata {
        let mut writer = MetadataWriter::new(buffer).expect("a writer");
        writer.push(0x8001, &[keep]).expect("retention");
        let bytes = writer.finish(ObjectKind::Route, SchemaClass::Put);
        IntentMetadata::of(&MetadataEnvelope::decode(bytes, MAX_PUT_ENVELOPE).expect("canonical")).expect("it fits")
    }

    /// A route patch (§4.2), carrying whichever fields the caller names.
    fn route_patch(buffer: &mut [u8], keep: Option<u8>, name: Option<&str>) -> IntentMetadata {
        let mut writer = MetadataWriter::new(buffer).expect("a writer");
        if let Some(keep) = keep {
            writer.push(0x8001, &[keep]).expect("retention");
        }
        if let Some(name) = name {
            writer.push(0x8003, name.as_bytes()).expect("display name");
        }
        let bytes = writer.finish(ObjectKind::Route, SchemaClass::Patch);
        IntentMetadata::of(&MetadataEnvelope::decode(bytes, MAX_PUT_ENVELOPE).expect("canonical")).expect("it fits")
    }

    fn intent(
        operation: OperationId,
        opcode: Opcode,
        target: Target,
        bytes: &[u8],
        metadata: IntentMetadata,
    ) -> ClaimIntent {
        let uploading = opcode == Opcode::StartUpload;
        ClaimIntent {
            operation_id: operation,
            principal: PRINCIPAL,
            opcode,
            digest: [operation.as_bytes()[0]; 32],
            kind: ObjectKind::Route,
            target,
            declared_length: if uploading { bytes.len() as u64 } else { 0 },
            expected_crc: if uploading { obc_crc::crc32(bytes) } else { 0 },
            metadata,
            target_operation_id: None,
        }
    }

    /// Drives one whole Put through the effect seam, as the engine drives it.
    fn upload(
        store: &mut CardStore<Card>,
        operation: OperationId,
        target: Target,
        bytes: &[u8],
        metadata: IntentMetadata,
    ) -> Result<LogicalObjectId, FailureCause> {
        let mut scratch = [0u8; 512];
        let intent = intent(operation, Opcode::StartUpload, target, bytes, metadata);
        let logical = match store.execute(Command::Claim(intent), &mut scratch) {
            Outcome::Claim(ClaimOutcome::Claimed { logical_object_id, .. }) => logical_object_id,
            other => panic!("the claim was refused: {other:?}"),
        };
        for (index, chunk) in bytes.chunks(64).enumerate() {
            let offset = (index * 64) as u64;
            match store.execute(Command::Append { operation_id: operation, offset, bytes: chunk }, &mut scratch) {
                Outcome::Appended => {}
                other => panic!("an append failed: {other:?}"),
            }
        }
        let sealed = store.execute(
            Command::Seal {
                operation_id: operation,
                declared_length: bytes.len() as u64,
                expected_crc: obc_crc::crc32(bytes),
            },
            &mut scratch,
        );
        assert!(matches!(sealed, Outcome::Sealed), "the seal failed: {sealed:?}");
        match store.execute(Command::Validate { operation_id: operation }, &mut scratch) {
            Outcome::Validated => {}
            Outcome::Failed(cause) => return Err(cause),
            other => panic!("an unexpected validation outcome: {other:?}"),
        }
        match store.execute(Command::Publish { operation_id: operation }, &mut scratch) {
            Outcome::Published(_) => Ok(logical),
            other => panic!("the publication failed: {other:?}"),
        }
    }

    /// The catalog projection one head carries, as bytes.
    fn projection_of(store: &mut CardStore<Card>, logical: LogicalObjectId) -> ([u8; MAX_CATALOG_ENVELOPE], usize) {
        let mut staged = [0u8; MAX_CATALOG_ENVELOPE];
        let len = store.routes().projection(logical, &mut staged).expect("the re-read").expect("a published head");
        (staged, len)
    }

    /// **The whole thread, end to end.** The name comes from the payload, the retention from the
    /// request, and what the head stores is a projection a client's own decoder accepts.
    #[test]
    fn a_route_head_carries_a_projection_derived_from_its_payload_and_its_request() {
        let mut store = store();
        let mut buffer = [0u8; MAX_PUT_ENVELOPE];
        let logical = upload(
            &mut store,
            OperationId::new([0xA1; 16]),
            Target::Create,
            ROUTE,
            route_put(&mut buffer, retention::WEEK),
        )
        .expect("a valid route publishes");

        let (staged, len) = projection_of(&mut store, logical);
        assert!(len > 8, "the head no longer carries the bare reservation: {len} bytes");
        let envelope = MetadataEnvelope::decode(&staged[..len], MAX_CATALOG_ENVELOPE).expect("canonical");
        Schema::lookup(ObjectKind::Route, SchemaClass::Catalog)
            .expect("registered")
            .validate(&envelope)
            .expect("a client's own decoder accepts it");
        assert_eq!(envelope.field(0x0001).and_then(|field| field.as_str()), Some("Vector Loop"));
        assert_eq!(envelope.field(0x0002).and_then(|field| field.as_u8()), Some(retention::WEEK));

        // And the repository reads its own policy back through the one place it is stored.
        assert_eq!(store.routes().retention(logical).expect("a re-read"), Some(retention::WEEK));
        assert_eq!(store.routes().selected(logical).expect("a re-read"), None, "an unheld fact is absent, not false");
        let mut name = [0u8; 48];
        let read = store.routes().display_name(logical, &mut name).expect("a re-read").expect("a name");
        assert_eq!(&name[..read], b"Vector Loop");
    }

    /// The registry gives the route namespace exactly one detail, and this is what earns it.
    #[test]
    fn bytes_that_are_not_a_route_are_refused_in_the_route_namespace() {
        let mut store = store();
        let mut buffer = [0u8; MAX_PUT_ENVELOPE];
        let refused = upload(
            &mut store,
            OperationId::new([0xA2; 16]),
            Target::Create,
            &[0x5A; 512],
            route_put(&mut buffer, retention::NEVER),
        )
        .expect_err("garbage is not a route");
        assert!(
            matches!(refused, FailureCause::SemanticValidation { kind: ObjectKind::Route, detail: 1 }),
            "the refusal was {refused:?}"
        );
        assert_eq!(store.head_count(ObjectKind::Route), 0, "a refused validation publishes nothing");
    }

    /// A truncated route is refused for the same reason a corrupt one is: its header names bytes the
    /// file does not have, and a reader that trusted the offset would fault.
    #[test]
    fn a_header_that_names_bytes_past_the_sealed_length_is_refused() {
        let mut store = store();
        let mut buffer = [0u8; MAX_PUT_ENVELOPE];
        let refused = upload(
            &mut store,
            OperationId::new([0xA3; 16]),
            Target::Create,
            &ROUTE[..160],
            route_put(&mut buffer, retention::NEVER),
        )
        .expect_err("the chunk index is past the end");
        assert!(matches!(refused, FailureCause::SemanticValidation { kind: ObjectKind::Route, detail: 1 }));
    }

    /// §4.2: "every present field is applied" — *applied*, not substituted for the whole projection.
    #[test]
    fn a_metadata_patch_is_applied_to_the_projection_rather_than_replacing_it() {
        let mut store = store();
        let mut buffer = [0u8; MAX_PUT_ENVELOPE];
        let logical = upload(
            &mut store,
            OperationId::new([0xB1; 16]),
            Target::Create,
            ROUTE,
            route_put(&mut buffer, retention::NEVER),
        )
        .expect("the route publishes");
        let revision = store.routes().resolve(logical).expect("a head").revision;

        let mut patch_buffer = [0u8; MAX_PUT_ENVELOPE];
        let patch = route_patch(&mut patch_buffer, Some(retention::MONTH), None);
        let operation = OperationId::new([0xB2; 16]);
        let mut scratch = [0u8; 512];
        let intent = intent(
            operation,
            Opcode::SetMetadata,
            Target::Replace { logical_object_id: logical, expected_revision: revision },
            &[],
            patch,
        );
        assert!(matches!(
            store.execute(Command::Claim(intent), &mut scratch),
            Outcome::Claim(ClaimOutcome::Claimed { .. })
        ));
        assert!(matches!(
            store.execute(Command::Validate { operation_id: operation }, &mut scratch),
            Outcome::Validated
        ));
        assert!(matches!(
            store.execute(Command::Publish { operation_id: operation }, &mut scratch),
            Outcome::Published(_)
        ));

        let (staged, len) = projection_of(&mut store, logical);
        let envelope = MetadataEnvelope::decode(&staged[..len], MAX_CATALOG_ENVELOPE).expect("canonical");
        assert_eq!(envelope.field(0x0002).and_then(|field| field.as_u8()), Some(retention::MONTH), "the patch applied");
        assert_eq!(
            envelope.field(0x0001).and_then(|field| field.as_str()),
            Some("Vector Loop"),
            "the name the payload gave the head survived a patch that never mentioned it"
        );
        let event = store.next_commit().expect("a wake");
        assert_eq!(event.change, ChangeKind::MetadataChanged);
    }

    /// §4: a durable commit wakes its repository, and consecutive ones coalesce.
    #[test]
    fn publications_wake_the_route_repository_and_coalesce_into_the_latest_revision() {
        let mut store = store();
        let mut buffer = [0u8; MAX_PUT_ENVELOPE];
        let first = upload(
            &mut store,
            OperationId::new([0xC1; 16]),
            Target::Create,
            ROUTE,
            route_put(&mut buffer, retention::WEEK),
        )
        .expect("the first route publishes");
        let second = upload(
            &mut store,
            OperationId::new([0xC2; 16]),
            Target::Create,
            ROUTE,
            route_put(&mut buffer, retention::WEEK),
        )
        .expect("the second route publishes");
        assert_ne!(first, second);

        assert_eq!(store.commits().pending(), 1, "two route commits are one wake");
        let event = store.next_commit().expect("a wake");
        assert_eq!(event.kind, ObjectKind::Route);
        assert_eq!(event.change, ChangeKind::Created);
        assert_eq!(event.store, STORE);
        assert_eq!(event.logical_object_id, Some(second), "the wake carries the newest commit");
        assert_eq!(
            event.revision,
            store.routes().revision(),
            "the event's revision is the repository revision a consumer would read"
        );
        assert!(store.next_commit().is_none());
        // The retained revision outlives the wake, which is what a late consumer catches up from.
        assert_eq!(store.commits().latest(ObjectKind::Route).map(|latest| latest.revision), Some(event.revision));
        assert_eq!(store.commits().latest(ObjectKind::Trip), None, "a repository nothing touched has no revision");
    }

    /// §11's preflight: it decides, and it creates nothing.
    #[test]
    fn plan_put_refuses_a_stale_revision_without_creating_state() {
        let mut store = store();
        let mut buffer = [0u8; MAX_PUT_ENVELOPE];
        let logical = upload(
            &mut store,
            OperationId::new([0xD1; 16]),
            Target::Create,
            ROUTE,
            route_put(&mut buffer, retention::WEEK),
        )
        .expect("the route publishes");
        let current = store.routes().resolve(logical).expect("a head").revision;

        let metadata = route_put(&mut buffer, retention::DAY);
        let mut declared = [0u8; MAX_REGISTERED_MUTATION_ENVELOPE];
        declared[..metadata.as_bytes().len()].copy_from_slice(metadata.as_bytes());
        let stale = PutIntent {
            target: Target::Replace { logical_object_id: logical, expected_revision: Revision::new(current.get() + 7) },
            declared_length: ROUTE.len() as u64,
            metadata: declared,
            metadata_len: metadata.as_bytes().len() as u16,
        };
        let refusal = store.routes().plan_put(&stale).expect_err("a stale expectation cannot be planned");
        assert!(
            matches!(refusal, FailureCause::RevisionConflict { current: reported, .. } if reported == current),
            "the refusal reports the authoritative revision: {refusal:?}"
        );

        let admitted =
            PutIntent { target: Target::Replace { logical_object_id: logical, expected_revision: current }, ..stale };
        let plan = store.routes().plan_put(&admitted).expect("the current revision plans");
        assert_eq!(plan.retention, retention::DAY);
        assert_eq!(plan.replaces.map(|head| head.logical_object_id), Some(logical));
        // Nothing above wrote anything: one head still, and one page holds it.
        assert_eq!(store.head_count(ObjectKind::Route), 1);
        let mut page = [crate::obc2::repositories::HeadView {
            logical_object_id: LogicalObjectId::ZERO,
            revision: Revision::ZERO,
            length: 0,
            crc32: 0,
        }; 4];
        assert_eq!(store.routes().list(None, &mut page), 1);
        assert_eq!(page[0].logical_object_id, logical);
        assert_eq!(store.routes().list(Some(logical), &mut page), 0, "the cursor is exclusive");
    }

    /// The route repository in the loop of the **whole wire path**: a real `StartUpload` with a real
    /// Put envelope, real stream frames, a real `FinishUpload`, and a head whose metadata is the
    /// client's own.
    ///
    /// The direct-command tests above prove the repository against the effect seam. This one proves
    /// the seam is reached from the wire: the envelope survives decode, the canonical-intent digest,
    /// the claim, the resident row and the publication, which is every hop the A1 thread has.
    #[test]
    fn a_route_uploaded_over_a_link_publishes_the_metadata_the_client_declared() {
        use obc_link::engine::{DeviceProfile, LinkChannel, LinkContext};
        use obc_link::harness::scenarios::{
            data_frame, decoded, hello, record, route_put as wire_route_put, start_upload,
        };
        use obc_link::harness::{Driver, FakeBleLink};
        use obc_link::hello::{LinkKind, PageKind, Subject, SubjectEntry};
        use obc_link::registry::{schema_version, subject_flags};
        use obc_link::upload::FinishUpload;
        use obc_link::{Request, Response};

        let mut profile = DeviceProfile::new(STORE);
        profile.checkpoint_granule = 1_024;
        assert!(profile.subjects.push(SubjectEntry {
            subject: Subject::Logical(ObjectKind::Route),
            operation_flags: subject_flags::PUT | subject_flags::GET | subject_flags::DELETE,
            policy_flags: 0,
            put_schema_version: schema_version::PUT,
            patch_schema_version: schema_version::PATCH,
            catalog_schema_version: schema_version::CATALOG,
            max_length: 1 << 20,
        }));

        let mut store = store();
        let context = LinkContext::new(LinkKind::Ble, PRINCIPAL, 1);
        let mut driver = Driver::new(FakeBleLink::new(context), profile, &mut *store);
        // `scenarios::negotiate` is written against the scenario `Store` seam, which this store does
        // not implement — it is a *production* store, not a fault-injecting fixture — so the two
        // Hello pages are delivered here directly.
        for (index, page) in [PageKind::Resources, PageKind::Subjects].into_iter().enumerate() {
            driver.link.deliver(LinkChannel::Control, &record(&Request::Hello(hello(page, 0)), index as u32 + 1));
            driver.pump().expect("the hello is answered");
        }

        let mut buffer = [0u8; 32];
        let metadata = wire_route_put(&mut buffer, retention::TWO_WEEKS);
        let operation = OperationId::new([0xE1; 16]);
        let request = start_upload(operation, Target::Create, ROUTE, metadata);
        driver.link.deliver(LinkChannel::Control, &record(&Request::StartUpload(request), 3));
        driver.pump().expect("the upload is accepted");
        let session = driver.engine.live_session().expect("an accepted upload owns a session");

        for (index, chunk) in ROUTE.chunks(128).enumerate() {
            let offset = (index * 128) as u64;
            driver.link.deliver(LinkChannel::Stream, &data_frame(session, offset, chunk));
            driver.pump().expect("the frame is accepted");
        }
        driver
            .link
            .deliver(LinkChannel::Control, &record(&Request::FinishUpload(FinishUpload { session_id: session }), 9));
        driver.pump().expect("the publication answers");

        let logical = match decoded(driver.link.sent(LinkChannel::Control).last().expect("a response")) {
            Response::UploadResult(obc_link::result::ResultEnvelope::Object(result)) => {
                assert_eq!(result.operation_id, operation);
                assert_eq!(result.store_id, STORE);
                result.logical_object_id
            }
            other => panic!("expected a committed object result, got {other:?}"),
        };

        let (staged, len) = projection_of(&mut store, logical);
        let envelope = MetadataEnvelope::decode(&staged[..len], MAX_CATALOG_ENVELOPE).expect("canonical");
        // Base tags: §2.2 makes the critical bit part of the encoding, not of the field's identity.
        assert_eq!(envelope.field(0x0001).and_then(|field| field.as_str()), Some("Vector Loop"));
        assert_eq!(envelope.field(0x0002).and_then(|field| field.as_u8()), Some(retention::TWO_WEEKS));
        assert_eq!(store.next_commit().map(|event| event.change), Some(ChangeKind::Created));
    }

    /// The weather repository derives a different projection from a different source, which is the
    /// point of it being a different type rather than an instance of a shared one.
    #[test]
    fn the_weather_repository_has_its_own_singleton_and_its_own_revision() {
        let mut store = store();
        // §3: the identity is reserved at initialization and exists before any head does.
        let singleton = store.weather().singleton().expect("a reserved singleton");
        assert!(store.weather().head().is_none(), "a reserved identity is not a published bundle");
        assert_eq!(store.weather().revision(), Revision::ZERO);
        assert_eq!(store.weather().answered_request().expect("a re-read"), None);
        // §3: "an ordinary `u64` value, not a sentinel" — including zero.
        let _ = singleton.get();
        assert_eq!(store.trips().count(), 0);
        assert_eq!(store.trips().revision(), Revision::ZERO);
    }
}
