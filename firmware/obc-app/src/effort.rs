//! Effort: heart rate and power against the rider's own limits.
//!
//! One [`Effort`] turns the raw BLE samples into what the riding views draw: a zone per metric with
//! hysteresis, a 10 s power average, and a five-minute history of 5 s buckets. Every consumer reads
//! the same smoothed power, so the PWR tile, its tint, the map gauge and the graph agree.
//!
//! Zones are indices `0..=4` for Z1..Z5. Without a limit (max HR or FTP of 0) nothing has a zone.

/// Bars in a history graph: five minutes of 5 s buckets.
pub const HISTORY_BARS: usize = 60;
const BUCKET_MS: u32 = 5_000;
/// The power average window, in one-second slots.
const SMOOTH_S: usize = 10;

/// Editor bounds for the two limits. `0` stores "not set".
pub const MAX_HR_MIN: u8 = 120;
pub const MAX_HR_MAX: u8 = 220;
pub const FTP_MIN: u16 = 50;
pub const FTP_MAX: u16 = 600;
pub const FTP_STEP: u16 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Hr,
    Power,
}

impl Metric {
    /// The Z2..Z5 floors, in whole percent of the limit.
    const fn floors(self) -> [u32; 4] {
        match self {
            Metric::Hr => [60, 70, 80, 90],
            Metric::Power => [55, 76, 91, 106],
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

    /// The Z4 floor in percent, which the graph marks with a dotted line.
    pub const fn z4_floor(self) -> u32 {
        self.floors()[2]
    }

    /// How far past a zone edge a value must go before the zone changes.
    const fn margin(self, limit: u32) -> u32 {
        match self {
            Metric::Hr => 2,
            Metric::Power => limit * 2 / 100,
        }
    }

    /// The zone of `value` against `limit`, with no hysteresis.
    pub fn zone_of(self, value: u32, limit: u32) -> u8 {
        let pct = value * 100 / limit.max(1);
        self.floors().iter().filter(|&&f| pct >= f).count() as u8
    }

    /// The zone after `value` arrives, given the zone shown before it.
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
        let f = self.floors();
        let edges = [lo, f[0], f[1], f[2], f[3], hi];
        let pct = value as f32 * 100.0 / limit.max(1) as f32;
        for z in 0..5 {
            let (a, b) = (edges[z] as f32, edges[z + 1] as f32);
            if pct < b {
                return z as f32 + ((pct - a) / (b - a)).max(0.0);
            }
        }
        5.0
    }
}

/// The rider's two limits, as the settings store them: `0` is not set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    pub max_hr: u8,
    pub ftp_w: u16,
}

