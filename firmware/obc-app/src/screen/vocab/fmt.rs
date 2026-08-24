//! Every **quantity readout** a screen prints — one formatter per quantity and output style.
//!
//! Before this module the same distance appeared through six code paths and two functions named
//! `fmt_int` formatted two different things. A quantity now has exactly one formatter here, and
//! every screen imports it, so two screens can never round the same number differently.
//!
//! Each function is named `<quantity>_<style>`: the quantity it prints, then the shape it prints
//! it in. Two styles of one quantity are two functions ([`distance_short`] compacts to `12.4km`,
//! [`write_distance_coarse`] to a whole `12km`) because their thresholds genuinely differ; they
//! are not folded together for looking alike. A `write_*` name appends into a caller-owned buffer
//! — the form a screen needs when the figure joins a longer line.
//!
//! Everything here is allocation-free: a bounded [`heapless::String`] out, or an append into one.
//!
//! **Semantic owner.** This module holds the *shapes* the device prints today, byte for byte.
//! The quantity *policy* behind them — the [`Units`] conversion table, the unit labels, and which
//! named distance style a given readout should use — belongs to #1399 slice T7, which changes the
//! rules in place here rather than moving them again.

use core::fmt::Write;

use crate::settings::{DateTime, Language, Units, FT_PER_M, FT_PER_MI};
use crate::{t, Msg};

/// The "no data" glyph every optional readout falls back to: an absent sensor sample, a
/// route-relative figure on a route-less ride, a climb delta with no honest number behind it.
pub(crate) fn dashes() -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = s.push_str("--");
    s
}

// ---------------------------------------------------------------------------------------------
// Distance
// ---------------------------------------------------------------------------------------------

/// A large-unit distance **figure** with no unit in it — the tile and header form, where the unit
/// rides in the caption. One decimal below 100, whole above, so the value stays ≤ 3 digits and
/// fits a half-width tile. Takes the already-converted figure (`units.dist(km)`).
pub(crate) fn distance_figure(value: f32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = if value >= 100.0 { write!(s, "{value:.0}") } else { write!(s, "{value:.1}") };
    s
}

/// A compact **whole-distance** readout with its unit tight against the number — the Map waypoint
/// chip, the Up ahead rows, the detour figures. Metric: `NNNm` below 1 km, `N.Nkm` to one decimal
/// below 100 km, whole `NNNkm` above. Imperial: `NNNft` below 1000 ft, `N.Nmi` below 100 mi, whole
/// `NNNmi` above. Rounds to the readout's own grain (nearest tenth / whole).
pub(crate) fn distance_short(d_m: u32, units: Units) -> heapless::String<8> {
    let mut s = heapless::String::new();
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft < 1000 {
            let _ = write!(s, "{ft}ft");
        } else if ft < 100 * FT_PER_MI {
            // One decimal mile, rounded to the nearest tenth.
            let tenths = (ft * 10 + FT_PER_MI / 2) / FT_PER_MI;
            let _ = write!(s, "{}.{}mi", tenths / 10, tenths % 10);
        } else {
            let _ = write!(s, "{}mi", (ft + FT_PER_MI / 2) / FT_PER_MI);
        }
    } else if d_m < 1000 {
        let _ = write!(s, "{d_m}m");
    } else if d_m < 100_000 {
        // One decimal km, rounded to the nearest tenth (100 m).
        let tenths = (d_m + 50) / 100;
        let _ = write!(s, "{}.{}km", tenths / 10, tenths % 10);
    } else {
        let _ = write!(s, "{}km", (d_m + 500) / 1000);
    }
    s
}

