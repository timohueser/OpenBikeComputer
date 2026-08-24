//! The resident **weather snapshot** and its honest-state derivation (WX11, epic #1185).
//!
//! The screens never stream OBCW at draw time: the host samples the mounted store **once** per
//! refresh/fix change into this compact resident snapshot ([`WeatherSnapshot::sample`] — bounded
//! reads through the WX7 cache), and every weather screen derives what it may *claim* from the
//! snapshot plus the frame's `now` ([`rain_outlook`]). Keeping the derivation pure and
//! time-parameterized is what makes the honesty laws testable: expired rain can never produce a
//! dry claim, incomplete two-hour coverage is **WEATHER UPDATE NEEDED**, and a corridor with no
//! precipitation grid at all is the explicit hourly-only state — never a fake map, never dry.
//!
//! The *ride decision* (WX12, #1197) keeps [`rain_outlook`] as the one derivation — what changed
//! is **where each frame is sampled**: with an active route the host samples frame `k` at the
//! rider's *projected* route position for `k`'s timestamp ([`WeatherSnapshot::sample_along`] —
//! progress advanced by a bounded recent moving-speed estimate, [`RideProjection`]), inside a
//! conservative one-cell corridor. So DRY FOR 2 HOURS claims dryness along the ride, not just at
//! the parked rider, and RAIN IN N MIN times the first *encounter* with rain. The known, accepted
//! approximation: the hourly section stays a point forecast for the request coordinate applied
//! along the projection — deliberately not "fixed" with mass point sampling (banned; a future
//! multi-point hourly section is an OBCW/provider change). The freshness arithmetic mirrors
//! [`WeatherReader::current_frame`]'s fail-closed rule (per-frame cap = min inter-frame spacing,
//! bounded by [`FRAME_CURRENT_CAP_S`]) and is pinned against it by test, so what the overlay may
//! *render* and what a screen may *say* can never disagree **about freshness**: both read the same
//! frame table through the same cap. They may well disagree *spatially* — and that is intended:
//! the map paints the cells under the camera, the card answers for the projected ride, so a card
//! reading RAIN IN 20 over a map showing dry ground at the rider means rain 20 minutes *along the
//! route*. (On the WX8 on-glass list: confirm that reads as informative rather than contradictory.)

use obc_formats::io::ByteSource;
use obc_formats::obcw::{HourlyRecord, HOURLY_COUNT, HOURLY_INTERVAL_SECONDS, INTENSITY_NODATA};
use obc_route::RouteReader;
use obc_weather::{Error as ReadError, WeatherCache, WeatherReader, FRAME_CURRENT_CAP_S};

/// Rain-frame samples the snapshot holds. OBCW's radar-de policy is nine 15-minute frames; a
/// bundle carrying more keeps its first sixteen and reports the truncation, which the outlook
/// treats as incomplete coverage past the last kept frame (never a silent dry claim).
pub const SNAPSHOT_MAX_FRAMES: usize = 16;

/// The two-hour claim window of the dashboard card, in seconds.
pub const OUTLOOK_WINDOW_S: i64 = 2 * 3_600;

/// Smallest 4-bit intensity band the outlook counts as rain (band 1 = < 0.10 mm/h).
pub const RAIN_MIN_INTENSITY: u8 = 1;

/// Smallest band the outlook counts as storm-grade: band 9 starts the >= 10 mm/h range — the same
/// boundary the epic locked for the heavy-rain alert (the [`weather_alerts`](crate::weather_alerts)
/// threshold table re-exports it so the card and the alert can never disagree on "storm").
pub const STORM_MIN_INTENSITY: u8 = 9;

/// The touring fallback speed the projection uses while the rider is stopped (or before any
/// moving sample exists): 18 km/h = 500 cm/s — a plain loaded-touring cruising pace. Chosen so a
/// rider checking the sky at a rest stop still gets the ride-ahead answer, not the parked one;
/// the exact figure is a tuning constant, not a law.
pub const TOURING_FALLBACK_CMS: u32 = 500;

/// Cap on any single speed sample feeding the projection: 15 m/s (54 km/h). Sustained faster
/// riding doesn't happen on a 2 h touring horizon, and pathological GPS speeds (multipath jumps,
/// teleports) must not fling the projected position tens of kilometres ahead.
pub const SPEED_CAP_CMS: u32 = 1_500;

/// Moving threshold for a speed sample (1 m/s) — mirrors the GPS-course trust rule: below it the
/// receiver's speed is noise, not riding.
pub const MOVING_MIN_CMS: u32 = 100;

/// **Tunable** (sits with the [`weather_alerts`](crate::weather_alerts) threshold table): the
/// plausible spread of a rider's pace around the projection's median estimate, in cm/s.
///
/// The projection advances progress at one number; a real rider's pace over the next two hours
/// spreads around it (a climb, a café, a tailwind). 125 cm/s = ±1.25 m/s ≈ ±4.5 km/h — measured
/// against a plausible touring pace spread this covers the observed positional uncertainty of
/// **1.7 / 2.4 / 3.5 cells at +15 / +30 / +45 min** on a 1 km grid: `1 + ⌊125·Δt/100 / 1000⌋`
/// gives 2 / 3 / 4 cells there, i.e. the reviewer's `1 + k` ladder at the 15-minute radar cadence.
/// Expressed as a speed rather than a per-frame step so a coarser source's own cadence and cell
/// size fold in by arithmetic instead of by assumption (a 27 km floor cell stays one cell wide
/// across the whole horizon).
///
/// It widens the corridor **only for the DRY/coverage claim** (see
/// [`sample_along`](WeatherSnapshot::sample_along)); warnings keep the one-cell rule.
pub const PACE_SPREAD_CMS: u32 = 125;

/// Ceiling on the pace-spread corridor's half-width in cells — an I/O guard, not a decision. At
/// the shipped 1 km radar cell the ladder reaches 10 at the +2 h horizon, so this never binds
/// there; it exists so a hypothetical sub-hectometre grid can't turn one snapshot sample into
/// thousands of tile probes.
pub const CORRIDOR_MAX_HALF_CELLS: u32 = 12;

/// One rain frame's timestamp plus the nearest-cell intensity sampled at the rider's position for
/// that frame's instant — the *current* position, or the route-projected one when the host passed
/// a [`RideProjection`] ([`INTENSITY_NODATA`] when the position lies outside the grid or the tile
/// read failed — missing data stays missing, it never reads as dry). `lat`/`lon` record the µdeg
/// position the sample was taken at (the alert engine's spatial anchor); for a `None` position
/// they are `(0, 0)` **and** the intensity is the no-data sentinel, so nothing downstream ever
/// reads that placeholder as a place (no candidate is derived from a no-data frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSample {
    pub valid_at: i64,
    pub intensity: u8,
    pub lat: i32,
    pub lon: i32,
    /// The projection ran off the end of the route for this frame, so the sample is the finish
    /// point held still. A finished rider keeps riding (onward, home, a second loop), so the
    /// finish point's weather says nothing about where they will be: such a frame counts as **no
    /// coverage** for the DRY claim. Warnings may still use it — rain actually parked on the
    /// destination is worth saying.
    pub past_route_end: bool,
    /// The [`PACE_SPREAD_CMS`] corridor around this frame's sample is **not** known dry-and-covered
    /// (a cell inside it is wet, no-data, unreadable, or outside the grid). Blocks the DRY claim
    /// only — the warning path never reads it. `false` on any frame that already blocks the claim
    /// for a cheaper reason (the scan is skipped there) and on every unprojected sample.
    pub spread_uncertain: bool,
}

impl FrameSample {
    /// May this frame stand as one honest, dry link of the two-hour coverage chain? Every reason
    /// it can't is fail-closed: missing data, a projection past the route end, or a pace-spread
    /// corridor that isn't wholly dry-and-covered.
    fn supports_dry_claim(&self) -> bool {
        self.intensity != INTENSITY_NODATA && !self.past_route_end && !self.spread_uncertain
    }
}

/// The compact resident snapshot of the active weather bundle: the 24 hourly records verbatim,
/// the rain-frame table sampled at one position, and the freshness metadata the honest states
/// derive from. ~0.8 KB resident; refilled by the host on bundle commit / position change, read
/// every frame by the weather screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherSnapshot {
    pub generated_at: i64,
    /// Base timestamp of hourly record zero (OBCW header `valid_from`).
    pub valid_from: i64,
    /// Overall validity ceiling — nothing in the bundle may be claimed past it.
    pub valid_until: i64,
    /// The 24 fixed hourly records, index `i` covering `[valid_from + i*3600, +3600)`.
    pub hourly: [HourlyRecord; HOURLY_COUNT],
    /// The rain frames ascending by `valid_at`, sampled at [`sampled_at`](Self::sampled_at).
    pub frames: heapless::Vec<FrameSample, SNAPSHOT_MAX_FRAMES>,
    /// Per-frame currency cap in seconds: `min(min inter-frame spacing, FRAME_CURRENT_CAP_S)` —
    /// the exact fail-closed rule of [`WeatherReader::current_frame`].
    pub frame_cap_s: i64,
    /// The `(lat, lon)` microdegree position the frame intensities were sampled at, or `None`
    /// when no position was available (every sample is then no-data).
    pub sampled_at: Option<(i32, i32)>,
    /// Some sampled (current or projected) position lies inside the rain grid's bbox. `false`
    /// means the rain grid covers no part of the sampled ride — the explicit hourly-only state.
    pub pos_in_grid: bool,
    /// The rider's **current** position lies inside the rain grid's bbox — deliberately *not* the
    /// same bit as [`pos_in_grid`](Self::pos_in_grid) since WX12 widened that one to "some
    /// projected position". A rider outside the rain grid whose ride enters it has
    /// `pos_in_grid = true` (so rain met along the way is still reported) but
    /// `current_pos_in_grid = false`, which is what keeps the honest **hourly-only** state
    /// reachable instead of degrading into WEATHER UPDATE NEEDED: the grid simply doesn't
    /// cover where the rider is.
    pub current_pos_in_grid: bool,
    /// The frame samples were route-projected (a [`RideProjection`] was supplied) — diagnostics
    /// and tests only; the derivation itself is projection-agnostic.
    pub projected: bool,
    /// The bundle carried more frames than [`SNAPSHOT_MAX_FRAMES`]; coverage past the last kept
    /// frame is unknown and the outlook refuses the dry claim there.
    pub frames_truncated: bool,
    /// The rain grid at its **densest** frame (the OBCW header bbox with the maximum
    /// cell dimensions across the whole table), or `None` for a frameless bundle — what the rain
    /// map's zoom clamp derives its floor from ([`rain_zoom_floor`](Self::rain_zoom_floor)).
    /// Densest-frame dims make the clamp the strictest any frame needs, so time-stepping between
    /// frames of different resolutions can never step out of regime.
    pub rain_grid: Option<obc_render::RainGrid>,
}

