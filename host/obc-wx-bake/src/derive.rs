//! **Derived sources** (WXR9 #1251): the two jobs the motion engine does for the cycle.
//!
//! [`crate::flow`] is the physics; this is where it meets the mosaic. Both jobs run between
//! `adapter.bake()` and `Mosaic::from_sources`, both produce ordinary [`BakedSource`]/[`BakedFrame`]
//! values on the parent source's own window, and neither needs a special case anywhere downstream —
//! which is #1251's own test of whether WXR3's mosaic was built widely enough ("if nowcasting needs
//! its own code path through the publisher, something in WXR3 was built too narrowly"). It does not.
//!
//! ## Job B: a genuine estimate at every canonical instant ([`uniform_frames`])
//!
//! The epic promises a uniform 15-minute dataset. Spatially that is honest today — a 27.75 km cell
//! replicated across 750 canonical cells is coarse but truthful. **Temporally it was not.** GFS and
//! ICON-EU publish hourly steps, so [`MosaicLayer::nearest`](crate::canonical::MosaicLayer) hands
//! the 11:00 step to 10:30, 10:45, 11:00 *and* 11:15, and in a GFS-only region — most of the planet
//! — the timeline changes once an hour and stands still in between. Four frames, one statement.
//!
//! So the gap gets filled properly: estimate the motion between the two bracketing steps and
//! [`morph`](crate::flow::morph) both of them to the instant in the middle. The result is a
//! forecast for *that* instant rather than a neighbouring one, which is the same standard the
//! forward-frames rule (#1248) holds observations to, finally applied to the model steps as well.
//! It closes the honesty gap that rule left open: `valid_at` is the cadence instant, and now the
//! data genuinely is an estimate for it.
//!
//! ## Job A: the radar nowcast ([`radar_nowcast`])
//!
//! #1251's original scope. Where a radar source carries an earlier observation as well as its
//! anchor ([`BakedSource::motion_history`]), the field is advected forward to +15 … +
//! [`NOWCAST_MAX_LEAD_MIN`] minutes and offered as its own source — ranked above the models and
//! below every radar observation. That is the proper fix for the quality cliff at f+15 that #1248
//! exposed: over CONUS the forward frames stop being 3 km HRRR and become the 1 km radar image,
//! moved.
//!
//! ## Where motion cannot be estimated
//!
//! Every fallback in this module is *do nothing*, and that is deliberate. A source with one frame,
//! a dry field, a pair too far apart to correlate, a slot with no bracketing pair — each of these
//! leaves the mosaic exactly as it was, which is a well-defined and already-shipped state: the
//! nearest model step within [`MAX_FRAME_SKEW_S`](crate::canonical::MAX_FRAME_SKEW_S), or intensity
//! 15 if there is none. Nothing here can make the dataset worse than the day before it landed, and
//! nothing here fabricates motion to avoid admitting it has none.

use crate::canonical::{CycleTimes, FRAME_STEP_MIN, MAX_FRAME_SKEW_S};
use crate::flow::{self, FlowParams};
use crate::source::{nowcast_of, BakedFrame, BakedSource, SourceClass};

