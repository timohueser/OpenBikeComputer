//! The resident **weather snapshot** and its honest-state derivation (WX11, epic #1185).
//!
//! The screens never stream OBCW at draw time: the host samples the mounted store **once** per
//! refresh/fix change into this compact resident snapshot ([`WeatherSnapshot::sample`] — bounded
//! reads through the WX7 cache), and every weather screen derives what it may *claim* from the
//! snapshot plus the frame's `now` ([`rain_outlook`]). Keeping the derivation pure and
//! time-parameterized is what makes the honesty laws testable: expired rain can never produce a
//! dry claim, incomplete two-hour coverage is **WEATHER UPDATE NEEDED**, and a corridor with no
//! precipitation product at all is the explicit hourly-only state — never a fake map, never dry.
//!
//! The *ride decision* proper (route-projected sampling, alert generation, dedup/cooldown) is
//! WX12; [`rain_outlook`] is deliberately the seam it will replace — a pure function of sampled
//! frame intensities at the rider's position. The freshness arithmetic mirrors
//! [`WeatherReader::current_frame`]'s fail-closed rule (per-frame cap = min inter-frame spacing,
//! bounded by [`FRAME_CURRENT_CAP_S`]) and is pinned against it by test, so what the overlay may
//! *render* and what a screen may *say* can never disagree.

use obc_formats::io::ByteSource;
use obc_formats::obcw::{HourlyRecord, HOURLY_COUNT, HOURLY_INTERVAL_SECONDS, INTENSITY_NODATA};
use obc_weather::{Error as WeatherError, WeatherCache, WeatherReader, FRAME_CURRENT_CAP_S};

/// Rain-frame samples the snapshot holds. OBCW's radar-de policy is nine 15-minute frames; a
/// bundle carrying more keeps its first sixteen and reports the truncation, which the outlook
/// treats as incomplete coverage past the last kept frame (never a silent dry claim).
pub const SNAPSHOT_MAX_FRAMES: usize = 16;

/// The two-hour claim window of the dashboard card, in seconds.
pub const OUTLOOK_WINDOW_S: i64 = 2 * 3_600;

/// Smallest 4-bit intensity band the outlook counts as rain (band 1 = < 0.10 mm/h).
pub const RAIN_MIN_INTENSITY: u8 = 1;

/// Smallest band the outlook counts as storm-grade: band 9 starts the >= 10 mm/h range — the same
/// boundary the epic locked for the heavy-rain alert. Provisional here in the sense that WX12 owns
/// the final decision thresholds; the constant lives in one place for that round.
pub const STORM_MIN_INTENSITY: u8 = 9;

/// One rain frame's timestamp plus the nearest-cell intensity sampled at the rider's position
/// ([`INTENSITY_NODATA`] when the position lies outside the grid or the tile read failed — missing
/// data stays missing, it never reads as dry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSample {
    pub valid_at: i64,
    pub intensity: u8,
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
    /// The sampled position lies inside the rain grid's bbox. `false` means the rain product
    /// does not cover the rider — the explicit hourly-only state.
    pub pos_in_grid: bool,
    /// The bundle carried more frames than [`SNAPSHOT_MAX_FRAMES`]; coverage past the last kept
    /// frame is unknown and the outlook refuses the dry claim there.
    pub frames_truncated: bool,
}