/// The host-supplied ride projection ([`WeatherSnapshot::sample_along`]): where the rider is on
/// the active route right now and how fast they've recently been moving. Frame `k`'s sample
/// position becomes the route point at `progress_m + speed_cms · (valid_at − now)`, clamped to
/// the route end — the epic's locked projection approximation (no per-position forecasts are
/// fetched; the *rain grid* is simply read where the ride will be).
///
/// `speed_cms` should be the bounded recent moving median ([`SpeedWindow`], capped at
/// [`SPEED_CAP_CMS`]) with [`TOURING_FALLBACK_CMS`] while stopped — [`crate::App::ride_projection`]
/// builds exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideProjection {
    /// Matched along-route progress (m) at `now`.
    pub progress_m: u32,
    /// The projection speed (cm/s) — recent moving median, capped, touring fallback when stopped.
    pub speed_cms: u32,
    /// The UTC unix instant the projection is anchored at (the sampling instant). Hosts resample
    /// at fix cadence, so the anchor never ages more than seconds.
    pub now: i64,
}

impl WeatherSnapshot {
    /// Sample the open bundle at the fixed position `pos` (`(lat, lon)` microdegrees) — the
    /// routeless path; see [`sample_along`](Self::sample_along) for the route-projected one.
    pub fn sample<S: ByteSource + ?Sized>(
        reader: &WeatherReader<'_, S>,
        cache: &mut WeatherCache,
        pos: Option<(i32, i32)>,
    ) -> Result<Self, ReadError> {
        Self::sample_along(reader, cache, pos, None)
    }

    /// Sample the open bundle for the ride: every kept frame is sampled at the rider's expected
    /// position for that frame's timestamp — the current `pos` without a projection, else the
    /// route point the [`RideProjection`] advances to. Projected sampling runs **two** corridors
    /// around that point, because warning and claiming want opposite conservatism:
    ///
    /// - the **warning** corridor is one cell (the four neighbours), *raise-only*: a wet cell
    ///   immediately beside the line counts as wet, while validity is still governed by the centre
    ///   cell. It can raise a warning, never manufacture coverage. Unchanged since WX12's first
    ///   cut, and the whole of the unprojected (WX11 screens) path.
    /// - the **claim** corridor grows with the horizon ([`PACE_SPREAD_CMS`]), because at +45 min
    ///   the projection's own positional uncertainty is already 2–4 cells wide: a DRY FOR 2 HOURS
    ///   is only honest if *every* cell the rider might plausibly be in is dry and covered. Any
    ///   wet / no-data / out-of-grid cell inside it sets
    ///   [`spread_uncertain`](FrameSample::spread_uncertain) and the frame stops supporting the
    ///   dry claim (it produces no warning of its own — warn early, claim dry conservatively).
    ///
    /// Bounded I/O through the WX7 fixed cache: 24 hourly reads, one descriptor sweep, and a
    /// handful of tile decodes per kept frame — plus, **only for frames that are otherwise dry and
    /// covered** (the claim corridor short-circuits on the first blocker and is skipped entirely
    /// when the frame already can't claim), `4 · half_width` further cell probes. Worst case on
    /// the shipped 1 km/15 min radar dataset (a wholly clean nine-frame sky, the one case that
    /// pays in full): 45 centre/warning probes + 148 further claim probes = 193 `intensity_at`
    /// calls per pass against a *single-entry* tile cache. The claim corridor reuses the warning
    /// sweep's four first-step results; most remaining probes are same-tile hits, since 16
    /// consecutive cells along an arm share one tile. Run at refresh/fix cadence by the host,
    /// never per rendered frame; the SD-read figure behind it is on the WX8 mount-time
    /// measurement list.
    pub fn sample_along<S: ByteSource + ?Sized>(
        reader: &WeatherReader<'_, S>,
        cache: &mut WeatherCache,
        pos: Option<(i32, i32)>,
        projection: Option<(&RouteReader<'_>, RideProjection)>,
    ) -> Result<Self, ReadError> {
        let header = reader.header();
        let hourly = reader.hourly_records()?;

        let frame_count = header.frame_count as usize;
        let kept = frame_count.min(SNAPSHOT_MAX_FRAMES);
        let mut frames: heapless::Vec<FrameSample, SNAPSHOT_MAX_FRAMES> = heapless::Vec::new();
        let mut frame_cap_s = FRAME_CURRENT_CAP_S;
        let mut previous: Option<i64> = None;
        let mut pos_in_grid = false;
        let mut max_cells = (0u16, 0u16);
        for index in 0..frame_count {
            let frame = reader.frame(index)?;
            max_cells = (max_cells.0.max(frame.width), max_cells.1.max(frame.height));
            if let Some(prior) = previous {
                // Validated strictly increasing, so every spacing is positive. The cap must scan
                // the *whole* table (not just the kept prefix) to stay the reader's exact rule.
                frame_cap_s = frame_cap_s.min(frame.valid_at.saturating_sub(prior));
            }
            previous = Some(frame.valid_at);
            if index >= kept {
                continue;
            }
            // The position this frame is sampled at: the projected route point for the frame's
            // timestamp when a projection was supplied (an undecodable route falls back to the
            // current position — sampling somewhere real beats sampling nowhere), else `pos`.
            // `past_route_end` records that the projection clamped at the finish.
            let (sample_pos, past_route_end) = match (pos, projection) {
                (Some(current), Some((route, proj))) => match projected_position(route, proj, frame.valid_at) {
                    Some((at, clamped)) => (Some(at), clamped),
                    None => (Some(current), false),
                },
                (current, _) => (current, false),
            };
            let mut spread_uncertain = false;
            let intensity = match sample_pos {
                Some((lat, lon)) => {
                    // µdeg-per-cell of *this* frame's grid, for the corridor offsets.
                    let cell = corridor_cell_udeg(header, frame.width, frame.height);
                    let center = match reader.intensity_at(index, lat, lon, cache) {
                        Ok(Some(value)) => {
                            pos_in_grid = true;
                            value
                        }
                        // Outside the grid, or a failed read: missing stays missing.
                        Ok(None) | Err(_) => INTENSITY_NODATA,
                    };
                    if projection.is_some() && center != INTENSITY_NODATA {
                        // Warning corridor: the four one-cell neighbours can only *raise* the
                        // sample. Neighbours outside the grid / unreadable are ignored — the
                        // corridor widens the warning, never the coverage claim.
                        let mut max = center;
                        let mut neighbours_support_dry_claim = true;
                        for (dlat, dlon) in [(cell.0, 0), (-cell.0, 0), (0, cell.1), (0, -cell.1)] {
                            match reader.intensity_at(index, lat.saturating_add(dlat), lon.saturating_add(dlon), cache)
                            {
                                Ok(Some(v)) if v != INTENSITY_NODATA => {
                                    max = max.max(v);
                                    neighbours_support_dry_claim &= v < RAIN_MIN_INTENSITY;
                                }
                                // The warning corridor deliberately ignores missing neighbours,
                                // but the dry claim must fail closed on them.
                                _ => neighbours_support_dry_claim = false,
                            }
                        }
                        // Claim corridor — only worth paying for while a dry claim is still alive.
                        if max < RAIN_MIN_INTENSITY && !past_route_end {
                            let lead_s = projection.map_or(0, |(_, p)| frame.valid_at.saturating_sub(p.now));
                            let (half, capped) = spread_half_cells(lead_s, frame.cell_size_m);
                            // Step one is exactly the four-neighbour warning sweep above. Reusing
                            // its dry/coverage verdict avoids four duplicate probes per clean
                            // frame while keeping the warning and claim rules distinct.
                            spread_uncertain = capped
                                || !neighbours_support_dry_claim
                                || !corridor_tail_is_dry(reader, cache, index, (lat, lon), cell, half);
                        }
                        max
                    } else {
                        center
                    }
                }
                None => INTENSITY_NODATA,
            };
            let (lat, lon) = sample_pos.unwrap_or((0, 0));
            // Capacity is `kept <= SNAPSHOT_MAX_FRAMES` by construction.
            let _ = frames.push(FrameSample {
                valid_at: frame.valid_at,
                intensity,
                lat,
                lon,
                past_route_end,
                spread_uncertain,
            });
        }

        let rain_grid = (frame_count > 0).then_some(obc_render::RainGrid {
            west_udeg: header.west_lon_udeg,
            south_udeg: header.south_lat_udeg,
            east_udeg: header.east_lon_udeg,
            north_udeg: header.north_lat_udeg,
            width_cells: max_cells.0,
            height_cells: max_cells.1,
        });
        Ok(Self {
            generated_at: header.generated_at,
            valid_from: header.valid_from,
            valid_until: header.valid_until,
            hourly,
            frames,
            frame_cap_s,
            sampled_at: pos,
            pos_in_grid,
            // The rider's own position against the bundle's half-open bbox — the exact bounds test
            // `WeatherReader::cell_index` applies, without spending a tile read on it. A frameless
            // bundle has no grid at all, so nothing is inside it.
            current_pos_in_grid: frame_count > 0
                && pos.is_some_and(|(lat, lon)| {
                    lat >= header.south_lat_udeg
                        && lat < header.north_lat_udeg
                        && lon >= header.west_lon_udeg
                        && lon < header.east_lon_udeg
                }),
            projected: pos.is_some() && projection.is_some(),
            frames_truncated: frame_count > SNAPSHOT_MAX_FRAMES,
            rain_grid,
        })
    }

    /// The hourly record covering `now`, with its index and start timestamp — `None` outside the
    /// 24 represented intervals or the bundle validity. Mirrors `WeatherReader::hourly_at`.
    pub fn hourly_at(&self, now: i64) -> Option<(usize, i64, &HourlyRecord)> {
        if now > self.valid_until {
            return None;
        }
        let delta = now.checked_sub(self.valid_from).filter(|d| *d >= 0)?;
        let index = (delta / HOURLY_INTERVAL_SECONDS as i64) as usize;
        if index >= HOURLY_COUNT {
            return None;
        }
        let valid_at = self.valid_from + index as i64 * HOURLY_INTERVAL_SECONDS as i64;
        Some((index, valid_at, &self.hourly[index]))
    }

    /// The index of the frame that is **current** at `now` under the snapshot's cap — the same
    /// verdict [`WeatherReader::current_frame`] gives on the underlying bundle (pinned by test).
    pub fn current_frame_index(&self, now: i64) -> Option<usize> {
        if now < self.valid_from || now > self.valid_until {
            return None;
        }
        let index = self.frames.iter().rposition(|f| f.valid_at <= now)?;
        (now.saturating_sub(self.frames[index].valid_at) <= self.frame_cap_s).then_some(index)
    }

    /// How many *future* frames exist past the current one — the rain map's time-step range
    /// (`0..=steps_ahead`). `0` when nothing is current or the current frame is the last.
    pub fn steps_ahead(&self, now: i64) -> u8 {
        match self.current_frame_index(now) {
            Some(index) => (self.frames.len() - 1 - index).min(u8::MAX as usize) as u8,
            None => 0,
        }
    }

    /// The end of frame `index`'s honest coverage window: its successor's start, bounded by
    /// `valid_at + cap`, bounded by the bundle's `valid_until`. `pub(crate)` for the alert
    /// engine, which must judge frame currency by the exact same arithmetic.
    pub(crate) fn window_end(&self, index: usize) -> i64 {
        let frame = &self.frames[index];
        let capped = frame.valid_at.saturating_add(self.frame_cap_s);
        let end = match self.frames.get(index + 1) {
            Some(next) => next.valid_at.min(capped),
            None => capped,
        };
        end.min(self.valid_until)
    }

    /// The rain map's zoom-out floor at the camera latitude — the smallest zoom at which the
    /// rain grid still renders ([`obc_render::rain_min_zoom`] over [`rain_grid`](Self::rain_grid)),
    /// or `None` with no rain grid (the clamp then stays disengaged). Hosts feed it into
    /// [`AppState::rain_zoom_min`](crate::AppState) alongside the snapshot.
    pub fn rain_zoom_floor(&self, cam_lat_udeg: i32) -> Option<f32> {
        let grid = self.rain_grid.as_ref()?;
        obc_render::rain_min_zoom(grid, obc_map_scene::cos_lat(cam_lat_udeg))
    }

    /// The sampled intensity governing instant `t`, or `None` when no frame's honest window
    /// covers it (a gap is a gap — the strip draws it as unknown, never dry). A
    /// [`INTENSITY_NODATA`] sample also reads as `None`: missing data never claims a look.
    pub fn intensity_covering(&self, t: i64) -> Option<u8> {
        if t < self.valid_from || t > self.valid_until {
            return None;
        }
        let index = self.frames.iter().rposition(|f| f.valid_at <= t)?;
        if t > self.window_end(index) {
            return None;
        }
        let intensity = self.frames[index].intensity;
        (intensity != INTENSITY_NODATA).then_some(intensity)
    }
}

/// What the dashboard's decision card may honestly claim at `now` — see [`rain_outlook`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RainOutlook {
    /// The bundle carries no rain grid covering the rider's position: the explicit
    /// hourly-only state. Hourly rows stay available; no rain claim of any kind is made.
    HourlyOnly,
    /// Rain data exists but cannot honestly answer the two-hour question at `now`: bundle
    /// expired, nothing current, a mid-window gap, no-data samples, or a truncated table —
    /// and nothing wet was seen in what *is* covered. Never rendered as dry.
    UpdateNeeded,
    /// Complete two-hour coverage, every covered sample dry.
    Dry,
    /// Rain reaches the position in `minutes` (0 = raining now).
    RainIn { minutes: u16 },
    /// Storm-grade intensity ([`STORM_MIN_INTENSITY`]) inside the window; `minutes` to the
    /// first wet frame (0 = already wet).
    StormIn { minutes: u16 },
}