/// **The published nowcast horizon**, in minutes, and the answer to the handoff question #1251
/// makes this issue own.
///
/// Advection skill decays and model skill does not, so there is a lead time past which the model is
/// the better answer and the nowcast must stop being published — "nowcast where we have radar" is
/// not automatically right at +120. Three shapes were available (a fixed cap, a lead-weighted
/// blend, or a per-cycle skill-driven switch) and this is a **fixed cap**, for reasons that are
/// about verifiability rather than elegance:
///
/// * a **blend** of two intensity-quantized fields produces codes neither source stated, over ground
///   with no provenance channel to explain them (#1242), and it makes the published frame a
///   function of a weight nobody downstream can see;
/// * a **skill-driven** switch needs verification data for the cycle being baked, and the truth for
///   the next two hours is, definitionally, in the future. It could only ever switch on *last*
///   cycle's skill, which is a different question asked about a different storm;
/// * a **fixed cap** is a number that can be measured once, argued about, and re-measured when a
///   pack disagrees with it. `tests/nowcast_skill.rs` is that measurement and it fails if this
///   constant promises skill the derecho pack does not show.
///
/// **Sixty minutes, and it is bound by memory rather than by skill — which is worth saying out
/// loud, because the two would give different answers.**
///
/// On the 2020-08-10 derecho pack the advected radar beats the 3 km model at every lead the truth
/// ladder reaches, by a margin that is not close (CSI at `>= 0.25 mm/h`: 0.60 against 0.26 at +60,
/// 0.52 against 0.22 at +90), and it beats frozen persistence everywhere too. The evidence supports
/// at least +90 and shows no crossover inside two hours.
///
/// What stops it there is `MemoryMax=1G`. A nowcast frame is a full copy of its parent's window —
/// 24.5 MB for MRMS, 25 MB for OPERA CIRRUS — so each extra 15 minutes of horizon is ~50 MB
/// resident for the whole cycle. Measured (`tests/nowcast_cost.rs`): 694 MB peak at this horizon,
/// ~793 MB at +90, ~892 MB at +120, against WXR1's 398 MB baseline. Two thirds of the ceiling with
/// a third in hand is a service; nine tenths of it is a cycle that OOMs on the first frame that
/// compresses badly.
///
/// So the number to move is not this one. **The follow-up that unlocks +120 is a lazy nowcast
/// layer** — a `SourceFrame` that holds the anchor plus the motion field and advects per shard
/// inside `Mosaic::fill`, at which point the horizon costs no resident memory at all and this
/// constant goes back to being purely a skill question. Until then, raising it means re-running
/// `tests/nowcast_skill.rs` on a *second* event (the derecho is the most advection-friendly case in
/// the catalogue and one favourable event is not a horizon) **and** re-running
/// `tests/nowcast_cost.rs`. It is not a taste parameter.
pub const NOWCAST_MAX_LEAD_MIN: u32 = 60;

/// The observation pair's separation must sit in this range for the motion to be trustworthy.
///
/// Too short and the displacement is a cell or two, which is mostly quantization noise; too long
/// and the field has evolved enough that the two frames are not pictures of the same thing and the
/// correlation the estimator relies on is gone. Ten minutes is the target and the window is
/// generous around it, because upstream cadences differ and an observation can be missing.
pub const MIN_MOTION_DT_S: i64 = 120;
pub const MAX_MOTION_DT_S: i64 = 1_800;

/// A canonical slot is "already answered" by a frame this close to it, and gets no derived frame.
/// One minute: close enough that morphing across it would be arithmetic for its own sake.
pub const SLOT_TOLERANCE_S: i64 = 60;

/// The widest gap job B will morph across. Two hours covers an hourly source that dropped a step;
/// beyond that the two ends are not the same weather and interpolating between them would be an
/// invention dressed as an estimate.
pub const MAX_BRACKET_S: i64 = 2 * 3_600;

/// What one cycle's derivation actually managed — reported, never warned.
///
/// The distinction is deliberate. Interpolating six frames onto the cadence is what this module
/// does on every healthy cycle, and a warning that fires every fifteen minutes forever is a warning
/// nobody reads. A *missing* upstream is a warning, and it is raised where it is discovered — in the
/// adapter that could not fetch the motion-history object — rather than a second time here.
///
/// What belongs in a report and not a warning is still worth having: `interpolated` suddenly going
/// to zero is an upstream that stopped publishing consecutive steps, and `skipped` naming a source
/// every cycle is a nowcast that is configured but never produced.
#[derive(Debug, Default, Clone)]
pub struct DeriveReport {
    /// `(source id, frames added)` for job B.
    pub interpolated: Vec<(String, usize)>,
    /// `(derived source id, forward frames)` for job A.
    pub nowcasts: Vec<(String, usize)>,
    /// `(parent source id, why not)` — a source with a nowcast row that produced none.
    pub skipped: Vec<(String, String)>,
    /// Cells freed by [`release_unusable`] — motion history, and frames outside the cycle's reach.
    pub released: usize,
}

