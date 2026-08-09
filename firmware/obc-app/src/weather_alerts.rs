//! Weather **alert generation** (WX12, epic #1185): deterministic, conservative thresholds over
//! the resident [`WeatherSnapshot`], deduplicated by event identity, cooldown persisted across
//! reboot — driving the WX11 alert card through
//! [`App::show_weather_alert`](crate::App::show_weather_alert).
//!
//! Laws:
//! - **Expired data never alerts.** Every candidate derives from samples whose honest windows are
//!   current under the snapshot's fail-closed cap (the rain classes) or from hourly records inside
//!   the bundle's validity (the forecast classes) — the same freshness arithmetic the screens use.
//! - **Deterministic**: the same snapshot + instant always evaluates to the same candidates, on
//!   firmware and simulator alike (no randomness, no host clocks other than `now`).
//! - **Deduplicated + cooldown-persisted**: one weather event fires at most one alert per
//!   [`COOLDOWN_S`] unless its severity *materially* escalates. The fired event's identity
//!   (class + onset instant + position) is an [`AlertMark`] persisted in the settings blob, so
//!   the same storm does not pop back up on the next frame — or the next boot.
//! - **Advisory, not official**: thresholds are bike-touring heuristics, not CAP warnings. The
//!   existing barometric-trend issue (#529) stays independent; no pressure trend is read here.
//!
//! ## Threshold rationale (the tuning table — epic risk #5)
//!
//! | class | trigger | horizon | why |
//! |---|---|---|---|
//! | [`HeavyRain`](AlertClass::HeavyRain) | intensity band ≥ [`HEAVY_RAIN_MIN_BAND`] (≥ 10 mm/h, the epic-locked boundary — the dashboard's STORM band) | [`HEAVY_RAIN_HORIZON_S`] (45 min) | ≥ 10 mm/h is soaked-through-in-minutes rain; 45 min ≈ shelter-finding time on tour, and radar frames beyond that are increasingly advective guesses. |
//! | [`Thunder`](AlertClass::Thunder) | canonical thunderstorm/hail condition in an hourly record | [`THUNDER_HORIZON_S`] (60 min) | lightning is a get-off-the-exposed-ridge hazard; the hourly section's resolution *is* the hour, so the horizon matches it. |
//! | [`Gust`](AlertClass::Gust) | gust ≥ [`GUST_MIN_DECI_MS`] (20 m/s) in an hourly record | [`GUST_HORIZON_S`] (60 min) | 20 m/s ≈ Beaufort 8 gusts — control-loss territory with luggage; same hourly-resolution horizon. |
//!
//! The table is deliberately centralized here (and pinned by boundary tests) so it can be tuned
//! from fixtures/on-road experience without touching OBCW, the screens, or the governor.

use obc_formats::obcw::{
    CONDITION_HAIL, CONDITION_THUNDERSTORM, HOURLY_INTERVAL_SECONDS, INTENSITY_NODATA, WIND_SPEED_UNAVAILABLE,
};

use crate::weather::WeatherSnapshot;

/// Heavy-rain trigger band: band 9 starts the ≥ 10 mm/h range — re-exported from the dashboard's
/// storm boundary so the card's STORM IN and the alert can never disagree.
pub const HEAVY_RAIN_MIN_BAND: u8 = crate::weather::STORM_MIN_INTENSITY;
/// Heavy rain must reach the corridor within this to alert (45 min).
pub const HEAVY_RAIN_HORIZON_S: i64 = 45 * 60;
/// Thunderstorm look-ahead (60 min).
pub const THUNDER_HORIZON_S: i64 = 60 * 60;
/// Dangerous-gust trigger, deci-m/s (200 = 20 m/s ≈ Beaufort 8 gusts).
pub const GUST_MIN_DECI_MS: u16 = 200;
/// Dangerous-gust look-ahead (60 min) — the hourly section's own resolution.
pub const GUST_HORIZON_S: i64 = 60 * 60;

