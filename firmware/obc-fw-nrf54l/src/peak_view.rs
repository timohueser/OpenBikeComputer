//! Peak View over the selected map's terrain and geographic summit records.
use crate::arena::PeakGuard;
use core::{mem::MaybeUninit, ptr::addr_of_mut};
use embassy_time::Instant;
use obc_app::{
    peak_view::{self, Panorama},
    App, PeakViewProfile,
};
use obc_formats::io::{ByteSource, WindowSource};
use obc_reader::{MapTables, Reader};

type Source = WindowSource<'static>;
static mut SOURCE: MaybeUninit<Source> = MaybeUninit::uninit();

#[inline(never)]
fn open(map: &'static dyn ByteSource, tables: &MapTables) -> Option<&'static Source> {
    let region = tables.terrain()?;
    let window = WindowSource::new(map, region.offset, region.len)?;
    obc_elevation::surface::SurfaceReader::parse(&window).ok()?;
    // SAFETY: initialized once by the ride-loop constructor, then immutable.
    Some(unsafe { (*addr_of_mut!(SOURCE)).write(window) })
}

use peak_view::runtime::{Failed, Lifecycle, Platform, Progress};

pub(crate) struct Runtime {
    lifecycle: Lifecycle,
    job: Job,
}
struct Job {
    source: Option<&'static Source>,
    arm: Option<PeakGuard>,
    started: Instant,
    search: peak_view::SummitSearch,
    revision: u64,
}
struct Hook<'a, 'r> {
    job: &'a mut Job,
    reader: &'a Reader<'r>,
}
impl Platform for Hook<'_, '_> {
    fn start(&mut self, app: &mut App, position: (i32, i32)) -> bool {
        let job = &mut self.job;
        job.started = Instant::now();
        let Some(source) = job.source else { return false };
        let mut profile = PeakViewProfile::at(position.0, position.1, 0);
        profile.default_heading_q4 = app.peak_view_heading_q4();
        let measured = app.recorder.fused_elevation_m();
        let Some(arm) = crate::arena::claim_peak(&mut profile, source, self.reader, measured) else {
            defmt::warn!("peak-view: could not start at {=i32},{=i32}", position.0, position.1);
            return false;
        };
        app.state.peak_view_profile = Some(profile);
        job.arm = Some(arm);
        job.search = Default::default();
        job.revision = 0;
        true
    }

    fn cancel(&mut self) {
        self.job.arm = None;
    }

    #[inline(never)]
    fn step(&mut self, app: &mut App) -> Result<Progress, Failed> {
        let job = &mut self.job;
        let arm = job.arm.as_deref_mut().ok_or(Failed)?;
        arm.builder.set_heading(app.peak_view_heading_q4());
        let started = Instant::now();
        while !arm.builder.complete() && started.elapsed().as_millis() < 50 {
            arm.builder.step(&mut arm.terrain, 16);
        }
        if arm.terrain.failed() {
            defmt::warn!(
                "peak-view: terrain failed after {=u64} ms, progress {=u8}",
                job.started.elapsed().as_millis(),
                arm.builder.progress()
            );
            return Err(Failed);
        }
        if arm.builder.complete() {
            if job.revision == 0 {
                defmt::info!("peak-view: generated in {=u64} ms", job.started.elapsed().as_millis());
            }
            job.revision += 1;
            job.search.refill(&mut arm.builder, self.reader).map_err(|error| {
                defmt::warn!("peak-view: summit refill failed: {}", defmt::Debug2Format(&error));
                Failed
            })?;
            if arm.builder.complete() {
                defmt::info!("peak-view: ready in {=u64} ms", job.started.elapsed().as_millis());
            }
        }
        for (out, peak) in app.state.peak_view_peaks.iter_mut().zip(arm.builder.display_peaks()) {
            *out = peak;
        }
        app.state.peak_view_peak_count = arm.builder.peaks.len() as u8;
        Ok(Progress {
            complete: arm.builder.complete(),
            revision: job.revision * 101 + u64::from(arm.builder.progress()),
        })
    }
}
impl Runtime {
    #[inline(never)]
    pub fn new(app: &mut App, map: &'static dyn ByteSource, tables: &MapTables) -> Self {
        let source = open(map, tables);
        app.state.peak_view_profile = source.map(|_| PeakViewProfile::at(0, 0, 0));
        #[cfg(feature = "peak-view-demo")]
        {
            app.state.user_fix =
                Some(obc_ports::Fix { lat: 46_585_000, lon: 7_961_000, course: None, speed_mps: Some(0.0) });
            app.state.compass_deg = Some(141.25);
            app.show_peak_view();
        }
        Self {
            lifecycle: Lifecycle::default(),
            job: Job { source, arm: None, started: Instant::now(), search: Default::default(), revision: 0 },
        }
    }

    pub fn reconcile(&mut self, app: &App) {
        if self.lifecycle.reconcile(app) {
            self.job.arm = None;
        }
    }
    pub fn panorama(&self) -> Option<&Panorama> {
        self.job.arm.as_ref().map(|arm| &arm.builder.panorama)
    }
    pub fn busy(&self) -> bool {
        self.lifecycle.busy()
    }
    pub fn note_frame_presented(&mut self, app: &App) {
        if let Some(ms) = self.lifecycle.note_presented(app, Instant::now().as_millis()) {
            defmt::info!("peak-view: first view in {=u64} ms", ms);
        }
    }
    pub fn update(&mut self, app: &mut App, reader: &Reader<'_>) {
        self.lifecycle.update(app, &mut Hook { job: &mut self.job, reader }, Instant::now().as_millis());
    }
}