/// Derive the dashboard headline from the snapshot at `now`. Pure and total — the honesty laws
/// live here, in one testable place:
///
/// - no rain frames, or no sampled position anywhere in the grid → [`RainOutlook::HourlyOnly`];
/// - **DRY only with complete coverage**: every 15-minute-grained instant of `[now, now+2h]`
///   inside a current frame's honest window, with no no-data sample, no truncation, no frame
///   projected past the route end, and no frame whose pace-spread corridor is unclaimed
///   ([`FrameSample::supports_dry_claim`] carries the last three);
/// - anything wet inside the covered part is reported even when coverage is partial —
///   a gap suppresses the dry claim, never a rain warning;
/// - nothing wet, coverage incomplete, and the rain grid doesn't even reach the rider's
///   *current* position → [`RainOutlook::HourlyOnly`] again: the honest "no rain grid here",
///   not a fetch instruction that would never help;
/// - otherwise → [`RainOutlook::UpdateNeeded`] (stale is never dry).
pub fn rain_outlook(snap: &WeatherSnapshot, now: i64) -> RainOutlook {
    if snap.frames.is_empty() || !snap.pos_in_grid {
        return RainOutlook::HourlyOnly;
    }
    if now < snap.valid_from || now > snap.valid_until {
        return RainOutlook::UpdateNeeded;
    }
    let horizon = now + OUTLOOK_WINDOW_S;

    // Wet detection walks the *frames* whose honest windows overlap `[now, horizon]`, so rain is
    // timed to the frame's real start — never quantized later by a sampling grain. A no-data
    // sample is neither wet nor dry; the coverage walk below refuses the dry claim for it.
    let mut first_wet_at: Option<i64> = None;
    let mut max_intensity = 0u8;
    for (index, frame) in snap.frames.iter().enumerate() {
        if snap.window_end(index) < now || frame.valid_at > horizon {
            continue;
        }
        if frame.intensity != INTENSITY_NODATA && frame.intensity >= RAIN_MIN_INTENSITY {
            first_wet_at.get_or_insert(frame.valid_at.max(now));
            max_intensity = max_intensity.max(frame.intensity);
        }
    }

    // Coverage: `[now, horizon]` must be one unbroken chain of honest windows with no no-data
    // sample — otherwise the dry claim is refused (WEATHER UPDATE NEEDED), while any rain found
    // above still reports. A truncated table never claims dry.
    let mut fully_covered = !snap.frames_truncated;
    match snap.frames.iter().rposition(|f| f.valid_at <= now) {
        None => fully_covered = false,
        Some(start) => {
            let mut index = start;
            loop {
                if !snap.frames[index].supports_dry_claim() {
                    fully_covered = false;
                    break;
                }
                let end = snap.window_end(index);
                if end >= horizon {
                    break;
                }
                match snap.frames.get(index + 1) {
                    Some(next) if next.valid_at <= end => index += 1,
                    _ => {
                        fully_covered = false;
                        break;
                    }
                }
            }
        }
    }

    match first_wet_at {
        Some(at) => {
            let minutes = (at.saturating_sub(now) / 60).min(u16::MAX as i64) as u16;
            if max_intensity >= STORM_MIN_INTENSITY {
                RainOutlook::StormIn { minutes }
            } else {
                RainOutlook::RainIn { minutes }
            }
        }
        None if fully_covered => RainOutlook::Dry,
        // Nothing wet, coverage incomplete — but if the grid does not cover where the rider
        // actually *is*, the gap isn't staleness, it's absence: the explicit hourly-only state.
        None if !snap.current_pos_in_grid => RainOutlook::HourlyOnly,
        None => RainOutlook::UpdateNeeded,
    }
}