impl DeriveReport {
    /// One line per derivation, for [`crate::canonical::CycleReport::summary`]. Empty when nothing
    /// was derived, so a cycle with no coarse sources reads exactly as it did before WXR9.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (id, frames) in &self.nowcasts {
            lines.push(format!("  {id}: {frames} advected forward frames"));
        }
        for (id, added) in &self.interpolated {
            lines.push(format!("  {id}: {added} frames interpolated onto the 15-minute cadence"));
        }
        for (id, reason) in &self.skipped {
            lines.push(format!("  {id}: no nowcast ({reason})"));
        }
        if self.released > 0 {
            lines.push(format!(
                "  released {} MB of cells no frame of this cycle can select",
                self.released / 1_048_576
            ));
        }
        lines
    }
}

/// Run both jobs over a cycle's baked sources, returning the sources the mosaic should be built
/// from and what was done to them.
///
/// Order matters and is the obvious one: the nowcast is derived from the parent's **observations**,
/// so it is unaffected by job B, and job B is not run over a nowcast layer (its frames are already
/// on the canonical cadence by construction). Nothing here can fail the cycle — every path that
/// cannot derive something leaves the mosaic exactly as it was.
pub fn derive_sources(mut sources: Vec<BakedSource>, times: CycleTimes) -> (Vec<BakedSource>, DeriveReport) {
    let mut report = DeriveReport::default();
    let mut derived = Vec::new();
    for source in &sources {
        match radar_nowcast(source, times) {
            Ok(Some(nowcast)) => {
                report.nowcasts.push((nowcast.id.to_string(), nowcast.frames.len()));
                derived.push(nowcast);
            }
            Ok(None) => {}
            Err(reason) => report.skipped.push((source.id.to_string(), reason)),
        }
    }
    for source in &mut sources {
        let added = uniform_frames(source, times);
        if added > 0 {
            report.interpolated.push((source.id.to_string(), added));
        }
    }
    sources.extend(derived);
    for source in &mut sources {
        report.released += release_unusable(source, times);
    }
    (sources, report)
}

/// Free every cell image the mosaic provably cannot select, once the derivation that needed it has
/// run. Returns the cells released.
///
/// Two kinds, and both are **output-neutral by construction** rather than by measurement:
///
/// * the **motion history**, whose only consumer is [`radar_nowcast`] and which the mosaic never
///   sees at all (see [`BakedSource::motion_history`]);
/// * frames outside `[first slot - skew, last slot + skew]`. `MosaicLayer::nearest` filters on
///   exactly that bound, so a frame beyond it can never be returned for any slot of this cycle. It
///   is not a small set: HRRR retains sixteen leads and GFS sixteen hourly steps, sized for
///   *pickup* worst cases rather than for the two hours a cycle publishes, so most of both is
///   ballast the moment the cycle's reference time is known.
///
/// This exists because the derivation stage roughly doubles the resident cell budget, and a
/// nowcast layer's frames are the most expensive cells in the cycle (24.5 MB each for MRMS). Paying
/// for them by dropping cells nothing can read is strictly better than paying for them with the
/// horizon.
fn release_unusable(source: &mut BakedSource, times: CycleTimes) -> usize {
    let mut released: usize = source.motion_history.iter().map(|frame| frame.cells.len()).sum();
    source.motion_history = Vec::new();
    let first = times.valid_at(0) - MAX_FRAME_SKEW_S;
    let last = times.valid_at((crate::canonical::CYCLE_FRAMES - 1) * FRAME_STEP_MIN) + MAX_FRAME_SKEW_S;
    source.frames.retain_mut(|frame| {
        if (first..=last).contains(&frame.valid_at) {
            return true;
        }
        released += frame.cells.len();
        false
    });
    released
}

