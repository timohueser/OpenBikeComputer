//! Every quantity readout a screen prints, one formatter per quantity and output style. Each
//! function is named `<quantity>_<style>`, and a `write_*` name appends into a caller-owned buffer.
//! Two styles of one quantity are two functions, because their thresholds differ.

use core::fmt::Write;

use crate::settings::{DateTime, Language, Units, FT_PER_M, FT_PER_MI};
use crate::{t, Msg};

/// The "no data" glyph every optional readout falls back to.
pub(crate) fn dashes() -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = s.push_str("--");
    s
}

/// A large-unit distance figure with no unit in it, for a tile whose caption carries the unit. One
/// decimal below 100 and whole above, so the value stays inside three digits. Takes the
/// already-converted figure (`units.dist(km)`).
pub(crate) fn distance_figure(value: f32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = if value >= 100.0 { write!(s, "{value:.0}") } else { write!(s, "{value:.1}") };
    s
}

/// A distance with its unit tight against the number. Metric: `NNNm` below 1 km, `N.Nkm` below
/// 100 km, whole `NNNkm` above. Imperial: `NNNft` below 1000 ft, `N.Nmi` below 100 mi, whole
/// `NNNmi` above.
pub(crate) fn distance_short(d_m: u32, units: Units) -> heapless::String<10> {
    let mut s = heapless::String::new();
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u64;
        if ft < 1000 {
            let _ = write!(s, "{ft}ft");
        } else if ft < 100 * u64::from(FT_PER_MI) {
            let tenths = (ft * 10 + u64::from(FT_PER_MI) / 2) / u64::from(FT_PER_MI);
            let _ = write!(s, "{}.{}mi", tenths / 10, tenths % 10);
        } else {
            let _ = write!(s, "{}mi", (ft + u64::from(FT_PER_MI) / 2) / u64::from(FT_PER_MI));
        }
    } else if d_m < 1000 {
        let _ = write!(s, "{d_m}m");
    } else if d_m < 100_000 {
        let tenths = (d_m + 50) / 100;
        let _ = write!(s, "{}.{}km", tenths / 10, tenths % 10);
    } else {
        let _ = write!(s, "{}km", (u64::from(d_m) + 500) / 1000);
    }
    s
}

/// Append a distance after `prefix`, compacted to a whole large unit past the crossover, so the
/// readout fits a chip or a header line. Metric: `NNNm` below 1 km, `NNkm` above. Imperial: `NNNft`
/// below a mile, `NNmi` above.
pub(crate) fn write_distance_coarse<const N: usize>(s: &mut heapless::String<N>, prefix: &str, d_m: u32, units: Units) {
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u64;
        if ft >= u64::from(FT_PER_MI) {
            let _ = write!(s, "{prefix}{}mi", (ft + u64::from(FT_PER_MI) / 2) / u64::from(FT_PER_MI));
        } else {
            let _ = write!(s, "{prefix}{ft}ft");
        }
    } else if d_m >= 1000 {
        let _ = write!(s, "{prefix}{}km", (u64::from(d_m) + 500) / 1000);
    } else {
        let _ = write!(s, "{prefix}{d_m}m");
    }
}

/// Append a distance as a spaced large unit: `NN.N km` or `NN.N mi`, compacted to a whole unit
/// from 100 up, so the longest metadata run stays inside an inset row's budget.
pub(crate) fn write_distance_spaced<const N: usize>(s: &mut heapless::String<N>, dist_m: u32, units: Units) {
    if units.is_imperial() {
        let mi10 = (dist_m as f32 * FT_PER_M / FT_PER_MI as f32 * 10.0) as u32;
        if mi10 >= 1000 {
            let _ = write!(s, "{} mi", (mi10 + 5) / 10);
        } else {
            let _ = write!(s, "{}.{} mi", mi10 / 10, mi10 % 10);
        }
    } else {
        let km10 = (u64::from(dist_m) + 50) / 100; // tenths of a km
        if km10 >= 1000 {
            let _ = write!(s, "{} km", (u64::from(dist_m) + 500) / 1000);
        } else {
            let _ = write!(s, "{}.{} km", km10 / 10, km10 % 10);
        }
    }
}

/// Append a distance split from its unit: the value goes into `s` and the unit suffix comes back,
/// for the caller to draw in its own font. Whole metres below 1 km and one-decimal km above; whole
/// feet below a mile and one-decimal miles above.
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

/// A speed figure to one decimal, or [`dashes`] when unknown. The unit rides in the caption.
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

/// A whole figure as plain digits. The unit rides in the caption.
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

/// A live-elevation figure rounded to a whole unit, or [`dashes`] when there is no altimeter
/// sample. It is signed, for a sub-sea-level reading, and rounds half away from zero without
/// `libm`.
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

/// A remaining-ascent readout with its unit tight against the number (`250m`), or [`dashes`].
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

/// A signed climb difference (`+120m` or `-40m`) in the rider's elevation unit, or [`dashes`].
/// `+0m` is an answer, not a missing one.
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

/// Clock components as `HH:MM`, without date or timezone conversion. Hours are not wrapped:
/// an opening-hours endpoint can be `24:00`.
pub(crate) fn clock_hm(hour: u8, minute: u8) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{hour:02}:{minute:02}");
    s
}

/// A duration in seconds as `H:MM`: hours uncapped, minutes zero-padded. It is not localised,
/// because hours and minutes are the same in every catalog language.
pub(crate) fn duration_hms(secs: f32) -> heapless::String<8> {
    let total_min = (secs as u32) / 60;
    let mut s = heapless::String::new();
    let _ = write!(s, "{}:{:02}", total_min / 60, total_min % 60);
    s
}
/// The 12 uppercase month-abbreviation catalog keys, in calendar order. The settings pages use
/// the mixed-case table in `settings::month_name`.
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