/// The host's per-frame weather feed into the render path — the snapshot borrow plus the
/// refresh-in-flight flag, bundled so the render entry points take one weather argument. Like the
/// rain lease it is host-owned per frame: the App keeps no resident weather state of its own
/// (the board's buffer lands with WX8's store mount, where it can be carved deliberately).
#[derive(Default)]
pub struct WeatherFeed<'a> {
    /// The resident snapshot, or `None` when nothing was ever fetched / no store is mounted.
    pub snapshot: Option<&'a WeatherSnapshot>,
    /// A refresh cycle is in flight (WX8's request/upload; the sim's flag) — the dashboard's one
    /// non-blocking cue. Cached content stays visible regardless.
    pub refreshing: bool,
}

impl WeatherFeed<'_> {
    /// No weather at all — the plain render entries' default.
    pub const NONE: WeatherFeed<'static> = WeatherFeed { snapshot: None, refreshing: false };
}

/// Route-relative wind classification for the hourly rows' arrows (locked UX: green tailwind /
/// orange crosswind / red headwind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindClass {
    Tail,
    Cross,
    Head,
}

/// Classify the wind route-relatively, or `None` when there is no trustworthy travel direction —
/// the arrows then render in neutral ink, never a false head/tail claim (the locked fallback).
///
/// `travel_deg` is the WX12 chain's output (the active route's general heading ahead of the
/// matched position, and nothing else — [`crate::App::travel_deg`], threaded to the rows as
/// `Render::travel_deg`). The classification: the wind's *to*-direction within 60° of travel is
/// a tailwind, within 60° of dead-opposite a headwind, everything between a crosswind.
pub fn wind_class(wind_from_deg: u16, travel_deg: Option<f32>) -> Option<WindClass> {
    let travel = travel_deg?;
    let wind_to = (wind_from_deg as f32 + 180.0) % 360.0;
    let mut diff = (wind_to - travel) % 360.0;
    if diff < 0.0 {
        diff += 360.0;
    }
    if diff > 180.0 {
        diff = 360.0 - diff;
    }
    Some(if diff <= 60.0 {
        WindClass::Tail
    } else if diff >= 120.0 {
        WindClass::Head
    } else {
        WindClass::Cross
    })
}

/// The meteorological *from*-direction's octant index (0 = N, 1 = NE, … clockwise) for the hourly
/// rows' `SW`-style labels.
pub fn wind_octant(wind_from_deg: u16) -> usize {
    (((wind_from_deg as u32 + 22) / 45) % 8) as usize
}

/// One grid cell of the frame's raster in microdegrees `(dlat, dlon)` — the corridor offset.
/// Never zero (a degenerate header would otherwise collapse the corridor onto the centre cell,
/// which is harmless but pointless).
fn corridor_cell_udeg(header: obc_formats::obcw::Header, width: u16, height: u16) -> (i32, i32) {
    let dlat = (header.north_lat_udeg as i64 - header.south_lat_udeg as i64) / height.max(1) as i64;
    let dlon = (header.east_lon_udeg as i64 - header.west_lon_udeg as i64) / width.max(1) as i64;
    ((dlat.clamp(1, i32::MAX as i64)) as i32, (dlon.clamp(1, i32::MAX as i64)) as i32)
}

/// The expected rider position `(lat, lon)` µdeg at instant `t` under the projection: progress
/// advanced by `speed · Δt` (frames at or before the anchor sample at the current progress — we
/// don't reconstruct the past), clamped to the route end by `position_at` itself. The returned
/// flag says the clamp fired — the projection ran out of route, so this sample is the finish point
/// standing still and may not carry a dry claim (see [`FrameSample::past_route_end`]). `None` when
/// the route geometry doesn't decode (flaky SD); the caller falls back to the current position.
fn projected_position(route: &RouteReader<'_>, proj: RideProjection, t: i64) -> Option<((i32, i32), bool)> {
    let dt_s = t.saturating_sub(proj.now).max(0) as u64;
    let ahead_m = (proj.speed_cms as u64 * dt_s / 100).min(u32::MAX as u64) as u32;
    let target_m = proj.progress_m.saturating_add(ahead_m);
    let position = route.position_at(target_m)?;
    Some(((position.lat, position.lon), target_m > route.total_distance_m))
}

/// Half-width, in cells, of the pace-spread corridor a frame `lead_s` seconds past the projection
/// anchor needs before it may carry a DRY claim: one cell (the projection's own grid granularity)
/// plus whole cells of [`PACE_SPREAD_CMS`] accumulated over the lead, capped by
/// [`CORRIDOR_MAX_HALF_CELLS`]. Frames at or before the anchor carry no pace uncertainty at all
/// and stay at one cell.
/// Also reports whether the cap truncated the wanted width: a corridor narrower than the pace
/// spread demands cannot support a DRY claim, so the caller folds saturation into
/// `spread_uncertain` — the guard fails closed instead of quietly narrowing (review #1232 delta).
fn spread_half_cells(lead_s: i64, cell_size_m: u16) -> (u32, bool) {
    let spread_m = (PACE_SPREAD_CMS as i64).saturating_mul(lead_s.max(0)) / 100;
    let cells = spread_m / cell_size_m.max(1) as i64;
    let wanted = 1 + cells;
    let half = wanted.clamp(1, CORRIDOR_MAX_HALF_CELLS as i64) as u32;
    (half, wanted > CORRIDOR_MAX_HALF_CELLS as i64)
}

/// Is every cell within `half` cells of `(lat, lon)` along both axes readable, in-grid and dry?
/// The DRY claim's gate — fail-closed in every direction (a wet cell, a no-data cell, a cell
/// outside the grid, or a failed read all answer `false`), and short-circuiting on the first
/// blocker so the cost is only paid by corridors that actually turn out clean.
fn corridor_tail_is_dry<S: ByteSource + ?Sized>(
    reader: &WeatherReader<'_, S>,
    cache: &mut WeatherCache,
    frame: usize,
    (lat, lon): (i32, i32),
    cell: (i32, i32),
    half: u32,
) -> bool {
    for (dlat, dlon) in [(cell.0, 0), (-cell.0, 0), (0, cell.1), (0, -cell.1)] {
        // Walk each arm outward from the centre: consecutive cells share a tile, so the
        // single-entry tile cache is hit for all but the few steps that cross a tile edge.
        for step in 2..=half as i32 {
            let probe_lat = lat.saturating_add(dlat.saturating_mul(step));
            let probe_lon = lon.saturating_add(dlon.saturating_mul(step));
            match reader.intensity_at(frame, probe_lat, probe_lon, cache) {
                Ok(Some(v)) if v != INTENSITY_NODATA && v < RAIN_MIN_INTENSITY => {}
                _ => return false,
            }
        }
    }
    true
}

/// Great-circle-free flat bearing from `a` to `b` (`(lat, lon)` µdeg), degrees CW from north in
/// `0..360` — the scale of a tangent step or a wind comparison, where the equirectangular
/// approximation is exact enough by orders of magnitude. `None` for coincident points.
pub fn bearing_deg(a: (i32, i32), b: (i32, i32)) -> Option<f32> {
    let cl = obc_map_scene::cos_lat(a.0);
    let dx = (b.1 - a.1) as f32 * cl;
    let dy = (b.0 - a.0) as f32;
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    let deg = libm::atan2f(dx, dy).to_degrees();
    Some(if deg < 0.0 { deg + 360.0 } else { deg })
}

/// Along-route chord over which the travel direction is measured — a *general* heading, not a
/// local tangent (owner tuning round). The wind question is about the ride ahead, so the chord is
/// long enough that switchbacks, a river path's meanders and roundabouts can't swing the arrows'
/// colour, and short enough that it still describes the leg the rider is actually on.
pub const TRAVEL_CHORD_M: u32 = 1_000;

/// The active route's general travel direction (degrees CW from north) at `progress_m`: the
/// bearing of the [`TRAVEL_CHORD_M`] chord ahead, stepped back at the route end so the last
/// kilometre keeps a direction. `None` when the route has no usable geometry — the arrows are then
/// neutral, never a fabricated head/tail.
pub fn route_heading_deg(route: &RouteReader<'_>, progress_m: u32) -> Option<f32> {
    let total = route.total_distance_m;
    if total < 2 {
        return None;
    }
    let step = TRAVEL_CHORD_M.min(total);
    let (from_m, to_m) =
        if progress_m.saturating_add(step) <= total { (progress_m, progress_m + step) } else { (total - step, total) };
    let from = route.position_at(from_m)?;
    let to = route.position_at(to_m)?;
    bearing_deg((from.lat, from.lon), (to.lat, to.lon))
}

/// The bounded recent moving-speed window feeding [`RideProjection::speed_cms`]: the last
/// [`SPEED_SAMPLES`](SpeedWindow::SPEED_SAMPLES) *moving* fixes' speeds (cm/s, each capped at
/// [`SPEED_CAP_CMS`]), read as their median — robust to single-fix GPS glitches in both
/// directions, and "recent" at fix cadence (≈ the last minute at 1 Hz). Stopped fixes are not
/// pushed: a café stop doesn't erode the estimate, per the locked touring-fallback rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeedWindow {
    samples: [u16; Self::SPEED_SAMPLES],
    len: u8,
    next: u8,
}

impl Default for SpeedWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeedWindow {
    /// Ring capacity — 64 moving fixes ≈ the last minute of riding at 1 Hz.
    pub const SPEED_SAMPLES: usize = 64;

    pub const fn new() -> Self {
        SpeedWindow { samples: [0; Self::SPEED_SAMPLES], len: 0, next: 0 }
    }

    /// Forget every sample (session restart).
    pub fn clear(&mut self) {
        self.len = 0;
        self.next = 0;
    }

