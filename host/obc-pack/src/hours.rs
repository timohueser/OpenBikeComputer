//! `hours.rs` — parse OSM `opening_hours` into a compact normalized weekly
//! schedule at pack time (issue #440, epic #439). The `opening_hours` grammar
//! never touches the MCU: the device does a trivial weekday lookup on the baked
//! blob (P3+).
//!
//! This is a **pragmatic subset parser**, deliberately dependency-free and fully
//! deterministic (a maintained crate would drag a grammar + its own time model
//! into the packer for patterns the real data — Monaco/Freiburg town shops —
//! never exercises). It covers: weekday ranges/lists (`Mo-Fr`, `Mo,We,Fr`),
//! `HH:MM-HH:MM` intervals, comma-separated intervals (split lunch), `;`-separated
//! rules (both day-scoped and time-only), `24/7`, `off`/`closed`, bare intervals
//! that apply to every day, and overnight wrap. Anything it can't model it
//! **drops and flags** (see [`Schedule`] flags) rather than guessing.
//!
//! ## Locked encoding (epic #439 planning, 2026-07-05 — do not re-litigate)
//! - **Resolution 15 min.** A time-of-day is quarter-hours from midnight,
//!   `0..=96` (`u8`; `96` = 24:00).
//! - **Per weekday: up to 2 open intervals**, each `(open_q, close_q)`. Unused
//!   slot = `(0, 0)`. **Closed day** = both slots `(0, 0)`. **24 h** = slot 0
//!   `(0, 96)`, slot 1 `(0, 0)`. **Overnight** (`close_q <= open_q`, both nonzero)
//!   wraps past midnight — stored as-is, never split across days.
//! - **Schedule blob = 29 bytes:** `flags u8` + `7 days × 2 slots × (open_q u8,
//!   close_q u8)`. Day order **Mon..Sun** (index 0 = Monday). `flags` bit 0 =
//!   seasonal, bit 1 = truncated/dropped-rules; other bits reserved 0.
//!
//! ## Representative week + rounding (documented choices)
//! - **Representative week:** a rule carrying a month/date/season selector (e.g.
//!   `Apr-Oct: Mo-Su 09:00-18:00`) is evaluated **as if in-season** — the
//!   in-season intervals are baked and the **seasonal** flag is set. The device
//!   ignores the flag in v1 (baked for a future season-aware pass, epic #439).
//! - **Rounding:** each `HH:MM` is rounded to the nearest quarter-hour with
//!   **round-half-to-even** (banker's rounding), the same house convention as the
//!   packer's coordinate rounding. Pinned in [`tests::rounding_half_to_even`].

use std::collections::HashMap;

// The normative hours-blob dimensions/flags are owned by `obc-formats`; imported under the
// packer-local names this encoder reads (`BLOB_LEN` is also read by `serialize.rs`'s width assert,
// hence `pub(crate)`). Not exported from the crate.
pub(crate) use obc_formats::obcm::{
    POI_HOURS_BLOB_LEN as BLOB_LEN, POI_HOURS_DAYS as DAYS, POI_HOURS_FLAG_SEASONAL as FLAG_SEASONAL,
    POI_HOURS_FLAG_TRUNCATED as FLAG_TRUNCATED, POI_HOURS_SLOTS_PER_DAY as SLOTS_PER_DAY,
};

/// One open interval, quarter-hours from midnight. `close_q <= open_q` (both
/// nonzero) is an overnight wrap; `(0, 0)` is an unused slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interval {
    pub open_q: u8,
    pub close_q: u8,
}

/// A normalized weekly schedule: seven days × up to two intervals, plus the
/// seasonal/truncated flags. This is the structured form the packer keeps in
/// memory; [`Schedule::encode`] renders the 29-byte blob P2 will pool + store.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schedule {
    /// `days[0]` = Monday .. `days[6]` = Sunday; each up to two intervals.
    pub days: [[Interval; SLOTS_PER_DAY]; DAYS],
    /// [`FLAG_SEASONAL`] | [`FLAG_TRUNCATED`].
    pub flags: u8,
}

impl Schedule {
    /// Render the locked 29-byte blob: `flags` then Mon..Sun × 2 slots ×
    /// `(open_q, close_q)`.
    pub fn encode(&self) -> [u8; BLOB_LEN] {
        let mut out = [0u8; BLOB_LEN];
        out[0] = self.flags;
        let mut i = 1;
        for day in &self.days {
            for slot in day {
                out[i] = slot.open_q;
                out[i + 1] = slot.close_q;
                i += 2;
            }
        }
        out
    }

