//! Heap-backed arena facade; all planning and transforms use production algorithms.
use crate::{detour::TransformStep, flat_store::Request, ride::NavStageSink};
use obc_formats::io::{ByteSource, Error};
use obc_route::{NavPlanner, NavScratch, RouteIndex, RouteReader, Splicer, Trimmer};
use obc_storage::flat::{Allocation, SealedAllocation};

pub const NAV_OUTPUT_STAGE_BYTES: usize = 16 * 1024;
enum Work {
    Sources,
    Plan(Box<NavPlanner>),
    Trim(Box<Trimmer>),
    Splice(Box<Splicer>),
}
pub struct NavGuard {
    work: Work,
    original: RouteIndex,
    leg: RouteIndex,
    scratch: Box<NavScratch>,
    tiles: obc_reader::NavTileCache,
    output: Box<[u8; NAV_OUTPUT_STAGE_BYTES]>,
    sealed: Box<Option<SealedAllocation<'static>>>,
    preview_chunk: usize,
    visit: Option<Box<obc_route::visit::VisitBuilder>>,
    measure: Box<obc_route::easier::Measure>,
}
pub fn claim_nav(_: obc_app::MapQuiesced) -> Result<NavGuard, ()> {
    Ok(NavGuard {
        work: Work::Sources,
        original: RouteIndex::empty(),
        leg: RouteIndex::empty(),
        scratch: NavScratch::new_boxed(),
        tiles: obc_reader::NavTileCache::new(),
        output: Box::new([0; NAV_OUTPUT_STAGE_BYTES]),
        sealed: Box::new(None),
        preview_chunk: 0,
        visit: None,
        measure: Box::default(),
    })
}
impl Drop for NavGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(self.sealed.is_none(), "sealed slot must drain before arena release");
        }
    }
}
impl NavGuard {
    pub fn begin_sources(&mut self) {
        assert!(self.sealed.is_none());
        self.work = Work::Sources;
        self.original = RouteIndex::empty();
        self.leg = RouteIndex::empty();
        self.output.fill(0);
    }
    pub fn sources(&mut self) -> (&mut RouteIndex, &mut RouteIndex) {
        (&mut self.original, &mut self.leg)
    }
    pub fn restore_plan(&mut self) {
        self.tiles.reset();
    }
    pub fn begin_plan(&mut self, planner: NavPlanner) {
        self.work = Work::Plan(Box::new(planner));
    }
    pub fn plan_parts(
        &mut self,
    ) -> Option<(&mut NavPlanner, &mut NavScratch, &mut obc_reader::NavTileCache, &mut [u8; NAV_OUTPUT_STAGE_BYTES])>
    {
        let Work::Plan(planner) = &mut self.work else { return None };
        Some((planner, &mut self.scratch, &mut self.tiles, &mut self.output))
    }
    pub fn begin_trim(&mut self, leg: obc_route::Leg, target: u32, elevation: bool) {
        self.work = Work::Trim(Box::new(Trimmer::new(leg, target, elevation)));
    }
    pub fn begin_splice(&mut self, leg: obc_route::Leg, split: u32, rejoin: u32, len: u32, elevation: bool) {
        self.work = Work::Splice(Box::new(Splicer::new(leg, split, rejoin, len, elevation, self.original.name())));
    }
    pub fn transform(&mut self, original: &dyn ByteSource, leg: &dyn ByteSource) -> (TransformStep, usize, usize) {
        let orig = RouteReader::new(&self.original, original);
        let leg = RouteReader::new(&self.leg, leg);
        let mut sink = NavStageSink { stage: &mut self.output, appended: 0, patch_len: 0 };
        let step = match &mut self.work {
            Work::Trim(t) => TransformStep::Trim(t.step(&orig, &leg, &mut sink)),
            Work::Splice(s) => TransformStep::Splice(s.step(&orig, &leg, &mut sink)),
            _ => panic!("not a transform"),
        };
        (step, sink.appended, sink.patch_len)
    }
    pub fn output(&self) -> &[u8; NAV_OUTPUT_STAGE_BYTES] {
        &self.output
    }
    pub fn seal_request(&mut self, allocation: Allocation) -> Request {
        assert!(self.sealed.is_none());
        // The actual executor retains this guard until the writer ticket drains.
        let out = unsafe { &mut *(&mut *self.sealed as *mut Option<SealedAllocation<'static>>) };
        Request::Seal { allocation, out }
    }
    pub fn is_transform(&self) -> bool {
        !matches!(self.work, Work::Plan(_))
    }
    pub fn take_sealed(&mut self) -> Option<SealedAllocation<'static>> {
        self.sealed.take()
    }
    pub fn put_sealed(&mut self, value: SealedAllocation<'static>) {
        assert!(self.sealed.replace(value).is_none());
    }
    pub fn begin_preview(&mut self) {
        self.work = Work::Sources;
        self.preview_chunk = 0;
    }
    pub fn preview_step(&mut self, source: &dyn ByteSource, app: &mut obc_app::App) -> Result<bool, Error> {
        let reader = RouteReader::new(&self.leg, source);
        if self.preview_chunk == reader.chunks().len() {
            return Ok(true);
        }
        let mut points = heapless::Vec::<_, { obc_route::MAX_POINTS_PER_CHUNK }>::new();
        reader.decode_chunk(self.preview_chunk, &mut points)?;
        // These fixtures have one short chunk; rendering/sampling is not under test here.
        let preview: Vec<_> = points.iter().map(|p| (p.lon, p.lat)).collect();
        app.set_detour_preview(&preview);
        self.preview_chunk += 1;
        Ok(false)
    }
}

impl NavGuard {
    pub fn begin_visit(
        &mut self,
        c: obc_app::navigator::ReviewContext,
        target: Option<obc_route::visit::VisitTarget>,
        rejoin: u32,
    ) -> Result<(), Error> {
        let original = c.original.ok_or(Error::BadOffset)?;
        let key = obc_formats::obcr::RouteSourceKey {
            store: c.store.bytes(),
            object: original.object,
            revision: original.revision,
        };
        let mut slot = Box::<obc_route::visit::VisitBuilder>::new_uninit();
        unsafe {
            if matches!(c.purpose, obc_app::navigator::ReviewPurpose::Easier(_)) {
                obc_route::visit::VisitBuilder::init_easier_in_place(slot.as_mut_ptr(), key, c.map, c.progress_m)?;
            } else if c.purpose == obc_app::navigator::ReviewPurpose::ReturnToRoute {
                obc_route::visit::VisitBuilder::init_return_in_place(slot.as_mut_ptr(), key, c.map, rejoin)?;
            } else {
                let target = target.ok_or(Error::BadOffset)?;
                obc_route::visit::VisitBuilder::init_in_place(
                    slot.as_mut_ptr(),
                    key,
                    c.map,
                    c.progress_m,
                    rejoin,
                    target.metadata.source,
                    target.approach(c.map, c.profile).ok_or(Error::BadOffset)?,
                )?;
            }
            self.visit = Some(slot.assume_init());
        }
        *self.measure = obc_route::easier::Measure::new();
        self.begin_sources();
        Ok(())
    }
    pub fn visit_begin_sources(&mut self) {
        self.begin_sources();
    }
    pub fn visit_begin_plan(&mut self, from: (i32, i32), to: (i32, i32), c: obc_app::navigator::ReviewContext) {
        self.restore_plan();
        let mut planner = NavPlanner::new(from, to, "Visit leg", c.profile);
        planner.set_attribution_map(c.map);
        if let obc_app::navigator::ReviewPurpose::Easier(objective) = c.purpose {
            planner.set_objective(objective);
        }
        self.begin_plan(planner);
    }
    pub fn visit_plan_parts(
        &mut self,
    ) -> (&mut NavPlanner, &mut NavScratch, &mut obc_reader::NavTileCache, &mut [u8; NAV_OUTPUT_STAGE_BYTES]) {
        self.plan_parts().unwrap()
    }
    pub fn visit_parts(
        &mut self,
    ) -> (&mut obc_route::visit::VisitBuilder, &mut RouteIndex, &mut RouteIndex, &mut [u8; NAV_OUTPUT_STAGE_BYTES])
    {
        (self.visit.as_mut().unwrap(), &mut self.original, &mut self.leg, &mut self.output)
    }
    pub fn visit_measure_parts(
        &mut self,
    ) -> (&mut obc_route::visit::VisitBuilder, &mut RouteIndex, &mut RouteIndex, &mut obc_route::easier::Measure) {
        (self.visit.as_mut().unwrap(), &mut self.original, &mut self.leg, &mut self.measure)
    }
    pub fn visit_seal_request(&mut self, allocation: Allocation) -> Request {
        self.seal_request(allocation)
    }
    pub fn visit_take_sealed(&mut self) -> Option<SealedAllocation<'static>> {
        self.take_sealed()
    }
}