impl WeatherSnapshot {
    /// Sample the open bundle at `pos` (`(lat, lon)` microdegrees). Bounded I/O through the WX7
    /// fixed cache: 24 hourly reads, one descriptor sweep, and at most one tile decode per kept
    /// frame — run at refresh cadence by the host, never per rendered frame.
    pub fn sample<S: ByteSource + ?Sized>(
        reader: &WeatherReader<'_, S>,
        cache: &mut WeatherCache,
        pos: Option<(i32, i32)>,
    ) -> Result<Self, WeatherError> {
        let header = reader.header();
        let mut hourly = [HourlyRecord {
            valid_time_offset_s: 0,
            temperature_deci_c: 0,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: 0,
            wind_from_deg: 0,
            wind_speed_deci_ms: 0,
            wind_gust_deci_ms: 0,
            flags: 0,
        }; HOURLY_COUNT];
        for (index, slot) in hourly.iter_mut().enumerate() {
            *slot = reader.hourly(index)?;
        }

        let frame_count = header.frame_count as usize;
        let kept = frame_count.min(SNAPSHOT_MAX_FRAMES);
        let mut frames: heapless::Vec<FrameSample, SNAPSHOT_MAX_FRAMES> = heapless::Vec::new();
        let mut frame_cap_s = FRAME_CURRENT_CAP_S;
        let mut previous: Option<i64> = None;
        let mut pos_in_grid = false;
        for index in 0..frame_count {
            let frame = reader.frame(index)?;
            if let Some(prior) = previous {
                // Validated strictly increasing, so every spacing is positive. The cap must scan
                // the *whole* table (not just the kept prefix) to stay the reader's exact rule.
                frame_cap_s = frame_cap_s.min(frame.valid_at.saturating_sub(prior));
            }
            previous = Some(frame.valid_at);
            if index >= kept {
                continue;
            }
            let intensity = match pos {
                Some((lat, lon)) => match reader.intensity_at(index, lat, lon, cache) {
                    Ok(Some(value)) => {
                        pos_in_grid = true;
                        value
                    }
                    // Outside the grid, or a failed read: missing stays missing.
                    Ok(None) | Err(_) => INTENSITY_NODATA,
                },
                None => INTENSITY_NODATA,
            };
            // Capacity is `kept <= SNAPSHOT_MAX_FRAMES` by construction.
            let _ = frames.push(FrameSample { valid_at: frame.valid_at, intensity });
        }

        Ok(Self {
            generated_at: header.generated_at,
            valid_from: header.valid_from,
            valid_until: header.valid_until,
            hourly,
            frames,
            frame_cap_s,
            sampled_at: pos,
            pos_in_grid,
            frames_truncated: frame_count > SNAPSHOT_MAX_FRAMES,
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
    /// `valid_at + cap`, bounded by the bundle's `valid_until`.
    fn window_end(&self, index: usize) -> i64 {
        let frame = &self.frames[index];
        let capped = frame.valid_at.saturating_add(self.frame_cap_s);
        let end = match self.frames.get(index + 1) {
            Some(next) => next.valid_at.min(capped),
            None => capped,
        };
        end.min(self.valid_until)
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
    /// The bundle carries no rain product covering the rider's position: the explicit
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
/// - no rain frames, or the position outside the grid → [`RainOutlook::HourlyOnly`];
/// - **DRY only with complete coverage**: every 15-minute-grained instant of `[now, now+2h]`
///   inside a current frame's honest window, with no no-data sample and no truncation;
/// - anything wet inside the covered part is reported even when coverage is partial —
///   a gap suppresses the dry claim, never a rain warning;
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
                if snap.frames[index].intensity == INTENSITY_NODATA {
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
/// **This is the WX12 seam**: computing `travel_deg` (active-route tangent at the expected
/// position, else trustworthy GPS course) is WX12's; until it lands every caller passes `None`.
/// The classification itself is fixed here so the coloring can't drift when WX12 arrives:
/// the wind's *to*-direction within 60° of travel is a tailwind, within 60° of dead-opposite a
/// headwind, everything between a crosswind.
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

/// Local `(hour, minute)` of a UTC unix instant under the device's UTC offset — the weather
/// screens' one time-of-day formatter (frame timestamps, hourly rows, the freshness line). Pure
/// modular arithmetic; negative instants are clamped to zero (pre-1970 weather does not exist).
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
        let mut frames = heapless::Vec::new();
        for (index, &intensity) in intensities.iter().enumerate() {
            frames.push(FrameSample { valid_at: t0 + index as i64 * 900, intensity }).unwrap();
        }
        WeatherSnapshot {
            valid_from: t0 - 3_600,
            valid_until: t0 + 24 * 3_600,
            frames,
            frame_cap_s: 900,
            pos_in_grid: true,
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
        gap.frames.truncate(0);
        for (index, at) in [0i64, 900, 1_800, 5_400, 6_300, 7_200].iter().enumerate() {
            let _ = index;
            gap.frames.push(FrameSample { valid_at: t0 + at, intensity: INTENSITY_DRY }).unwrap();
        }
        gap.frame_cap_s = 900;
        assert_eq!(rain_outlook(&gap, t0), RainOutlook::UpdateNeeded, "a bake gap goes dark, not dry");
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

    /// Local time folding: offsets shift across midnight correctly.
    #[test]
    fn local_hour_minute_folds_offsets() {
        assert_eq!(local_hour_minute(0, 0), (0, 0));
        assert_eq!(local_hour_minute(13 * 3_600 + 35 * 60, 0), (13, 35));
        assert_eq!(local_hour_minute(23 * 3_600, 120), (1, 0), "+2 h rolls past midnight");
        assert_eq!(local_hour_minute(3_600, -120), (23, 0), "-2 h rolls back across midnight");
    }
}