/// Append a distance after `prefix`, compacted to a **whole large unit** past the crossover so the
/// readout stays within a chip or a header line. Metric: `NNNm` below 1 km, `NNkm` above
/// (rounded). Imperial: `NNNft` below a mile, `NNmi` above. Shared by the Statistics header, the
/// Map's off-route pill, the POI distance columns and the Up ahead side hint.
pub(crate) fn write_distance_coarse<const N: usize>(s: &mut heapless::String<N>, prefix: &str, d_m: u32, units: Units) {
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft >= FT_PER_MI {
            let _ = write!(s, "{prefix}{}mi", (ft + FT_PER_MI / 2) / FT_PER_MI);
        } else {
            let _ = write!(s, "{prefix}{ft}ft");
        }
    } else if d_m >= 1000 {
        let _ = write!(s, "{prefix}{}km", (d_m + 500) / 1000);
    } else {
        let _ = write!(s, "{prefix}{d_m}m");
    }
}

/// Append a straight-line distance as a spaced unit-value plus the catalog's trailing word:
/// `600 m away` below 1 km, else `2.3 km away`; imperial, whole feet below a mile, else
/// one-decimal miles. `away` is the translated suffix, so the phrase reads as value + word in
/// every language.
pub(crate) fn write_distance_away<const N: usize>(s: &mut heapless::String<N>, d_m: u32, units: Units, away: &str) {
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft >= FT_PER_MI {
            let _ = write!(s, "{:.1} mi {away}", ft as f32 / FT_PER_MI as f32);
        } else {
            let _ = write!(s, "{ft} ft {away}");
        }
    } else if d_m >= 1000 {
        let _ = write!(s, "{:.1} km {away}", d_m as f32 / 1000.0);
    } else {
        let _ = write!(s, "{d_m} m {away}");
    }
}

/// Append a distance as a **spaced large unit**: `NN.N km` / `NN.N mi`, compacting to a whole unit
/// (`142 km`) from 100 up — the tenths stop meaning anything at that magnitude, and the whole
/// figure keeps the worst legitimate metadata run inside an inset row's budget. The rides-list and
/// ride-detail metadata shape.
pub(crate) fn write_distance_spaced<const N: usize>(s: &mut heapless::String<N>, dist_m: u32, units: Units) {
    if units.is_imperial() {
        let mi10 = (dist_m as f32 * FT_PER_M / FT_PER_MI as f32 * 10.0) as u32;
        if mi10 >= 1000 {
            let _ = write!(s, "{} mi", (mi10 + 5) / 10);
        } else {
            let _ = write!(s, "{}.{} mi", mi10 / 10, mi10 % 10);
        }
    } else {
        let km10 = (dist_m + 50) / 100; // tenths of a km
        if km10 >= 1000 {
            let _ = write!(s, "{} km", (dist_m + 500) / 1000);
        } else {
            let _ = write!(s, "{}.{} km", km10 / 10, km10 % 10);
        }
    }
}

/// Append a distance **split from its unit**: the value goes into `s`, the unit suffix comes back
/// for the caller to draw in its own font. Whole metres below 1 km, one-decimal km above —
/// imperial twin: whole feet below a mile, one-decimal miles. The ledger form, where value and
/// unit are drawn as two runs.
pub(crate) fn write_distance_split(s: &mut heapless::String<8>, total_m: u32, units: Units) -> &'static str {
    if units.is_imperial() {
        let ft = (total_m as f32 * FT_PER_M) as u32;
        if ft < FT_PER_MI {
            let _ = write!(s, "{ft}");
            "ft"
        } else {
            let _ = write!(s, "{:.1}", units.dist(total_m as f32 / 1000.0));
            "mi"
        }
    } else if total_m < 1000 {
        let _ = write!(s, "{total_m}");
        "m"
    } else {
        let _ = write!(s, "{:.1}", total_m as f32 / 1000.0);
        "km"
    }
}

// ---------------------------------------------------------------------------------------------
// Speed, plain integers, percent
// ---------------------------------------------------------------------------------------------