    fn seasonal(&self) -> bool {
        self.flags & FLAG_SEASONAL != 0
    }

    fn truncated(&self) -> bool {
        self.flags & FLAG_TRUNCATED != 0
    }
}

/// Parse a raw OSM `opening_hours` value into a [`Schedule`], or `None` when the
/// string is **fully** unparseable (nothing recognized) — the POI then has no
/// hours (`hours_ref` will be none in P2). A partially-parseable string returns
/// `Some` with the [`FLAG_TRUNCATED`] flag set for whatever was dropped.
pub fn parse(raw: &str) -> Option<Schedule> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let mut sched = Schedule::default();
    let mut any_recognized = false;

    // `24/7` is its own top-level token (may appear alone or as the whole value).
    if raw == "24/7" {
        for d in 0..DAYS {
            sched.days[d][0] = Interval { open_q: 0, close_q: 96 };
        }
        return Some(sched);
    }

    // Rules are `;`-separated. Each rule is either day-scoped (`Mo-Fr 08:00-18:00`,
    // `Sa off`) or time-only (`09:00-12:30` — applies to every day).
    for rule in raw.split(';') {
        let rule = rule.trim();
        if rule.is_empty() {
            continue;
        }
        match apply_rule(rule, &mut sched) {
            RuleOutcome::Applied => any_recognized = true,
            RuleOutcome::Dropped => sched.flags |= FLAG_TRUNCATED,
        }
    }

    if any_recognized {
        Some(sched)
    } else {
        // Nothing at all parsed (e.g. `"garbage;;"`) ⇒ no schedule.
        None
    }
}

enum RuleOutcome {
    /// The rule set at least one interval or explicitly closed a day.
    Applied,
    /// The rule was recognized-as-out-of-scope or unparseable and dropped.
    Dropped,
}

/// Apply one `;`-delimited rule to the schedule in place.
fn apply_rule(rule: &str, sched: &mut Schedule) -> RuleOutcome {
    // A month/date/season selector prefixes the rule (`Apr-Oct: Mo-Su 09:00-18:00`)
    // or stands as a bare month/date token. Bake a representative (in-season) week:
    // strip the selector, evaluate the rest, and flag seasonal. If the whole rule
    // is just a season selector with nothing after it, there's nothing to bake.
    let rule = match strip_season_selector(rule) {
        SeasonStrip::None => rule,
        SeasonStrip::Stripped(rest) => {
            sched.flags |= FLAG_SEASONAL;
            let rest = rest.trim();
            if rest.is_empty() {
                return RuleOutcome::Dropped;
            }
            rest
        }
    };

    // Split the rule into a leading selector (weekday/PH tokens) and the rest
    // (intervals or `off`/`closed`). The selector runs up to the first token that
    // looks like a time interval or an `off`/`closed`/`24/7` keyword.
    let (selector, body) = split_selector(rule);
    let sel = selector.trim();
    let body = body.trim();

    // Public-/school-holiday and unsupported named selectors: `PH off` is a real
    // closed signal we could honor, but we have no PH slot in the weekly blob, so
    // any PH/SH rule is dropped (and flagged). Same for sunrise/sunset/week/easter.
    if sel.eq_ignore_ascii_case("PH") || sel.eq_ignore_ascii_case("SH") {
        return RuleOutcome::Dropped;
    }

    // Resolve which weekdays this rule targets. An empty selector = every day
    // (bare time-only rule, or a bare `off`).
    let days = if sel.is_empty() { Some((0..DAYS as u8).collect::<Vec<u8>>()) } else { parse_weekday_selector(sel) };
    let Some(days) = days else {
        return RuleOutcome::Dropped;
    };
    if days.is_empty() {
        return RuleOutcome::Dropped;
    }

    // `off` / `closed`: explicitly clear those days (a closed day = both slots 0).
    if body.eq_ignore_ascii_case("off") || body.eq_ignore_ascii_case("closed") {
        for d in days {
            sched.days[d as usize] = [Interval::default(); SLOTS_PER_DAY];
        }
        return RuleOutcome::Applied;
    }

    // `24/7` after a weekday selector (`Mo-Su 24/7`).
    if body == "24/7" {
        for d in days {
            set_day_intervals(sched, d as usize, &[Interval { open_q: 0, close_q: 96 }]);
        }
        return RuleOutcome::Applied;
    }

    // Otherwise the body is comma-separated `HH:MM-HH:MM` intervals.
    let mut intervals = Vec::new();
    let mut dropped_any = false;
    for part in body.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match parse_interval(part) {
            Some(iv) => intervals.push(iv),
            None => dropped_any = true,
        }
    }
    if intervals.is_empty() {
        // Body present but no interval parsed (e.g. `Mo sunrise-sunset`) ⇒ drop.
        return RuleOutcome::Dropped;
    }

    let mut truncated = dropped_any;
    for d in days {
        if set_day_intervals(sched, d as usize, &intervals) {
            truncated = true;
        }
    }
    if truncated {
        sched.flags |= FLAG_TRUNCATED;
    }
    RuleOutcome::Applied
}

