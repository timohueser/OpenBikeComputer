//! Host adapter for the shared visit compositor. Only one candidate and one leg exist at a time.
use crate::{NavPlan, VecSink};
use obc_app::navigator::{NavigatorError, ReviewContext, ReviewPurpose};
use obc_formats::io::SliceSource;
use obc_formats::obcr::RouteSourceKey;
use obc_route::visit::{visit_anchor, LegCost, VisitBuilder, VisitChoice, VisitLegs, VisitTarget};
use obc_route::{RouteIndex, RouteReader, RouteStats};

/// The leg searches of one Find candidate, kept only for their figures: out to the place and,
/// for a visit, back to where the outbound leg left the route.
pub struct LegMeasure {
    leg: NavPlan,
    from: (i32, i32),
    context: ReviewContext,
    outbound: Option<LegCost>,
}
impl LegMeasure {
    pub fn start(request: &obc_app::NavRequest, context: ReviewContext) -> Self {
        Self { leg: Self::leg(request.from, request.to, context), from: request.from, context, outbound: None }
    }
    fn leg(from: (i32, i32), to: (i32, i32), context: ReviewContext) -> NavPlan {
        let mut plan = NavPlan::start(&obc_app::NavRequest::new(from, to, "Visit leg"), context.profile);
        plan.set_attribution_map(context.map);
        plan
    }
    /// One planner step. `Some` holds both legs' figures.
    pub fn step(
        &mut self,
        reader: &obc_reader::Reader,
        elev: &mut dyn obc_route::ElevationSource,
    ) -> Result<Option<VisitLegs>, NavigatorError> {
        let stats = match self.leg.step(reader, elev) {
            obc_route::Step::Running => return Ok(None),
            obc_route::Step::Failed(error) => return Err(NavigatorError::Plan(error)),
            obc_route::Step::Done(stats) => LegCost::from(stats),
        };
        match self.outbound {
            None if self.context.purpose == ReviewPurpose::Visit => {
                self.outbound = Some(stats.joined_at(self.from, self.leg.snapped_start()));
                self.leg = Self::leg(self.leg.snapped_goal(), self.from, self.context);
                Ok(None)
            }
            None => Ok(Some(VisitLegs { outbound: stats, back: None })),
            Some(outbound) => {
                Ok(Some(VisitLegs { outbound, back: Some(stats.joined_at(self.from, self.leg.snapped_goal())) }))
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Prefix,
    Plan,
    Append,
    Finish,
    Ready,
}
pub struct VisitPlan {
    /// An easier trial: the composed bytes are measured and dropped, never published.
    pub measure: bool,
    context: ReviewContext,
    target: Option<VisitTarget>,
    approach: (i32, i32),
    choice: VisitChoice,
    rejoin_m: u32,
    builder: Box<VisitBuilder>,
    output: VecSink,
    leg: Option<NavPlan>,
    index: Option<Box<RouteIndex>>,
    returning: bool,
    stage: Stage,
    stats: Option<RouteStats>,
}
impl VisitPlan {
    #[inline(never)]
    pub fn start(
        context: ReviewContext,
        target: Option<VisitTarget>,
        original: &RouteReader,
    ) -> Result<Self, NavigatorError> {
        let returning = context.purpose == ReviewPurpose::ReturnToRoute;
        let descriptor = original.visit_descriptor().map_err(|_| NavigatorError::Unavailable)?;
        if original.has_unresolved_avoidance() {
            return Err(NavigatorError::Unavailable);
        }
        let easier = matches!(context.purpose, ReviewPurpose::Easier(_));
        let (approach, rejoin_m) = if easier {
            (context.origin, context.progress_m)
        } else if returning {
            let visit = descriptor.ok_or(NavigatorError::Unavailable)?;
            let rejoin = visit.accepted_anchors_m[2];
            if context.required_anchors_m != [rejoin; 3] {
                return Err(NavigatorError::SourceChanged);
            }
            let at = original.position_at(rejoin).ok_or(NavigatorError::Unavailable)?;
            ((at.lon, at.lat), rejoin)
        } else {
            if descriptor.is_some_and(|v| context.progress_m < v.accepted_anchors_m[2]) {
                return Err(NavigatorError::Unavailable);
            }
            let approach = target
                .and_then(|target| target.approach(context.map, context.profile))
                .ok_or(NavigatorError::Unavailable)?;
            (approach, visit_anchor(original, context.progress_m, approach).map_err(|_| NavigatorError::Unavailable)?)
        };
        let builder = Self::builder(context, target, approach, rejoin_m)?;
        let mut plan = Self {
            measure: false,
            context,
            target,
            approach,
            choice: VisitChoice::new(),
            rejoin_m,
            builder,
            output: VecSink::default(),
            leg: None,
            index: None,
            returning,
            stage: Stage::Plan,
            stats: None,
        };
        plan.builder.begin(&mut plan.output).map_err(|_| NavigatorError::Store)?;
        if easier {
            plan.builder.prepare_easier(original).map_err(|_| NavigatorError::Unavailable)?;
            let (from, to) = plan
                .builder
                .easier_leg(original, context.origin)
                .map_err(|_| NavigatorError::Unavailable)?
                .ok_or(NavigatorError::Unavailable)?;
            plan.start_leg(from, to)?;
        } else if returning {
            plan.start_leg(context.origin, approach)?;
        } else {
            plan.stage = Stage::Prefix;
        }
        Ok(plan)
    }
    #[inline(never)]
    fn builder(
        context: ReviewContext,
        target: Option<VisitTarget>,
        approach: (i32, i32),
        rejoin: u32,
    ) -> Result<Box<VisitBuilder>, NavigatorError> {
        let mut slot = Box::<VisitBuilder>::new_uninit();
        unsafe {
            Self::init_builder(slot.as_mut_ptr(), context, target, approach, rejoin)?;
            Ok(slot.assume_init())
        }
    }
    unsafe fn init_builder(
        slot: *mut VisitBuilder,
        context: ReviewContext,
        target: Option<VisitTarget>,
        approach: (i32, i32),
        rejoin: u32,
    ) -> Result<(), NavigatorError> {
        let original = context.original.ok_or(NavigatorError::Unavailable)?;
        let source =
            RouteSourceKey { store: context.store.bytes(), object: original.object, revision: original.revision };
        unsafe {
            if matches!(context.purpose, ReviewPurpose::Easier(_)) {
                VisitBuilder::init_easier_in_place(slot, source, context.map, context.progress_m)
            } else if context.purpose == ReviewPurpose::ReturnToRoute {
                VisitBuilder::init_return_in_place(slot, source, context.map, rejoin)
            } else {
                let target = target.ok_or(NavigatorError::Unavailable)?;
                VisitBuilder::init_in_place(
                    slot,
                    source,
                    context.map,
                    context.progress_m,
                    rejoin,
                    target.metadata.source,
                    approach,
                )
                .and_then(|()| (*slot).keep_prefix(rejoin))
            }
            .map_err(|_| NavigatorError::Unavailable)
        }
    }
    fn start_leg(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), NavigatorError> {
        self.choice
            .search(!matches!(self.context.purpose, ReviewPurpose::Easier(_)))
            .map_err(|_| NavigatorError::Unavailable)?;
        let mut plan = NavPlan::start(&obc_app::NavRequest::new(from, to, "Visit leg"), self.context.profile);
        plan.set_attribution_map(self.context.map);
        if let ReviewPurpose::Easier(objective) = self.context.purpose {
            plan.set_objective(objective);
        }
        self.leg = Some(plan);
        self.stage = Stage::Plan;
        Ok(())
    }
    /// One planner step, one source chunk, or one waypoint. `Some` holds the chosen complete bytes.
    pub fn step(
        &mut self,
        reader: &obc_reader::Reader,
        original: &RouteReader,
        elev: &mut dyn obc_route::ElevationSource,
    ) -> Result<Option<RouteStats>, NavigatorError> {
        match self.stage {
            Stage::Prefix => {
                if self
                    .builder
                    .append_prefix_step(original, &mut self.output)
                    .map_err(|_| NavigatorError::Unavailable)?
                {
                    let from = original.position_at(self.rejoin_m).ok_or(NavigatorError::Unavailable)?;
                    self.start_leg((from.lon, from.lat), self.approach)?;
                }
            }
            Stage::Plan => match self.leg.as_mut().ok_or(NavigatorError::Workspace)?.step(reader, elev) {
                obc_route::Step::Running => {}
                obc_route::Step::Failed(error) => {
                    return Err(NavigatorError::Plan(error));
                }
                obc_route::Step::Done(_) => {
                    let source = SliceSource(self.leg.as_ref().unwrap().bytes());
                    self.index = Some(Box::new(RouteIndex::read(&source).map_err(|_| NavigatorError::Store)?));
                    if self.context.purpose == ReviewPurpose::Visit && !self.returning {
                        self.builder
                            .resolve_destination(
                                self.target.ok_or(NavigatorError::Unavailable)?,
                                &source,
                                self.context.profile,
                            )
                            .map_err(|_| NavigatorError::Unavailable)?;
                        self.approach = self.builder.destination().ok_or(NavigatorError::Unavailable)?;
                    }
                    self.stage = Stage::Append;
                }
            },
            Stage::Append => {
                let source = SliceSource(self.leg.as_ref().ok_or(NavigatorError::Workspace)?.bytes());
                let leg = RouteReader::new(self.index.as_ref().ok_or(NavigatorError::Workspace)?, &source);
                let appended = self.builder.append_leg_step(&leg, &mut self.output);
                if appended.map_err(|_| NavigatorError::Unavailable)? {
                    self.leg = None;
                    self.index = None;
                    if matches!(self.context.purpose, ReviewPurpose::Easier(_)) {
                        self.builder.finish_easier_leg(original).map_err(|_| NavigatorError::Unavailable)?;
                        match self
                            .builder
                            .easier_leg(original, self.context.origin)
                            .map_err(|_| NavigatorError::Unavailable)?
                        {
                            Some((from, to)) => self.start_leg(from, to)?,
                            None => self.stage = Stage::Finish,
                        }
                    } else if self.returning {
                        self.stage = Stage::Finish;
                    } else {
                        self.returning = true;
                        let to = original.position_at(self.rejoin_m).ok_or(NavigatorError::Unavailable)?;
                        self.start_leg(self.approach, (to.lon, to.lat))?;
                    }
                }
            }
            Stage::Finish => {
                let finished = self.builder.finish_step(original, &mut self.output);
                if let Some(stats) = finished.map_err(|_| NavigatorError::Unavailable)? {
                    self.stats = Some(stats);
                    self.stage = Stage::Ready;
                    return Ok(Some(stats));
                }
            }
            Stage::Ready => return Ok(self.stats),
        }
        Ok(None)
    }
    pub fn bytes(&self) -> &[u8] {
        self.output.bytes()
    }
    pub fn original_anchors(&self) -> [u32; 3] {
        self.builder.original_anchors()
    }
    pub fn searches(&self) -> u8 {
        self.choice.searches()
    }
}