/// A speed figure to one decimal, or [`dashes`] when unknown (no fix / no moving time yet). The
/// unit rides in the caption.
pub(crate) fn speed_figure(v: Option<f32>) -> heapless::String<8> {
    let mut s = heapless::String::new();
    match v {
        Some(v) => {
            let _ = write!(s, "{v:.1}");
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

/// A whole figure as plain digits — a climb in the rider's elevation unit, a bpm / watt / rpm
/// reading. The unit rides in the caption.
pub(crate) fn integer(v: u32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{v}");
    s
}

/// [`integer`], or [`dashes`] when the reading is absent or stale.
pub(crate) fn integer_opt(v: Option<u32>) -> heapless::String<8> {
    match v {
        Some(v) => integer(v),
        None => dashes(),
    }
}

/// A grade figure: signed whole percent with a `%` suffix.
pub(crate) fn percent(pct: i32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{pct}%");
    s
}

// ---------------------------------------------------------------------------------------------
// Elevation
// ---------------------------------------------------------------------------------------------

/// A live-elevation figure rounded to a whole unit — signed, so a sub-sea-level reading shows a
/// `-` rather than wrapping — or [`dashes`] when there is no altimeter sample yet. Rounds half
/// away from zero without `libm` (the codebase keeps elevation maths off the math lib).
pub(crate) fn elevation_rounded(v: Option<f32>) -> heapless::String<8> {
    let mut s = heapless::String::new();
    match v {
        Some(v) => {
            let rounded = (v + if v >= 0.0 { 0.5 } else { -0.5 }) as i32;
            let _ = write!(s, "{rounded}");
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

/// A remaining-ascent readout with its unit tight against the number (`250m`), or [`dashes`] when
/// there is no figure — the form for a row that shares its line with other text.
pub(crate) fn elevation_short(value_m: Option<u32>, units: Units) -> heapless::String<12> {
    let mut s = heapless::String::new();
    match value_m {
        Some(m) => {
            let shown = (units.elev(m as f32) + 0.5) as u32;
            let _ = write!(s, "{shown}{}", units.elev_label());
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

/// A **signed** climb difference (`+120m` / `-40m`) in the rider's elevation unit, or [`dashes`]
/// when there is no honest number. `+0m` is an answer, not a missing one.
pub(crate) fn elevation_delta(delta_m: Option<i32>, units: Units) -> heapless::String<12> {
    let mut s: heapless::String<12> = heapless::String::new();
    let Some(delta_m) = delta_m else {
        let _ = s.push_str("--");
        return s;
    };
    let magnitude = (units.elev(delta_m.unsigned_abs() as f32) + 0.5) as u32;
    let sign = if delta_m < 0 { '-' } else { '+' };
    let _ = write!(s, "{sign}{magnitude}{}", units.elev_label());
    s
}

// ---------------------------------------------------------------------------------------------
// Durations and dates
// ---------------------------------------------------------------------------------------------

/// A duration in seconds as `H:MM` — hours uncapped, minutes zero-padded. Hours and minutes are
/// hours and minutes in every catalog language and both unit systems, so this is not localised.
pub(crate) fn duration_hms(secs: f32) -> heapless::String<8> {
    let total_min = (secs as u32) / 60;
    let mut s = heapless::String::new();
    let _ = write!(s, "{}:{:02}", total_min / 60, total_min % 60);
    s
}

/// The time left until `deadline` from `now_utc`, in the locked expiry format (epic #638 S5):
/// `≥ 2 days → "in N d"`, `≥ 1 hour (and < 48 h) → "in N h"`, anything sooner — the sub-hour tail
/// or an already-past deadline the hourly sweep hasn't collected yet — `"soon"`. Whole units
/// (floor); the sub-hour fold avoids an "in 0 h" readout in the final hour. Not localised — the
/// format is pinned by the issue.
pub(crate) fn expiry_short(deadline: u32, now_utc: u32) -> heapless::String<12> {
    use crate::retention::DAY_SECS;
    let mut s = heapless::String::new();
    let secs = deadline.saturating_sub(now_utc);
    if secs >= 2 * DAY_SECS {
        let _ = write!(s, "in {} d", secs / DAY_SECS);
    } else if secs >= 3600 {
        let _ = write!(s, "in {} h", secs / 3600);
    } else {
        let _ = s.push_str("soon");
    }
    s
}

/// The 12 uppercase month-abbreviation catalog keys (the `[date]` section) in calendar order — the
/// short-date table the Home date line and the rides rows share. Distinct from the Date & Time
/// stepper's mixed-case `[month]` table.
pub(crate) const DATE_MONTHS: [Msg; 12] = [
    Msg::DateJan,
    Msg::DateFeb,
    Msg::DateMar,
    Msg::DateApr,
    Msg::DateMay,
    Msg::DateJun,
    Msg::DateJul,
    Msg::DateAug,
    Msg::DateSep,
    Msg::DateOct,
    Msg::DateNov,
    Msg::DateDec,
];

/// Append a unix instant as the short day-first date `D MON` (UTC) — no leading zero, the month
/// from [`DATE_MONTHS`]. Day-first in all four languages (the locked shared shape).
pub(crate) fn write_date_short<const N: usize>(s: &mut heapless::String<N>, unix: u32, lang: Language) {
    let d = DateTime::from_unix(unix);
    let _ = write!(s, "{} {}", d.day, t(DATE_MONTHS[(d.month.clamp(1, 12) - 1) as usize], lang));
}

/// A unix instant as a compact `YYYY-MM-DD` (UTC) — the rides list's and the Ride detail's shared
/// date shape. (Local-time formatting would need the app's UTC offset threaded in; the date rarely
/// differs and the extra plumbing isn't worth it.)
pub(crate) fn date_iso(unix: u32) -> heapless::String<12> {
    let d = DateTime::from_unix(unix);
    let mut s = heapless::String::new();
    let _ = write!(s, "{:04}-{:02}-{:02}", d.year, d.month, d.day);
    s
}

/// A UTC offset as `±HH:MM` — the sign is always printed, zero reading `+00:00`.
pub(crate) fn utc_offset(min: i16) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let sign = if min < 0 { '-' } else { '+' };
    let a = min.unsigned_abs();
    let _ = write!(s, "{sign}{:02}:{:02}", a / 60, a % 60);
    s
}

// ---------------------------------------------------------------------------------------------
// Weather, storage, addresses
// ---------------------------------------------------------------------------------------------

/// The temperature as a compact `14°` readout, or `None` on the wire sentinel — shared by the
/// weather dashboard card and the hourly rows so the two can never round differently.
pub(crate) fn temperature_short(deci_c: i16) -> Option<heapless::String<8>> {
    if deci_c == obc_formats::obcw::TEMP_UNAVAILABLE {
        return None;
    }
    let deg = ((deci_c as i32) + if deci_c >= 0 { 5 } else { -5 }) / 10;
    let mut s: heapless::String<8> = heapless::String::new();
    let _ = write!(s, "{}°", deg.clamp(-99, 99));
    Some(s)
}

/// Append a byte count as a compact `N.N GB` / `NNN MB` / `NNN KB` — GB with one decimal at or
/// above 1 GiB, whole MB / KB below (rounded). Binary units throughout.
pub(crate) fn write_bytes_short(s: &mut heapless::String<16>, bytes: u64) {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        let tenths = (bytes * 10 + GIB / 2) / GIB; // round to 0.1 GB
        let _ = write!(s, "{}.{} GB", tenths / 10, tenths % 10);
    } else if bytes >= MIB {
        let _ = write!(s, "{} MB", (bytes + MIB / 2) / MIB);
    } else {
        let _ = write!(s, "{} KB", (bytes + KIB / 2) / KIB);
    }
}

/// Append a BLE address big-endian (`AA:BB:…`), the conventional display order — the stored bytes
/// are little-endian, as the wire carries them.
pub(crate) fn write_ble_address(buf: &mut heapless::String<24>, addr: &[u8; 6]) {
    for (i, b) in addr.iter().rev().enumerate() {
        if i > 0 {
            let _ = buf.push(':');
        }
        let _ = write!(buf, "{b:02X}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::DAY_SECS;

    /// Metres below 1 km, one-decimal km up to 100 km, whole km above — pinned across both
    /// crossovers.
    #[test]
    fn distance_short_metric_crossovers() {
        assert_eq!(distance_short(0, Units::Metric).as_str(), "0m");
        assert_eq!(distance_short(487, Units::Metric).as_str(), "487m");
        assert_eq!(distance_short(999, Units::Metric).as_str(), "999m", "just under 1 km stays metres");
        assert_eq!(distance_short(1000, Units::Metric).as_str(), "1.0km", "1 km crosses to one-decimal km");
        assert_eq!(distance_short(12_400, Units::Metric).as_str(), "12.4km");
        assert_eq!(distance_short(99_900, Units::Metric).as_str(), "99.9km", "just under 100 km keeps a decimal");
        assert_eq!(distance_short(100_000, Units::Metric).as_str(), "100km", "100 km crosses to whole km");
        assert_eq!(distance_short(153_000, Units::Metric).as_str(), "153km");
    }

    /// Feet below 1000 ft, one-decimal miles up to 100 mi, whole miles above — pinned across the
    /// ft→mi and 100 mi crossovers.
    #[test]
    fn distance_short_imperial_crossovers() {
        assert_eq!(distance_short(0, Units::Imperial).as_str(), "0ft");
        assert_eq!(distance_short(300, Units::Imperial).as_str(), "984ft", "300 m ≈ 984 ft stays feet");
        // 1000 ft ≈ 304.8 m — the feet→miles crossover; 305 m ≈ 1000 ft reads a fractional mile.
        assert_eq!(distance_short(305, Units::Imperial).as_str(), "0.2mi", "past 1000 ft crosses to decimal miles");
        assert_eq!(distance_short(15_933, Units::Imperial).as_str(), "9.9mi");
        // 100 mi = 528000 ft ≈ 160934 m — the decimal→whole-miles crossover.
        assert_eq!(distance_short(160_000, Units::Imperial).as_str(), "99.4mi", "just under 100 mi keeps a decimal");
        assert_eq!(distance_short(200_000, Units::Imperial).as_str(), "124mi", "well past 100 mi is whole miles");
    }

    /// The coarse chip readout compacts straight to a whole large unit — no decimal band at all,
    /// which is exactly what separates it from [`distance_short`].
    #[test]
    fn distance_coarse_compacts_to_a_whole_large_unit() {
        let coarse = |d_m, units| {
            let mut s: heapless::String<24> = heapless::String::new();
            write_distance_coarse(&mut s, "", d_m, units);
            s
        };
        assert_eq!(coarse(0, Units::Metric).as_str(), "0m");
        assert_eq!(coarse(999, Units::Metric).as_str(), "999m", "just under 1 km stays metres");
        assert_eq!(coarse(1000, Units::Metric).as_str(), "1km", "1 km crosses to whole km, not 1.0km");
        assert_eq!(coarse(1500, Units::Metric).as_str(), "2km", "rounds to the nearest whole km");
        assert_eq!(coarse(153_000, Units::Metric).as_str(), "153km");
        assert_eq!(coarse(300, Units::Imperial).as_str(), "984ft");
        assert_eq!(coarse(1610, Units::Imperial).as_str(), "1mi", "a full mile crosses to whole miles");
        assert_eq!(coarse(1609, Units::Imperial).as_str(), "5278ft", "a metre short of the mile is still feet");

        // The prefix is the caller's translated lead-in, appended in front of the figure.
        let mut pilled: heapless::String<24> = heapless::String::new();
        write_distance_coarse(&mut pilled, "OFF ", 1500, Units::Metric);
        assert_eq!(pilled.as_str(), "OFF 2km");
    }

    /// The spaced away-phrase: whole metres below 1 km, one-decimal km from there, with the
    /// catalog's trailing word after the unit.
    #[test]
    fn distance_away_switches_at_one_large_unit() {
        let away = |d_m, units| {
            let mut s: heapless::String<20> = heapless::String::new();
            write_distance_away(&mut s, d_m, units, "away");
            s
        };
        assert_eq!(away(0, Units::Metric).as_str(), "0 m away");
        assert_eq!(away(600, Units::Metric).as_str(), "600 m away");
        assert_eq!(away(999, Units::Metric).as_str(), "999 m away");
        assert_eq!(away(1000, Units::Metric).as_str(), "1.0 km away");
        assert_eq!(away(2300, Units::Metric).as_str(), "2.3 km away");
        assert_eq!(away(100, Units::Imperial).as_str(), "328 ft away");
        assert_eq!(away(2000, Units::Imperial).as_str(), "1.2 mi away");
    }

    /// The metadata shape is always a large unit — a spaced `0.5 km`, never `500 m` — and drops
    /// its decimal from 100 up.
    #[test]
    fn distance_spaced_stays_a_large_unit_and_compacts_at_a_hundred() {
        let spaced = |d_m, units| {
            let mut s: heapless::String<16> = heapless::String::new();
            write_distance_spaced(&mut s, d_m, units);
            s
        };
        assert_eq!(spaced(0, Units::Metric).as_str(), "0.0 km");
        assert_eq!(spaced(500, Units::Metric).as_str(), "0.5 km", "sub-kilometre still reads in km");
        assert_eq!(spaced(99_949, Units::Metric).as_str(), "99.9 km", "just under 100 km keeps a decimal");
        assert_eq!(spaced(99_950, Units::Metric).as_str(), "100 km", "100 km crosses to whole km");
        assert_eq!(spaced(142_000, Units::Metric).as_str(), "142 km");
        assert_eq!(spaced(500, Units::Imperial).as_str(), "0.3 mi");
        assert_eq!(spaced(200_000, Units::Imperial).as_str(), "124 mi");
    }

    /// The ledger form hands the unit back rather than printing it, and switches at the first
    /// whole large unit.
    #[test]
    fn distance_split_returns_its_unit() {
        let split = |total_m, units| {
            let mut s: heapless::String<8> = heapless::String::new();
            let unit = write_distance_split(&mut s, total_m, units);
            (s, unit)
        };
        let (v, u) = split(0, Units::Metric);
        assert_eq!((v.as_str(), u), ("0", "m"));
        let (v, u) = split(999, Units::Metric);
        assert_eq!((v.as_str(), u), ("999", "m"));
        let (v, u) = split(1000, Units::Metric);
        assert_eq!((v.as_str(), u), ("1.0", "km"));
        let (v, u) = split(44_000, Units::Metric);
        assert_eq!((v.as_str(), u), ("44.0", "km"));
        let (v, u) = split(1600, Units::Imperial);
        assert_eq!((v.as_str(), u), ("5249", "ft"), "one metre short of a mile is still feet");
        let (v, u) = split(1610, Units::Imperial);
        assert_eq!((v.as_str(), u), ("1.0", "mi"));
    }

    /// The unit-less tile figure keeps a decimal below 100 and drops it above, so the value never
    /// exceeds three digits.
    #[test]
    fn distance_figure_drops_its_decimal_at_a_hundred() {
        assert_eq!(distance_figure(0.0).as_str(), "0.0");
        assert_eq!(distance_figure(12.34).as_str(), "12.3");
        assert_eq!(distance_figure(99.94).as_str(), "99.9");
        assert_eq!(distance_figure(100.0).as_str(), "100", "100 drops the decimal");
        assert_eq!(distance_figure(142.6).as_str(), "143");
    }

    /// Speed, plain integers and percent: the present value, and the `--` fallback where one
    /// exists.
    #[test]
    fn figures_and_their_dashes() {
        assert_eq!(speed_figure(Some(0.0)).as_str(), "0.0");
        assert_eq!(speed_figure(Some(24.46)).as_str(), "24.5");
        assert_eq!(speed_figure(None).as_str(), "--", "no fix → dashes");
        assert_eq!(integer(0).as_str(), "0");
        assert_eq!(integer(1234).as_str(), "1234");
        assert_eq!(integer_opt(Some(72)).as_str(), "72");
        assert_eq!(integer_opt(None).as_str(), "--", "a stale sensor → dashes");
        assert_eq!(dashes().as_str(), "--");
        assert_eq!(percent(0).as_str(), "0%");
        assert_eq!(percent(-7).as_str(), "-7%", "a descent keeps its sign");
        assert_eq!(percent(12).as_str(), "12%");
    }

    /// Elevation rounds half away from zero and keeps a sub-sea-level sign; an absent sample is
    /// dashes, not a zero.
    #[test]
    fn elevation_rounded_signs_and_rounds() {
        assert_eq!(elevation_rounded(Some(0.0)).as_str(), "0");
        assert_eq!(elevation_rounded(Some(1249.5)).as_str(), "1250");
        assert_eq!(elevation_rounded(Some(-3.5)).as_str(), "-4", "below sea level rounds away from zero");
        assert_eq!(elevation_rounded(Some(-0.4)).as_str(), "0");
        assert_eq!(elevation_rounded(None).as_str(), "--");
    }

    /// The unit-suffixed climb readouts, in both unit systems, with and without a value.
    #[test]
    fn elevation_short_and_delta_carry_their_unit() {
        assert_eq!(elevation_short(Some(0), Units::Metric).as_str(), "0m");
        assert_eq!(elevation_short(Some(250), Units::Metric).as_str(), "250m");
        assert_eq!(elevation_short(Some(250), Units::Imperial).as_str(), "820ft");
        assert_eq!(elevation_short(None, Units::Metric).as_str(), "--");

        assert_eq!(elevation_delta(Some(0), Units::Metric).as_str(), "+0m", "a wash is +0, not dashes");
        assert_eq!(elevation_delta(Some(120), Units::Metric).as_str(), "+120m");
        assert_eq!(elevation_delta(Some(-40), Units::Metric).as_str(), "-40m");
        assert_eq!(elevation_delta(Some(-40), Units::Imperial).as_str(), "-131ft");
        assert_eq!(elevation_delta(None, Units::Metric).as_str(), "--");
    }

    /// `H:MM` at the minute and hour boundaries — minutes zero-padded, hours uncapped.
    #[test]
    fn duration_hms_boundaries() {
        assert_eq!(duration_hms(0.0).as_str(), "0:00");
        assert_eq!(duration_hms(59.0).as_str(), "0:00", "under a minute is still 0:00");
        assert_eq!(duration_hms(60.0).as_str(), "0:01");
        assert_eq!(duration_hms(59.0 * 60.0).as_str(), "0:59");
        assert_eq!(duration_hms(60.0 * 60.0).as_str(), "1:00");
        assert_eq!(duration_hms(35_999.0).as_str(), "9:59");
        assert_eq!(duration_hms(360_000.0).as_str(), "100:00", "hours are uncapped");
    }

    /// The locked expiry format at every boundary: the day/hour cutover at exactly 48 h, the hour
    /// band, the sub-hour fold to "soon", and past-due.
    #[test]
    fn expiry_short_boundaries() {
        let now = 1_000_000;
        let at = |secs: u32| expiry_short(now + secs, now);
        // ≥ 2 days → whole days. 48 h *exactly* is the first day-grain tick (not "in 47 h").
        assert_eq!(at(2 * DAY_SECS).as_str(), "in 2 d", "48 h exactly reads as 2 days");
        assert_eq!(at(12 * DAY_SECS).as_str(), "in 12 d");
        assert_eq!(at(2 * DAY_SECS - 1).as_str(), "in 47 h", "one second under 48 h is still the hour band");
        // < 48 h → whole hours, down to the last full hour.
        assert_eq!(at(5 * 3600).as_str(), "in 5 h");
        assert_eq!(at(3600).as_str(), "in 1 h", "exactly one hour left");
        // The final sub-hour tail folds to "soon" rather than "in 0 h".
        assert_eq!(at(3599).as_str(), "soon", "under an hour → soon, never \"in 0 h\"");
        // Past-due (the sweep hasn't collected it yet): now == deadline, and now > deadline.
        assert_eq!(expiry_short(now, now).as_str(), "soon", "exactly due → soon");
        assert_eq!(expiry_short(now - DAY_SECS, now).as_str(), "soon", "past-due → soon (saturating)");
    }

    /// Both date shapes off the same instant: day-first `D MON` for a row, ISO for the detail.
    #[test]
    fn date_shapes() {
        // 2026-03-07T09:41:00Z.
        const T: u32 = 1_772_876_460;
        let mut short: heapless::String<16> = heapless::String::new();
        write_date_short(&mut short, T, Language::En);
        assert_eq!(short.as_str(), "7 MAR", "day-first, no leading zero");
        assert_eq!(date_iso(T).as_str(), "2026-03-07");
        assert_eq!(date_iso(0).as_str(), "1970-01-01", "the epoch itself");
    }

    /// The UTC offset always prints its sign and pads both fields.
    #[test]
    fn utc_offset_signs() {
        assert_eq!(utc_offset(0).as_str(), "+00:00", "zero reads positive");
        assert_eq!(utc_offset(60).as_str(), "+01:00");
        assert_eq!(utc_offset(330).as_str(), "+05:30");
        assert_eq!(utc_offset(-60).as_str(), "-01:00");
        assert_eq!(utc_offset(-570).as_str(), "-09:30");
    }

    /// Temperature rounds half away from zero, clamps to two digits, and reports the wire sentinel
    /// as absent rather than as a number.
    #[test]
    fn temperature_short_rounds_and_clamps() {
        assert_eq!(temperature_short(0).unwrap().as_str(), "0°");
        assert_eq!(temperature_short(145).unwrap().as_str(), "15°", "half rounds away from zero");
        assert_eq!(temperature_short(-145).unwrap().as_str(), "-15°");
        assert_eq!(temperature_short(-55).unwrap().as_str(), "-6°");
        assert_eq!(temperature_short(-54).unwrap().as_str(), "-5°");
        assert_eq!(temperature_short(1500).unwrap().as_str(), "99°", "clamped to two digits");
        assert_eq!(temperature_short(obc_formats::obcw::TEMP_UNAVAILABLE), None);
    }

    /// Each displayed byte-unit boundary, and the rounding at it.
    #[test]
    fn bytes_short_unit_boundaries() {
        let show = |bytes| {
            let mut s: heapless::String<16> = heapless::String::new();
            write_bytes_short(&mut s, bytes);
            s
        };
        const KIB: u64 = 1024;
        const MIB: u64 = KIB * 1024;
        const GIB: u64 = MIB * 1024;
        assert_eq!(show(0).as_str(), "0 KB", "an empty card still reads in KB");
        assert_eq!(show(KIB).as_str(), "1 KB");
        assert_eq!(show(MIB - 1).as_str(), "1024 KB", "just under a MiB is still KB");
        assert_eq!(show(MIB).as_str(), "1 MB");
        assert_eq!(show(GIB - 1).as_str(), "1024 MB", "just under a GiB is still MB");
        assert_eq!(show(GIB).as_str(), "1.0 GB");
        assert_eq!(show(29 * GIB + GIB / 2).as_str(), "29.5 GB");
    }

    /// Addresses render big-endian with colons, from the little-endian bytes the wire carries.
    #[test]
    fn ble_address_is_big_endian() {
        let mut b = heapless::String::<24>::new();
        write_ble_address(&mut b, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert_eq!(b.as_str(), "66:55:44:33:22:11");
        b.clear();
        write_ble_address(&mut b, &[0; 6]);
        assert_eq!(b.as_str(), "00:00:00:00:00:00", "the minimum address");
        b.clear();
        write_ble_address(&mut b, &[0xFF; 6]);
        assert_eq!(b.as_str(), "FF:FF:FF:FF:FF:FF", "the maximum address");
    }
}