    /// Record one fix's speed (m/s). Ignored below [`MOVING_MIN_CMS`] (stopped is not a pace);
    /// capped at [`SPEED_CAP_CMS`] (a teleport is not a pace either).
    pub fn push_mps(&mut self, speed_mps: f32) {
        let cms = (speed_mps.max(0.0) * 100.0).min(SPEED_CAP_CMS as f32) as u32;
        if cms < MOVING_MIN_CMS {
            return;
        }
        self.samples[self.next as usize] = cms as u16;
        self.next = (self.next + 1) % Self::SPEED_SAMPLES as u8;
        self.len = (self.len + 1).min(Self::SPEED_SAMPLES as u8);
    }

    /// The median of the recorded moving speeds (cm/s), or `None` before any moving fix — the
    /// caller then uses [`TOURING_FALLBACK_CMS`].
    pub fn median_cms(&self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        let n = self.len as usize;
        let mut sorted = [0u16; Self::SPEED_SAMPLES];
        sorted[..n].copy_from_slice(&self.samples[..n]);
        sorted[..n].sort_unstable();
        Some(sorted[n / 2] as u32)
    }
}

/// Local `(hour, minute)` of a UTC unix instant under the device's UTC offset — the weather
/// screens' one time-of-day formatter (frame timestamps, hourly rows, the freshness line). Pure
/// modular arithmetic; negative instants wrap via `rem_euclid` into a valid time of day.
pub fn local_hour_minute(unix_utc: i64, utc_offset_min: i16) -> (u8, u8) {
    let local = unix_utc + utc_offset_min as i64 * 60;
    let minutes_of_day = local.div_euclid(60).rem_euclid(24 * 60) as u32;
    ((minutes_of_day / 60) as u8, (minutes_of_day % 60) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::SliceSource;
    use obc_formats::obcw::INTENSITY_DRY;

    const DWD: &[u8] = include_bytes!("../../../specs/vectors/weather-dwd-96x96-9f.obcw");

    fn snapshot_at_center() -> (WeatherSnapshot, i64) {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let header = reader.header();
        let mut cache = WeatherCache::new();
        let pos =
            ((header.south_lat_udeg + header.north_lat_udeg) / 2, (header.west_lon_udeg + header.east_lon_udeg) / 2);
        let snap = WeatherSnapshot::sample(&reader, &mut cache, Some(pos)).unwrap();
        (snap, reader.frame(0).unwrap().valid_at)
    }

    /// The snapshot's current-frame verdict agrees with the reader's own `current_frame` gate —
    /// the one WX10 freshness authority — across the whole bundle life and past it.
    #[test]
    fn snapshot_currency_mirrors_the_readers_gate() {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let header = reader.header();
        let (snap, first) = snapshot_at_center();
        let mut cache = WeatherCache::new();
        for offset in (-1_000..12_000).step_by(37) {
            let now = first + offset;
            let expect = reader.current_frame(now, &mut cache).unwrap().map(|(index, _)| index);
            assert_eq!(snap.current_frame_index(now), expect, "offset {offset}");
        }
        assert_eq!(snap.current_frame_index(header.valid_until + 1), None);
    }

    /// The sampled intensities agree with the reader's nearest-neighbour lookup at the same
    /// position, frame by frame.
    #[test]
    fn sampled_intensities_match_the_reader() {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let (snap, _) = snapshot_at_center();
        let (lat, lon) = snap.sampled_at.unwrap();
        assert!(snap.pos_in_grid);
        assert_eq!(snap.frames.len(), 9);
        let mut cache = WeatherCache::new();
        for (index, frame) in snap.frames.iter().enumerate() {
            let expect = reader.intensity_at(index, lat, lon, &mut cache).unwrap().unwrap();
            assert_eq!(frame.intensity, expect, "frame {index}");
        }
    }

    /// A position outside the grid is the explicit hourly-only state — never dry, never a rain
    /// claim fabricated from cells the rider isn't under.
    #[test]
    fn outside_the_grid_is_hourly_only() {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let header = reader.header();
        let mut cache = WeatherCache::new();
        let outside = (header.north_lat_udeg + 1_000_000, header.west_lon_udeg);
        let snap = WeatherSnapshot::sample(&reader, &mut cache, Some(outside)).unwrap();
        assert!(!snap.pos_in_grid);
        assert_eq!(rain_outlook(&snap, header.valid_from + 60), RainOutlook::HourlyOnly);
        // And so is having no position at all.
        let snap = WeatherSnapshot::sample(&reader, &mut cache, None).unwrap();
        assert_eq!(rain_outlook(&snap, header.valid_from + 60), RainOutlook::HourlyOnly);
    }

    /// Synthetic snapshot helper: nine 15-minute frames from `t0` with the given intensities.
    fn synthetic(intensities: &[u8], t0: i64) -> WeatherSnapshot {
        let (snap, _) = snapshot_at_center();
        let (lat, lon) = snap.sampled_at.unwrap();
        let mut frames = heapless::Vec::new();
        for (index, &intensity) in intensities.iter().enumerate() {
            frames
                .push(FrameSample {
                    valid_at: t0 + index as i64 * 900,
                    intensity,
                    lat,
                    lon,
                    past_route_end: false,
                    spread_uncertain: false,
                })
                .unwrap();
        }
        WeatherSnapshot {
            valid_from: t0 - 3_600,
            valid_until: t0 + 24 * 3_600,
            frames,
            frame_cap_s: 900,
            pos_in_grid: true,
            current_pos_in_grid: true,
            frames_truncated: false,
            ..snap
        }
    }

    /// The four honest headlines, at the boundaries that matter.
    #[test]
    fn outlook_headlines_and_boundaries() {
        let t0 = 1_800_000_000;
        // All dry, nine frames: covers now..now+2h exactly (frame 8 window ends at +8*900+900 = 2h+900).
        let dry = synthetic(&[0; 9], t0);
        assert_eq!(rain_outlook(&dry, t0), RainOutlook::Dry);
        // 35 minutes before frame 4 (t0+3600) turns wet: RAIN IN 35 — but the tail past the last
        // frame's window is then uncovered for a dry claim, which rain doesn't need.
        let rain = synthetic(&[0, 0, 0, 0, 4, 4, 0, 0, 0], t0);
        assert_eq!(rain_outlook(&rain, t0 + 3_600 - 35 * 60), RainOutlook::RainIn { minutes: 35 });
        // Raining in the frame covering `now`: zero minutes.
        assert_eq!(rain_outlook(&rain, t0 + 3_600), RainOutlook::RainIn { minutes: 0 });
        // A storm-grade band anywhere in the window classifies the headline as storm, timed to
        // the first wet frame.
        let storm = synthetic(&[0, 0, 3, 0, 10, 0, 0, 0, 0], t0);
        assert_eq!(rain_outlook(&storm, t0), RainOutlook::StormIn { minutes: 30 });
        // Expired bundle: update needed, never dry.
        let stale = synthetic(&[0; 9], t0);
        assert_eq!(rain_outlook(&stale, stale.valid_until + 1), RainOutlook::UpdateNeeded);
    }

    /// Incomplete coverage refuses the dry claim: a mid-table gap wider than the cap, a no-data
    /// sample, a truncated table, and a window reaching past the last frame's currency all say
    /// WEATHER UPDATE NEEDED — while a wet frame inside the covered part still reports rain.
    #[test]
    fn incomplete_coverage_never_claims_dry() {
        let t0 = 1_800_000_000;
        // Fewer frames than the two-hour window needs.
        let short = synthetic(&[0, 0, 0], t0);
        assert_eq!(rain_outlook(&short, t0), RainOutlook::UpdateNeeded);
        // A no-data sample mid-window.
        let holed = synthetic(&[0, 0, INTENSITY_NODATA, 0, 0, 0, 0, 0, 0], t0);
        assert_eq!(rain_outlook(&holed, t0), RainOutlook::UpdateNeeded);
        // A truncated table can never claim dry.
        let mut truncated = synthetic(&[0; 9], t0);
        truncated.frames_truncated = true;
        assert_eq!(rain_outlook(&truncated, t0), RainOutlook::UpdateNeeded);
        // …but rain seen inside the covered part still reports, gap or no gap.
        let wet_then_gap = synthetic(&[0, 6, 0], t0);
        assert_eq!(rain_outlook(&wet_then_gap, t0), RainOutlook::RainIn { minutes: 15 });
        // A mid-table cadence gap: frames at 0/900/…, then a hole. Build directly.
        let mut gap = synthetic(&[0, 0, 0, 0, 0, 0, 0, 0, 0], t0);
        let (lat, lon) = gap.sampled_at.unwrap();
        gap.frames.truncate(0);
        for (index, at) in [0i64, 900, 1_800, 5_400, 6_300, 7_200].iter().enumerate() {
            let _ = index;
            gap.frames
                .push(FrameSample {
                    valid_at: t0 + at,
                    intensity: INTENSITY_DRY,
                    lat,
                    lon,
                    past_route_end: false,
                    spread_uncertain: false,
                })
                .unwrap();
        }
        gap.frame_cap_s = 900;
        assert_eq!(rain_outlook(&gap, t0), RainOutlook::UpdateNeeded, "a bake gap goes dark, not dry");
    }

    /// The two WX12 dry-claim blockers, at the level the derivation sees them: a frame the
    /// projection clamped past the route end, and a frame whose pace-spread corridor isn't wholly
    /// dry-and-covered, each refuse DRY on their own — while neither invents a warning.
    #[test]
    fn projection_blockers_refuse_dry_without_inventing_rain() {
        let t0 = 1_800_000_000;
        // Baseline: nine dry frames are an honest DRY FOR 2 HOURS.
        assert_eq!(rain_outlook(&synthetic(&[0; 9], t0), t0), RainOutlook::Dry);

        // The rider reaches the finish inside the window: from there the projection stands still
        // at the destination, which says nothing about where the rider will actually be.
        let mut clamped = synthetic(&[0; 9], t0);
        for frame in clamped.frames.iter_mut().skip(5) {
            frame.past_route_end = true;
        }
        assert_eq!(rain_outlook(&clamped, t0), RainOutlook::UpdateNeeded, "a finished projection can't claim dry");
        assert_eq!(rain_outlook(&clamped, t0 + 90 * 60), RainOutlook::UpdateNeeded);

        // …but rain parked on the destination is still worth saying.
        let mut wet_end = clamped.clone();
        wet_end.frames[6].intensity = 6;
        assert_eq!(rain_outlook(&wet_end, t0), RainOutlook::RainIn { minutes: 90 }, "warnings still use them");

        // A wet/no-data/out-of-grid cell inside the widened claim corridor: no dry claim, and
        // (deliberately) no warning either — warn early on the one-cell rule, claim dry widely.
        let mut spread = synthetic(&[0; 9], t0);
        spread.frames[4].spread_uncertain = true;
        assert_eq!(rain_outlook(&spread, t0), RainOutlook::UpdateNeeded, "an unclaimed corridor refuses dry");
    }

    /// The pace-spread ladder: one cell at the anchor, and the reviewer's measured 2 / 3 / 4 cells
    /// at +15 / +30 / +45 min on a 1 km grid — while a coarse floor source stays one cell wide
    /// across the whole horizon, and the I/O cap bounds a pathological fine grid.
    #[test]
    fn spread_half_cells_ladder_covers_the_measured_uncertainty() {
        assert_eq!(spread_half_cells(0, 1_000), (1, false), "no lead, no pace uncertainty");
        assert_eq!(spread_half_cells(-900, 1_000), (1, false), "a past frame samples here, exactly");
        assert_eq!(spread_half_cells(15 * 60, 1_000), (2, false), "measured 1.7 cells at +15 min");
        assert_eq!(spread_half_cells(30 * 60, 1_000), (3, false), "measured 2.4 cells at +30 min");
        assert_eq!(spread_half_cells(45 * 60, 1_000), (4, false), "measured 3.5 cells at +45 min");
        // A 27 km global-floor cell already swallows two hours of pace spread whole.
        assert_eq!(spread_half_cells(2 * 3_600, 27_000), (1, false), "a coarse cell absorbs the spread");
        // And nothing can run the probe count away.
        assert_eq!(
            spread_half_cells(2 * 3_600, 1),
            (CORRIDOR_MAX_HALF_CELLS, true),
            "a truncated corridor reports saturation so the DRY claim fails closed"
        );
    }

    /// F6: `pos_in_grid` ("some projected sample is covered") and `current_pos_in_grid` ("the
    /// rider is covered") are different questions, and the honest **hourly-only** state hangs off
    /// the second one — a rider outside the bundle's rain grid whose ride enters it must not read
    /// as a stale cache that a refresh would fix.
    #[test]
    fn a_ride_entering_the_rain_grid_stays_hourly_only_where_the_rider_is() {
        let t0 = 1_800_000_000;
        // The ride enters the grid partway: the early frames are no-data, the later ones dry.
        let mut entering = synthetic(&[INTENSITY_NODATA, INTENSITY_NODATA, 0, 0, 0, 0, 0, 0, 0], t0);
        entering.current_pos_in_grid = false; // the rider themselves is outside the rain grid
        assert_eq!(rain_outlook(&entering, t0), RainOutlook::HourlyOnly, "no rain grid here — not 'update needed'");
        // Rain met along the way is still reported: hourly-only never swallows a warning.
        entering.frames[4].intensity = 7;
        assert_eq!(rain_outlook(&entering, t0), RainOutlook::RainIn { minutes: 60 });
        // The same holes with the rider *inside* the rain grid are the old, correct verdict.
        let mut inside = synthetic(&[INTENSITY_NODATA, INTENSITY_NODATA, 0, 0, 0, 0, 0, 0, 0], t0);
        inside.current_pos_in_grid = true;
        assert_eq!(rain_outlook(&inside, t0), RainOutlook::UpdateNeeded);
    }

    /// The strip's covering rule: inside a window the frame's sample answers, past the cap and
    /// before the first frame nothing does, and the no-data sentinel never answers.
    #[test]
    fn intensity_covering_respects_windows() {
        let t0 = 1_800_000_000;
        let snap = synthetic(&[2, 0, 7], t0);
        assert_eq!(snap.intensity_covering(t0 - 1), None, "before the first frame");
        assert_eq!(snap.intensity_covering(t0), Some(2));
        assert_eq!(snap.intensity_covering(t0 + 899), Some(2));
        assert_eq!(snap.intensity_covering(t0 + 900), Some(0));
        assert_eq!(snap.intensity_covering(t0 + 1_800 + 900), Some(7), "last frame holds through its cap");
        assert_eq!(snap.intensity_covering(t0 + 1_800 + 901), None, "and not a second longer");
        let holed = synthetic(&[2, INTENSITY_NODATA, 7], t0);
        assert_eq!(holed.intensity_covering(t0 + 1_000), None, "no-data never answers");
    }

    /// Time-step range: with the DWD table current at its first frame there are eight future
    /// frames; past the last frame's cap there are none.
    #[test]
    fn steps_ahead_tracks_currency() {
        let (snap, first) = snapshot_at_center();
        assert_eq!(snap.steps_ahead(first), 8);
        assert_eq!(snap.steps_ahead(first + 4 * 900), 4);
        assert_eq!(snap.steps_ahead(first + 9 * 900 + 1), 0, "nothing current, nothing to step");
    }

    /// The resident snapshot stays within its declared budget (measured on the host; the board
    /// figure lands in the resource report with the WX8 mount).
    #[test]
    fn snapshot_resident_size_is_bounded() {
        assert!(core::mem::size_of::<WeatherSnapshot>() <= 1_024, "snapshot must stay ~sub-KiB resident");
        // The two dry-claim flags land in the padding `valid_at`'s alignment already forced, so
        // the frame table (16 × this) costs the host buffer nothing new.
        assert_eq!(core::mem::size_of::<FrameSample>(), 24);
    }

    /// The route-relative classification (WX12's seam): no travel direction is `None` — never a
    /// fabricated head/tail — and the three sectors land where the locked UX puts them.
    #[test]
    fn wind_classification_sectors_and_neutral_fallback() {
        assert_eq!(wind_class(225, None), None, "no travel direction: neutral, never a false claim");
        // Travelling due north: wind FROM the south blows north — a tailwind.
        assert_eq!(wind_class(180, Some(0.0)), Some(WindClass::Tail));
        // Wind from the north, travelling north: a headwind.
        assert_eq!(wind_class(0, Some(0.0)), Some(WindClass::Head));
        // Wind from the west, travelling north: a crosswind (90° off).
        assert_eq!(wind_class(270, Some(0.0)), Some(WindClass::Cross));
        // Sector boundaries: 60° = still tail, 120° = already head.
        assert_eq!(wind_class(240, Some(0.0)), Some(WindClass::Tail));
        assert_eq!(wind_class(300, Some(0.0)), Some(WindClass::Head));
    }

    /// Octant labels: the from-direction rounds to the nearest of 8 compass points.
    #[test]
    fn wind_octants_round_to_the_nearest_point() {
        assert_eq!(wind_octant(0), 0); // N
        assert_eq!(wind_octant(22), 0);
        assert_eq!(wind_octant(23), 1); // NE
        assert_eq!(wind_octant(225), 5); // SW
        assert_eq!(wind_octant(359), 0); // wraps back to N
    }

    /// The projection's pace window: stopped fixes never erode the estimate, a GPS teleport is
    /// capped (and out-voted by the median anyway), and an empty window reads `None` so callers
    /// apply the touring fallback.
    #[test]
    fn speed_window_median_ignores_stops_and_caps_teleports() {
        let mut w = SpeedWindow::new();
        assert_eq!(w.median_cms(), None, "no moving sample yet → fallback territory");
        w.push_mps(0.0); // parked
        w.push_mps(0.4); // pushing the bike — below the moving threshold
        assert_eq!(w.median_cms(), None, "stopped fixes are not a pace");
        for mps in [3.0, 5.0, 4.0] {
            w.push_mps(mps);
        }
        assert_eq!(w.median_cms(), Some(400));
        w.push_mps(80.0); // multipath teleport
        let capped = w.median_cms().unwrap();
        assert!(capped <= SPEED_CAP_CMS, "no single sample exceeds the cap");
        // Even-length windows take the upper middle: {300,400,500,1500(capped)} → 500 — one
        // glitch shifts the estimate a rank, never to the glitch.
        assert_eq!(capped, 500, "…and the median barely notices one glitch");
        // Saturate the ring: the estimate follows the recent pace, not ancient history.
        for _ in 0..SpeedWindow::SPEED_SAMPLES {
            w.push_mps(6.0);
        }
        assert_eq!(w.median_cms(), Some(600));
    }

    /// The tangent/bearing arithmetic: cardinal directions land on their degrees and coincident
    /// points refuse an answer.
    #[test]
    fn bearing_deg_cardinals_and_degenerate() {
        let a = (47_000_000, 8_000_000);
        assert_eq!(bearing_deg(a, (47_010_000, 8_000_000)).map(|d| d.round()), Some(0.0), "north");
        assert_eq!(bearing_deg(a, (47_000_000, 8_010_000)).map(|d| d.round()), Some(90.0), "east");
        assert_eq!(bearing_deg(a, (46_990_000, 8_000_000)).map(|d| d.round()), Some(180.0), "south");
        assert_eq!(bearing_deg(a, (47_000_000, 7_990_000)).map(|d| d.round()), Some(270.0), "west");
        assert_eq!(bearing_deg(a, a), None, "coincident points have no bearing");
    }

    /// Local time folding: offsets shift across midnight correctly.
    #[test]
    fn local_hour_minute_folds_offsets() {
        assert_eq!(local_hour_minute(0, 0), (0, 0));
        assert_eq!(local_hour_minute(13 * 3_600 + 35 * 60, 0), (13, 35));
        assert_eq!(local_hour_minute(23 * 3_600, 120), (1, 0), "+2 h rolls past midnight");
        assert_eq!(local_hour_minute(3_600, -120), (23, 0), "-2 h rolls back across midnight");
    }
}

// ==================== the Weather domain protocol (#1436) ====================
//
// WeatherDomain owns what the rider may be *told*: visible freshness, the alert policy, and the
// identity of the installed data. The platform weather task owns the radio, the provider's timing,
// decoding and store access — it reports typed outcomes and external facts and decides none of the
// honesty rules this module derives.
//
// The bundle never crosses. An `OpenInstalledData` outcome names the product it opened; the frames
// themselves stay in the store behind [`WeatherSnapshot::sample`], exactly as they do today.

use crate::device_core::{
    DataIdentity, OperationToken, Revision, TokenSource, WeatherCapabilities, WeatherData, WeatherTag,
};

/// What the UI asks of the weather domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherIntent {
    /// Fetch fresh weather for the current position. Idempotent: a repeat while one is in flight
    /// coalesces rather than stacking a second request on a metered link.
    RefreshRequested,
}