/// Merge `intervals` into a day, appending to whatever a prior rule already set
/// (so two time-only rules layer into two slots). Returns `true` if the day
/// overflowed two slots and extra intervals were dropped (truncated).
fn set_day_intervals(sched: &mut Schedule, day: usize, intervals: &[Interval]) -> bool {
    // Count already-filled slots (a non-`(0,0)` slot is used).
    let mut used = sched.days[day].iter().take_while(|iv| **iv != Interval::default()).count();
    let mut overflow = false;
    for &iv in intervals {
        if used < SLOTS_PER_DAY {
            sched.days[day][used] = iv;
            used += 1;
        } else {
            overflow = true;
        }
    }
    overflow
}

enum SeasonStrip<'a> {
    None,
    Stripped(&'a str),
}

/// Month/date/season names that mark a rule as seasonal. A rule containing any of
/// these carries a date selector we don't model per-week; we bake a representative
/// (in-season) week and flag seasonal.
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// If a rule carries a month/date/season selector, strip it and return the rest
/// (the in-season body to bake). The common form is `<selector>: <body>` — e.g.
/// `Apr-Oct: Mo-Su 09:00-18:00`. We only strip the `:`-delimited prefix so the
/// weekday+interval body is evaluated cleanly; a bare month token with no colon
/// (`Dec 25 off`) is treated as seasonal with the month word removed.
fn strip_season_selector(rule: &str) -> SeasonStrip<'_> {
    // `<selector>: <body>` form: colon separates a date/season selector from the
    // weekly rule. Only treat the prefix as a season selector when it actually
    // mentions a month (so we don't mis-split, though `:` in opening_hours is only
    // ever a date-range separator here).
    if let Some((prefix, rest)) = rule.split_once(':') {
        // Guard against `HH:MM` false positives: a time has digits either side of
        // the colon. A season prefix contains a month word.
        if MONTHS.iter().any(|m| prefix.contains(m)) {
            return SeasonStrip::Stripped(rest);
        }
    }
    // Bare month token anywhere with no colon selector (rare): flag seasonal but
    // there's no clean in-season body to extract, so treat the whole thing as the
    // body with the month stripped is unreliable — just signal seasonal + no strip
    // and let the rule parse/drop normally.
    if MONTHS.iter().any(|m| rule.split(|c: char| !c.is_ascii_alphanumeric()).any(|tok| tok == *m)) {
        // Return the rule unchanged as the body but mark it seasonal via a strip
        // of an empty prefix. We can't cleanly remove a bare month, so keep the
        // rule and let apply_rule flag seasonal.
        return SeasonStrip::Stripped(rule);
    }
    SeasonStrip::None
}

/// Split a rule into its leading weekday/named selector and the body (intervals or
/// `off`). The split point is the first token that starts an interval (a digit or
/// `24/7`) or an `off`/`closed` keyword.
fn split_selector(rule: &str) -> (&str, &str) {
    let bytes = rule.as_bytes();
    // Walk to the first space-delimited token that looks like a time/keyword.
    let mut i = 0;
    while i < bytes.len() {
        // Skip leading spaces.
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' {
            i += 1;
        }
        let token = &rule[start..i];
        if token_is_body_start(token) {
            return (&rule[..start], &rule[start..]);
        }
    }
    // No body token found ⇒ whole thing is the selector, empty body.
    (rule, "")
}

/// Does this token begin the body (an interval or an `off`/`closed`/`24/7`)?
fn token_is_body_start(token: &str) -> bool {
    let t = token.trim_start_matches(',');
    t.starts_with(|c: char| c.is_ascii_digit())
        || t == "24/7"
        || t.eq_ignore_ascii_case("off")
        || t.eq_ignore_ascii_case("closed")
}