/// One alert per event per this span, unless severity materially escalates. Also the temporal
/// half of the dedup identity: a candidate whose onset lies within this of the marked event (and
/// within [`DEDUP_DIST_M`] of it) *is* that event.
pub const COOLDOWN_S: i64 = 60 * 60;
/// Spatial half of the dedup identity: a slow front re-detected a few km along the route is the
/// same storm; a system met again 50 km later is a new encounter worth a new alert.
pub const DEDUP_DIST_M: f32 = 50_000.0;
/// A heavy-rain event has *materially escalated* when its intensity band rose by this much over
/// the marked severity (≥ 2 bands ≈ a doubling of rain rate at the table's log-ish spacing).
pub const ESCALATE_RAIN_BANDS: u8 = 2;
/// A gust event has materially escalated when the forecast gust rose by this much (m/s) over the
/// marked severity.
pub const ESCALATE_GUST_MS: u8 = 5;

/// The three alert classes, in **priority order** (most dangerous first — when several classes
/// qualify at once, the card shows the highest one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertClass {
    /// Canonical thunderstorm (or hail) in the hourly forecast.
    Thunder = 0,
    /// ≥ 10 mm/h rain reaching the ridden corridor (radar/model grid, route-projected samples).
    HeavyRain = 1,
    /// Dangerous gusts in the hourly forecast.
    Gust = 2,
}

/// How many classes exist — sizes the persisted mark table.
pub const ALERT_CLASSES: usize = 3;

impl AlertClass {
    /// Evaluation/priority order.
    pub const ORDER: [AlertClass; ALERT_CLASSES] = [AlertClass::Thunder, AlertClass::HeavyRain, AlertClass::Gust];

    /// The persisted mark slot for this class.
    pub const fn slot(self) -> usize {
        self as usize
    }

    /// The WX11 card face this class drives: heavy rain is the RAIN AHEAD card ("Heavy rain on
    /// the route ahead."), thunder the STORM AHEAD card, gusts the STRONG WIND card.
    pub fn kind(self) -> crate::screen::WeatherAlertKind {
        match self {
            AlertClass::Thunder => crate::screen::WeatherAlertKind::Storm,
            AlertClass::HeavyRain => crate::screen::WeatherAlertKind::Rain,
            AlertClass::Gust => crate::screen::WeatherAlertKind::Gust,
        }
    }

    /// Material-escalation rule, per class: does `new` severity outrank `marked` enough to break
    /// the cooldown? Thunder has one severity step (hail over plain thunder); the rain/gust
    /// deltas are the table constants above.
    fn escalated(self, marked: u8, new: u8) -> bool {
        match self {
            AlertClass::Thunder => new > marked,
            AlertClass::HeavyRain => new >= marked.saturating_add(ESCALATE_RAIN_BANDS),
            AlertClass::Gust => new >= marked.saturating_add(ESCALATE_GUST_MS),
        }
    }
}

/// One qualifying event this evaluation found: the class, when it reaches the rider (`minutes`
/// for the card, `onset` as the absolute event identity), where (`(lat, lon)` µdeg — the
/// projected sample position for rain, the hourly request coordinate for the forecast classes),
/// and its class-scaled severity (rain: intensity band; gust: m/s; thunder: 1, hail: 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertCandidate {
    pub class: AlertClass,
    pub minutes: u16,
    pub onset: i64,
    pub pos: Option<(i32, i32)>,
    pub severity: u8,
}

/// The persisted identity of the last **fired** alert of a class: the dedup/cooldown anchor.
/// Lives in the settings blob (v16) so it survives reboot — dedup compares *event* times, not
/// elapsed device time, so it needs no trusted clock at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertMark {
    /// The fired event's onset (UTC unix).
    pub onset: i64,
    /// The fired event's position (µdeg).
    pub lat: i32,
    pub lon: i32,
    /// The fired event's class-scaled severity.
    pub severity: u8,
}

/// The persisted per-class mark table, indexed by [`AlertClass::slot`].
pub type AlertMarks = [Option<AlertMark>; ALERT_CLASSES];

/// Evaluate every class against the snapshot at `now` — pure and total; at most one candidate
/// per class, in [`AlertClass::ORDER`]. Expired/stale/no-data never yields a candidate.
pub fn evaluate(snap: &WeatherSnapshot, now: i64) -> heapless::Vec<AlertCandidate, ALERT_CLASSES> {
    let mut out = heapless::Vec::new();
    if now < snap.valid_from || now > snap.valid_until {
        return out; // the whole bundle is outside its validity: nothing may alert (law)
    }
    if let Some(c) = evaluate_thunder(snap, now) {
        let _ = out.push(c);
    }
    if let Some(c) = evaluate_heavy_rain(snap, now) {
        let _ = out.push(c);
    }
    if let Some(c) = evaluate_gust(snap, now) {
        let _ = out.push(c);
    }
    out
}

