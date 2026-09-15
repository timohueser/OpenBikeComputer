//! Host adapter for the shared visit compositor. Only one candidate and one leg exist at a time.
use crate::{NavPlan, VecSink};
use obc_app::navigator::{NavigatorError, ReviewContext, ReviewPurpose};
use obc_formats::io::SliceSource;
use obc_formats::obcr::RouteSourceKey;
use obc_route::visit::{forward_rejoin, VisitBuilder, VisitChoice, VisitTarget};
use obc_route::{RouteIndex, RouteReader, RouteStats};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Plan,
    Append,
    Finish,
    Ready,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    OutAndBack,
    Forward,
    Rebuild,
}

pub struct VisitPlan {
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
    variant: Variant,
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
        let (approach, rejoin_m, forward) = if returning {
            let visit = descriptor.ok_or(NavigatorError::Unavailable)?;
            let rejoin = visit.accepted_anchors_m[2];
            if context.required_anchors_m != [rejoin; 3] {
                return Err(NavigatorError::SourceChanged);
            }
            let at = original.position_at(rejoin).ok_or(NavigatorError::Unavailable)?;
            ((at.lon, at.lat), rejoin, None)
        } else {
            if descriptor.is_some_and(|v| context.progress_m < v.accepted_anchors_m[2]) {
                return Err(NavigatorError::Unavailable);
            }
            let approach = target
                .and_then(|target| target.approach(context.map, context.profile))
                .ok_or(NavigatorError::Unavailable)?;
            (
                approach,
                context.progress_m,
                forward_rejoin(original, context.progress_m).map_err(|_| NavigatorError::Unavailable)?,
            )
        };
        let builder = Self::builder(context, target, approach, rejoin_m)?;
        let mut plan = Self {
            context,
            target,
            approach,
            choice: VisitChoice::new(context.progress_m, forward),
            rejoin_m,
            builder,
            output: VecSink::default(),
            leg: None,
            index: None,
            returning,
            stage: Stage::Plan,
            variant: Variant::OutAndBack,
            stats: None,
        };
        plan.builder.begin(&mut plan.output).map_err(|_| NavigatorError::Store)?;
        plan.start_leg(context.origin, approach)?;
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
            if context.purpose == ReviewPurpose::ReturnToRoute {
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
            }
            .map_err(|_| NavigatorError::Unavailable)
        }
    }
    fn start_leg(&mut self, from: (i32, i32), to: (i32, i32)) -> Result<(), NavigatorError> {
        self.choice.search().map_err(|_| NavigatorError::Unavailable)?;
        let mut plan = NavPlan::start(&obc_app::NavRequest::new(from, to, "Visit leg"), self.context.profile);
        plan.set_attribution_map(self.context.map);
        self.leg = Some(plan);
        self.stage = Stage::Plan;
        Ok(())
    }
    #[inline(never)]
    fn rebuild(&mut self, rejoin: u32, variant: Variant) -> Result<(), NavigatorError> {
        // Discard the old A and B before allocating the next complete variant.
        self.output = VecSink::default();
        self.leg = None;
        self.index = None;
        self.rejoin_m = rejoin;
        self.variant = variant;
        self.returning = false;
        unsafe {
            Self::init_builder(&mut *self.builder, self.context, self.target, self.approach, rejoin)?;
        }
        self.builder.begin(&mut self.output).map_err(|_| NavigatorError::Store)?;
        self.start_leg(self.context.origin, self.approach)
    }
    /// One planner step, one source chunk, or one waypoint. `Some` holds the chosen complete bytes.
    pub fn step(
        &mut self,
        reader: &obc_reader::Reader,
        original: &RouteReader,
        elev: &mut dyn obc_route::ElevationSource,
    ) -> Result<Option<RouteStats>, NavigatorError> {
        match self.stage {
            Stage::Plan => match self.leg.as_mut().ok_or(NavigatorError::Workspace)?.step(reader, elev) {
                obc_route::Step::Running => {}
                obc_route::Step::Failed(error) => {
                    if self.variant == Variant::Forward {
                        self.rebuild(self.context.progress_m, Variant::Rebuild)?;
                    } else {
                        return Err(NavigatorError::Plan(error));
                    }
                }
                obc_route::Step::Done(_) => {
                    let source = SliceSource(self.leg.as_ref().unwrap().bytes());
                    self.index = Some(Box::new(RouteIndex::read(&source).map_err(|_| NavigatorError::Store)?));
                    self.stage = Stage::Append;
                }
            },
            Stage::Append => {
                let source = SliceSource(self.leg.as_ref().ok_or(NavigatorError::Workspace)?.bytes());
                let leg = RouteReader::new(self.index.as_ref().ok_or(NavigatorError::Workspace)?, &source);
                if self.builder.append_leg_step(&leg, &mut self.output).map_err(|_| NavigatorError::Unavailable)? {
                    self.leg = None;
                    self.index = None;
                    if self.returning {
                        self.stage = Stage::Finish;
                    } else {
                        self.returning = true;
                        let to = original.position_at(self.rejoin_m).ok_or(NavigatorError::Unavailable)?;
                        self.start_leg(self.approach, (to.lon, to.lat))?;
                    }
                }
            }
            Stage::Finish => {
                if let Some(stats) =
                    self.builder.finish_step(original, &mut self.output).map_err(|_| NavigatorError::Unavailable)?
                {
                    match self.variant {
                        Variant::OutAndBack if self.choice.forward_m.is_some() => {
                            self.choice.remember_out_and_back(stats.total_distance_m);
                            self.rebuild(self.choice.forward_m.unwrap(), Variant::Forward)?;
                        }
                        Variant::Forward if !self.choice.prefer_forward(stats.total_distance_m) => {
                            self.rebuild(self.context.progress_m, Variant::Rebuild)?;
                        }
                        _ => {
                            self.stats = Some(stats);
                            self.stage = Stage::Ready;
                            return Ok(Some(stats));
                        }
                    }
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
