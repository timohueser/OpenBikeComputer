//! Sequential easier-route trials, measured and never stored. Only the candidate the rider
//! selects goes through Navigator's review and is published.
use crate::{
    device_core::{NavigatorTag, OperationToken},
    navigator::{NavigatorError, NavigatorOutcome, ReviewContext, ReviewPurpose, ReviewStatus, VisitUnavailable},
    App, NavRequest,
};
use obc_formats::obcr::RouteSourceKey;
use obc_route::{
    easier::{next_trial, Costs, Goal},
    nav::Objective,
    ElevationSource, RouteReader,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Idle,
    Trials,
    Releasing,
    SelectRelease,
    Rebuild,
    Ready,
    /// The search could not run, or it failed.
    Failed,
    /// Every evaluable goal was searched and none improved.
    NoBetter,
    /// The rider or the route moved on, so the comparison no longer applies.
    Stale,
}
impl Phase {
    fn after(error: crate::navigator::NavigatorError) -> Self {
        if error == crate::navigator::NavigatorError::Movement {
            Self::Stale
        } else {
            Self::Failed
        }
    }
}
/// A deterministic winner; the owner reconstructs its exact candidate before review.
#[derive(Clone, Copy)]
pub(crate) struct Candidate {
    pub objective: Objective,
    pub costs: Costs,
    pub crc: u32,
}
pub(crate) struct State {
    pub context: Option<ReviewContext>,
    pub current: Costs,
    pub choices: [Option<Candidate>; 3],
    pub bounds: obc_map_scene::BBox,
    pub phase: Phase,
    pub trial: u8,
    /// The trial to run after the current one, chosen from its result.
    next: Option<usize>,
    pub selected: u8,
    pub review: bool,
    pub failure: Option<obc_route::NavError>,
    destination: (i32, i32),
}
impl State {
    pub const fn new() -> Self {
        Self {
            context: None,
            current: Costs {
                distance_m: 0,
                ascent_m: 0,
                rough_m: 0,
                unknown_m: 0,
                elevation_complete: false,
                surface_attributed: false,
            },
            choices: [None; 3],
            bounds: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            phase: Phase::Idle,
            trial: 0,
            next: None,
            selected: 0,
            review: false,
            failure: None,
            destination: (0, 0),
        }
    }
}
impl App {
    fn cancel_easier(&mut self) {
        let (Some(mut owned), Some(current)) = (self.easier.context, self.assistant_review_context()) else {
            return;
        };
        if !matches!(current.purpose, ReviewPurpose::Easier(_)) {
            return;
        }
        owned.purpose = current.purpose;
        if owned.original.is_none() {
            owned.original = current.original;
        }
        if owned == current {
            self.cancel_assistant();
        }
    }
    /// Production entry. The executor binds the original and its facts before the first trial.
    pub fn open_easier_routes(&mut self, map: RouteSourceKey) -> Result<(), VisitUnavailable> {
        if self.active_visit() {
            return Err(VisitUnavailable::Avoidance);
        }
        if !self.assistant_planner_released()
            || matches!(
                self.assistant_review_status(),
                ReviewStatus::Planning | ReviewStatus::Preview | ReviewStatus::Saving | ReviewStatus::Unresolved
            )
        {
            return Err(VisitUnavailable::Busy);
        }
        let origin = self.current_review_origin().ok_or(VisitUnavailable::NoFix)?;
        if !origin.trustworthy || self.active_route_index().is_none() {
            return Err(VisitUnavailable::Unmatched);
        }
        self.request_easier(map, Objective::Profile, origin.fix)?;
        let mut state = State::new();
        state.context = self.assistant_review_context();
        state.phase = Phase::Trials;
        self.easier = state;
        if !matches!(self.ui.stack.last(), Some(crate::screen::Screen::Easier(_)))
            && self.ui.stack.push(crate::screen::Screen::Easier(crate::screen::EasierScreen::new())).is_err()
        {
            self.cancel_easier();
            self.easier.phase = Phase::Idle;
            return Err(VisitUnavailable::Busy);
        }
        self.refresh_easier_screen();
        self.ui.map_dirty = true;
        Ok(())
    }
    /// Called by either executor after its exact original-source and map checks.
    pub fn assistant_easier_original(
        &mut self,
        context: ReviewContext,
        route: &RouteReader,
        elev: &mut dyn ElevationSource,
    ) -> bool {
        if !matches!(context.purpose, ReviewPurpose::Easier(_)) {
            return true;
        }
        if route.has_unresolved_avoidance()
            || route
                .visit_descriptor()
                .map_or(true, |v| v.is_some_and(|v| context.progress_m < v.accepted_anchors_m[2]))
        {
            return false;
        }
        if self.easier.phase == Phase::Idle {
            return true;
        }
        let Some(frozen) = self.easier.context else {
            return false;
        };
        if let Some(original) = frozen.original {
            return context.original == Some(original) && context.map == frozen.map;
        }
        let Ok(current) = Costs::remaining(route, context.progress_m, context.map, elev) else {
            return false;
        };
        let Some(end) = route.position_at(route.total_distance_m) else {
            return false;
        };
        self.easier.current = current;
        self.easier.context = Some(context);
        self.easier.destination = (end.lon, end.lat);
        let mut bounds = obc_map_scene::BBox { min_lon: end.lon, max_lon: end.lon, min_lat: end.lat, max_lat: end.lat };
        route.visit_points_between(context.progress_m, route.total_distance_m, |points| {
            for &(lon, lat) in points {
                bounds.min_lon = bounds.min_lon.min(lon);
                bounds.max_lon = bounds.max_lon.max(lon);
                bounds.min_lat = bounds.min_lat.min(lat);
                bounds.max_lat = bounds.max_lat.max(lat);
            }
        });
        self.easier.bounds = bounds;
        true
    }
    /// Use the exact candidate index bounds, not extrema from its decimated display shape. Only the
    /// selected candidate widens the map, so it holds still while trials run.
    pub fn assistant_easier_bounds(
        &mut self,
        source: obc_formats::assistant::PayloadFingerprint,
        bounds: obc_map_scene::BBox,
    ) {
        if self.easier.phase == Phase::Rebuild && self.assistant_preview().is_some_and(|p| p.source == source) {
            let b = &mut self.easier.bounds;
            b.min_lon = b.min_lon.min(bounds.min_lon);
            b.max_lon = b.max_lon.max(bounds.max_lon);
            b.min_lat = b.min_lat.min(bounds.min_lat);
            b.max_lat = b.max_lat.max(bounds.max_lat);
        }
    }
    fn easier_current(&self) -> bool {
        let Some(c) = self.easier.context else { return false };
        self.current_review_origin().is_some_and(|o| c.accepts_origin(self.settings().bike_type, o))
            && c.original.is_none_or(|p| {
                self.active_route_index().and_then(|i| self.route_ids().get(i)).copied() == Some(p.object)
            })
            && !self.active_visit()
    }
    /// A trial is measured, never stored. Only the selected candidate is composed again and
    /// published.
    fn start_easier_trial(&mut self, objective: Objective, measure: bool) {
        let Some(mut context) = self.easier.context else { return };
        context.purpose = ReviewPurpose::Easier(objective);
        let request = NavRequest::new(context.origin, self.easier.destination, "Easier route");
        if measure {
            self.measure_easier(request, context);
        } else {
            self.plan_assistant(request, context);
        }
    }
    pub(crate) fn measure_easier(&mut self, request: NavRequest, context: ReviewContext) {
        self.navigator.request_review(request, context, true);
        self.ui.map_dirty = true;
    }
    /// The executor's answer to a trial: the costs and the stored checksum of the candidate it
    /// composed into a measuring sink.
    pub fn assistant_easier_measured(
        &mut self,
        token: OperationToken<NavigatorTag>,
        costs: Costs,
        crc: u32,
    ) -> NavigatorOutcome {
        let outcome = NavigatorOutcome::ReviewReady { token };
        if !self.navigator.accepts(&outcome) || !self.assistant_measuring() || self.easier.phase != Phase::Trials {
            return NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged };
        }
        let objective = Objective::TRIALS[self.easier.trial as usize];
        let current = self.easier.current;
        for (i, goal) in Goal::ALL.iter().enumerate() {
            if goal.eligible(current, costs)
                && self.easier.choices[i]
                    .is_none_or(|old| goal.saving(current, costs) > goal.saving(current, old.costs))
            {
                self.easier.choices[i] = Some(Candidate { objective, costs, crc });
            }
        }
        self.easier.next = next_trial(self.easier.trial as usize, Some(costs), current);
        self.easier.phase = Phase::Releasing;
        outcome
    }
    pub(crate) fn advance_easier(&mut self) {
        if self.easier.phase == Phase::Idle {
            return;
        }
        if !self.ui.stack.iter().any(|s| matches!(s, crate::screen::Screen::Easier(_))) {
            self.cancel_easier();
            self.easier.phase = Phase::Idle;
            return;
        }
        if self.easier.phase == Phase::Ready
            && self.easier.review
            && self.assistant_review_status() == ReviewStatus::Accepted
        {
            self.easier.phase = Phase::Idle;
            if matches!(self.ui.stack.last(), Some(crate::screen::Screen::Easier(_))) {
                crate::screen::apply(
                    &mut self.ui.stack,
                    crate::screen::Transition::Root(crate::screen::Screen::Map(crate::screen::MapScreen::new())),
                );
            }
            self.ui.map_dirty = true;
            return;
        }
        if !matches!(self.easier.phase, Phase::Failed | Phase::NoBetter | Phase::Stale)
            && !self.easier_current()
            && !matches!(self.assistant_review_status(), ReviewStatus::Saving | ReviewStatus::Unresolved)
        {
            self.cancel_easier();
            self.easier.phase = Phase::Stale;
        }
        if let (Phase::Ready, ReviewStatus::Failed(error)) = (self.easier.phase, self.assistant_review_status()) {
            self.cancel_easier();
            self.easier.phase = Phase::after(error);
        }
        // The executor binds `original` when it measures the original. With no complete terrain no
        // goal can use a candidate, so the first trial stops there.
        if self.easier.phase == Phase::Trials
            && self.easier.context.is_some_and(|c| c.original.is_some())
            && !Goal::LessClimb.evaluable(self.easier.current)
        {
            self.cancel_easier();
            self.easier.phase = Phase::NoBetter;
        }
        match self.easier.phase {
            Phase::Trials | Phase::Rebuild => match self.assistant_review_status() {
                ReviewStatus::Preview if self.easier.phase == Phase::Rebuild => {
                    let Some(preview) = self.assistant_preview() else { return };
                    let expected = self.easier.choices[self.easier.selected as usize];
                    self.easier.context = self.assistant_review_context();
                    if expected.is_some_and(|r| r.crc == preview.source.crc && Some(r.costs) == preview.easier) {
                        self.easier.phase = Phase::Ready;
                    } else {
                        self.cancel_easier();
                        self.easier.phase = Phase::Failed;
                    }
                }
                ReviewStatus::Failed(crate::navigator::NavigatorError::Plan(error))
                    if self.easier.phase == Phase::Trials =>
                {
                    if self.easier.failure != Some(obc_route::NavError::Exhausted) {
                        self.easier.failure = Some(error);
                    }
                    self.easier.next = next_trial(self.easier.trial as usize, None, self.easier.current);
                    self.easier.context = self.assistant_review_context();
                    self.cancel_easier();
                    self.easier.phase = Phase::Releasing;
                }
                ReviewStatus::Failed(error) => {
                    self.cancel_easier();
                    self.easier.phase = Phase::after(error);
                }
                ReviewStatus::Unresolved => {
                    self.cancel_easier();
                    self.easier.phase = Phase::Failed;
                }
                _ => {}
            },
            Phase::SelectRelease if self.assistant_planner_released() => {
                self.start_easier_trial(self.easier.choices[self.easier.selected as usize].unwrap().objective, false);
                self.easier.phase = Phase::Rebuild;
            }
            Phase::Releasing if self.assistant_planner_released() => {
                if let Some(next) = self.easier.next {
                    self.easier.trial = next as u8;
                    self.start_easier_trial(Objective::TRIALS[next], true);
                    self.easier.phase = Phase::Trials;
                } else {
                    for i in 0..3 {
                        for j in 0..i {
                            if self.easier.choices[i]
                                .zip(self.easier.choices[j])
                                .is_some_and(|(a, b)| a.crc == b.crc || a.costs == b.costs)
                            {
                                self.easier.choices[i] = None;
                            }
                        }
                    }
                    if let Some(i) = self.easier.choices.iter().position(Option::is_some) {
                        self.easier.selected = i as u8;
                        // A trial's plan error no longer describes what follows.
                        self.easier.failure = None;
                        self.start_easier_trial(self.easier.choices[i].unwrap().objective, false);
                        self.easier.phase = Phase::Rebuild;
                    } else if self.easier.failure.is_some() {
                        self.easier.phase = Phase::Failed;
                    } else {
                        self.easier.phase = Phase::NoBetter;
                    }
                }
            }
            _ => {}
        }
        self.refresh_easier_screen();
    }
    pub(crate) fn easier_gesture(&mut self, g: crate::Gesture) -> bool {
        if !matches!(self.ui.stack.last(), Some(crate::screen::Screen::Easier(_))) {
            return false;
        }
        match g {
            crate::Gesture::Back if self.easier.review => self.easier.review = false,
            crate::Gesture::Back | crate::Gesture::BackHold => {
                self.cancel_easier();
                self.easier.phase = Phase::Idle;
                self.ui.stack.pop();
                if g == crate::Gesture::BackHold {
                    return false;
                }
            }
            crate::Gesture::Press
                if self.easier.phase == Phase::Ready
                    && self.assistant_review_status() == ReviewStatus::Preview
                    && self.easier_current() =>
            {
                if self.easier.review {
                    if let Some(origin) = self.current_review_origin() {
                        self.accept_assistant(origin);
                    }
                } else {
                    self.easier.review = true;
                }
            }
            crate::Gesture::Press if self.easier.phase == Phase::Stale => {
                if let Some(context) = self.easier.context {
                    // A refusal keeps the stale page, so a later press can retry.
                    let _ = self.open_easier_routes(context.map);
                }
            }
            crate::Gesture::Step(delta) if self.easier.phase == Phase::Ready && !self.easier.review => {
                let step = if delta < 0 { 2 } else { 1 };
                let mut i = self.easier.selected as usize;
                for _ in 0..3 {
                    i = (i + step) % 3;
                    if self.easier.choices[i].is_some() {
                        break;
                    }
                }
                if i != self.easier.selected as usize {
                    self.easier.selected = i as u8;
                    self.cancel_easier();
                    // A distinct selected request waits for the same physical release acknowledgement.
                    self.easier.phase = Phase::SelectRelease;
                }
            }
            _ => {}
        }
        self.refresh_easier_screen();
        self.ui.map_dirty = true;
        true
    }
    fn refresh_easier_screen(&mut self) {
        let status = self.assistant_review_status();
        if let Some(crate::screen::Screen::Easier(screen)) =
            self.ui.stack.iter_mut().find(|s| matches!(s, crate::screen::Screen::Easier(_)))
        {
            self.ui.map_dirty |= screen.update(&self.easier, status);
        }
    }
}