/// Heavy rain: the earliest frame whose honest window overlaps `[now, now + 45 min]` with a
/// (route-projected, corridor-widened) sample at band ≥ 9. Frame currency is the snapshot's own
/// fail-closed window arithmetic, so an expired frame can't qualify. Severity is the maximum
/// qualifying band inside the horizon (the event's punch, not just its leading edge).
fn evaluate_heavy_rain(snap: &WeatherSnapshot, now: i64) -> Option<AlertCandidate> {
    let horizon = now + HEAVY_RAIN_HORIZON_S;
    let mut onset: Option<(i64, (i32, i32))> = None;
    let mut severity = 0u8;
    for (index, frame) in snap.frames.iter().enumerate() {
        if snap.window_end(index) < now || frame.valid_at > horizon {
            continue;
        }
        if frame.intensity != INTENSITY_NODATA && frame.intensity >= HEAVY_RAIN_MIN_BAND {
            if onset.is_none() {
                onset = Some((frame.valid_at.max(now), (frame.lat, frame.lon)));
            }
            severity = severity.max(frame.intensity);
        }
    }
    let (at, pos) = onset?;
    Some(AlertCandidate {
        class: AlertClass::HeavyRain,
        minutes: minutes_until(now, at),
        onset: at,
        pos: Some(pos),
        severity,
    })
}

/// Thunder: the earliest hourly record whose hour overlaps `[now, now + 60 min]` carrying the
/// canonical thunderstorm (severity 1) or hail (severity 2) condition. The hourly section is a
/// point forecast for the request coordinate — the epic's accepted approximation — so the event
/// position is the snapshot's sampled position.
fn evaluate_thunder(snap: &WeatherSnapshot, now: i64) -> Option<AlertCandidate> {
    hourly_scan(snap, now, THUNDER_HORIZON_S, |rec| match rec.condition {
        CONDITION_THUNDERSTORM => Some(1),
        CONDITION_HAIL => Some(2),
        _ => None,
    })
    .map(|(at, severity)| AlertCandidate {
        class: AlertClass::Thunder,
        minutes: minutes_until(now, at),
        onset: at,
        pos: snap.sampled_at,
        severity,
    })
}

/// Gusts: the earliest hourly record inside `[now, now + 60 min]` forecasting gusts at
/// ≥ 20 m/s. The wire's unavailable sentinel never qualifies. Severity is the gust in whole m/s.
fn evaluate_gust(snap: &WeatherSnapshot, now: i64) -> Option<AlertCandidate> {
    hourly_scan(snap, now, GUST_HORIZON_S, |rec| {
        (rec.wind_gust_deci_ms != WIND_SPEED_UNAVAILABLE && rec.wind_gust_deci_ms >= GUST_MIN_DECI_MS)
            .then(|| (rec.wind_gust_deci_ms / 10).min(u8::MAX as u16) as u8)
    })
    .map(|(at, severity)| AlertCandidate {
        class: AlertClass::Gust,
        minutes: minutes_until(now, at),
        onset: at,
        pos: snap.sampled_at,
        severity,
    })
}

/// Walk the hourly records whose hour-intervals overlap `[now, now + horizon]` (inside the
/// bundle's validity — the caller already gated `now` itself) and return the first qualifying
/// record's clamped onset + severity.
fn hourly_scan(
    snap: &WeatherSnapshot,
    now: i64,
    horizon_s: i64,
    qualify: impl Fn(&obc_formats::obcw::HourlyRecord) -> Option<u8>,
) -> Option<(i64, u8)> {
    let horizon = now + horizon_s;
    for (index, rec) in snap.hourly.iter().enumerate() {
        let start = snap.valid_from + index as i64 * HOURLY_INTERVAL_SECONDS as i64;
        let end = start + HOURLY_INTERVAL_SECONDS as i64;
        if end <= now || start > horizon || start > snap.valid_until {
            continue;
        }
        if let Some(severity) = qualify(rec) {
            return Some((start.max(now), severity));
        }
    }
    None
}