/// Parse a weekday selector: comma-separated single days or `Mo-Fr` ranges,
/// e.g. `Mo`, `Mo,We,Fr`, `Mo-Fr`, `Mo-Fr,Su`. Returns the 0-based weekday indices
/// (Mon = 0), or `None` if any token is unrecognized.
fn parse_weekday_selector(sel: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for part in sel.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let (a, b) = (weekday_index(a.trim())?, weekday_index(b.trim())?);
            // Wrap-around ranges (`Sa-Su`, or even `Fr-Mo`) walk forward mod 7.
            let mut d = a;
            loop {
                if !out.contains(&d) {
                    out.push(d);
                }
                if d == b {
                    break;
                }
                d = (d + 1) % DAYS as u8;
            }
        } else {
            let d = weekday_index(part)?;
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    Some(out)
}

/// Two-letter weekday abbreviation → 0-based index (Mon = 0). Case-insensitive.
fn weekday_index(s: &str) -> Option<u8> {
    Some(match s.to_ascii_lowercase().as_str() {
        "mo" => 0,
        "tu" => 1,
        "we" => 2,
        "th" => 3,
        "fr" => 4,
        "sa" => 5,
        "su" => 6,
        _ => return None,
    })
}

/// Parse one `HH:MM-HH:MM` interval into quarter-hour endpoints. Times run
/// `00:00..24:00`; `24:00` → `q=96`. Returns `None` if either endpoint is
/// unparseable (e.g. `sunrise`, a malformed time).
fn parse_interval(part: &str) -> Option<Interval> {
    let (open, close) = part.split_once('-')?;
    let open_q = parse_time_q(open.trim())?;
    let close_q = parse_time_q(close.trim())?;
    Some(Interval { open_q, close_q })
}

/// `HH:MM` → quarter-hours from midnight, `0..=96`, round-half-to-even. `24:00`
/// is the only hour past 23 we accept (→ 96). Rejects out-of-range or non-numeric.
fn parse_time_q(s: &str) -> Option<u8> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if m >= 60 {
        return None;
    }
    if h == 24 {
        return if m == 0 { Some(96) } else { None };
    }
    if h > 23 {
        return None;
    }
    // Minutes-from-midnight → quarter-hours, round-half-to-even (banker's), the
    // house rounding convention. 15 min per quarter; a boundary exactly on 7.5 min
    // rounds to the even quarter.
    let minutes = h * 60 + m;
    let q = round_half_even(minutes as f64 / 15.0);
    // 00:00..23:59 rounds into 0..=96 (23:52+ rounds up to 96 = 24:00).
    Some(q.min(96) as u8)
}

/// Round-half-to-even (banker's rounding), matching the packer's coordinate
/// rounding convention.
fn round_half_even(x: f64) -> u32 {
    let floor = x.floor();
    let diff = x - floor;
    let rounded = if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else {
        // Exactly halfway → round to even.
        let f = floor as i64;
        if f % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    };
    rounded as u32
}

/// Build the dedup pool P2 consumes: collapse identical 29-byte blobs to one
/// unique entry, and return a per-POI index aligned to the input slice.
///
/// Returns `(pool, refs)` where `pool` is the unique-blob list (a POI's blob `i`
/// is `pool[refs[k] as usize]` when `refs[k]` is `Some(i)`), and `refs[k]` is the
/// pool index for input POI `k`, or `None` when that POI has no parseable hours.
/// O(n) via a `HashMap` keyed on the 29 blob bytes.
///
/// The `hours` accessor lets the caller pass any POI-like slice (P2 passes the
/// real `&[Poi]`); here it's generic so the pool builder stays decoupled from the
/// `Poi` struct's other fields.
pub fn build_hours_pool<T>(
    items: &[T],
    hours: impl Fn(&T) -> Option<&Schedule>,
) -> (Vec<[u8; BLOB_LEN]>, Vec<Option<u16>>) {
    let mut pool: Vec<[u8; BLOB_LEN]> = Vec::new();
    let mut index: HashMap<[u8; BLOB_LEN], u16> = HashMap::new();
    let mut refs: Vec<Option<u16>> = Vec::with_capacity(items.len());
    for item in items {
        match hours(item) {
            Some(sched) => {
                let blob = sched.encode();
                let idx = *index.entry(blob).or_insert_with(|| {
                    let i = pool.len() as u16;
                    pool.push(blob);
                    i
                });
                refs.push(Some(idx));
            }
            None => refs.push(None),
        }
    }
    (pool, refs)
}

