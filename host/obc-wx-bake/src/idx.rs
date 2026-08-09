//! wgrib2-style `.idx` selection: the byte-range fast path into NOAA's multi-hundred-megabyte
//! GRIB objects.
//!
//! A NOAA HRRR subhourly object is ~200 MB and a GFS 0.25-degree object ~500 MB, of which the
//! baker needs one 30-600 KB message. Every object is published beside a text index of
//! `record:offset:date:parameter:level:interval:` lines, so the exact message range is
//! computable without downloading anything but the index. This is the mechanism WX1 pinned
//! (AWS NODD only; NOMADS is deliberately never contacted).
//!
//! Fail-closed, verbatim from the WX1 decision record: offsets must be strictly increasing and
//! inside the object, the selector must match a contracted number of *consecutive* records, and
//! the index text is never accepted as temporal identity — the caller re-derives valid times
//! from the decoded GRIB bytes.

/// An inclusive HTTP byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

impl ByteRange {
    /// Byte count of the range. `resolve` never produces an empty one.
    pub fn len(self) -> u64 {
        self.end_inclusive.saturating_sub(self.start) + 1
    }

    pub fn is_empty(self) -> bool {
        self.end_inclusive < self.start
    }
}

/// Largest `.idx` document the baker will read: GFS's is ~41 KB, HRRR's ~12 KB.
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;

/// Resolve `needle` to the byte range spanning its matching records.
///
/// `accepted_matches` lists the record counts the source contract allows — `&[1]` for a unique
/// record, `&[1, 2]` for GFS's deliberately duplicated `APCP` entries (fetched as one span and
/// proven identical after decode). `object_len` bounds the final record, which has no successor
/// offset to end it.
pub fn resolve(
    index: &str,
    needle: &str,
    object_len: u64,
    accepted_matches: &[usize],
) -> Result<(ByteRange, usize), String> {
    if needle.is_empty() || object_len == 0 || accepted_matches.is_empty() {
        return Err("idx selector, object length and accepted match counts must be non-empty".into());
    }
    let mut entries: Vec<(u64, &str)> = Vec::new();
    for (number, line) in index.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(3, ':');
        let _record = fields.next();
        let offset = fields
            .next()
            .ok_or_else(|| format!("idx line {} has no offset field", number + 1))?
            .trim()
            .parse::<u64>()
            .map_err(|error| format!("idx line {}: {error}", number + 1))?;
        entries.push((offset, line));
    }
    if entries.is_empty() {
        return Err("idx is empty".into());
    }
    if entries.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err("idx offsets are not strictly increasing".into());
    }
    if entries.last().expect("not empty").0 >= object_len {
        return Err("idx offset lies beyond the object".into());
    }

    let matches: Vec<usize> =
        entries.iter().enumerate().filter(|(_, (_, line))| line.contains(needle)).map(|(index, _)| index).collect();
    if !accepted_matches.contains(&matches.len()) {
        return Err(format!("idx selector {needle:?} matched {} records, outside the contract", matches.len()));
    }
    if matches.windows(2).any(|pair| pair[1] != pair[0] + 1) {
        return Err(format!("idx records matching {needle:?} are not consecutive"));
    }
    let first = matches[0];
    let last = *matches.last().expect("accepted counts are nonzero");
    let start = entries[first].0;
    let end = entries.get(last + 1).map_or(object_len, |entry| entry.0);
    if end <= start {
        return Err("idx selection resolves to an empty range".into());
    }
    Ok((ByteRange { start, end_inclusive: end - 1 }, matches.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = "1:0:d=2026080912:PRATE:surface:15 min fcst:\n\
                         2:100:d=2026080912:APCP:surface:0-1 hour acc fcst:\n\
                         3:250:d=2026080912:APCP:surface:0-1 hour acc fcst:\n\
                         4:400:d=2026080912:ACPCP:surface:0-1 hour acc fcst:\n";

    #[test]
    fn a_unique_record_resolves_to_its_own_span() {
        let (range, matched) = resolve(INDEX, ":PRATE:surface:15 min fcst:", 1_000, &[1]).unwrap();
        assert_eq!(matched, 1);
        assert_eq!(range, ByteRange { start: 0, end_inclusive: 99 });
        assert_eq!(range.len(), 100);
    }

    #[test]
    fn a_duplicated_record_resolves_to_one_consecutive_span() {
        let (range, matched) = resolve(INDEX, ":APCP:surface:0-1 hour acc fcst:", 1_000, &[1, 2]).unwrap();
        assert_eq!(matched, 2);
        assert_eq!(range, ByteRange { start: 100, end_inclusive: 399 });
    }

    #[test]
    fn the_final_record_is_bounded_by_the_object_length() {
        let (range, _) = resolve(INDEX, ":ACPCP:", 1_000, &[1]).unwrap();
        assert_eq!(range, ByteRange { start: 400, end_inclusive: 999 });
    }

    #[test]
    fn ambiguity_and_corruption_fail_closed() {
        // An unexpected duplicate is never silently disambiguated.
        assert!(resolve(INDEX, ":APCP:surface:0-1 hour acc fcst:", 1_000, &[1]).is_err());
        // No match at all.
        assert!(resolve(INDEX, ":TMP:surface:", 1_000, &[1]).is_err());
        // Offsets outside the object, or not increasing.
        assert!(resolve(INDEX, ":ACPCP:", 400, &[1]).is_err());
        let scrambled = "1:500:d=x:A:\n2:100:d=x:B:\n";
        assert!(resolve(scrambled, ":B:", 1_000, &[1]).is_err());
        // A non-numeric offset is a schema surprise, not a zero.
        assert!(resolve("1:abc:d=x:A:\n", ":A:", 1_000, &[1]).is_err());
        assert!(resolve("", ":A:", 1_000, &[1]).is_err());
    }

    #[test]
    fn non_consecutive_matches_fail() {
        let split = "1:0:d=x:APCP:surface:0-1 hour acc fcst:\n\
                     2:100:d=x:TMP:surface:\n\
                     3:200:d=x:APCP:surface:0-1 hour acc fcst:\n";
        assert!(resolve(split, ":APCP:surface:0-1 hour acc fcst:", 1_000, &[2]).is_err());
    }
}