impl Limits {
    pub fn of(self, m: Metric) -> Option<u32> {
        let v = match m {
            Metric::Hr => self.max_hr as u32,
            Metric::Power => self.ftp_w as u32,
        };
        (v > 0).then_some(v)
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

/// The fill length of a `w` px gauge at `position` (`0.0..=5.0`).
fn fill_px(position: f32, w: i32) -> u16 {
    let slot = (w / 5).max(0);
    ((position * slot as f32) as i32).clamp(0, 5 * slot) as u16
}

/// One metric's derived state.
#[derive(Debug, Clone, Copy)]
struct Channel {
    /// The latest value: raw heart rate, or the smoothed power.
    last: u16,
    /// The zone shown for `last`, held by hysteresis.
    zone: Option<u8>,
    /// Closed bucket averages, indexed by bucket number modulo [`HISTORY_BARS`]. `0` is no data.
    hist: [u16; HISTORY_BARS],
    /// The open bucket's running sum and count.
    sum: u32,
    n: u16,
}

impl Channel {
    const fn new() -> Self {
        Channel { last: 0, zone: None, hist: [0; HISTORY_BARS], sum: 0, n: 0 }
    }
}

/// The 10 s power average: one slot per second, each holding its second and that second's samples.
#[derive(Debug, Clone, Copy)]
struct Smoother {
    sec: [u32; SMOOTH_S],
    sum: [u32; SMOOTH_S],
    n: [u8; SMOOTH_S],
}

impl Smoother {
    /// Add a sample and return the mean over the samples of the last 10 s.
    fn push(&mut self, watts: u16, now_ms: u32) -> u16 {
        let s = now_ms / 1000;
        let i = s as usize % SMOOTH_S;
        if self.sec[i] != s {
            (self.sec[i], self.sum[i], self.n[i]) = (s, 0, 0);
        }
        self.sum[i] += watts as u32;
        self.n[i] = self.n[i].saturating_add(1);
        let (mut sum, mut n) = (0u32, 0u32);
        for k in 0..SMOOTH_S {
            if self.n[k] > 0 && s.wrapping_sub(self.sec[k]) < SMOOTH_S as u32 {
                sum += self.sum[k];
                n += self.n[k] as u32;
            }
        }
        (sum / n) as u16
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Effort {
    limits: Limits,
    hr: Channel,
    power: Channel,
    smooth: Smoother,
    /// The open bucket's number, `now_ms / 5000`. Both channels share it.
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
            limits: Limits { max_hr: 0, ftp_w: 0 },
            hr: Channel::new(),
            power: Channel::new(),
            smooth: Smoother { sec: [0; SMOOTH_S], sum: [0; SMOOTH_S], n: [0; SMOOTH_S] },
            bucket: 0,
        }
    }

    fn channel(&self, m: Metric) -> &Channel {
        match m {
            Metric::Hr => &self.hr,
            Metric::Power => &self.power,
        }
    }

    fn channel_mut(&mut self, m: Metric) -> &mut Channel {
        match m {
            Metric::Hr => &mut self.hr,
            Metric::Power => &mut self.power,
        }
    }

    /// Feed one fresh sensor sample.
    pub(crate) fn sample(&mut self, m: Metric, value: u16, now_ms: u32) {
        self.roll(now_ms);
        let value = match m {
            Metric::Hr => value,
            Metric::Power => self.smooth.push(value, now_ms),
        };
        let limit = self.limits.of(m);
        let ch = self.channel_mut(m);
        ch.last = value;
        ch.zone = limit.map(|l| m.step(ch.zone, value as u32, l));
        ch.sum += value as u32;
        ch.n = ch.n.saturating_add(1);
    }

    /// Advance the history to `now_ms` and adopt the rider's limits. Called once per pass, so the
    /// graph scrolls through a dropout and a changed limit re-zones the last value.
    pub(crate) fn advance(&mut self, now_ms: u32, limits: Limits) {
        self.roll(now_ms);
        if limits != self.limits {
            self.limits = limits;
            for m in [Metric::Hr, Metric::Power] {
                let ch = self.channel_mut(m);
                ch.zone = limits.of(m).map(|l| m.zone_of(ch.last as u32, l));
            }
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
        for ch in [&mut self.hr, &mut self.power] {
            let closed = if ch.n > 0 { (ch.sum / ch.n as u32) as u16 } else { 0 };
            for k in 0..passed.min(HISTORY_BARS as u32) as usize {
                ch.hist[(open + k) % HISTORY_BARS] = 0;
            }
            // A bucket more than five minutes old is off the graph.
            if passed <= HISTORY_BARS as u32 {
                ch.hist[open] = closed;
            }
            (ch.sum, ch.n) = (0, 0);
        }
        self.bucket = b;
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The open bucket's number. The graph changes only when it moves.
    pub fn bucket(&self) -> u32 {
        self.bucket
    }

    /// The latest value and its zone, or `None` when the sensor is not `live`.
    pub fn reading(&self, m: Metric, live: bool) -> Option<Reading> {
        let ch = self.channel(m);
        live.then_some(Reading { value: ch.last, zone: ch.zone })
    }

    /// The closed bucket averages, oldest first. `0` is a bucket with no data.
    pub fn history(&self, m: Metric) -> impl Iterator<Item = u16> + '_ {
        let ch = self.channel(m);
        let oldest = (self.bucket % HISTORY_BARS as u32) as usize;
        (0..HISTORY_BARS).map(move |i| ch.hist[(oldest + i) % HISTORY_BARS])
    }

    /// The map gauge for a `w` px wide panel: power when the meter is live and FTP is set,
    /// otherwise heart rate when the strap is live and max HR is set, otherwise none.
    pub fn gauge(&self, power_live: bool, hr_live: bool, w: i32) -> Option<Gauge> {
        let m = [(Metric::Power, power_live), (Metric::Hr, hr_live)]
            .into_iter()
            .find(|&(m, live)| live && self.limits.of(m).is_some())?
            .0;
        let ch = self.channel(m);
        let limit = self.limits.of(m)?;
        Some(Gauge { zone: ch.zone?, fill_px: fill_px(m.position(ch.last as u32, limit), w) })
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

    const LIMITS: Limits = Limits { max_hr: 200, ftp_w: 200 };

    fn effort() -> Effort {
        let mut e = Effort::new();
        e.advance(0, LIMITS);
        e
    }

    #[test]
    fn zone_edges_follow_the_tables() {
        // Max HR 200: Z2 from 120, Z3 from 140, Z4 from 160, Z5 from 180.
        let hr = [119, 120, 139, 140, 159, 160, 179, 180].map(|v| Metric::Hr.zone_of(v, 200));
        assert_eq!(hr, [0, 1, 1, 2, 2, 3, 3, 4]);
        // FTP 200: Z2 from 55 %, Z3 from 76 %, Z4 from 91 %, Z5 from 106 %.
        let pw = [109, 110, 151, 152, 181, 182, 211, 212].map(|v| Metric::Power.zone_of(v, 200));
        assert_eq!(pw, [0, 1, 1, 2, 2, 3, 3, 4]);
    }

    #[test]
    fn a_zone_changes_only_past_the_margin() {
        let mut e = effort();
        let zone = |e: &Effort| e.reading(Metric::Hr, true).unwrap().zone;
        e.sample(Metric::Hr, 150, 0);
        assert_eq!(zone(&e), Some(2), "the first value takes its zone outright");
        e.sample(Metric::Hr, 161, 1_000);
        assert_eq!(zone(&e), Some(2), "1 bpm past the Z4 edge is inside the 2 bpm margin");
        e.sample(Metric::Hr, 162, 2_000);
        assert_eq!(zone(&e), Some(3), "2 bpm past it, the zone moves up");
        e.sample(Metric::Hr, 159, 3_000);
        assert_eq!(zone(&e), Some(3), "1 bpm under the edge holds Z4");
        e.sample(Metric::Hr, 157, 4_000);
        assert_eq!(zone(&e), Some(2), "past the margin, it drops");
        e.sample(Metric::Hr, 200, 5_000);
        assert_eq!(zone(&e), Some(4), "a jump crosses several zones at once");
    }

    #[test]
    fn a_new_limit_rezones_without_waiting_for_a_sample() {
        let mut e = Effort::new();
        e.sample(Metric::Hr, 150, 0);
        assert_eq!(e.reading(Metric::Hr, true), Some(Reading { value: 150, zone: None }), "no max HR, no zone");
        e.advance(0, LIMITS);
        assert_eq!(e.reading(Metric::Hr, true).unwrap().zone, Some(2));
        assert_eq!(e.reading(Metric::Hr, false), None, "a stale strap reads nothing");
    }

    #[test]
    fn power_is_the_mean_of_the_last_ten_seconds() {
        let mut e = effort();
        let watts = |e: &Effort| e.reading(Metric::Power, true).unwrap().value;
        for s in 0..10 {
            e.sample(Metric::Power, if s % 2 == 0 { 100 } else { 300 }, s * 1000);
        }
        assert_eq!(watts(&e), 200, "ten alternating seconds average out");
        e.sample(Metric::Power, 400, 10_000);
        assert_eq!(watts(&e), (100 * 4 + 300 * 5 + 400) / 10, "the oldest second leaves the window");
        e.sample(Metric::Power, 1000, 30_000);
        assert_eq!(watts(&e), 1000, "after a dropout, old seconds do not come back");
    }

    #[test]
    fn history_is_bucket_averages_with_gaps_for_dropouts() {
        let mut e = effort();
        e.sample(Metric::Hr, 100, 0);
        e.sample(Metric::Hr, 110, 2_000);
        e.sample(Metric::Hr, 130, 6_000);
        e.advance(20_000, LIMITS);
        let h: std::vec::Vec<u16> = e.history(Metric::Hr).collect();
        assert_eq!(h.len(), HISTORY_BARS);
        assert_eq!(&h[HISTORY_BARS - 4..], [105, 130, 0, 0], "two filled buckets, then two empty ones");
        e.advance(20_000 + 5 * 60_000, LIMITS);
        assert!(e.history(Metric::Hr).all(|v| v == 0), "five quiet minutes empty the graph");
    }

    #[test]
    fn gauge_prefers_live_power_and_fills_through_equal_slots() {
        let mut e = effort();
        e.sample(Metric::Power, 200, 0); // 100 % FTP: 9/15 through Z4
        e.sample(Metric::Hr, 150, 0); // 75 % max HR: halfway through Z3
        let w = 240; // five 48 px slots
        assert_eq!(e.gauge(true, true, w), Some(Gauge { zone: 3, fill_px: (3.6 * 48.0) as u16 }));
        assert_eq!(e.gauge(false, true, w), Some(Gauge { zone: 2, fill_px: 2 * 48 + 24 }), "stale power falls to HR");
        assert_eq!(e.gauge(false, false, w), None, "no live sensor, no gauge");
        e.advance(0, Limits { max_hr: 200, ftp_w: 0 });
        assert_eq!(e.gauge(true, false, w), None, "live power without FTP draws no gauge");

        e.advance(0, LIMITS);
        e.sample(Metric::Hr, 90, 1_000);
        assert_eq!(e.gauge(false, true, w).unwrap().fill_px, 0, "below the gauge span is empty");
        e.sample(Metric::Hr, 220, 2_000);
        assert_eq!(e.gauge(false, true, w).unwrap().fill_px, 240, "above it is full");
    }
}
