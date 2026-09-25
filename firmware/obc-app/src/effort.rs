//! Effort: heart rate and power against the rider's own limits.
//!
//! [`Effort`] turns the raw BLE samples into what the riding views draw: a zone per metric with
//! hysteresis, a 10 s power average, and a five-minute history of 5 s buckets. Every consumer reads
//! the same smoothed power, so the PWR tile, its tint, the map gauge and the graph agree.
//!
//! Zones are indices `0..=4` for Z1..Z5. Without a limit (max HR or FTP of 0) nothing has a zone.
//! The limits are the rider's settings and are passed in, never copied here.

pub use obc_formats::ride::EffortLimits;
use obc_route::POWER_STEP_W;

/// Bars in a history graph: five minutes of 5 s buckets.
pub const HISTORY_BARS: usize = 60;
const BUCKET_MS: u32 = 5_000;
/// The power average window, in one-second slots.
const SMOOTH_S: u32 = 10;
/// The stored "no zone" of a metric without a limit or a value.
const NO_ZONE: u8 = u8::MAX;

/// Editor bounds for the two limits. `0` stores "not set"; an unset editor opens on the start.
pub const MAX_HR_MIN: u8 = 120;
pub const MAX_HR_MAX: u8 = 220;
pub const MAX_HR_START: u8 = 180;
pub const FTP_MIN: u16 = 50;
pub const FTP_MAX: u16 = 600;
pub const FTP_STEP: u16 = 5;
pub const FTP_START: u16 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Hr,
    Power,
}

impl Metric {
    /// The Z2..Z5 edges in percent of the limit, and whether a value exactly on the edge is
    /// already in the upper zone. They follow the issue's table: HR `60-70, 70-80` shares its
    /// ends upward, and power `55-75, 76-90` puts 75.5 % in Z3 and 105.6 % in Z5.
    const fn edges(self) -> [(u32, bool); 4] {
        match self {
            Metric::Hr => [(60, true), (70, true), (80, true), (90, true)],
            Metric::Power => [(55, true), (75, false), (90, false), (105, false)],
        }
    }

    /// The gauge's percent span: its left and right ends.
    const fn gauge_span(self) -> (u32, u32) {
        match self {
            Metric::Hr => (50, 100),
            Metric::Power => (0, 130),
        }
    }

    /// The graph's percent span: its baseline and its top.
    pub const fn graph_span(self) -> (u32, u32) {
        match self {
            Metric::Hr => (50, 100),
            Metric::Power => (0, 120),
        }
    }

    /// How far past a zone edge a value must go before the zone changes.
    const fn margin(self, limit: u32) -> u32 {
        match self {
            Metric::Hr => 2,
            Metric::Power => limit * 2 / 100,
        }
    }

    /// This metric's limit, or `None` when it is not set.
    pub fn limit(self, limits: EffortLimits) -> Option<u32> {
        let v = match self {
            Metric::Hr => limits.max_hr as u32,
            Metric::Power => limits.ftp_w as u32,
        };
        (v > 0).then_some(v)
    }

    /// The zone of `value` against `limit`, with no hysteresis.
    pub fn zone_of(self, value: u32, limit: u32) -> u8 {
        let v = value * 100;
        self.edges().iter().filter(|&&(e, on)| if on { v >= e * limit } else { v > e * limit }).count() as u8
    }

    /// The zone after `value`, given the zone shown before it.
    fn step(self, shown: Option<u8>, value: u32, limit: u32) -> u8 {
        let Some(shown) = shown else { return self.zone_of(value, limit) };
        let m = self.margin(limit);
        let up = self.zone_of(value.saturating_sub(m), limit);
        let down = self.zone_of(value + m, limit);
        if up > shown {
            up
        } else if down < shown {
            down
        } else {
            shown
        }
    }

    /// The position through the five zones, `0.0..=5.0`: the zone index plus the fraction through
    /// it. Each zone is one equal slot of the gauge.
    fn position(self, value: u32, limit: u32) -> f32 {
        let (lo, hi) = self.gauge_span();
        let e = self.edges();
        let edges = [lo, e[0].0, e[1].0, e[2].0, e[3].0, hi];
        let pct = value as f32 * 100.0 / limit.max(1) as f32;
        for z in 0..5 {
            let (a, b) = (edges[z] as f32, edges[z + 1] as f32);
            if pct < b {
                return z as f32 + ((pct - a) / (b - a)).max(0.0);
            }
        }
        5.0
    }