/// Clamped whole minutes from `now` to `at` (0 = already on the rider) — the card's number.
fn minutes_until(now: i64, at: i64) -> u16 {
    (at.saturating_sub(now).max(0) / 60).min(u16::MAX as i64) as u16
}

/// Is `candidate` the **same event** as `mark` (its class's last fired alert)? Same iff its onset
/// lies within [`COOLDOWN_S`] of the marked onset *and* its position within [`DEDUP_DIST_M`] of
/// the marked one (a candidate or mark without usable geometry compares by time alone — the
/// conservative read: fewer repeat pop-ups). A same-event candidate is suppressed unless it
/// [`escalated`](AlertClass::escalated).
pub fn same_event(class: AlertClass, candidate: &AlertCandidate, mark: &AlertMark) -> bool {
    let _ = class;
    if (candidate.onset - mark.onset).abs() > COOLDOWN_S {
        return false;
    }
    match candidate.pos {
        Some((lat, lon)) => {
            let cl = obc_map_scene::cos_lat(lat);
            obc_map_scene::ground_dist_m_cl((lon, lat), (mark.lon, mark.lat), cl) <= DEDUP_DIST_M
        }
        None => true,
    }
}

/// The governor's verdict for one evaluation pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertAction {
    /// Nothing qualifies (or everything qualifying is inside its cooldown and no card is up).
    None,
    /// Fire (or re-fire) the card with this candidate and **write its mark** — a new event, or a
    /// material escalation of the marked one.
    Fire(AlertCandidate),
    /// A card of this candidate's class is already up and the candidate is the same marked event:
    /// refresh the card's copy in place (kind + minutes), but do **not** rewrite the mark.
    Update(AlertCandidate),
}

/// Decide this pass's action: the highest-priority candidate that is *not* suppressed fires; a
/// suppressed candidate whose class matches the card already on the stack still updates that
/// card's countdown in place (the WX11 update-in-place seam). Pure — the caller owns pushing the
/// card and persisting the mark.
pub fn govern(
    candidates: &[AlertCandidate],
    marks: &AlertMarks,
    open_card: Option<crate::screen::WeatherAlertKind>,
) -> AlertAction {
    // Highest priority first: candidates arrive in `AlertClass::ORDER` from `evaluate`.
    for c in candidates {
        let suppressed = match marks[c.class.slot()] {
            Some(mark) => same_event(c.class, c, &mark) && !c.class.escalated(mark.severity, c.severity),
            None => false,
        };
        if !suppressed {
            return AlertAction::Fire(*c);
        }
        if open_card == Some(c.class.kind()) {
            return AlertAction::Update(*c);
        }
    }
    AlertAction::None
}