/// Human-readable one-line rendering of a schedule for `--dump-hours`, e.g.
/// `Mo 08:30-19:30 | Tu 08:30-19:30 | ... | Su closed [flags: -]`.
pub fn describe(sched: &Schedule) -> String {
    const NAMES: [&str; DAYS] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    let mut parts = Vec::with_capacity(DAYS);
    for (d, day) in sched.days.iter().enumerate() {
        let ivs: Vec<String> = day
            .iter()
            .filter(|iv| **iv != Interval::default())
            .map(|iv| format!("{}-{}", fmt_q(iv.open_q), fmt_q(iv.close_q)))
            .collect();
        let body = if ivs.is_empty() { "closed".to_string() } else { ivs.join(",") };
        parts.push(format!("{} {}", NAMES[d], body));
    }
    let mut flag_str = String::new();
    if sched.seasonal() {
        flag_str.push('S');
    }
    if sched.truncated() {
        flag_str.push('T');
    }
    if flag_str.is_empty() {
        flag_str.push('-');
    }
    format!("{} [flags: {}]", parts.join(" | "), flag_str)
}

/// Quarter-hours → `HH:MM` (with `96` → `24:00`).
fn fmt_q(q: u8) -> String {
    let minutes = q as u32 * 15;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schedule blob for one day set to the given intervals (Mon = 0).
    fn day(sched: &Schedule, d: usize) -> [Interval; SLOTS_PER_DAY] {
        sched.days[d]
    }

    fn iv(open_q: u8, close_q: u8) -> Interval {
        Interval { open_q, close_q }
    }

    #[test]
    fn parse_weekday_range_single_interval() {
        // Mo-Fr 08:00-18:00 → 08:00=q32, 18:00=q72 on Mon..Fri, closed Sat/Sun.
        let s = parse("Mo-Fr 08:00-18:00").unwrap();
        for d in 0..5 {
            assert_eq!(day(&s, d), [iv(32, 72), Interval::default()], "day {d}");
        }
        assert_eq!(day(&s, 5), [Interval::default(); 2], "Sat closed");
        assert_eq!(day(&s, 6), [Interval::default(); 2], "Sun closed");
        assert_eq!(s.flags, 0, "no flags");
    }

    #[test]
    fn parse_split_lunch_two_slots() {
        // Mo-Fr 08:00-12:00,14:00-18:00 → two intervals per weekday.
        let s = parse("Mo-Fr 08:00-12:00,14:00-18:00").unwrap();
        for d in 0..5 {
            assert_eq!(day(&s, d), [iv(32, 48), iv(56, 72)], "day {d}");
        }
        assert_eq!(s.flags, 0);
    }

    #[test]
    fn parse_24_7() {
        let s = parse("24/7").unwrap();
        for d in 0..DAYS {
            assert_eq!(day(&s, d), [iv(0, 96), Interval::default()], "day {d} open all day");
        }
        assert_eq!(s.flags, 0);
    }

    #[test]
    fn parse_full_day_midnight_to_2400() {
        // Mo-Su 00:00-24:00 → every day (0,96), 24:00 = q96.
        let s = parse("Mo-Su 00:00-24:00").unwrap();
        for d in 0..DAYS {
            assert_eq!(day(&s, d), [iv(0, 96), Interval::default()], "day {d}");
        }
        assert_eq!(s.flags, 0);
    }

    #[test]
    fn parse_off_day() {
        // A base rule + a `Sa off` overrides Saturday to closed.
        let s = parse("Mo-Sa 08:00-18:00; Sa off").unwrap();
        assert_eq!(day(&s, 4), [iv(32, 72), Interval::default()], "Fri still open");
        assert_eq!(day(&s, 5), [Interval::default(); 2], "Sat off");
        // Su off standalone.
        let s2 = parse("Mo-Su 09:00-17:00; Su off").unwrap();
        assert_eq!(day(&s2, 6), [Interval::default(); 2], "Sun off");
    }

    #[test]
    fn parse_multi_rule_different_days() {
        // Two rules setting different day sets.
        let s = parse("Mo-Sa 08:00-21:00; Su 09:00-19:00").unwrap();
        assert_eq!(day(&s, 0), [iv(32, 84), Interval::default()], "Mon 08-21");
        assert_eq!(day(&s, 5), [iv(32, 84), Interval::default()], "Sat 08-21");
        assert_eq!(day(&s, 6), [iv(36, 76), Interval::default()], "Sun 09-19");
        assert_eq!(s.flags, 0);
    }

    #[test]
    fn parse_time_only_rules_layer_every_day() {
        // Two bare intervals apply to every day → two slots each; Su off closes Sun.
        let s = parse("09:00-12:30; 14:30-19:00; Su off").unwrap();
        for d in 0..6 {
            assert_eq!(day(&s, d), [iv(36, 50), iv(58, 76)], "day {d} two intervals");
        }
        assert_eq!(day(&s, 6), [Interval::default(); 2], "Sun off");
        assert_eq!(s.flags, 0);
    }

    #[test]
    fn parse_bare_interval_every_day() {
        let s = parse("08:00-20:00").unwrap();
        for d in 0..DAYS {
            assert_eq!(day(&s, d), [iv(32, 80), Interval::default()], "day {d}");
        }
    }

    #[test]
    fn parse_single_and_list_weekdays() {
        let s = parse("Mo 08:00-12:00").unwrap();
        assert_eq!(day(&s, 0), [iv(32, 48), Interval::default()]);
        assert_eq!(day(&s, 1), [Interval::default(); 2], "Tue untouched");

        let s2 = parse("Mo,We,Fr 10:00-14:00").unwrap();
        for d in [0, 2, 4] {
            assert_eq!(day(&s2, d), [iv(40, 56), Interval::default()], "day {d}");
        }
        for d in [1, 3, 5, 6] {
            assert_eq!(day(&s2, d), [Interval::default(); 2], "day {d} closed");
        }

        let s3 = parse("Mo-Fr,Su 09:00-17:00").unwrap();
        for d in [0, 1, 2, 3, 4, 6] {
            assert_eq!(day(&s3, d), [iv(36, 68), Interval::default()], "day {d}");
        }
        assert_eq!(day(&s3, 5), [Interval::default(); 2], "Sat closed");
    }

    #[test]
    fn parse_seasonal_representative_week() {
        // Apr-Oct: Mo-Su 09:00-18:00 → representative (in-season) week + seasonal flag.
        let s = parse("Apr-Oct: Mo-Su 09:00-18:00").unwrap();
        for d in 0..DAYS {
            assert_eq!(day(&s, d), [iv(36, 72), Interval::default()], "day {d} in-season");
        }
        assert_eq!(s.flags & FLAG_SEASONAL, FLAG_SEASONAL, "seasonal flag set");
        assert_eq!(s.flags & FLAG_TRUNCATED, 0, "not truncated");
    }

    #[test]
    fn parse_more_than_two_intervals_truncates() {
        // Three intervals on a day → first two kept, truncated flag set.
        let s = parse("Mo 08:00-10:00,11:00-13:00,14:00-16:00").unwrap();
        assert_eq!(day(&s, 0), [iv(32, 40), iv(44, 52)], "first two kept");
        assert_eq!(s.flags & FLAG_TRUNCATED, FLAG_TRUNCATED, "truncated flag set");
    }

    #[test]
    fn parse_overnight_wrap_not_split() {
        // Mo 22:00-02:00 → open_q=88, close_q=8 stored as-is (wrap, not split).
        let s = parse("Mo 22:00-02:00").unwrap();
        assert_eq!(day(&s, 0), [iv(88, 8), Interval::default()], "overnight stored as-is");
        // The wrap does NOT touch Tuesday.
        assert_eq!(day(&s, 1), [Interval::default(); 2], "Tue untouched");
    }

    #[test]
    fn parse_ph_off_dropped_and_flagged() {
        // A base rule parses; the PH rule is dropped (no PH slot) and flags truncated.
        let s = parse("Mo-Fr 08:00-18:00; PH off").unwrap();
        assert_eq!(day(&s, 0), [iv(32, 72), Interval::default()]);
        assert_eq!(s.flags & FLAG_TRUNCATED, FLAG_TRUNCATED, "PH drop flags truncated");
    }

    #[test]
    fn parse_fully_unparseable_is_none() {
        assert_eq!(parse("garbage;;"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
        // sunrise/sunset with no numeric fallback ⇒ nothing recognized.
        assert_eq!(parse("sunrise-sunset"), None);
    }

    #[test]
    fn parse_partial_keeps_recognized_flags_dropped() {
        // One good rule + one garbage rule ⇒ Some with truncated flag.
        let s = parse("Mo-Fr 08:00-18:00; xyzzy").unwrap();
        assert_eq!(day(&s, 0), [iv(32, 72), Interval::default()]);
        assert_eq!(s.flags & FLAG_TRUNCATED, FLAG_TRUNCATED);
    }

    #[test]
    fn rounding_half_to_even() {
        // 08:07 = 487 min = 32.466.. quarters → rounds down to 32 (08:00).
        assert_eq!(parse_time_q("08:07"), Some(32));
        // 08:08 = 488 min = 32.533.. → rounds up to 33 (08:15).
        assert_eq!(parse_time_q("08:08"), Some(33));
        // Exactly halfway: 00:07:30 isn't representable in HH:MM, but a quarter
        // boundary at an odd/even quarter exercises the tie rule. 7.5 min = 0.5
        // quarters → ties to even (0). 22.5 min = 1.5 quarters → ties to even (2).
        assert_eq!(round_half_even(0.5), 0, "0.5 ties to even 0");
        assert_eq!(round_half_even(1.5), 2, "1.5 ties to even 2");
        assert_eq!(round_half_even(2.5), 2, "2.5 ties to even 2");
        assert_eq!(round_half_even(3.5), 4, "3.5 ties to even 4");
        // Non-tie fractions round normally.
        assert_eq!(round_half_even(2.4), 2);
        assert_eq!(round_half_even(2.6), 3);
    }

    #[test]
    fn time_parsing_bounds() {
        assert_eq!(parse_time_q("00:00"), Some(0));
        assert_eq!(parse_time_q("24:00"), Some(96));
        assert_eq!(parse_time_q("24:01"), None, "past 24:00 invalid");
        assert_eq!(parse_time_q("25:00"), None);
        assert_eq!(parse_time_q("08:60"), None, "minute overflow");
        assert_eq!(parse_time_q("noon"), None);
    }

    #[test]
    fn encode_blob_layout() {
        // A schedule with Monday 08:00-18:00 and truncated flag encodes to the
        // documented 29-byte layout.
        let mut s = Schedule::default();
        s.days[0][0] = iv(32, 72);
        s.flags = FLAG_TRUNCATED;
        let blob = s.encode();
        assert_eq!(blob.len(), 29);
        assert_eq!(blob[0], FLAG_TRUNCATED, "flags byte");
        // Monday slot 0 = bytes [1..3].
        assert_eq!((blob[1], blob[2]), (32, 72), "Mon slot 0");
        assert_eq!((blob[3], blob[4]), (0, 0), "Mon slot 1 unused");
        // Tuesday onward all zero.
        assert!(blob[5..].iter().all(|&b| b == 0), "rest closed");
    }

    #[test]
    fn dedup_pool_collapses_identical_and_keeps_none() {
        // Three schedules: A, A (dup), B, plus a None.
        let a = parse("Mo-Fr 08:00-18:00").unwrap();
        let a_dup = parse("Mo-Fr 08:00-18:00").unwrap();
        let b = parse("24/7").unwrap();
        let items: Vec<Option<Schedule>> = vec![Some(a), Some(a_dup), Some(b), None];
        let (pool, refs) = build_hours_pool(&items, |o| o.as_ref());
        // A and A_dup collapse to pool[0]; B is pool[1]; None → None ref.
        assert_eq!(pool.len(), 2, "two unique blobs");
        assert_eq!(refs, vec![Some(0), Some(0), Some(1), None]);
        // The referenced blobs match the schedules.
        assert_eq!(pool[0], parse("Mo-Fr 08:00-18:00").unwrap().encode());
        assert_eq!(pool[1], parse("24/7").unwrap().encode());
    }

    #[test]
    fn describe_is_human_readable() {
        let s = parse("Mo-Sa 08:30-19:30; Su off").unwrap();
        let d = describe(&s);
        assert!(d.contains("Mo 08:30-19:30"), "Mon shown: {d}");
        assert!(d.contains("Su closed"), "Sun closed: {d}");
        assert!(d.contains("[flags: -]"), "no flags: {d}");
        let seasonal = parse("Apr-Oct: Mo-Su 09:00-18:00").unwrap();
        assert!(describe(&seasonal).contains("[flags: S]"), "seasonal flag shown");
    }
}