    /// A bucket average as one history byte, and back. `0` is a bucket with no data.
    fn to_bar(self, v: u16) -> u8 {
        match self {
            Metric::Hr => v.min(255) as u8,
            Metric::Power => v.div_ceil(POWER_STEP_W).min(255) as u8,
        }
    }

    fn bar_value(self, b: u8) -> u16 {
        match self {
            Metric::Hr => b as u16,
            Metric::Power => b as u16 * POWER_STEP_W,
        }
    }
}

/// A live value and its zone. The zone is `None` without a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub value: u16,
    pub zone: Option<u8>,
}

/// The map gauge as drawn: the zone colour and the fill length in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gauge {
    pub zone: u8,
    pub fill_px: u16,
}

/// The map gauge for a `w` px wide panel: power when the meter is live and FTP is set, otherwise
/// heart rate when the strap is live and max HR is set, otherwise none.
pub fn gauge(power: Option<Reading>, hr: Option<Reading>, limits: EffortLimits, w: i32) -> Option<Gauge> {
    let (m, r, limit) =
        [(Metric::Power, power), (Metric::Hr, hr)].into_iter().find_map(|(m, r)| Some((m, r?, m.limit(limits)?)))?;
    let slot = (w / 5).max(0);
    let fill = (m.position(r.value as u32, limit) * slot as f32) as i32;
    Some(Gauge { zone: r.zone?, fill_px: fill.clamp(0, 5 * slot) as u16 })
}

/// The 10 s power average: the latest watts of each of the last ten seconds.
#[derive(Debug)]
struct Smoother {
    watts: [u16; SMOOTH_S as usize],
    /// Bit `k` is set when slot `k` holds a second inside the window.
    valid: u16,
    /// The newest second a sample arrived in.
    newest: u32,
}

impl Smoother {
    fn push(&mut self, watts: u16, now_ms: u32) {
        let s = now_ms / 1000;
        let passed = s.wrapping_sub(self.newest);
        if passed >= SMOOTH_S {
            // A gap of a window or more, or a clock that went back, empties the window.
            self.valid = 0;
        } else {
            for k in 1..=passed {
                self.valid &= !(1 << ((self.newest + k) % SMOOTH_S));
            }
        }
        let i = s % SMOOTH_S;
        self.watts[i as usize] = watts;
        self.valid |= 1 << i;
        self.newest = s;
    }

    fn mean(&self) -> u16 {
        let n = self.valid.count_ones();
        let sum: u32 =
            (0..SMOOTH_S).filter(|k| self.valid & (1 << k) != 0).map(|k| self.watts[k as usize] as u32).sum();
        (sum / n.max(1)) as u16
    }
}

#[derive(Debug)]
pub struct Effort {
    /// Closed bucket averages per metric, indexed by bucket number modulo [`HISTORY_BARS`].
    hist: [[u8; HISTORY_BARS]; 2],
    /// The open bucket's running sum and count per metric.
    sum: [u16; 2],
    n: [u8; 2],
    /// The zone shown per metric, held by hysteresis, or [`NO_ZONE`].
    zone: [u8; 2],
    smooth: Smoother,
    /// The open bucket's number, `now_ms / 5000`. Both metrics share it.
    bucket: u32,
}

impl Default for Effort {
    fn default() -> Self {
        Self::new()
    }
}

impl Effort {
    pub const fn new() -> Self {
        Effort {
            hist: [[0; HISTORY_BARS]; 2],
            sum: [0; 2],
            n: [0; 2],
            zone: [NO_ZONE; 2],
            smooth: Smoother { watts: [0; SMOOTH_S as usize], valid: 0, newest: 0 },
            bucket: 0,
        }
    }

    /// Feed one fresh sensor sample.
    pub(crate) fn sample(&mut self, m: Metric, value: u16, now_ms: u32) {
        self.roll(now_ms);
        let value = match m {
            Metric::Hr => value,
            Metric::Power => {
                self.smooth.push(value, now_ms);
                self.smooth.mean()
            }
        };
        self.sum[m as usize] = self.sum[m as usize].saturating_add(value);
        self.n[m as usize] = self.n[m as usize].saturating_add(1);
    }

