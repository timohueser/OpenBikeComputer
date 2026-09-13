//! Peak View over the selected map's terrain and geographic summit records.
use crate::arena::PeakGuard;
use core::{cell::Cell, mem::MaybeUninit, ptr::addr_of_mut};
use embassy_time::Instant;
use obc_app::{
    peak_view::{self, Panorama},
    App, PeakViewProfile,
};
use obc_formats::io::{ByteSource, Error, WindowSource};
use obc_reader::{MapTables, Reader};

struct Source {
    window: WindowSource<'static>,
    reads: Cell<u32>,
    bytes: Cell<u32>,
    read_us: Cell<u64>,
}
static mut SOURCE: MaybeUninit<Source> = MaybeUninit::uninit();
impl ByteSource for Source {
    fn len(&self) -> u64 {
        self.window.len()
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let started = Instant::now();
        let result = self.window.read_at(offset, out);
        self.reads.set(self.reads.get() + 1);
        self.bytes.set(self.bytes.get() + out.len() as u32);
        self.read_us.set(self.read_us.get() + started.elapsed().as_micros());
        result
    }
}

#[inline(never)]
fn open(map: &'static dyn ByteSource, tables: &MapTables) -> Option<&'static Source> {
    let region = tables.terrain()?;
    let window = WindowSource::new(map, region.offset, region.len)?;
    obc_elevation::surface::SurfaceReader::parse(&window).ok()?;
    let source = Source { window, reads: Cell::new(0), bytes: Cell::new(0), read_us: Cell::new(0) };
    // SAFETY: initialized once by the ride-loop constructor, then immutable except Cell counters.
    Some(unsafe { (*addr_of_mut!(SOURCE)).write(source) })
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
    work_us: u64,
    reported: u8,
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
        let Some(arm) = crate::arena::claim_peak(&mut profile, source, self.reader) else { return false };
        app.state.peak_view_profile = Some(profile);
        source.reads.set(0);
        source.bytes.set(0);
        source.read_us.set(0);
        job.arm = Some(arm);
        job.work_us = 0;
        job.reported = 0;
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
        job.work_us += started.elapsed().as_micros();
        let progress = arm.builder.progress() / 5;
        if progress > job.reported {
            job.reported = progress;
            let source = job.source.unwrap();
            defmt::info!(
                "peak-perf: {=u8}% wall {=u64} ms; work {=u64} ms; IO {=u64} ms / {=u32} reads / {=u32} bytes",
                progress * 5,
                job.started.elapsed().as_millis(),
                job.work_us / 1000,
                source.read_us.get() / 1000,
                source.reads.get(),
                source.bytes.get()
            );
        }
        if arm.terrain.failed() {
            return Err(Failed);
        }
        if arm.builder.complete() {
            if job.revision == 0 {
                defmt::info!(
                    "peak-view: generated in {=u64} ms; {=u32} cells / {=u32} nodes, {=u32} missing; arena {=usize} B",
                    job.started.elapsed().as_millis(),
                    arm.builder.samples,
                    arm.builder.nodes,
                    arm.builder.missing,
                    core::mem::size_of::<crate::arena::PeakArm>()
                );
            }
            job.revision += 1;
            job.search.refill(&mut arm.builder, self.reader).map_err(|_| Failed)?;
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
            job: Job {
                source,
                arm: None,
                started: Instant::now(),
                work_us: 0,
                reported: 0,
                search: Default::default(),
                revision: 0,
            },
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
