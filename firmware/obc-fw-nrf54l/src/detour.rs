//! Physical detour work. Navigator admits each operation and acknowledges its cleanup.
use crate::{
    arena::NavGuard,
    flat_store::{FlatCard, Outcome, Request, Ticket, Writer},
};
use obc_app::device_core::{NavigatorTag, OperationToken};
use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlanFamily, PlannerProgress, PlannerWork};
use obc_app::{App, DetourRequest};
use obc_formats::{
    io::ByteSource,
    obcr::{CHUNK_META_LEN, HEADER_FULL_LEN, POINT_RECORD_LEN, WAYPOINT_LEN},
};
use obc_reader::{MapCache, MapTables, Reader};
use obc_route::{RouteReader, SpliceStep, Step, TrimStep};
use obc_storage::flat::{Allocation, FlatStore, ObjectId, Revision, SealedAllocation, StoreError, StoreSource};
const RESERVE: u64 = (HEADER_FULL_LEN
    + obc_route::MAX_ROUTE_CHUNKS * ((obc_route::MAX_POINTS_PER_CHUNK - 1) * POINT_RECORD_LEN + CHUNK_META_LEN)
    + obc_route::MAX_WAYPOINTS * WAYPOINT_LEN) as u64;

#[derive(Clone, Copy)]
pub(crate) enum TransformStep {
    Trim(TrimStep),
    Splice(SpliceStep),
}
#[derive(Clone, Copy)]
enum Work {
    Plan,
    Trim,
    Preview,
    Splice,
}
#[derive(Clone, Copy)]
enum Completion {
    Running,
    Plan { length_m: u32, ascent_m: u32, has_elevation: bool },
    Trim(obc_route::TrimOutcome),
    Splice,
}
#[derive(Clone, Copy)]
enum After {
    Acquire,
    StartTrim,
    StartSplice,
    Flush(Work, Completion),
    SealPlan,
    SealTrim,
    Trimmed,
    NoTrim,
    Cancel,
    DropLeg,
    DropSlot,
    Close,
    Publish,
    Compensate,
}
#[derive(Clone, Copy)]
enum Phase {
    Empty,
    Ready(Work),
    Step(Work),
    Allocate(After),
    Flush(After, usize, usize),
    Seal(After),
    Await(Ticket, After),
    NoTrim,
    ReplaceLeg,
    Publish,
    Stopped,
    Preview,
}

