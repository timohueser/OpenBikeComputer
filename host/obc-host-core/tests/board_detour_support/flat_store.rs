//! Deterministic ticket transport backed by the real FlatStore writer operations.
use obc_storage::flat::*;
use std::cell::{Cell, RefCell};
pub type FlatCard = &'static obc_storage::flat::sim::FaultOnce<&'static obc_storage::flat::sim::SparseDisk>;
type Answer = Result<Outcome, StoreError>;
#[derive(Default)]
pub struct Reply(RefCell<Option<(Ticket, Answer)>>);
impl Reply {
    pub fn signaled(&self) -> bool {
        self.0.borrow().is_some()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Allocate,
    Write,
    Seal,
    ReleaseSealed,
    Publish,
    Remove,
    Cancel,
    Close,
}
pub enum Request {
    Allocate {
        bytes: u64,
    },
    WriteComputedRoute {
        allocation: Allocation,
        bytes: &'static [u8],
        header: &'static [u8],
    },
    Seal {
        allocation: Allocation,
        out: &'static mut Option<SealedAllocation<'static>>,
    },
    ReleaseSealed {
        sealed: SealedAllocation<'static>,
    },
    PublishComputedRoute {
        allocation: Allocation,
        name: DisplayName,
        original: Option<(ObjectId, Revision)>,
        built_day: bool,
    },
    RemoveComputedRoute {
        id: ObjectId,
        revision: Revision,
    },
    Cancel {
        allocation: Allocation,
    },
    Close {
        handle: Handle,
    },
}
impl Request {
    fn kind(&self) -> Kind {
        match self {
            Self::Allocate { .. } => Kind::Allocate,
            Self::WriteComputedRoute { .. } => Kind::Write,
            Self::Seal { .. } => Kind::Seal,
            Self::ReleaseSealed { .. } => Kind::ReleaseSealed,
            Self::PublishComputedRoute { .. } => Kind::Publish,
            Self::RemoveComputedRoute { .. } => Kind::Remove,
            Self::Cancel { .. } => Kind::Cancel,
            Self::Close { .. } => Kind::Close,
        }
    }
}
pub enum Outcome {
    Allocated(Allocation),
    Wrote(Allocation),
    Done,
    Published(ObjectId),
}
struct Job {
    request: Request,
    reply: &'static Reply,
    ticket: Ticket,
}
pub struct Transport {
    store: &'static FlatStore<FlatCard>,
    job: RefCell<Option<Job>>,
    next: Cell<u32>,
    pub full: Cell<bool>,
    pub fail: Cell<Option<Kind>>,
    pub completed: RefCell<Vec<Kind>>,
}
#[derive(Clone, Copy)]
pub struct Writer(&'static Transport);
impl Writer {
    pub fn new(store: &'static FlatStore<FlatCard>) -> Self {
        Self(Box::leak(Box::new(Transport {
            store,
            job: RefCell::new(None),
            next: Cell::new(0),
            full: Cell::new(false),
            fail: Cell::new(None),
            completed: RefCell::new(Vec::new()),
        })))
    }
    pub fn transport(self) -> &'static Transport {
        self.0
    }
    pub fn pending(self) -> Option<Kind> {
        self.0.job.borrow().as_ref().map(|j| j.request.kind())
    }
    pub fn try_call(&self, request: Request, reply: &'static Reply) -> Result<Ticket, ()> {
        self.try_call_owned(request, reply).map_err(|_| ())
    }
    pub fn try_call_owned(&self, request: Request, reply: &'static Reply) -> Result<Ticket, Request> {
        if self.0.full.get() || self.0.job.borrow().is_some() {
            return Err(request);
        }
        let ticket = Ticket(self.0.next.get() + 1);
        self.0.next.set(ticket.0);
        *self.0.job.borrow_mut() = Some(Job { request, reply, ticket });
        Ok(ticket)
    }
    pub fn try_result(&self, ticket: Ticket, reply: &'static Reply) -> Option<Answer> {
        let (answered, result) = reply.0.borrow_mut().take()?;
        assert_eq!(ticket, answered);
        Some(result)
    }
    pub fn complete(self) {
        let job = self.0.job.borrow_mut().take().expect("pending ticket");
        let kind = job.request.kind();
        if self.0.fail.get() == Some(kind) {
            self.0.fail.set(None);
            assert!(matches!(kind, Kind::Write | Kind::Seal | Kind::Publish));
            self.0.store.device().fault_next(obc_storage::flat::sim::MediaOp::Write);
        }
        let result = execute(self.0.store, job.request);
        self.0.completed.borrow_mut().push(kind);
        assert!(job.reply.0.borrow_mut().replace((job.ticket, result)).is_none());
    }
}
fn execute(store: &'static FlatStore<FlatCard>, request: Request) -> Answer {
    match request {
        Request::Allocate { bytes } => store.allocate(bytes).map(Outcome::Allocated),
        Request::WriteComputedRoute { mut allocation, bytes, header } => {
            if !header.is_empty() {
                store.patch_allocation(&allocation, 0, header)?;
            }
            store.write(&mut allocation, bytes)?;
            Ok(Outcome::Wrote(allocation))
        }
        Request::Seal { allocation, out } => {
            *out = Some(store.seal(allocation)?);
            Ok(Outcome::Done)
        }
        Request::ReleaseSealed { sealed } => {
            store.release_sealed(sealed).map_err(|_| StoreError::Invalid)?;
            Ok(Outcome::Done)
        }
        Request::PublishComputedRoute { allocation, name, original, built_day } => {
            assert!(!built_day, "these tests publish detours, not built trip days");
            if original.is_some_and(|(id, revision)| store.current_revision(id) != Ok(Some(revision)))
                || !planner_map_current()
            {
                return Err(StoreError::NotFound);
            }
            if !store.has_commit_capacity(2) {
                return Err(StoreError::ReadOnly);
            }
            let id = store.next_object_id();
            let meta = EntryMeta {
                added_at_utc: 0,
                id,
                revision: Revision(1),
                kind: ObjectKind::Route,
                flags: EntryFlags::NONE,
                payload_len: allocation.written_bytes(),
                payload_crc: store.allocation_crc(&allocation)?,
                name,
            };
            store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])?;
            Ok(Outcome::Published(id))
        }
        Request::RemoveComputedRoute { id, revision } => {
            store.commit(&[Mutation::Remove { id, revision }])?;
            Ok(Outcome::Done)
        }
        Request::Cancel { allocation } => {
            store.cancel(allocation);
            Ok(Outcome::Done)
        }
        Request::Close { handle } => {
            store.close(handle);
            Ok(Outcome::Done)
        }
    }
}
type MountedSources = (&'static FlatStore<FlatCard>, ObjectId, Revision, ObjectId, Revision);
thread_local! {
    static FINGERPRINT_READS: Cell<u32> = const { Cell::new(0) };
    static SOURCES: RefCell<Option<MountedSources>> = const { RefCell::new(None) };
}
pub fn mount_sources(
    store: &'static FlatStore<FlatCard>,
    original: &StoreSource<'_, FlatCard>,
    map: &StoreSource<'_, FlatCard>,
) {
    SOURCES.with(|s| *s.borrow_mut() = Some((store, original.id(), original.revision(), map.id(), map.revision())));
}
pub fn planner_map_current() -> bool {
    SOURCES.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|(store, _, _, id, revision)| store.current_revision(*id) == Ok(Some(*revision)))
    })
}
pub fn planner_original(
    store: &'static FlatStore<FlatCard>,
    id: ObjectId,
) -> Result<StoreSource<'static, FlatCard>, StoreError> {
    SOURCES.with(|s| {
        let sources = s.borrow();
        let (_, active, revision, _, _) = sources.as_ref().ok_or(StoreError::NotFound)?;
        if id != *active || store.current_revision(id)? != Some(*revision) {
            return Err(StoreError::NotFound);
        }
        store.source(id, Some(*revision))
    })
}
pub fn load_routes(store: &FlatStore<FlatCard>, app: &mut obc_app::App) {
    let mut summaries = Vec::new();
    let mut ids = Vec::new();
    for meta in store.entries().filter(|m| m.kind == ObjectKind::Route && m.flags.is_route_head()) {
        summaries.push(
            store
                .with_source(meta.id, Some(meta.revision), |source| obc_route::RouteSummary::read(source))
                .unwrap()
                .unwrap(),
        );
        ids.push(meta.id.0);
    }
    app.set_routes_with_ids(&summaries, &ids);
}

pub fn fingerprint_reads() -> u32 {
    FINGERPRINT_READS.get()
}
pub fn route_fingerprint(store: &FlatStore<FlatCard>, id: u64) -> Option<obc_formats::assistant::PayloadFingerprint> {
    FINGERPRINT_READS.set(FINGERPRINT_READS.get() + 1);
    store.entries().find(|e| e.id.0 == id).map(metadata::fingerprint)
}
pub fn planner_map_key(store: &FlatStore<FlatCard>) -> obc_formats::obcr::RouteSourceKey {
    SOURCES.with(|s| {
        let s = s.borrow();
        let (_, _, _, id, revision) = s.as_ref().unwrap();
        obc_formats::obcr::RouteSourceKey { store: store.store_id().0, object: id.0, revision: revision.0 }
    })
}