/// **Job B.** Give `source` a frame at every canonical instant its own steps skip, by morphing
/// across the bracketing pair. Returns how many frames were added.
///
/// Three conditions, all of them about not displacing something better:
///
/// 1. the slot must not already have a frame within [`SLOT_TOLERANCE_S`];
/// 2. it must be **bracketed** by two `Forecast` frames no more than [`MAX_BRACKET_S`] apart.
///    Interpolation only, never extrapolation — running a model step past its own end is what job A
///    does deliberately from an observation, and would be an accident here;
/// 3. at the **anchor** slot, the source must hold no observation within
///    [`MAX_FRAME_SKEW_S`]. f0 is the one frame an observation may paint, and a derived frame valid
///    at exactly the anchor instant would out-distance a real measurement three minutes off it.
///    No shipped source both observes and forecasts around now — DWD RV's lead 0 is its only
///    observation and its forecasts are already on the 15-minute grid — so this is a guard against
///    the next source rather than a live case, which is exactly when a guard is cheap.
///
/// Every frame produced is a [`SourceClass::Forecast`], without exception and regardless of what its
/// parents were. It is an estimate for an instant nothing measured, so it is a prediction; that also
/// means it can never claim `FLAG_OBSERVED` and can never be mistaken for one, which is the
/// property #1248's [`frame_is_eligible`](crate::canonical::frame_is_eligible) is built on.
pub fn uniform_frames(source: &mut BakedSource, times: CycleTimes) -> usize {
    if source.frames.len() < 2 {
        return 0;
    }
    let width = source.geometry.width;
    let height = source.geometry.height;
    let params = FlowParams::for_cells(f64::from(source.geometry.cell_size_m));

    let mut added: Vec<BakedFrame> = Vec::new();
    // One motion field per bracketing **pair**, not per slot. An hourly source has three or four
    // canonical instants inside one of its steps, and estimating the same flow three times over is
    // the single most expensive thing this function could do by accident: on DWD RV's 1.76 M-cell
    // window it is a five-level pyramid and 27 k least-squares solves, repeated for nothing.
    let mut solved: Vec<((usize, usize), Option<flow::MotionField>)> = Vec::new();
    for offset_min in times.offsets_min() {
        let slot = times.slot(offset_min);
        let target = slot.valid_at();
        let answered =
            source.frames.iter().chain(&added).any(|frame| (frame.valid_at - target).abs() <= SLOT_TOLERANCE_S);
        if answered {
            continue;
        }
        if offset_min == 0
            && source
                .frames
                .iter()
                .any(|frame| frame.class.is_observation() && (frame.valid_at - target).abs() <= MAX_FRAME_SKEW_S)
        {
            continue;
        }
        let Some((before, after)) = bracket(&source.frames, target) else { continue };
        let dt = (source.frames[after].valid_at - source.frames[before].valid_at) as f64;
        let offset = (target - source.frames[before].valid_at) as f64;
        if !solved.iter().any(|(pair, _)| *pair == (before, after)) {
            let motion = flow::estimate_motion(
                &source.frames[before].cells,
                &source.frames[after].cells,
                width,
                height,
                dt,
                params,
            );
            solved.push(((before, after), motion));
        }
        let Some(motion) =
            solved.iter().find(|(pair, _)| *pair == (before, after)).and_then(|(_, motion)| motion.as_ref())
        else {
            // No motion signal between these two steps — a dry pair, most often. Nothing is
            // published for the slot; the mosaic keeps doing what it did before WXR9, which is to
            // reach for the nearest step inside the skew window.
            continue;
        };
        let cells =
            flow::morph(&source.frames[before].cells, &source.frames[after].cells, width, height, motion, dt, offset);
        added.push(BakedFrame {
            offset_min: ((target - source.reference_time).max(0) / 60) as u32,
            valid_at: target,
            class: SourceClass::Forecast,
            cells,
        });
    }
    let count = added.len();
    source.frames.extend(added);
    source.frames.sort_by_key(|frame| frame.valid_at);
    count
}

/// The indices of the two `Forecast` frames that bracket `target`, or `None` if it is not bracketed
/// or the bracket is wider than [`MAX_BRACKET_S`].
fn bracket(frames: &[BakedFrame], target: i64) -> Option<(usize, usize)> {
    let mut before: Option<usize> = None;
    let mut after: Option<usize> = None;
    for (index, frame) in frames.iter().enumerate() {
        if !matches!(frame.class, SourceClass::Forecast) {
            continue;
        }
        if frame.valid_at <= target && before.is_none_or(|current| frame.valid_at > frames[current].valid_at) {
            before = Some(index);
        }
        if frame.valid_at >= target && after.is_none_or(|current| frame.valid_at < frames[current].valid_at) {
            after = Some(index);
        }
    }
    let (before, after) = (before?, after?);
    let span = frames[after].valid_at - frames[before].valid_at;
    (span > 0 && span <= MAX_BRACKET_S).then_some((before, after))
}