/// Only immutable bytes, exact original lease and frozen figures survive preview. No index or
/// emitter leaves the arena. A second sealed leg exists only in its arena result slot during trim.
pub(crate) struct Executor {
    original: Option<StoreSource<'static, FlatCard>>,
    leg: Option<SealedAllocation<'static>>,
    allocation: Option<Allocation>,
    phase: Phase,
    token: Option<OperationToken<NavigatorTag>>,
    release: Option<bool>,
    progress_m: u32,
    rejoin_m: u32,
    kind: obc_route::Leg,
    length_m: u32,
    ascent_m: u32,
    has_elevation: bool,
    published: Option<ObjectId>,
    trimmed: Option<obc_route::TrimOutcome>,
}
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<Executor>() <= 256);
impl Executor {
    pub(crate) fn new() -> Self {
        Self {
            original: None,
            leg: None,
            allocation: None,
            phase: Phase::Empty,
            token: None,
            release: None,
            progress_m: 0,
            rejoin_m: 0,
            kind: obc_route::Leg::Detour,
            length_m: 0,
            ascent_m: 0,
            has_elevation: false,
            published: None,
            trimmed: None,
        }
    }
    pub(crate) fn active(&self) -> bool {
        !matches!(self.phase, Phase::Empty | Phase::Preview)
    }
    pub(crate) fn accepts(&self, effect: &NavigatorEffect) -> bool {
        matches!(
            effect,
            NavigatorEffect::Acquire { work: PlannerWork::Detour(_), .. }
                | NavigatorEffect::CommitDetour { .. }
                | NavigatorEffect::Release { family: PlanFamily::Detour, .. }
        ) || self.active() && matches!(effect, NavigatorEffect::Step { .. })
    }
    fn current(&self) -> bool {
        crate::flat_store::planner_map_current() && self.original.as_ref().is_some_and(StoreSource::is_current)
    }
    fn fail(&mut self, error: NavigatorError) -> Option<NavigatorOutcome> {
        self.phase = Phase::Stopped;
        self.token.take().map(|token| NavigatorOutcome::Failed { token, error })
    }
    fn stepped(&mut self, work: Work) -> Option<NavigatorOutcome> {
        if matches!(work, Work::Splice) {
            self.phase = Phase::Step(work);
            return None;
        }
        self.phase = Phase::Ready(work);
        self.token.take().map(|token| NavigatorOutcome::Stepped { token, progress: PlannerProgress::Searching })
    }
    pub(crate) fn accept(
        &mut self,
        effect: NavigatorEffect,
        app: &App,
        store: &'static FlatStore<FlatCard>,
        guard: &mut Option<NavGuard>,
        profile: obc_route::BikeType,
    ) -> Option<NavigatorOutcome> {
        let token = effect.token();
        match effect {
            NavigatorEffect::Release { retain_result, .. } => {
                self.token = Some(token);
                self.release = Some(retain_result);
                None
            }
            NavigatorEffect::Acquire { work: PlannerWork::Detour(request), .. } => {
                if self.active() || self.original.is_some() || guard.is_some() {
                    return Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace });
                }
                self.token = Some(token);
                let Some(id) = app.route_ids().get(request.route).copied() else {
                    return self.fail(NavigatorError::SourceChanged);
                };
                let original = match crate::flat_store::planner_original(store, ObjectId(id)) {
                    Ok(source) => source,
                    Err(_) => return self.fail(NavigatorError::SourceChanged),
                };
                self.original = Some(original);
                self.progress_m = request.progress_m;
                self.rejoin_m = request.target_m;
                self.kind = request.leg;
                // A rest is the stored day before over the span the request names, so there is
                // nothing to plan.
                if let obc_route::Leg::Rest { from_m, to_m } = request.leg {
                    if self.rest_route(app).is_none() {
                        return self.fail(NavigatorError::SourceChanged);
                    }
                    self.length_m = to_m.saturating_sub(from_m);
                    self.has_elevation = true;
                    self.phase = Phase::Ready(Work::Plan);
                    return self.token.take().map(|token| NavigatorOutcome::Acquired { token });
                }
                let Some(quiesced) = app.nav_arena_precondition() else { return self.fail(NavigatorError::Workspace) };
                *guard = match crate::arena::claim_nav(quiesced) {
                    Ok(g) => Some(g),
                    Err(_) => return self.fail(NavigatorError::Workspace),
                };
                if self.begin_plan(guard.as_mut().unwrap(), request, profile).is_err() {
                    return self.fail(NavigatorError::SourceChanged);
                };
                self.phase = Phase::Allocate(After::Acquire);
                None
            }
            NavigatorEffect::Step { .. } => {
                if !matches!(self.phase, Phase::Ready(_)) {
                    return Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace });
                }
                self.token = Some(token);
                if !self.current() {
                    return self.fail(NavigatorError::SourceChanged);
                };
                // A rest has nothing to search: its leg is the stored day before.
                if self.rest_route(app).is_some() {
                    self.phase = Phase::Preview;
                    let preview = obc_app::DetourPreview {
                        cost_delta_m: 0,
                        total_distance_m: self.length_m,
                        rejoin_m: self.rejoin_m,
                        ascent_m: None,
                    };
                    return self.token.take().map(|token| NavigatorOutcome::DetourFinished { token, preview });
                }
                if let Phase::Ready(work) = self.phase {
                    self.phase = Phase::Step(work);
                    None
                } else {
                    self.fail(NavigatorError::Workspace)
                }
            }
            NavigatorEffect::CommitDetour { .. } => {
                if !matches!(self.phase, Phase::Preview)
                    || (self.leg.is_none() && self.rest_route(app).is_none())
                    || guard.is_some()
                {
                    return Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace });
                }
                self.token = Some(token);
                if !self.current() {
                    return self.fail(NavigatorError::SourceChanged);
                };
                let Some(quiesced) = app.nav_arena_precondition() else { return self.fail(NavigatorError::Workspace) };
                *guard = match crate::arena::claim_nav(quiesced) {
                    Ok(g) => Some(g),
                    Err(_) => return self.fail(NavigatorError::Workspace),
                };
                let g = guard.as_mut().unwrap();
                g.begin_sources();
                if self.parse_sources(app, store, g).is_err() {
                    return self.fail(NavigatorError::Store);
                };
                g.begin_splice(self.kind, self.progress_m, self.rejoin_m, self.length_m, self.has_elevation);
                self.phase = Phase::Allocate(After::StartSplice);
                None
            }
            _ => Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace }),
        }
    }
    #[inline(never)]
    fn begin_plan(&self, guard: &mut NavGuard, request: DetourRequest, profile: obc_route::BikeType) -> Result<(), ()> {
        guard.begin_sources();
        let (index, _) = guard.sources();
        let original = self.original.as_ref().ok_or(())?;
        index.read_into(original).map_err(|_| ())?;
        let reader = RouteReader::new(index, original);
        let to = reader.position_at(request.target_m).ok_or(())?;
        let corridor = obc_route::Corridor::build(&reader, request.progress_m, request.target_m);
        // Initialize the planner arm only after the source-index references have ended.
        guard.restore_plan();
        guard.begin_plan(obc_route::NavPlanner::new_detour(
            request.from,
            (to.lon, to.lat),
            "Detour leg",
            profile,
            corridor,
        ));
        Ok(())
    }
    /// The day before's stored route, while the leg is a rest: the leg reads it in place of a
    /// planned leg. It is looked up from the trip on each read, so the executor holds no handle for
    /// it.
    fn rest_route(&self, app: &App) -> Option<ObjectId> {
        if !matches!(self.kind, obc_route::Leg::Rest { .. }) {
            return None;
        }
        let day = obc_app::trip::trip_day(app.trips(), self.original.as_ref()?.id().0)?;
        let trip = app.trips().iter().find(|trip| trip.key == day.key())?;
        trip.stage_ids.get(usize::from(day.day_index()).checked_sub(1)?).map(|&id| ObjectId(id))
    }
    fn parse_sources(&self, app: &App, store: &FlatStore<FlatCard>, guard: &mut NavGuard) -> Result<(), ()> {
        let rest = self.rest_route(app);
        let (orig, leg) = guard.sources();
        orig.read_into(self.original.as_ref().ok_or(())?).map_err(|_| ())?;
        match rest {
            Some(id) => store.with_source(id, None, |rest| leg.read_into(rest)).map_err(|_| ())?.map_err(|_| ()),
            None => leg.read_into(&store.sealed_source(self.leg.as_ref().ok_or(())?)).map_err(|_| ()),
        }
    }
    #[allow(clippy::too_many_arguments)] // Borrow the ride loop's existing views for one pass.
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
            let result = self.answered(after, answer, app, store, guard);
            if self.release.is_some() {
                return None;
            }
            return result;
        }
        if let Some(retain) = self.release {
            return self.cleanup(retain, store, writer, guard, reply);
        }
        if matches!(self.phase, Phase::Empty | Phase::Preview | Phase::Ready(_) | Phase::Stopped) {
            return None;
        }
        if !self.current() {
            return self.fail(NavigatorError::SourceChanged);
        }
        let Some(g) = guard.as_mut() else { return self.fail(NavigatorError::Workspace) };
        match self.phase {
            Phase::Allocate(after) => {
                if let Ok(ticket) = writer.try_call(Request::Allocate { bytes: RESERVE }, reply) {
                    self.phase = Phase::Await(ticket, after);
                }
            }
            Phase::Step(work) => {
                if matches!(work, Work::Preview) {
                    let Some(leg) = self.leg.as_ref() else { return self.fail(NavigatorError::Store) };
                    match g.preview_step(&store.sealed_source(leg), app) {
                        Ok(false) => return self.stepped(work),
                        Err(_) => return self.fail(NavigatorError::Store),
                        Ok(true) => {
                            self.phase = Phase::Stopped;
                            let preview = obc_app::DetourPreview {
                                cost_delta_m: (i64::from(self.length_m)
                                    - i64::from(self.rejoin_m.saturating_sub(self.progress_m)))
                                    as i32,
                                total_distance_m: self.length_m,
                                rejoin_m: self.rejoin_m,
                                ascent_m: self.has_elevation.then_some(self.ascent_m),
                            };
                            return self.token.take().map(|token| NavigatorOutcome::DetourFinished { token, preview });
                        }
                    }
                }
                let (result, trim, appended, patch) = if matches!(work, Work::Plan) {
                    let Some((planner, scratch, tiles, output)) = g.plan_parts() else {
                        return self.fail(NavigatorError::Workspace);
                    };
                    let mut sink = crate::ride::NavStageSink { stage: output, appended: 0, patch_len: 0 };
                    let result = match planner.step(&Reader::new(map, tables, cache), scratch, tiles, elev, &mut sink) {
                        Step::Running => Ok(None),
                        Step::Done(stats) => Ok(Some(stats)),
                        Step::Failed(e) => Err(NavigatorError::Plan(e)),
                    };
                    (result, None, sink.appended, sink.patch_len)
                } else {
                    let Some(orig) = self.original.as_ref() else { return self.fail(NavigatorError::Store) };
                    let transformed = match (self.rest_route(app), self.leg.as_ref()) {
                        (Some(id), _) => store.with_source(id, None, |rest| g.transform(orig, rest)),
                        (None, Some(leg)) => Ok(g.transform(orig, &store.sealed_source(leg))),
                        (None, None) => return self.fail(NavigatorError::Store),
                    };
                    let Ok((step, appended, patch)) = transformed else { return self.fail(NavigatorError::Store) };
                    match step {
                        TransformStep::Trim(TrimStep::Running) => (Ok(None), None, appended, patch),
                        TransformStep::Trim(TrimStep::Done(outcome)) => {
                            if outcome.is_none() {
                                self.phase = Phase::NoTrim;
                                return None;
                            }
                            (Ok(None), outcome, appended, patch)
                        }
                        TransformStep::Trim(TrimStep::Failed(_)) => {
                            self.phase = Phase::NoTrim;
                            return None;
                        }
                        TransformStep::Splice(SpliceStep::Running) => (Ok(None), None, appended, patch),
                        TransformStep::Splice(SpliceStep::Done(stats)) => (Ok(Some(stats)), None, appended, patch),
                        TransformStep::Splice(SpliceStep::Failed(_)) => {
                            (Err(NavigatorError::Store), None, appended, patch)
                        }
                    }
                };
                if let Err(error) = result {
                    return self.fail(error);
                }
                let done = if let Some(trim) = trim {
                    Completion::Trim(trim)
                } else if let Ok(Some(stats)) = result {
                    if matches!(work, Work::Plan) {
                        Completion::Plan {
                            length_m: stats.total_distance_m,
                            ascent_m: stats.total_ascent_m,
                            has_elevation: stats.has_elevation,
                        }
                    } else {
                        Completion::Splice
                    }
                } else {
                    Completion::Running
                };
                let after = After::Flush(work, done);
                if appended == 0 && patch == 0 {
                    return self.after_flush(work, done, g);
                }
                self.phase = Phase::Flush(after, appended, patch);
            }
            Phase::Flush(after, appended, patch) => {
                let Some(allocation) = self.allocation else { return self.fail(NavigatorError::Store) };
                let base = g.output().as_ptr();
                // The held guard and pending ticket exclude every arena read/write until completion.
                let (bytes, header) = unsafe {
                    (
                        core::slice::from_raw_parts(base.add(HEADER_FULL_LEN), appended),
                        core::slice::from_raw_parts(base, patch),
                    )
                };
                if let Ok(ticket) = writer.try_call(Request::WriteComputedRoute { allocation, bytes, header }, reply) {
                    self.phase = Phase::Await(ticket, after);
                }
            }
            Phase::Seal(after) => {
                let Some(allocation) = self.allocation else { return self.fail(NavigatorError::Store) };
                if let Ok(ticket) = writer.try_call(g.seal_request(allocation), reply) {
                    self.phase = Phase::Await(ticket, after);
                }
            }
            Phase::NoTrim => {
                if let Some(allocation) = self.allocation {
                    if let Ok(ticket) = writer.try_call(Request::Cancel { allocation }, reply) {
                        self.phase = Phase::Await(ticket, After::NoTrim);
                    }
                } else {
                    return self.start_preview(store, g);
                }
            }
            Phase::ReplaceLeg => {
                let Some(sealed) = self.leg.take() else { return self.fail(NavigatorError::Store) };
                match writer.try_call_owned(Request::ReleaseSealed { sealed }, reply) {
                    Ok(t) => self.phase = Phase::Await(t, After::Trimmed),
                    Err(Request::ReleaseSealed { sealed }) => self.leg = Some(sealed),
                    _ => unreachable!(),
                }
            }
            Phase::Publish => {
                let Some(allocation) = self.allocation else { return self.fail(NavigatorError::Store) };
                let output = g.output();
                let len = usize::from(output[6]).min(48);
                let Some(name) =
                    core::str::from_utf8(&output[64..64 + len]).ok().and_then(obc_storage::flat::DisplayName::new)
                else {
                    return self.fail(NavigatorError::Store);
                };
                let original = self.original.as_ref().map(|s| (s.id(), s.revision()));
                let built_day = matches!(self.kind, obc_route::Leg::Rest { .. });
                if let Ok(t) =
                    writer.try_call(Request::PublishComputedRoute { allocation, name, original, built_day }, reply)
                {
                    self.phase = Phase::Await(t, After::Publish);
                }
            }
            _ => {}
        }
        None
    }
    fn after_flush(&mut self, work: Work, done: Completion, guard: &mut NavGuard) -> Option<NavigatorOutcome> {
        match done {
            Completion::Plan { length_m, ascent_m, has_elevation } => {
                self.length_m = length_m;
                self.ascent_m = ascent_m;
                self.has_elevation = has_elevation;
                guard.begin_sources();
                self.phase = Phase::Seal(After::SealPlan);
                None
            }
            Completion::Trim(outcome) => {
                self.trimmed = Some(outcome);
                self.phase = Phase::Seal(After::SealTrim);
                None
            }
            Completion::Splice => {
                self.phase = Phase::Publish;
                None
            }
            Completion::Running => self.stepped(work),
        }
    }
    fn start_preview(&mut self, store: &FlatStore<FlatCard>, guard: &mut NavGuard) -> Option<NavigatorOutcome> {
        let Some(leg) = self.leg.as_ref() else { return self.fail(NavigatorError::Store) };
        let (_, index) = guard.sources();
        if index.read_into(&store.sealed_source(leg)).is_err() {
            return self.fail(NavigatorError::Store);
        };
        guard.begin_preview();
        self.stepped(Work::Preview)
    }
    fn answered(
        &mut self,
        after: After,
        answer: Result<Outcome, StoreError>,
        app: &mut App,
        store: &'static FlatStore<FlatCard>,
        guard: &mut Option<NavGuard>,
    ) -> Option<NavigatorOutcome> {
        self.phase = Phase::Stopped;
        match (after, answer) {
            (After::Acquire, Ok(Outcome::Allocated(allocation))) => {
                self.allocation = Some(allocation);
                if self.release.is_some() {
                    return None;
                }
                self.phase = Phase::Ready(Work::Plan);
                self.token.take().map(|token| NavigatorOutcome::Acquired { token })
            }
            (After::StartTrim, Ok(Outcome::Allocated(allocation))) => {
                self.allocation = Some(allocation);
                if self.release.is_some() {
                    return None;
                }
                guard.as_mut()?.begin_trim(self.kind, self.rejoin_m, self.has_elevation);
                self.stepped(Work::Trim)
            }
            (After::StartTrim, Err(_)) => {
                if self.release.is_some() {
                    return None;
                }
                self.start_preview(store, guard.as_mut()?)
            }
            (After::StartSplice, Ok(Outcome::Allocated(allocation))) => {
                self.allocation = Some(allocation);
                if self.release.is_none() {
                    self.phase = Phase::Step(Work::Splice);
                }
                None
            }
            (After::Flush(work, done), Ok(Outcome::Wrote(allocation))) => {
                self.allocation = Some(allocation);
                if self.release.is_some() {
                    return None;
                }
                self.after_flush(work, done, guard.as_mut()?)
            }
            (After::Flush(Work::Trim, ..) | After::SealTrim, Err(_)) => {
                if self.release.is_none() {
                    self.phase = Phase::NoTrim;
                }
                None
            }
            (After::SealPlan, Ok(Outcome::Done)) => {
                self.allocation = None;
                self.leg = guard.as_mut()?.take_sealed();
                if self.release.is_some() {
                    return None;
                }
                if self.parse_sources(app, store, guard.as_mut()?).is_err() {
                    return self.fail(NavigatorError::Store);
                };
                self.phase = Phase::Allocate(After::StartTrim);
                None
            }
            (After::SealTrim, Ok(Outcome::Done)) => {
                self.allocation = None;
                if self.release.is_none() {
                    self.phase = Phase::ReplaceLeg;
                }
                None
            }
            (After::Trimmed, Ok(Outcome::Done)) => {
                self.leg = guard.as_mut()?.take_sealed();
                if let Some(outcome) = self.trimmed.take() {
                    self.rejoin_m = outcome.rejoin_m;
                    self.length_m = outcome.detour_len_m;
                    self.ascent_m = outcome.ascent_m;
                }
                if self.release.is_some() {
                    return None;
                }
                self.start_preview(store, guard.as_mut()?)
            }
            (After::NoTrim, Ok(Outcome::Done)) => {
                self.allocation = None;
                if self.release.is_some() {
                    return None;
                }
                self.start_preview(store, guard.as_mut()?)
            }
            (After::Cancel, Ok(Outcome::Done)) => {
                self.allocation = None;
                None
            }
            (After::DropLeg | After::DropSlot | After::Close, Ok(Outcome::Done)) => None,
            (After::Publish, Ok(Outcome::Published(id))) => {
                self.allocation = None;
                self.published = Some(id);
                if self.release.is_some() {
                    return None;
                }
                crate::flat_store::load_routes(store, app);
                self.token.take().map(|token| NavigatorOutcome::DetourCommitted { token, route: id.0 })
            }
            (After::Compensate, answer) => {
                match answer {
                    Ok(Outcome::Done) | Err(StoreError::NotFound) => self.published = None,
                    Err(StoreError::Media | StoreError::Busy) => {}
                    _ => {
                        defmt::error!("detour: cancellation compensation refused; route remains unaccepted");
                        self.published = None;
                    }
                }
                None
            }
            (_, Err(error)) => {
                if self.release.is_some() {
                    return None;
                }
                self.fail(if error == StoreError::NotFound {
                    NavigatorError::SourceChanged
                } else {
                    NavigatorError::Store
                })
            }
            _ if self.release.is_some() => None,
            _ => self.fail(NavigatorError::Store),
        }
    }
    fn cleanup(
        &mut self,
        retain: bool,
        store: &'static FlatStore<FlatCard>,
        writer: Writer,
        guard: &mut Option<NavGuard>,
        reply: &'static crate::flat_store::Reply,
    ) -> Option<NavigatorOutcome> {
        if let Some(allocation) = self.allocation {
            if let Ok(t) = writer.try_call(Request::Cancel { allocation }, reply) {
                self.phase = Phase::Await(t, After::Cancel);
            }
            return None;
        }
        if let Some(g) = guard.as_mut().filter(|g| g.is_transform()) {
            if let Some(sealed) = g.take_sealed() {
                match writer.try_call_owned(Request::ReleaseSealed { sealed }, reply) {
                    Ok(t) => self.phase = Phase::Await(t, After::DropSlot),
                    Err(Request::ReleaseSealed { sealed }) => g.put_sealed(sealed),
                    _ => unreachable!(),
                }
                return None;
            }
        }
        let keep_preview = retain && self.published.is_none() && self.leg.is_some();
        if !keep_preview {
            if let Some(sealed) = self.leg.take() {
                match writer.try_call_owned(Request::ReleaseSealed { sealed }, reply) {
                    Ok(t) => self.phase = Phase::Await(t, After::DropLeg),
                    Err(Request::ReleaseSealed { sealed }) => self.leg = Some(sealed),
                    _ => unreachable!(),
                }
                return None;
            }
            if let Some(original) = self.original.take() {
                match writer.try_call_owned(Request::Close { handle: original.release() }, reply) {
                    Ok(t) => self.phase = Phase::Await(t, After::Close),
                    Err(Request::Close { handle }) => {
                        self.original = Some(StoreSource::over(store, handle).expect("original mount"))
                    }
                    _ => unreachable!(),
                }
                return None;
            }
        }
        if let Some(id) = self.published.filter(|_| !retain) {
            if let Ok(t) = writer.try_call(Request::RemoveComputedRoute { id, revision: Revision(1) }, reply) {
                self.phase = Phase::Await(t, After::Compensate);
            }
            return None;
        }
        *guard = None;
        self.release = None;
        self.phase = if keep_preview { Phase::Preview } else { Phase::Empty };
        self.published = None;
        self.token.take().map(|token| NavigatorOutcome::Released { token })
    }
}