    /// Once per pass: scroll the history to `now_ms`, so the graph moves through a dropout, and
    /// step each zone to the latest value against the rider's limits.
    /// `hr` and `power` are the latest values, `None` before a metric's first sample.
    pub(crate) fn advance(&mut self, now_ms: u32, limits: EffortLimits, hr: Option<u16>, power: Option<u16>) {
        self.roll(now_ms);
        for (m, value) in [(Metric::Hr, hr), (Metric::Power, power)] {
            let shown = Some(self.zone[m as usize]).filter(|&z| z != NO_ZONE);
            self.zone[m as usize] = match (value, m.limit(limits)) {
                (Some(v), Some(l)) => m.step(shown, v as u32, l),
                _ => NO_ZONE,
            };
        }
    }

    /// Close the open bucket once `now_ms` has left it, and blank the buckets no sample reached.
    fn roll(&mut self, now_ms: u32) {
        let b = now_ms / BUCKET_MS;
        let passed = b.wrapping_sub(self.bucket);
        if passed == 0 {
            return;
        }
        let open = (self.bucket % HISTORY_BARS as u32) as usize;
        for m in [Metric::Hr, Metric::Power] {
            let i = m as usize;
            let closed = if self.n[i] > 0 { m.to_bar(self.sum[i] / self.n[i] as u16) } else { 0 };
            for k in 0..passed.min(HISTORY_BARS as u32) as usize {
                self.hist[i][(open + k) % HISTORY_BARS] = 0;
            }
            // A bucket more than five minutes old is off the graph.
            if passed <= HISTORY_BARS as u32 {
                self.hist[i][open] = closed;
            }
            (self.sum[i], self.n[i]) = (0, 0);
        }
        self.bucket = b;
    }

    /// The 10 s power average as of the latest sample.
    pub fn power(&self) -> u16 {
        self.smooth.mean()
    }

    /// The zone shown for `m`, or `None` without a limit.
    pub fn zone(&self, m: Metric) -> Option<u8> {
        Some(self.zone[m as usize]).filter(|&z| z != NO_ZONE)
    }

    /// The open bucket's number. The graph changes only when it moves.
    pub fn bucket(&self) -> u32 {
        self.bucket
    }

    /// The closed bucket averages, oldest first. `0` is a bucket with no data.
    pub fn history(&self, m: Metric) -> impl ExactSizeIterator<Item = u16> + Clone + '_ {
        let oldest = (self.bucket % HISTORY_BARS as u32) as usize;
        (0..HISTORY_BARS).map(move |i| m.bar_value(self.hist[m as usize][(oldest + i) % HISTORY_BARS]))
    }

    /// Whether any bar of `m`'s graph holds data.
    pub fn has_history(&self, m: Metric) -> bool {
        self.hist[m as usize].iter().any(|&b| b != 0)
    }
}

/// Clamp a stored max HR into the editor's range, keeping `0` as not set.
pub(crate) fn sanitize_max_hr(v: &mut u8) {
    if *v != 0 {
        *v = (*v).clamp(MAX_HR_MIN, MAX_HR_MAX);
    }
}

