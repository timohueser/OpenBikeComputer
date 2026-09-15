//! Board visit construction through the existing planner arena and serialized card writer.
use crate::{
    arena::NavGuard,
    flat_store::{FlatCard, Outcome, Request, Ticket, Writer},
};
use obc_app::device_core::{NavigatorTag, OperationToken};
use obc_app::{
    navigator::{
        NavigatorEffect, NavigatorError, NavigatorOutcome, PlanFamily, PlannerProgress, PlannerWork, ReviewPurpose,
        ReviewStatus,
    },
    App,
};
use obc_formats::{
    io::ByteSource,
    obcr::{CHUNK_META_LEN, HEADER_FULL_LEN, POINT_RECORD_LEN, VISIT_DESCRIPTOR_LEN, WAYPOINT_LEN},
};
use obc_reader::{MapCache, MapTables, Reader};
use obc_route::{
    visit::{forward_rejoin, VisitChoice},
    RouteReader, RouteStats, Step,
};
use obc_storage::flat::{Allocation, FlatStore, ObjectId, Revision, SealedAllocation, StoreError, StoreSource};

const RESERVE: u64 = (HEADER_FULL_LEN
    + obc_route::MAX_ROUTE_CHUNKS * ((obc_route::MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN + CHUNK_META_LEN)
    + obc_route::MAX_WAYPOINTS * WAYPOINT_LEN
    + VISIT_DESCRIPTOR_LEN) as u64;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Work {
    Begin,
    Leg,
    Append,
    Finish,
}
#[derive(Clone, Copy)]
enum Done {
    Running,
    Begin,
    Leg,
    Append,
    Finish(RouteStats),
}
#[derive(Clone, Copy)]
enum After {
    AllocateA,
    AllocateB,
    Flush(Work, Done),
    Seal,
    DropLeg,
    Restart,
    CancelA,
    CancelB,
    Close,
    Publish,
    Remove,
}
#[derive(Clone, Copy)]
enum Phase {
    Empty,
    AllocateA,
    AllocateB,
    Ready(Work),
    Step(Work),
    Flush(Work, Done, usize, usize),
    Seal,
    DropLeg,
    Restart,
    Await(Ticket, After),
    Publish,
    Preview,
    Stopped,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    OutAndBack,
    Forward,
    Rebuild,
}

pub(crate) struct Executor {
    original: Option<StoreSource<'static, FlatCard>>,
    a: Option<Allocation>,
    b: Option<Allocation>,
    leg: Option<SealedAllocation<'static>>,
    phase: Phase,
    token: Option<OperationToken<NavigatorTag>>,
    release: Option<bool>,
    choice: VisitChoice,
    variant: Variant,
    rejoin: u32,
    return_to: (i32, i32),
    returning: bool,
    published: Option<ObjectId>,
    uncertain: bool,
}
impl Executor {
    pub(crate) fn new() -> Self {
        Self {
            original: None,
            a: None,
            b: None,
            leg: None,
            phase: Phase::Empty,
            token: None,
            release: None,
            choice: VisitChoice::new(0, None),
            variant: Variant::OutAndBack,
            rejoin: 0,
            return_to: (0, 0),
            returning: false,
            published: None,
            uncertain: false,
        }
    }
    pub(crate) fn active(&self) -> bool {
        !matches!(self.phase, Phase::Empty)
    }
    pub(crate) fn accepts(&self, effect: &NavigatorEffect, app: &App) -> bool {
        (self.active()
            && matches!(
                effect,
                NavigatorEffect::Step { .. }
                    | NavigatorEffect::CommitRoute { .. }
                    | NavigatorEffect::Release { family: PlanFamily::Route, .. }
            ))
            || matches!(effect, NavigatorEffect::Acquire { work: PlannerWork::AssistantRoute(_), .. })
                && app
                    .assistant_review_context()
                    .is_some_and(|c| matches!(c.purpose, ReviewPurpose::Visit | ReviewPurpose::ReturnToRoute))
    }
    pub(crate) fn original_current(&self) -> bool {
        self.original.as_ref().is_some_and(StoreSource::is_current)
    }
    fn current(&self, app: &App, store: &FlatStore<FlatCard>) -> bool {
        app.assistant_review_context().is_some_and(|c| {
            c.map == crate::flat_store::planner_map_key(store)
                && c.store.bytes() == store.store_id().0
                && c.profile == app.settings().bike_profile_idx
                && c.original.is_some_and(|p| crate::flat_store::route_fingerprint(store, p.object) == Some(p))
        }) && crate::flat_store::planner_map_current()
            && self.original_current()
    }
    fn fail(&mut self, error: NavigatorError) -> Option<NavigatorOutcome> {
        self.phase = Phase::Stopped;
        self.token.take().map(|token| NavigatorOutcome::Failed { token, error })
    }
    fn ready(&mut self, work: Work) -> Option<NavigatorOutcome> {
        self.phase = Phase::Ready(work);
        self.token.take().map(|token| NavigatorOutcome::Stepped { token, progress: PlannerProgress::Searching })
    }
    pub(crate) fn accept(
        &mut self,
        effect: NavigatorEffect,
        app: &mut App,
        store: &'static FlatStore<FlatCard>,
        guard: &mut Option<NavGuard>,
    ) -> Option<NavigatorOutcome> {
        let token = effect.token();
        if matches!(self.phase, Phase::Await(..)) && !matches!(effect, NavigatorEffect::Release { .. }) {
            return Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace });
        }
        match effect {
            NavigatorEffect::Release { retain_result, .. } => {
                self.token = Some(token);
                self.release = Some(retain_result);
                None
            }
            NavigatorEffect::Acquire { work: PlannerWork::AssistantRoute(_), .. } => {
                self.token = Some(token);
                if !matches!(self.phase, Phase::Empty) || guard.is_some() {
                    return self.fail(NavigatorError::Workspace);
                }
                let Some(context) = app.assistant_review_context() else {
                    return self.fail(NavigatorError::Unavailable);
                };
                let Some(original) = context.original else { return self.fail(NavigatorError::SourceChanged) };
                if !crate::assistant::original_allowed(
                    store,
                    context,
                    app.active_route_index().and_then(|i| app.route_ids().get(i).copied()),
                ) {
                    return self.fail(NavigatorError::SourceChanged);
                }
                self.original = match crate::flat_store::planner_original(store, ObjectId(original.object)) {
                    Ok(source) => Some(source),
                    Err(_) => return self.fail(NavigatorError::SourceChanged),
                };
                if !self.current(app, store) {
                    return self.fail(NavigatorError::SourceChanged);
                }
                let Some(quiesced) = app.nav_arena_precondition() else { return self.fail(NavigatorError::Workspace) };
                *guard = match crate::arena::claim_nav(quiesced) {
                    Ok(g) => Some(g),
                    Err(_) => return self.fail(NavigatorError::Workspace),
                };
                self.rejoin = if context.purpose == ReviewPurpose::ReturnToRoute {
                    context.required_anchors_m[2]
                } else {
                    context.progress_m
                };
                self.variant = Variant::OutAndBack;
                if self.begin_variant(app, guard.as_mut().unwrap()).is_err() {
                    return self.fail(NavigatorError::Unavailable);
                }
                let (_, index, _, _) = guard.as_mut().unwrap().visit_parts();
                let reader = RouteReader::new(index, self.original.as_ref().unwrap());
                self.choice = match if context.purpose == ReviewPurpose::ReturnToRoute {
                    Ok(None)
                } else {
                    forward_rejoin(&reader, context.progress_m)
                } {
                    Ok(forward) => VisitChoice::new(context.progress_m, forward),
                    Err(_) => return self.fail(NavigatorError::Unavailable),
                };
                self.phase = Phase::AllocateA;
                None
            }
            NavigatorEffect::Step { .. } => {
                self.token = Some(token);
                if !self.current(app, store) {
                    return self.fail(NavigatorError::SourceChanged);
                }
                if let Phase::Ready(work) = self.phase {
                    self.phase = Phase::Step(work);
                    None
                } else {
                    self.fail(NavigatorError::Workspace)
                }
            }
            NavigatorEffect::CommitRoute { .. } => {
                self.token = Some(token);
                if !self.current(app, store) {
                    return self.fail(NavigatorError::SourceChanged);
                }
                if self.a.is_none() || self.b.is_some() || self.leg.is_some() {
                    return self.fail(NavigatorError::Workspace);
                }
                self.phase = Phase::Publish;
                None
            }
            _ => Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace }),
        }
    }
    #[inline(never)]
    fn begin_variant(&mut self, app: &App, guard: &mut NavGuard) -> Result<(), ()> {
        let c = app.assistant_review_context().ok_or(())?;
        let target = app.assistant_visit_target();
        guard.begin_visit(c, target, self.rejoin).map_err(|_| ())?;
        let (_, original, _, _) = guard.visit_parts();
        original.read_into(self.original.as_ref().ok_or(())?).map_err(|_| ())?;
        let route = RouteReader::new(original, self.original.as_ref().ok_or(())?);
        let to = route.position_at(self.rejoin).ok_or(())?;
        self.return_to = (to.lon, to.lat);
        self.returning = c.purpose == ReviewPurpose::ReturnToRoute;
        if self.returning
            && (c.required_anchors_m != [self.rejoin; 3]
                || route.visit_descriptor().map_err(|_| ())?.map(|v| v.accepted_anchors_m[2]) != Some(self.rejoin))
        {
            return Err(());
        }
        Ok(())
    }
    fn start_leg(&mut self, app: &App, guard: &mut NavGuard) -> Result<(), ()> {
        let c = app.assistant_review_context().ok_or(())?;
        self.choice.search().map_err(|_| ())?;
        let (from, to) = if c.purpose == ReviewPurpose::ReturnToRoute {
            (c.origin, self.return_to)
        } else {
            let approach = app.assistant_visit_target().ok_or(())?.approach(c.map, c.profile).ok_or(())?;
            if self.returning {
                (approach, self.return_to)
            } else {
                (c.origin, approach)
            }
        };
        guard.visit_begin_plan(from, to, c);
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn poll(
        &mut self,
        app: &mut App,
        store: &'static FlatStore<FlatCard>,
        writer: Writer,
        guard: &mut Option<NavGuard>,
        map: &dyn ByteSource,
        tables: &MapTables,
        cache: &MapCache,
        elev: &mut dyn obc_route::ElevationSource,
        reply: &'static crate::flat_store::Reply,
    ) -> Option<NavigatorOutcome> {
        if let Phase::Await(ticket, after) = self.phase {
            let answer = writer.try_result(ticket, reply)?;
            return self.answered(after, answer, app, store, guard);
        }
        if self.release.is_some() {
            return self.cleanup(app, store, writer, guard, reply);
        }
        if matches!(self.phase, Phase::Empty | Phase::Ready(_) | Phase::Preview | Phase::Stopped) {
            return None;
        }
        if !self.current(app, store) {
            return self.fail(NavigatorError::SourceChanged);
        }
        let Some(g) = guard.as_mut() else { return self.fail(NavigatorError::Workspace) };
        match self.phase {
            Phase::AllocateA | Phase::AllocateB => {
                let after = if matches!(self.phase, Phase::AllocateA) { After::AllocateA } else { After::AllocateB };
                if let Ok(t) = writer.try_call(Request::Allocate { bytes: RESERVE }, reply) {
                    self.phase = Phase::Await(t, after);
                }
            }
            Phase::Step(work) => {
                let (done, appended, patch) = if work == Work::Leg {
                    let (planner, scratch, tiles, output) = g.visit_plan_parts();
                    let mut sink = crate::ride::NavStageSink { stage: output, appended: 0, patch_len: 0 };
                    let done = match planner.step(&Reader::new(map, tables, cache), scratch, tiles, elev, &mut sink) {
                        Step::Running => Done::Running,
                        Step::Done(_) => Done::Leg,
                        Step::Failed(error) => {
                            if self.variant == Variant::Forward {
                                self.variant = Variant::Rebuild;
                                self.rejoin = self.choice.departure_m;
                                self.phase = Phase::Restart;
                                return None;
                            }
                            return self.fail(NavigatorError::Plan(error));
                        }
                    };
                    (done, sink.appended, sink.patch_len)
                } else {
                    let (builder, original, leg, output) = g.visit_parts();
                    let mut sink = crate::ride::NavStageSink { stage: output, appended: 0, patch_len: 0 };
                    let result = match work {
                        Work::Begin => builder.begin(&mut sink).map(|_| Done::Begin),
                        Work::Append => {
                            let Some(sealed) = self.leg.as_ref() else { return self.fail(NavigatorError::Store) };
                            let source = store.sealed_source(sealed);
                            builder.append_leg_step(&RouteReader::new(leg, &source), &mut sink).map(|done| {
                                if done {
                                    Done::Append
                                } else {
                                    Done::Running
                                }
                            })
                        }
                        Work::Finish => builder
                            .finish_step(&RouteReader::new(original, self.original.as_ref().unwrap()), &mut sink)
                            .map(|done| done.map_or(Done::Running, Done::Finish)),
                        Work::Leg => unreachable!(),
                    };
                    let done = match result {
                        Ok(done) => done,
                        Err(_) => return self.fail(NavigatorError::Unavailable),
                    };
                    (done, sink.appended, sink.patch_len)
                };
                if appended == 0 && patch == 0 {
                    return self.after_flush(work, done, app, g);
                }
                self.phase = Phase::Flush(work, done, appended, patch);
            }
            Phase::Flush(work, done, appended, patch) => {
                let allocation = if work == Work::Leg { self.b } else { self.a };
                let Some(allocation) = allocation else { return self.fail(NavigatorError::Store) };
                let base = g.output().as_ptr();
                // The pending ticket keeps the guard and staged bytes unavailable until answered.
                let bytes = unsafe { core::slice::from_raw_parts(base.add(HEADER_FULL_LEN), appended) };
                let header = unsafe { core::slice::from_raw_parts(base, patch) };
                if let Ok(t) = writer.try_call(Request::WriteComputedRoute { allocation, bytes, header }, reply) {
                    self.phase = Phase::Await(t, After::Flush(work, done));
                }
            }
            Phase::Seal => {
                if let Some(b) = self.b {
                    if let Ok(t) = writer.try_call(g.visit_seal_request(b), reply) {
                        self.phase = Phase::Await(t, After::Seal);
                    }
                }
            }
            Phase::DropLeg => {
                if let Some(sealed) = self.leg.take() {
                    match writer.try_call_owned(Request::ReleaseSealed { sealed }, reply) {
                        Ok(t) => self.phase = Phase::Await(t, After::DropLeg),
                        Err(Request::ReleaseSealed { sealed }) => self.leg = Some(sealed),
                        _ => unreachable!(),
                    }
                }
            }
            Phase::Restart => {
                if self.b.is_some() || self.leg.is_some() {
                    return self.cleanup_buffers(store, writer, g, reply);
                }
                if let Some(allocation) = self.a {
                    if let Ok(t) = writer.try_call(Request::Cancel { allocation }, reply) {
                        self.phase = Phase::Await(t, After::Restart);
                    }
                }
            }
            Phase::Publish => {
                let Some(allocation) = self.a else { return self.fail(NavigatorError::Store) };
                let original = self.original.as_ref().map(|s| (s.id(), s.revision()));
                if let Ok(t) = writer.try_call(
                    Request::PublishComputedRoute {
                        allocation,
                        name: obc_storage::flat::DisplayName::new("Visit").unwrap(),
                        original,
                    },
                    reply,
                ) {
                    self.phase = Phase::Await(t, After::Publish);
                }
            }
            _ => {}
        }
        None
    }
    fn after_flush(&mut self, work: Work, done: Done, app: &mut App, guard: &mut NavGuard) -> Option<NavigatorOutcome> {
        match done {
            Done::Begin => {
                self.phase = Phase::AllocateB;
                None
            }
            Done::Leg => {
                guard.visit_begin_sources();
                self.phase = Phase::Seal;
                None
            }
            Done::Append => {
                self.phase = Phase::DropLeg;
                None
            }
            Done::Finish(stats) => match self.variant {
                Variant::OutAndBack if self.choice.forward_m.is_some() => {
                    self.choice.remember_out_and_back(stats.total_distance_m);
                    self.rejoin = self.choice.forward_m.unwrap();
                    self.variant = Variant::Forward;
                    self.phase = Phase::Restart;
                    None
                }
                Variant::Forward if !self.choice.prefer_forward(stats.total_distance_m) => {
                    self.rejoin = self.choice.departure_m;
                    self.variant = Variant::Rebuild;
                    self.phase = Phase::Restart;
                    None
                }
                _ => {
                    let anchors = guard.visit_parts().0.original_anchors();
                    let token = self.token.take()?;
                    self.phase = Phase::Stopped;
                    Some(
                        if app.assistant_review_context().is_some_and(|c| {
                            c.purpose == ReviewPurpose::ReturnToRoute && c.required_anchors_m == anchors
                        }) || app.assistant_visit_variant(token, anchors)
                        {
                            NavigatorOutcome::Stepped { token, progress: PlannerProgress::Reached }
                        } else {
                            NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged }
                        },
                    )
                }
            },
            Done::Running => self.ready(work),
        }
    }
    fn answered(
        &mut self,
        after: After,
        answer: Result<Outcome, StoreError>,
        app: &mut App,
        store: &'static FlatStore<FlatCard>,
        guard: &mut Option<NavGuard>,
    ) -> Option<NavigatorOutcome> {
        let releasing = self.release.is_some();
        match (after, answer) {
            (After::AllocateA, Ok(Outcome::Allocated(a))) => {
                self.a = Some(a);
                self.phase = Phase::Step(Work::Begin);
            }
            (After::AllocateB, Ok(Outcome::Allocated(b))) => {
                self.b = Some(b);
                if !releasing && self.start_leg(app, guard.as_mut()?).is_err() {
                    return self.fail(NavigatorError::Unavailable);
                }
                self.phase = Phase::Ready(Work::Leg);
                if !releasing {
                    return self.token.take().map(|token| {
                        if self.choice.searches() == 1 {
                            NavigatorOutcome::Acquired { token }
                        } else {
                            NavigatorOutcome::Stepped { token, progress: PlannerProgress::Searching }
                        }
                    });
                }
            }
            (After::Flush(work, done), Ok(Outcome::Wrote(a))) => {
                if work == Work::Leg {
                    self.b = Some(a);
                } else {
                    self.a = Some(a);
                }
                self.phase = Phase::Stopped;
                if !releasing {
                    return self.after_flush(work, done, app, guard.as_mut()?);
                }
            }
            (After::Seal, Ok(Outcome::Done)) => {
                self.b = None;
                self.leg = guard.as_mut()?.visit_take_sealed();
                self.phase = Phase::Stopped;
                if !releasing {
                    let (_, original, leg, _) = guard.as_mut()?.visit_parts();
                    if original.read_into(self.original.as_ref()?).is_err()
                        || leg.read_into(&store.sealed_source(self.leg.as_ref()?)).is_err()
                    {
                        return self.fail(NavigatorError::Store);
                    }
                    return self.ready(Work::Append);
                }
            }
            (After::DropLeg, Ok(Outcome::Done)) => {
                if !releasing {
                    if self.returning {
                        return self.ready(Work::Finish);
                    } else {
                        self.returning = true;
                        self.phase = Phase::AllocateB;
                    }
                } else {
                    self.phase = Phase::Stopped;
                }
            }
            (After::Restart, Ok(Outcome::Done)) => {
                self.a = None;
                self.phase = Phase::Stopped;
                if !releasing {
                    if self.begin_variant(app, guard.as_mut()?).is_err() {
                        return self.fail(NavigatorError::Unavailable);
                    }
                    self.phase = Phase::AllocateA;
                }
            }
            (After::CancelA, Ok(Outcome::Done)) => {
                self.a = None;
                self.phase = Phase::Stopped;
            }
            (After::CancelB, Ok(Outcome::Done)) => {
                self.b = None;
                self.phase = if releasing { Phase::Stopped } else { Phase::Restart };
            }
            (After::Close, Ok(Outcome::Done)) => {
                self.phase = Phase::Stopped;
            }
            (After::Publish, Ok(Outcome::Published(id))) => {
                self.a = None;
                self.published = Some(id);
                self.phase = Phase::Stopped;
                if !releasing {
                    crate::flat_store::load_routes(store, app);
                    let context = app.assistant_review_context()?;
                    let Some(fingerprint) = crate::flat_store::route_fingerprint(store, id.0) else {
                        self.uncertain = true;
                        return self.fail(NavigatorError::DurabilityUnknown);
                    };
                    let preview = store.with_source(id, Some(Revision(1)), |source| {
                        let preview = obc_app::navigator::ReviewedRoute::read(fingerprint, source, context)?;
                        let shape = crate::assistant::preview_shape(
                            guard.as_mut().ok_or(NavigatorError::Workspace)?.visit_parts().2,
                            source,
                        )
                        .map_err(|_| NavigatorError::Store)?;
                        Ok::<_, NavigatorError>((preview, shape))
                    });
                    let token = self.token.take()?;
                    return Some(match preview {
                        Ok(Ok((preview, shape))) => {
                            let outcome = app.assistant_preview_outcome(token, preview);
                            if matches!(outcome, NavigatorOutcome::ReviewReady { .. })
                                && !app.set_assistant_preview_shape(token, preview.source, &shape)
                            {
                                NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged }
                            } else {
                                outcome
                            }
                        }
                        _ => {
                            self.uncertain = true;
                            NavigatorOutcome::Failed { token, error: NavigatorError::DurabilityUnknown }
                        }
                    });
                }
            }
            (After::Remove, Ok(Outcome::Done) | Err(StoreError::NotFound)) => {
                self.published = None;
                self.phase = Phase::Stopped;
                crate::flat_store::load_routes(store, app);
            }
            (After::Publish, Err(StoreError::Media | StoreError::ReadOnly)) => {
                self.uncertain = true;
                self.a = None;
                self.phase = Phase::Stopped;
                if !releasing {
                    return self.fail(NavigatorError::DurabilityUnknown);
                }
            }
            (After::Remove, Err(StoreError::ReadOnly)) => {
                self.uncertain = true;
                self.phase = Phase::Stopped;
            }
            (_, Err(_)) => {
                self.phase =
                    if matches!(after, After::CancelB) && !releasing { Phase::Restart } else { Phase::Stopped };
                if !releasing {
                    return self.fail(NavigatorError::Store);
                }
            }
            _ => return self.fail(NavigatorError::Store),
        }
        None
    }
    fn cleanup_buffers(
        &mut self,
        _store: &FlatStore<FlatCard>,
        writer: Writer,
        guard: &mut NavGuard,
        reply: &'static crate::flat_store::Reply,
    ) -> Option<NavigatorOutcome> {
        if let Some(allocation) = self.b {
            if let Ok(t) = writer.try_call(Request::Cancel { allocation }, reply) {
                self.phase = Phase::Await(t, After::CancelB);
            }
            return None;
        }
        if let Some(sealed) = self.leg.take().or_else(|| guard.visit_take_sealed()) {
            match writer.try_call_owned(Request::ReleaseSealed { sealed }, reply) {
                Ok(t) => self.phase = Phase::Await(t, After::DropLeg),
                Err(Request::ReleaseSealed { sealed }) => self.leg = Some(sealed),
                _ => unreachable!(),
            }
        }
        None
    }
    fn cleanup(
        &mut self,
        app: &App,
        store: &'static FlatStore<FlatCard>,
        writer: Writer,
        guard: &mut Option<NavGuard>,
        reply: &'static crate::flat_store::Reply,
    ) -> Option<NavigatorOutcome> {
        if self.b.is_some() || self.leg.is_some() {
            return self.cleanup_buffers(store, writer, guard.as_mut()?, reply);
        }
        if let Some(allocation) = self.a {
            if let Ok(t) = writer.try_call(Request::Cancel { allocation }, reply) {
                self.phase = Phase::Await(t, After::CancelA);
            }
            return None;
        }
        let retain = self.release.unwrap_or(false);
        let keep = self.uncertain || retain && app.assistant_preview().is_some();
        if !keep {
            if let Some(source) = self.original.take() {
                match writer.try_call_owned(Request::Close { handle: source.release() }, reply) {
                    Ok(t) => self.phase = Phase::Await(t, After::Close),
                    Err(Request::Close { handle }) => self.original = Some(StoreSource::over(store, handle).unwrap()),
                    _ => unreachable!(),
                }
                return None;
            }
        }
        if let Some(id) = self.published.filter(|_| !retain && !self.uncertain) {
            if let Ok(t) = writer.try_call(Request::RemoveComputedRoute { id, revision: Revision(1) }, reply) {
                self.phase = Phase::Await(t, After::Remove);
            }
            return None;
        }
        *guard = None;
        self.release = None;
        self.phase = if keep { Phase::Preview } else { Phase::Empty };
        self.token.take().map(|token| {
            if self.uncertain {
                NavigatorOutcome::ReleaseUnresolved { token }
            } else {
                NavigatorOutcome::Released { token }
            }
        })
    }
    pub(crate) fn accepted(&mut self, app: &App, store: &FlatStore<FlatCard>) {
        if app.assistant_review_status() == ReviewStatus::Accepted && matches!(self.phase, Phase::Preview) {
            crate::assistant::release_original(store, &mut self.original, false);
            self.published = None;
            self.phase = Phase::Empty;
        }
    }
}