/// One bounded physical weather operation, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherEffect {
    /// Ask the companion for a fresh bundle and install it.
    RequestRefresh { token: OperationToken<WeatherTag> },
    /// Open the installed data set so the screens can sample it.
    OpenInstalledData { token: OperationToken<WeatherTag>, data: DataIdentity },
}

impl WeatherEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<WeatherTag> {
        match self {
            WeatherEffect::RequestRefresh { token } | WeatherEffect::OpenInstalledData { token, .. } => *token,
        }
    }
}

/// Why a weather operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherError {
    /// The companion link dropped mid-request.
    LinkLost,
    /// The provider had nothing for this position, or refused.
    NoData,
    /// The installed data could not be read or decoded.
    Unreadable,
}

/// The result of one [`WeatherEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherOutcome {
    /// A fresh bundle was installed as `data` at `revision`.
    Refreshed { token: OperationToken<WeatherTag>, data: DataIdentity, revision: Revision },
    /// The installed data set is open and samplable.
    Opened { token: OperationToken<WeatherTag>, data: DataIdentity, revision: Revision },
    /// The operation failed. Nothing is claimed about the weather on this path — a failed refresh
    /// leaves the previous snapshot standing and ages honestly.
    Failed { token: OperationToken<WeatherTag>, error: WeatherError },
    /// The executor abandoned the operation without completing it.
    Cancelled { token: OperationToken<WeatherTag> },
}