/// Clamp a stored FTP into the editor's range, keeping `0` as not set.
pub(crate) fn sanitize_ftp(v: &mut u16) {
    if *v != 0 {
        *v = (*v).clamp(FTP_MIN, FTP_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: EffortLimits = EffortLimits { max_hr: 200, ftp_w: 200 };

    #[test]
    fn zone_edges_follow_the_table() {
        // Max HR 200: Z2 from 120, Z3 from 140, Z4 from 160, Z5 from 180.
        let hr = [119, 120, 139, 140, 159, 160, 179, 180].map(|v| Metric::Hr.zone_of(v, 200));
        assert_eq!(hr, [0, 1, 1, 2, 2, 3, 3, 4]);
        // FTP 200: Z2 from 55 %, Z3 above 75 %, Z4 above 90 %, Z5 above 105 %.
        let pw = [109, 110, 150, 151, 180, 181, 210, 211].map(|v| Metric::Power.zone_of(v, 200));
        assert_eq!(pw, [0, 1, 1, 2, 2, 3, 3, 4]);
        assert_eq!(Metric::Power.zone_of(264, 250), 4, "105.6 % is Z5, with no percent truncated away");
    }

    /// Pass one HR sample, then the pass boundary that zones it.
    fn hr(e: &mut Effort, bpm: u16, now_ms: u32) -> Option<u8> {
        e.sample(Metric::Hr, bpm, now_ms);
        e.advance(now_ms, LIMITS, Some(bpm), None);
        e.zone(Metric::Hr)
    }

    #[test]
    fn a_zone_changes_only_past_the_margin() {
        let mut e = Effort::new();
        assert_eq!(hr(&mut e, 150, 0), Some(2), "the first value takes its zone outright");
        assert_eq!(hr(&mut e, 161, 1_000), Some(2), "1 bpm past the Z4 edge is inside the 2 bpm margin");
        assert_eq!(hr(&mut e, 162, 2_000), Some(3), "2 bpm past it, the zone moves up");
        assert_eq!(hr(&mut e, 159, 3_000), Some(3), "1 bpm under the edge holds Z4");
        assert_eq!(hr(&mut e, 157, 4_000), Some(2), "past the margin, it drops");
        assert_eq!(hr(&mut e, 200, 5_000), Some(4), "a jump crosses several zones at once");
        e.advance(5_000, EffortLimits { max_hr: 0, ftp_w: 200 }, Some(200), None);
        assert_eq!(e.zone(Metric::Hr), None, "no max HR, no zone");
    }

    #[test]
    fn the_first_power_reading_takes_its_zone_outright() {
        let mut e = Effort::new();
        let ftp = EffortLimits { max_hr: 0, ftp_w: 250 };
        e.advance(0, ftp, None, None);
        assert_eq!(e.zone(Metric::Power), None, "no sample yet, no zone");
        e.sample(Metric::Power, 190, 1_000);
        let power = e.power();
        e.advance(1_000, ftp, None, Some(power));
        assert_eq!(e.zone(Metric::Power), Some(2), "76 % of FTP is Z3 at once, with no climb through Z1 and Z2");
    }

    #[test]
    fn power_is_the_mean_of_the_last_ten_seconds() {
        let mut e = Effort::new();
        for s in 0..10 {
            e.sample(Metric::Power, if s % 2 == 0 { 100 } else { 300 }, s * 1000);
        }
        assert_eq!(e.power(), 200, "ten alternating seconds average out");
        e.sample(Metric::Power, 400, 10_000);
        assert_eq!(e.power(), (100 * 4 + 300 * 5 + 400) / 10, "the oldest second leaves the window");
        e.sample(Metric::Power, 500, 10_500);
        assert_eq!(e.power(), (100 * 4 + 300 * 5 + 500) / 10, "a second holds its latest sample");
        e.sample(Metric::Power, 1000, 30_000);
        assert_eq!(e.power(), 1000, "after a dropout, old seconds do not come back");
    }

    #[test]
    fn history_is_bucket_averages_with_gaps_for_dropouts() {
        let mut e = Effort::new();
        e.sample(Metric::Hr, 100, 0);
        e.sample(Metric::Hr, 110, 2_000);
        e.sample(Metric::Hr, 130, 6_000);
        e.sample(Metric::Power, 1_000, 6_000);
        e.advance(20_000, LIMITS, None, None);
        let h: std::vec::Vec<u16> = e.history(Metric::Hr).collect();
        assert_eq!(h.len(), HISTORY_BARS);
        assert_eq!(&h[HISTORY_BARS - 4..], [105, 130, 0, 0], "two filled buckets, then two empty ones");
        assert_eq!(e.history(Metric::Power).nth(HISTORY_BARS - 3), Some(1000), "power keeps 4 W steps");
        e.advance(20_000 + 5 * 60_000, LIMITS, None, None);
        assert!(!e.has_history(Metric::Hr), "five quiet minutes empty the graph");
    }

    #[test]
    fn gauge_prefers_live_power_and_fills_through_equal_slots() {
        let w = 240; // five 48 px slots
        let reading = |value, zone| Some(Reading { value, zone: Some(zone) });
        let power = reading(200, 3); // 100 % FTP: 10/15 through Z4
        let hr = reading(150, 2); // 75 % max HR: halfway through Z3
        let fill = (48.0 * (3.0 + 10.0 / 15.0)) as u16;
        assert_eq!(gauge(power, hr, LIMITS, w), Some(Gauge { zone: 3, fill_px: fill }));
        assert_eq!(gauge(None, hr, LIMITS, w), Some(Gauge { zone: 2, fill_px: 2 * 48 + 24 }), "stale power: HR");
        assert_eq!(gauge(None, None, LIMITS, w), None, "no live sensor, no gauge");
        let no_ftp = EffortLimits { max_hr: 0, ftp_w: 0 };
        assert_eq!(gauge(power, None, no_ftp, w), None, "live power without FTP draws no gauge");
        assert_eq!(gauge(None, reading(90, 0), LIMITS, w).unwrap().fill_px, 0, "below the span is empty");
        assert_eq!(gauge(None, reading(220, 4), LIMITS, w).unwrap().fill_px, 240, "above it is full");
    }
}