/// Append a unix instant as the short day-first date `D MON` (UTC), with no leading zero. It is
/// day-first in every language.
pub(crate) fn write_date_short<const N: usize>(s: &mut heapless::String<N>, unix: u32, lang: Language) {
    let d = DateTime::from_unix(unix);
    let _ = write!(s, "{} {}", d.day, t(DATE_MONTHS[(d.month.clamp(1, 12) - 1) as usize], lang));
}

/// A unix instant as a compact `YYYY-MM-DD` (UTC). Local time would need the app's UTC offset
/// threaded in, and the date rarely differs.
pub(crate) fn date_iso(unix: u32) -> heapless::String<12> {
    let d = DateTime::from_unix(unix);
    let mut s = heapless::String::new();
    let _ = write!(s, "{:04}-{:02}-{:02}", d.year, d.month, d.day);
    s
}

/// A UTC offset as `±HH:MM`. The sign is always printed, so zero reads `+00:00`.
pub(crate) fn utc_offset(min: i16) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let sign = if min < 0 { '-' } else { '+' };
    let a = min.unsigned_abs();
    let _ = write!(s, "{sign}{:02}:{:02}", a / 60, a % 60);
    s
}

/// Append a byte count as `N.N GB`, `NNN MB` or `NNN KB`. The units are binary.
pub(crate) fn write_bytes_short(s: &mut heapless::String<16>, bytes: u64) {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        let tenths = (bytes * 10 + GIB / 2) / GIB;
        let _ = write!(s, "{}.{} GB", tenths / 10, tenths % 10);
    } else if bytes >= MIB {
        let _ = write!(s, "{} MB", (bytes + MIB / 2) / MIB);
    } else {
        let _ = write!(s, "{} KB", (bytes + KIB / 2) / KIB);
    }
}

/// Append a BLE address big-endian (`AA:BB:…`), the display order. The stored bytes are
/// little-endian, as the wire carries them.
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

    #[test]
    fn distance_short_imperial_crossovers() {
        assert_eq!(distance_short(0, Units::Imperial).as_str(), "0ft");
        assert_eq!(distance_short(300, Units::Imperial).as_str(), "984ft", "300 m ≈ 984 ft stays feet");
        assert_eq!(distance_short(305, Units::Imperial).as_str(), "0.2mi", "past 1000 ft crosses to decimal miles");
        assert_eq!(distance_short(15_933, Units::Imperial).as_str(), "9.9mi");
        assert_eq!(distance_short(160_000, Units::Imperial).as_str(), "99.4mi", "just under 100 mi keeps a decimal");
        assert_eq!(distance_short(200_000, Units::Imperial).as_str(), "124mi", "well past 100 mi is whole miles");
    }

    #[test]
    fn distance_rounding_preserves_large_u32_values() {
        for metres in [u32::MAX - 1, u32::MAX] {
            for (units, short, spaced) in
                [(Units::Metric, "4294967km", "4294967 km"), (Units::Imperial, "2668769mi", "2668769 mi")]
            {
                assert_eq!(distance_short(metres, units).as_str(), short);
                let mut value: heapless::String<24> = heapless::String::new();
                write_distance_coarse(&mut value, "", metres, units);
                assert_eq!(value.as_str(), short);
                value.clear();
                write_distance_spaced(&mut value, metres, units);
                assert_eq!(value.as_str(), spaced);
            }
        }
    }

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

        let mut pilled: heapless::String<24> = heapless::String::new();
        write_distance_coarse(&mut pilled, "OFF ", 1500, Units::Metric);
        assert_eq!(pilled.as_str(), "OFF 2km");
    }

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

    #[test]
    fn distance_figure_drops_its_decimal_at_a_hundred() {
        assert_eq!(distance_figure(0.0).as_str(), "0.0");
        assert_eq!(distance_figure(12.34).as_str(), "12.3");
        assert_eq!(distance_figure(99.94).as_str(), "99.9");
        assert_eq!(distance_figure(100.0).as_str(), "100", "100 drops the decimal");
        assert_eq!(distance_figure(142.6).as_str(), "143");
    }

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

    #[test]
    fn elevation_rounded_signs_and_rounds() {
        assert_eq!(elevation_rounded(Some(0.0)).as_str(), "0");
        assert_eq!(elevation_rounded(Some(1249.5)).as_str(), "1250");
        assert_eq!(elevation_rounded(Some(-3.5)).as_str(), "-4", "below sea level rounds away from zero");
        assert_eq!(elevation_rounded(Some(-0.4)).as_str(), "0");
        assert_eq!(elevation_rounded(None).as_str(), "--");
    }

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

    #[test]
    fn clock_hm_boundaries() {
        for (hour, minute, expected) in [(0, 0, "00:00"), (9, 5, "09:05"), (23, 59, "23:59"), (24, 0, "24:00")] {
            assert_eq!(clock_hm(hour, minute).as_str(), expected);
        }
    }

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

    #[test]
    fn utc_offset_signs() {
        assert_eq!(utc_offset(0).as_str(), "+00:00", "zero reads positive");
        assert_eq!(utc_offset(60).as_str(), "+01:00");
        assert_eq!(utc_offset(330).as_str(), "+05:30");
        assert_eq!(utc_offset(-60).as_str(), "-01:00");
        assert_eq!(utc_offset(-570).as_str(), "-09:30");
    }

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