impl WeatherOutcome {
    /// The operation this outcome answers.
    pub fn token(&self) -> OperationToken<WeatherTag> {
        match self {
            WeatherOutcome::Refreshed { token, .. }
            | WeatherOutcome::Opened { token, .. }
            | WeatherOutcome::Failed { token, .. }
            | WeatherOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires: an identity and a revision — never a bundle, a frame or a grid.
const _: () = assert!(core::mem::size_of::<WeatherIntent>() == 0, "one fieldless request");
const _: () = assert!(core::mem::size_of::<WeatherEffect>() <= 16, "a token and a data identity");
const _: () = assert!(core::mem::size_of::<WeatherOutcome>() <= 24, "a token, an identity and a revision");
const _: () = assert!(core::mem::size_of::<WeatherError>() <= 1, "a verdict, not a report");

// ==================== WeatherDomain (#1437) ====================

/// How the last completed refresh ended — the terminal state the rider is owed an honest answer
/// from. A failure never invents weather: the previous bundle stays and keeps ageing under its own
/// arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshResult {
    /// A fresh bundle was installed.
    Installed,
    /// The refresh failed for this reason.
    Failed(WeatherError),
    /// The platform abandoned the refresh without completing it.
    Cancelled,
}

/// Everything the weather screens may say about freshness, in one value.
///
/// The bundle's own honesty arithmetic ([`rain_outlook`]) answers *what may be claimed*; this adds
/// the device-side half — is there data at all, and is an update running right now — that a
/// snapshot cannot know about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherVisible {
    /// What the dashboard may honestly claim at `now`, or `None` when no bundle is sampled.
    pub outlook: Option<RainOutlook>,
    /// A refresh is in flight: the screens raise the non-blocking UPDATING cue over the cached
    /// content — they never blank it, because stale-and-labelled beats empty.
    pub refreshing: bool,
    /// Weather data is installed on the device at all.
    pub installed: bool,
}

/// The **one owner** of what the rider is told about weather (epic #1433 §5, #1437): the installed
/// data's identity and revision, visible freshness, the refresh request and its in-flight
/// operation, the last terminal result, and the alert decision.
///
/// The split with the platform is total. The platform owns provider timing, radio work, decoding
/// and storage access, and reports back through [`WeatherOutcome`] and the installed-data external
/// fact. It decides none of the honesty rules: not whether a bundle is fresh enough to claim
/// anything, not whether an alert fires, not whether a repeat request is worth a second radio trip.
///
/// **The bundle never lives here.** Frames stay in the store behind [`WeatherSnapshot::sample`];
/// this type holds identities, revisions and a handful of flags.
///
/// ## Where the cooldown lives
///
/// Alert dedup marks ([`AlertMarks`](crate::weather_alerts::AlertMarks)) must survive a reboot, so
/// their bytes sit in the persisted settings blob. This type is their only interpreter: nothing
/// else reads them, and [`mark_fired`](WeatherDomain::mark_fired) is the only thing that writes one.
/// Ownership of the *policy* and ownership of the *bytes* are deliberately separate — duplicating
/// the table here would be one more copy to keep in step for no gain.
#[derive(Debug)]
pub struct WeatherDomain {
    ops: TokenSource<WeatherTag>,
    installed: Option<WeatherData>,
    refresh_requested: bool,
    in_flight: Option<OperationToken<WeatherTag>>,
    last_result: Option<RefreshResult>,
}

impl WeatherDomain {
    /// The boot state: nothing installed, nothing requested, nothing in flight.
    pub const fn new() -> Self {
        WeatherDomain {
            ops: TokenSource::new(),
            installed: None,
            refresh_requested: false,
            in_flight: None,
            last_result: None,
        }
    }

    /// The installed data set and its revision, or `None` when none is installed.
    pub fn installed(&self) -> Option<WeatherData> {
        self.installed
    }

    /// The platform installed data (the DC2 external fact). Same rule as
    /// [`ExternalFacts::note_weather_data`](crate::device_core::ExternalFacts::note_weather_data):
    /// a *newer* revision of the same product wins, and a different product always replaces — two
    /// products' revisions have no order to compare.
    pub fn note_installed(&mut self, fact: WeatherData) {
        let keep = matches!(self.installed, Some(have) if have.data == fact.data && have.revision > fact.revision);
        if !keep {
            self.installed = Some(fact);
        }
    }

    /// Apply a [`WeatherIntent`]. A repeat while one is already requested **or in flight**
    /// coalesces, which is [`WeatherIntent::RefreshRequested`]'s own contract: the companion link is
    /// metered, and two taps of the same button are one question. The in-flight answer *is* the
    /// answer to the second tap, so it is dropped rather than queued behind the first.
    ///
    /// The trade-off, stated because it is a real one: a rider who moves a long way and taps refresh
    /// mid-fetch gets the fetch that was started at the old position. #1401 owns the request cutover
    /// and can revisit it against a real position delta rather than a guess.
    pub fn apply_intent(&mut self, intent: WeatherIntent) {
        match intent {
            WeatherIntent::RefreshRequested => self.refresh_requested |= self.in_flight.is_none(),
        }
    }