/// **Job A.** The radar nowcast derived from `source`, or `None` if this source has no nowcast row
/// in [`crate::source::DERIVED_NOWCASTS`].
///
/// `Err` is a source that *should* have produced one and could not — a missing or unusable
/// observation pair. That is a warning rather than a failure: the mosaic without it is the mosaic
/// that shipped before WXR9.
///
/// The lead times are measured from the **observation's own instant**, not from the cycle anchor.
/// An MRMS scan at 18:48 published in a cycle anchored at 18:45 is advected 12, 27, 42 … minutes to
/// reach the 19:00, 19:15, 19:30 … frames, not 15, 30, 45. Getting that wrong would displace every
/// forecast by the observation's age, which is up to a whole frame step.
pub fn radar_nowcast(source: &BakedSource, times: CycleTimes) -> Result<Option<BakedSource>, String> {
    let Some(derived) = nowcast_of(source.id) else { return Ok(None) };
    if source.motion_history.is_empty() {
        return Err("no earlier observation was fetched to estimate motion from".to_string());
    }
    let anchor = source
        .frames
        .iter()
        .filter(|frame| frame.class.is_observation())
        .max_by_key(|frame| frame.valid_at)
        .ok_or("the source contributed no observation to advect")?;
    let history = source
        .motion_history
        .iter()
        .filter(|frame| {
            let dt = anchor.valid_at - frame.valid_at;
            (MIN_MOTION_DT_S..=MAX_MOTION_DT_S).contains(&dt)
        })
        .max_by_key(|frame| frame.valid_at)
        .ok_or_else(|| {
            format!(
                "no earlier observation sits {MIN_MOTION_DT_S}-{MAX_MOTION_DT_S} s before the anchor at {}",
                crate::timefmt::rfc3339(anchor.valid_at)
            )
        })?;

    let width = source.geometry.width;
    let height = source.geometry.height;
    let dt = (anchor.valid_at - history.valid_at) as f64;
    let params = FlowParams::for_cells(f64::from(source.geometry.cell_size_m));
    let Some(motion) = flow::estimate_motion(&history.cells, &anchor.cells, width, height, dt, params) else {
        return Err("the observation pair carries no motion signal (a dry or unscanned field)".to_string());
    };

    let mut frames = Vec::new();
    for offset_min in times.offsets_min() {
        if offset_min == 0 || offset_min > NOWCAST_MAX_LEAD_MIN {
            continue;
        }
        let slot = times.slot(offset_min);
        let lead = slot.valid_at() - anchor.valid_at;
        if lead <= 0 {
            // The observation is *after* this frame's instant, which the anchoring rule allows for
            // the first slot or two. Advecting backwards would be a hindcast of an instant the
            // anchor frame already answers better than anything derived from it.
            continue;
        }
        frames.push(BakedFrame {
            offset_min: (lead / 60) as u32,
            valid_at: slot.valid_at(),
            class: SourceClass::Forecast,
            cells: flow::advect(&anchor.cells, width, height, &motion, lead as f64),
        });
    }
    if frames.is_empty() {
        return Err("no canonical frame sits ahead of the anchor observation".to_string());
    }
    Ok(Some(BakedSource {
        id: derived.id,
        geometry: source.geometry,
        reference_time: anchor.valid_at,
        attribution: derived.attribution,
        frames,
        motion_history: Vec::new(),
    }))
}