/// The mark a fired candidate persists.
pub fn mark_of(candidate: &AlertCandidate) -> AlertMark {
    let (lat, lon) = candidate.pos.unwrap_or((0, 0));
    AlertMark { onset: candidate.onset, lat, lon, severity: candidate.severity }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather::{FrameSample, WeatherSnapshot, SNAPSHOT_MAX_FRAMES};
    use obc_formats::obcw::{HourlyRecord, CONDITION_RAIN, HOURLY_COUNT};

    const T0: i64 = 1_800_000_000;
    const POS: (i32, i32) = (47_000_000, 8_000_000);

    /// A synthetic snapshot: nine 15-min frames of `intensities` from `T0`, dry-ish hourly rows.
    fn snap(intensities: &[u8]) -> WeatherSnapshot {
        assert!(intensities.len() <= SNAPSHOT_MAX_FRAMES);
        let mut frames = heapless::Vec::new();
        for (i, &intensity) in intensities.iter().enumerate() {
            frames.push(FrameSample { valid_at: T0 + i as i64 * 900, intensity, lat: POS.0, lon: POS.1 }).unwrap();
        }
        WeatherSnapshot {
            generated_at: T0,
            valid_from: T0 - 3_600,
            valid_until: T0 + 24 * 3_600,
            hourly: [HourlyRecord {
                valid_time_offset_s: 0,
                temperature_deci_c: 150,
                precipitation_tenth_mm: 0,
                precipitation_probability_pct: 0,
                condition: CONDITION_RAIN,
                wind_from_deg: 200,
                wind_speed_deci_ms: 40,
                wind_gust_deci_ms: 80,
                flags: 0,
            }; HOURLY_COUNT],
            frames,
            frame_cap_s: 900,
            sampled_at: Some(POS),
            pos_in_grid: true,
            projected: true,
            frames_truncated: false,
            rain_grid: None,
        }
    }

    /// The record index covering instant `t` for `snap`'s validity origin.
    fn hour_of(t: i64) -> usize {
        ((t - (T0 - 3_600)) / 3_600) as usize
    }

    #[test]
    fn heavy_rain_threshold_and_horizon_boundaries() {
        // Band 8 (just under 10 mm/h) never alerts; band 9 does — the epic-locked boundary.
        assert!(evaluate_heavy_rain(&snap(&[0, 8, 0, 0, 0, 0, 0, 0, 0]), T0).is_none());
        let c = evaluate_heavy_rain(&snap(&[0, 9, 0, 0, 0, 0, 0, 0, 0]), T0).unwrap();
        assert_eq!((c.minutes, c.severity), (15, 9));
        assert_eq!(c.pos, Some(POS));
        // Horizon: a band-9 frame at +45 min exactly alerts; at +45 min + one frame it doesn't.
        let at_45 = snap(&[0, 0, 0, 9, 0, 0, 0, 0, 0]); // frame 3 = +45 min
        assert_eq!(evaluate_heavy_rain(&at_45, T0).unwrap().minutes, 45);
        let past_45 = snap(&[0, 0, 0, 0, 9, 0, 0, 0, 0]); // frame 4 = +60 min
        assert!(evaluate_heavy_rain(&past_45, T0).is_none());
        // Severity is the max band inside the horizon, onset the first crossing.
        let c = evaluate_heavy_rain(&snap(&[0, 9, 12, 0, 0, 0, 0, 0, 0]), T0).unwrap();
        assert_eq!((c.minutes, c.severity), (15, 12));
        // Raining band-9 *now*: zero minutes.
        assert_eq!(evaluate_heavy_rain(&snap(&[9, 0, 0, 0, 0, 0, 0, 0, 0]), T0).unwrap().minutes, 0);
    }

    #[test]
    fn expired_or_nodata_never_alerts() {
        let s = snap(&[12, 12, 12, 12, 12, 12, 12, 12, 12]);
        // Past the bundle validity: nothing, no matter how violent the (stale) samples.
        assert!(evaluate(&s, s.valid_until + 1).is_empty());
        assert!(evaluate(&s, s.valid_from - 1).is_empty());
        // Past every frame's currency window (but inside validity): the rain class stays silent.
        assert!(evaluate_heavy_rain(&s, T0 + 9 * 900 + 901).is_none());
        // A no-data sample is not rain.
        let s = snap(&[obc_formats::obcw::INTENSITY_NODATA; 9]);
        assert!(evaluate_heavy_rain(&s, T0).is_none());
    }

    #[test]
    fn thunder_and_gust_scan_the_hourly_window() {
        let mut s = snap(&[0; 9]);
        // Thunder in the record covering now: fires at 0 minutes, severity 1.
        s.hourly[hour_of(T0)].condition = obc_formats::obcw::CONDITION_THUNDERSTORM;
        let c = evaluate_thunder(&s, T0).unwrap();
        assert_eq!((c.minutes, c.severity), (0, 1));
        // Hail is the escalated severity of the same class.
        s.hourly[hour_of(T0)].condition = obc_formats::obcw::CONDITION_HAIL;
        assert_eq!(evaluate_thunder(&s, T0).unwrap().severity, 2);
        // Thunder in the *next* hour alerts (its start is inside the 60-min horizon)…
        let mut s = snap(&[0; 9]);
        s.hourly[hour_of(T0) + 1].condition = obc_formats::obcw::CONDITION_THUNDERSTORM;
        assert!(evaluate_thunder(&s, T0).is_some());
        // …but two hours out does not.
        let mut s = snap(&[0; 9]);
        s.hourly[hour_of(T0) + 2].condition = obc_formats::obcw::CONDITION_THUNDERSTORM;
        assert!(evaluate_thunder(&s, T0).is_none());

        // Gusts: 19.9 m/s never, 20.0 m/s fires; the unavailable sentinel never.
        let mut s = snap(&[0; 9]);
        s.hourly[hour_of(T0)].wind_gust_deci_ms = GUST_MIN_DECI_MS - 1;
        assert!(evaluate_gust(&s, T0).is_none());
        s.hourly[hour_of(T0)].wind_gust_deci_ms = GUST_MIN_DECI_MS;
        assert_eq!(evaluate_gust(&s, T0).unwrap().severity, 20);
        s.hourly[hour_of(T0)].wind_gust_deci_ms = WIND_SPEED_UNAVAILABLE;
        assert!(evaluate_gust(&s, T0).is_none());
    }

    #[test]
    fn priority_order_is_thunder_rain_gust() {
        let mut s = snap(&[9, 0, 0, 0, 0, 0, 0, 0, 0]);
        s.hourly[hour_of(T0)].condition = obc_formats::obcw::CONDITION_THUNDERSTORM;
        s.hourly[hour_of(T0)].wind_gust_deci_ms = 250;
        let cands = evaluate(&s, T0);
        assert_eq!(cands.len(), 3);
        assert_eq!(cands[0].class, AlertClass::Thunder);
        assert_eq!(cands[1].class, AlertClass::HeavyRain);
        assert_eq!(cands[2].class, AlertClass::Gust);
        // The governor fires the highest-priority unsuppressed one.
        let marks: AlertMarks = [None; ALERT_CLASSES];
        assert!(matches!(govern(&cands, &marks, None), AlertAction::Fire(c) if c.class == AlertClass::Thunder));
        // With thunder marked (same event), heavy rain fires next.
        let mut marks = marks;
        marks[AlertClass::Thunder.slot()] = Some(mark_of(&cands[0]));
        assert!(matches!(govern(&cands, &marks, None), AlertAction::Fire(c) if c.class == AlertClass::HeavyRain));
    }

    #[test]
    fn dedup_cooldown_and_escalation() {
        let s = snap(&[0, 9, 0, 0, 0, 0, 0, 0, 0]);
        let c = evaluate_heavy_rain(&s, T0).unwrap();
        let mut marks: AlertMarks = [None; ALERT_CLASSES];
        // First sight: fires.
        assert!(matches!(govern(&[c], &marks, None), AlertAction::Fire(_)));
        marks[AlertClass::HeavyRain.slot()] = Some(mark_of(&c));
        // Same event a frame later: suppressed with no card, updates an open card in place.
        assert_eq!(govern(&[c], &marks, None), AlertAction::None);
        assert!(matches!(govern(&[c], &marks, Some(crate::screen::WeatherAlertKind::Rain)), AlertAction::Update(_)));
        // A card of a *different* class does not adopt it.
        assert_eq!(govern(&[c], &marks, Some(crate::screen::WeatherAlertKind::Storm)), AlertAction::None);
        // One band hotter: still the same event, still suppressed (< the escalation delta)…
        let hotter = AlertCandidate { severity: c.severity + 1, ..c };
        assert_eq!(govern(&[hotter], &marks, None), AlertAction::None);
        // …but two bands hotter is a material escalation: re-fires.
        let escalated = AlertCandidate { severity: c.severity + ESCALATE_RAIN_BANDS, ..c };
        assert!(matches!(govern(&[escalated], &marks, None), AlertAction::Fire(_)));
        // An onset past the cooldown is a new event.
        let later = AlertCandidate { onset: c.onset + COOLDOWN_S + 1, ..c };
        assert!(matches!(govern(&[later], &marks, None), AlertAction::Fire(_)));
        // A storm met again far along the route is a new encounter (same clock, far position).
        let far = AlertCandidate { pos: Some((POS.0 + 900_000, POS.1)), ..c }; // ~100 km north
        assert!(matches!(govern(&[far], &marks, None), AlertAction::Fire(_)));
        // …while a re-detection a couple of km on is the same storm.
        let near = AlertCandidate { pos: Some((POS.0 + 20_000, POS.1)), ..c }; // ~2 km north
        assert_eq!(govern(&[near], &marks, None), AlertAction::None);
    }

    #[test]
    fn class_kind_mapping_matches_the_card_copy() {
        use crate::screen::WeatherAlertKind;
        assert_eq!(AlertClass::HeavyRain.kind(), WeatherAlertKind::Rain);
        assert_eq!(AlertClass::Thunder.kind(), WeatherAlertKind::Storm);
        assert_eq!(AlertClass::Gust.kind(), WeatherAlertKind::Gust);
    }
}