    /// Whether a refresh is in flight — the screens' non-blocking UPDATING cue.
    pub fn refreshing(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Whether a requested refresh has not gone out yet (no capability, or the slot was busy).
    pub fn refresh_pending(&self) -> bool {
        self.refresh_requested
    }

    /// How the last completed refresh ended, or `None` when none has completed this boot.
    pub fn last_refresh(&self) -> Option<RefreshResult> {
        self.last_result
    }

    /// The next bounded weather operation, or `None`. A refresh goes out only when one was asked
    /// for, the device can actually reach a companion ([`WeatherCapabilities::refresh`]), and
    /// nothing is already in flight. Withdrawing the capability does not drop the request — the
    /// rider asked, and the link coming back is what answers them.
    pub fn next_effect(&mut self, caps: WeatherCapabilities) -> Option<WeatherEffect> {
        if !self.refresh_requested || self.in_flight.is_some() || !caps.refresh {
            return None;
        }
        self.refresh_requested = false;
        let token = self.ops.issue();
        self.in_flight = Some(token);
        Some(WeatherEffect::RequestRefresh { token })
    }

    /// Consume the answer to a [`WeatherEffect`]. A stale token — a superseded operation, or a
    /// repeat of one already accounted for — changes nothing.
    pub fn apply_outcome(&mut self, outcome: WeatherOutcome) {
        if !self.ops.is_current(outcome.token()) {
            return;
        }
        self.ops.invalidate(); // terminal: a duplicate of this outcome is no longer current
        self.in_flight = None;
        match outcome {
            WeatherOutcome::Refreshed { data, revision, .. } => {
                self.note_installed(WeatherData { data, revision });
                self.last_result = Some(RefreshResult::Installed);
            }
            // Opening installed data is not a refresh: it fetched nothing, so it records the
            // identity it opened and leaves the last *refresh* verdict standing. Reporting
            // "installed" here would tell the rider a fetch succeeded when none ran.
            //
            // Unreachable through this domain today and therefore untested: `next_effect` only ever
            // emits `RequestRefresh`, so the only way to observe this arm would be to fabricate an
            // outcome no executor can legitimately send. #1401 is where `OpenInstalledData` starts
            // being issued; the rule is stated here so that cutover does not have to rediscover it.
            WeatherOutcome::Opened { data, revision, .. } => self.note_installed(WeatherData { data, revision }),
            WeatherOutcome::Failed { error, .. } => self.last_result = Some(RefreshResult::Failed(error)),
            WeatherOutcome::Cancelled { .. } => self.last_result = Some(RefreshResult::Cancelled),
        }
    }

    /// Everything the screens may say about freshness right now: the snapshot's own honest claim,
    /// plus the two device-side facts a snapshot cannot know about itself.
    ///
    /// The one part of this domain that is a *new shape* rather than a moved value, so it has no
    /// reader before the UI cutover. It exists as one call rather than three getters precisely so
    /// #1401 consumes it whole instead of re-deriving freshness from
    /// [`installed`](Self::installed) and [`refreshing`](Self::refreshing) at each screen — which is
    /// how the three states drifted apart in the first place.
    pub fn visible(&self, snapshot: Option<&WeatherSnapshot>, now: i64) -> WeatherVisible {
        WeatherVisible {
            outlook: snapshot.map(|snap| rain_outlook(snap, now)),
            refreshing: self.refreshing(),
            installed: self.installed.is_some(),
        }
    }

    /// Decide what the alert engine wants shown this pass: evaluate the centralized threshold table
    /// against `snapshot` at `now`, then govern the result against the persisted cooldown marks and
    /// the card already on the stack. No snapshot never alerts, and neither does expired data — the
    /// engine's own law.
    pub fn alert_action(
        &self,
        snapshot: Option<&WeatherSnapshot>,
        now: i64,
        marks: &crate::weather_alerts::AlertMarks,
        open_card: Option<crate::screen::WeatherAlertKind>,
    ) -> crate::weather_alerts::AlertAction {
        let Some(snapshot) = snapshot else {
            return crate::weather_alerts::AlertAction::None;
        };
        let candidates = crate::weather_alerts::evaluate(snapshot, now);
        crate::weather_alerts::govern(&candidates, marks, open_card)
    }

    /// Record that `candidate`'s card actually reached the rider, starting its persisted cooldown.
    ///
    /// Deliberately separate from [`alert_action`](WeatherDomain::alert_action): the presentation
    /// seam can refuse (a passkey prompt outranks the card, and so does a full screen stack), and
    /// marking a card nobody saw would sit on that storm for a whole persisted cooldown in silence.
    pub fn mark_fired(
        &self,
        candidate: &crate::weather_alerts::AlertCandidate,
        marks: &mut crate::weather_alerts::AlertMarks,
    ) {
        marks[candidate.class.slot()] = Some(crate::weather_alerts::mark_of(candidate));
    }
}

impl Default for WeatherDomain {
    fn default() -> Self {
        WeatherDomain::new()
    }
}

// Layout tripwire: identities, a token and three flags. The snapshot is an order of magnitude
// bigger and stays out.
const _: () = assert!(core::mem::size_of::<WeatherDomain>() <= 48, "identities and flags, never a bundle");

#[cfg(test)]
mod domain_tests {
    use super::*;
    use crate::device_core::{DataIdentity, Revision};

    fn data(id: u64, revision: u64) -> WeatherData {
        WeatherData { data: DataIdentity::new(id), revision: Revision::new(revision) }
    }

    fn can_refresh() -> WeatherCapabilities {
        WeatherCapabilities { refresh: true, installed_data: true }
    }

    /// The request lifecycle: one effect per request, coalesced repeats, and a terminal outcome that
    /// both frees the slot and records how it ended.
    #[test]
    fn a_refresh_goes_out_once_and_its_outcome_is_terminal() {
        let mut wx = WeatherDomain::new();
        assert!(wx.next_effect(can_refresh()).is_none(), "nothing was asked for");

        wx.apply_intent(WeatherIntent::RefreshRequested);
        wx.apply_intent(WeatherIntent::RefreshRequested); // a second tap is the same question
        assert!(wx.refresh_pending());

        let Some(WeatherEffect::RequestRefresh { token }) = wx.next_effect(can_refresh()) else {
            panic!("the request goes out");
        };
        assert!(wx.refreshing() && !wx.refresh_pending());
        assert!(wx.next_effect(can_refresh()).is_none(), "one refresh in flight at a time");

        wx.apply_outcome(WeatherOutcome::Refreshed { token, data: DataIdentity::new(4), revision: Revision::new(2) });
        assert_eq!(wx.last_refresh(), Some(RefreshResult::Installed));
        assert_eq!(wx.installed(), Some(data(4, 2)));
        assert!(!wx.refreshing());

        // The same answer arriving twice must not re-run any of that.
        wx.apply_outcome(WeatherOutcome::Failed { token, error: WeatherError::LinkLost });
        assert_eq!(wx.last_refresh(), Some(RefreshResult::Installed), "a repeated outcome is stale");
    }

    /// A request raised while one is in flight is dropped, not queued: the in-flight answer is the
    /// answer to it. Only once that operation is terminal does a fresh tap start a second fetch.
    #[test]
    fn a_repeat_while_one_is_in_flight_is_coalesced_away() {
        let mut wx = WeatherDomain::new();
        wx.apply_intent(WeatherIntent::RefreshRequested);
        let Some(effect) = wx.next_effect(can_refresh()) else { panic!("the first request goes out") };

        wx.apply_intent(WeatherIntent::RefreshRequested); // the rider taps again mid-fetch
        assert!(!wx.refresh_pending(), "the repeat coalesced into the operation already running");

        wx.apply_outcome(WeatherOutcome::Refreshed {
            token: effect.token(),
            data: DataIdentity::new(1),
            revision: Revision::new(1),
        });
        assert!(wx.next_effect(can_refresh()).is_none(), "the coalesced tap did not queue a second fetch");

        // …and a tap *after* the operation ended is a new question, which does go out.
        wx.apply_intent(WeatherIntent::RefreshRequested);
        assert!(wx.next_effect(can_refresh()).is_some());
    }

    /// Without a companion there is nothing to ask, but the *request* survives: the rider asked, and
    /// the link coming back is what answers them.
    #[test]
    fn a_request_waits_for_the_capability_instead_of_failing() {
        let mut wx = WeatherDomain::new();
        wx.apply_intent(WeatherIntent::RefreshRequested);

        let offline = WeatherCapabilities { refresh: false, installed_data: false };
        assert!(wx.next_effect(offline).is_none(), "no link, no radio trip");
        assert!(wx.refresh_pending(), "and no invented failure either");

        assert!(wx.next_effect(can_refresh()).is_some(), "the link returns and the question goes out");
        assert_eq!(wx.last_refresh(), None, "nothing completed yet");
    }

    /// A failed refresh is terminal and honest: it records the reason, frees the slot for a retry,
    /// and leaves the previously installed data exactly where it was.
    #[test]
    fn a_failed_refresh_leaves_the_installed_data_standing() {
        let mut wx = WeatherDomain::new();
        wx.note_installed(data(1, 5));

        wx.apply_intent(WeatherIntent::RefreshRequested);
        let Some(effect) = wx.next_effect(can_refresh()) else { panic!("requested") };
        wx.apply_outcome(WeatherOutcome::Failed { token: effect.token(), error: WeatherError::NoData });

        assert_eq!(wx.last_refresh(), Some(RefreshResult::Failed(WeatherError::NoData)));
        assert_eq!(wx.installed(), Some(data(1, 5)), "a failure never drops what is installed");
        assert!(!wx.refreshing());
    }

    /// Installed-data identity follows the DC2 fact rule: a stale revision of the same product
    /// cannot walk the level backwards, and a different product is a replacement.
    #[test]
    fn installed_data_never_walks_backwards() {
        let mut wx = WeatherDomain::new();
        wx.note_installed(data(1, 7));
        wx.note_installed(data(1, 4));
        assert_eq!(wx.installed(), Some(data(1, 7)), "a late report cannot un-install newer data");

        wx.note_installed(data(2, 0));
        assert_eq!(wx.installed(), Some(data(2, 0)), "a different product replaces");
    }

    /// Visible freshness carries the device-side half a snapshot cannot know about itself.
    #[test]
    fn visible_state_reports_installation_and_refresh() {
        let mut wx = WeatherDomain::new();
        let blank = wx.visible(None, 0);
        assert_eq!(blank, WeatherVisible { outlook: None, refreshing: false, installed: false });

        wx.note_installed(data(1, 1));
        wx.apply_intent(WeatherIntent::RefreshRequested);
        let _ = wx.next_effect(can_refresh());
        let live = wx.visible(None, 0);
        assert!(live.installed && live.refreshing);
    }
}