/// The frame step, restated where the two jobs can see it, so a cadence change moves both.
const _: () = assert!(FRAME_STEP_MIN == 15, "the nowcast horizon is stated in whole frame steps");
const _: () = assert!(NOWCAST_MAX_LEAD_MIN.is_multiple_of(FRAME_STEP_MIN), "the horizon must land on a frame");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::GridGeometry;
    use crate::source::Attribution;
    use obc_formats::precip4;

    const WINDOW: GridGeometry = GridGeometry {
        south_lat_udeg: 0,
        west_lon_udeg: 0,
        cell_lat_udeg: 10_000,
        cell_lon_udeg: 10_000,
        width: 256,
        height: 192,
        cell_size_m: 1_000,
        tile_edge: 64,
        entries_per_page: 128,
    };

    fn blob(x: i32, y: i32) -> Vec<u8> {
        let (width, height) = (WINDOW.width as i32, WINDOW.height as i32);
        let mut cells = vec![precip4::INTENSITY_DRY; (width * height) as usize];
        for row in (y - 20)..=(y + 20) {
            for col in (x - 20)..=(x + 20) {
                if row < 0 || col < 0 || row >= height || col >= width {
                    continue;
                }
                let distance = (((col - x).pow(2) + (row - y).pow(2)) as f32).sqrt();
                let value = (10.0 * (1.0 - distance / 20.0)).round();
                if value > 0.0 {
                    cells[(row * width + col) as usize] = value as u8;
                }
            }
        }
        cells
    }

    fn source(id: &'static str, reference_time: i64, frames: Vec<BakedFrame>) -> BakedSource {
        BakedSource {
            id,
            geometry: WINDOW,
            reference_time,
            attribution: Attribution { text: "test", url: "https://example.invalid" },
            frames,
            motion_history: Vec::new(),
        }
    }

    fn centroid_x(cells: &[u8]) -> f32 {
        let (mut sum, mut mass) = (0.0f64, 0.0f64);
        for (index, &code) in cells.iter().enumerate() {
            if code == precip4::INTENSITY_DRY || code == precip4::INTENSITY_NODATA {
                continue;
            }
            sum += f64::from(code) * (index as u32 % WINDOW.width) as f64;
            mass += f64::from(code);
        }
        (sum / mass) as f32
    }

    /// **The headline of the expanded scope**: an hourly source comes out of job B with a frame at
    /// every one of the nine canonical instants, and each one is in a different place.
    #[test]
    fn an_hourly_source_gains_a_frame_at_every_canonical_instant() {
        let times = CycleTimes::anchored_at(0);
        // Steps at -1 h, 0, +1 h and +2 h relative to the anchor, the blob moving 24 cells an hour
        // — 6.7 m/s on this 1 km window, and slow enough that a 40-cell-wide feature still overlaps
        // itself between consecutive steps, which is the condition any differential method needs.
        let mut hourly = source(
            "hourly",
            -3_600,
            (0..4)
                .map(|step| BakedFrame {
                    offset_min: step * 60,
                    valid_at: -3_600 + i64::from(step) * 3_600,
                    class: SourceClass::Forecast,
                    cells: blob(40 + 24 * step as i32, 96),
                })
                .collect(),
        );
        let added = uniform_frames(&mut hourly, times);
        assert_eq!(added, 6, "the six quarter-hours the hourly steps skip");

        let mut positions = Vec::new();
        for offset_min in times.offsets_min() {
            let target = times.valid_at(offset_min);
            let frame = hourly
                .frames
                .iter()
                .find(|frame| frame.valid_at == target)
                .unwrap_or_else(|| panic!("f+{offset_min} has no frame valid at its own instant"));
            assert!(matches!(frame.class, SourceClass::Forecast), "a derived frame is a forecast");
            positions.push(centroid_x(&frame.cells));
        }
        // Strictly eastward, one quarter of an hourly step at a time: 6 cells per 15 minutes.
        for pair in positions.windows(2) {
            let step = pair[1] - pair[0];
            assert!((step - 6.0).abs() < 3.0, "consecutive frames move {step} cells, expected ~6: {positions:?}");
        }
    }

    /// Job B never displaces the observation the anchor frame is *for*.
    #[test]
    fn the_anchor_keeps_its_observation() {
        let times = CycleTimes::anchored_at(0);
        let mut mixed = source(
            "mixed",
            -180,
            vec![
                BakedFrame { offset_min: 0, valid_at: -180, class: SourceClass::Observation, cells: blob(40, 96) },
                BakedFrame { offset_min: 60, valid_at: 3_420, class: SourceClass::Forecast, cells: blob(88, 96) },
                BakedFrame { offset_min: 120, valid_at: 7_020, class: SourceClass::Forecast, cells: blob(136, 96) },
            ],
        );
        uniform_frames(&mut mixed, times);
        assert!(
            !mixed.frames.iter().any(|frame| frame.valid_at == 0),
            "no derived frame may land on the anchor instant while an observation is near it"
        );
    }

    /// A source with nothing to interpolate between is left exactly as it was — the honest fallback,
    /// and the one every failure in this module takes.
    #[test]
    fn nothing_is_derived_where_there_is_nothing_to_derive_from() {
        let times = CycleTimes::anchored_at(0);
        // One frame.
        let mut single = source(
            "single",
            0,
            vec![BakedFrame { offset_min: 0, valid_at: 0, class: SourceClass::Observation, cells: blob(40, 96) }],
        );
        assert_eq!(uniform_frames(&mut single, times), 0);
        assert_eq!(single.frames.len(), 1);

        // Two frames, both dry: bracketed, but with no motion signal between them.
        let dry = vec![precip4::INTENSITY_DRY; WINDOW.cells()];
        let mut quiet = source(
            "quiet",
            0,
            vec![
                BakedFrame { offset_min: 0, valid_at: 0, class: SourceClass::Forecast, cells: dry.clone() },
                BakedFrame { offset_min: 60, valid_at: 3_600, class: SourceClass::Forecast, cells: dry },
            ],
        );
        assert_eq!(uniform_frames(&mut quiet, times), 0, "a dry pair must not be morphed into eight dry frames");

        // Two frames four hours apart: not the same weather, so not bracketed.
        let mut distant = source(
            "distant",
            0,
            vec![
                BakedFrame { offset_min: 0, valid_at: -7_200, class: SourceClass::Forecast, cells: blob(40, 96) },
                BakedFrame { offset_min: 240, valid_at: 7_200, class: SourceClass::Forecast, cells: blob(200, 96) },
            ],
        );
        assert_eq!(uniform_frames(&mut distant, times), 0, "a four-hour bracket is not an interpolation");
    }

    /// Job A: the nowcast's leads are measured from the observation, not from the cycle anchor.
    #[test]
    fn a_nowcast_leads_from_the_observation_not_from_the_anchor() {
        // The cycle anchors at 18:45; the observation is at 18:48, three minutes into it.
        let anchor_time = 0i64;
        let observed_at = 180i64;
        let times = CycleTimes::anchored_at(anchor_time);
        let mut radar = source(
            crate::source::mrms::ID,
            observed_at,
            vec![BakedFrame {
                offset_min: 0,
                valid_at: observed_at,
                class: SourceClass::Observation,
                cells: blob(60, 96),
            }],
        );
        // Without a motion history there is no nowcast, and it is an explained failure.
        let error = radar_nowcast(&radar, times).unwrap_err();
        assert!(error.contains("no earlier observation"), "{error}");

        radar.motion_history.push(BakedFrame {
            offset_min: 0,
            valid_at: observed_at - 600,
            class: SourceClass::Observation,
            cells: blob(36, 96),
        });
        let nowcast = radar_nowcast(&radar, times).expect("a moving field").expect("mrms has a nowcast row");
        assert_eq!(nowcast.id, crate::source::mrms::NOWCAST.id);
        // f+15 (900 s) is 720 s past the 18:48 observation, so the blob moves 24 * 720 / 600 = 28.8
        // cells, not 24 * 900 / 600 = 36.
        let first = nowcast.frames.first().expect("a first frame");
        assert_eq!(first.valid_at, times.valid_at(FRAME_STEP_MIN));
        assert!(matches!(first.class, SourceClass::Forecast));
        let moved = centroid_x(&first.cells) - centroid_x(&radar.frames[0].cells);
        assert!((moved - 28.8).abs() < 5.0, "advected {moved} cells, expected ~28.8 (720 s, not 900 s)");
        // The horizon is honoured exactly.
        assert_eq!(
            nowcast.frames.last().expect("a last frame").valid_at,
            times.valid_at(NOWCAST_MAX_LEAD_MIN),
            "the nowcast must stop at NOWCAST_MAX_LEAD_MIN"
        );
        // A derived source never carries a motion history of its own — there is nothing to nowcast
        // a nowcast from.
        assert!(nowcast.motion_history.is_empty());
        assert!(radar_nowcast(&nowcast, times).expect("no row").is_none());
    }
}
