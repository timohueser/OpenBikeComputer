//! Typed lookup over published artifacts and validated known-empty row ranges.

use std::collections::BTreeMap;

use super::validate::parse_strict_id;

/// An artifact, a verified-empty square, or no published coverage.
pub(super) enum IndexedCoverage<'a, T> {
    Artifact(&'a T),
    KnownEmpty,
}

/// The lookup mechanics shared by cell bands and terrain after each owner has
/// validated its domain-specific documents, revisions, provenance, and ordering.
pub(super) struct CoverageIndex<'a, T> {
    artifacts: BTreeMap<&'a str, &'a T>,
    empty_by_row: BTreeMap<i64, Vec<(i64, i64)>>,
}

impl<'a, T> CoverageIndex<'a, T> {
    pub(super) fn new<'r>(
        artifacts: impl IntoIterator<Item = (&'a str, &'a T)>,
        known_empty: impl IntoIterator<Item = (&'r str, &'r str)>,
    ) -> Result<Self, String> {
        let mut empty_by_row: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
        for (start, end) in known_empty {
            let start = parse_strict_id(start)?;
            let end = parse_strict_id(end)?;
            empty_by_row.entry(start.i).or_default().push((start.j, end.j));
        }
        Ok(Self { artifacts: artifacts.into_iter().collect(), empty_by_row })
    }

    pub(super) fn get(&self, id: &str) -> Result<Option<IndexedCoverage<'_, T>>, String> {
        if let Some(artifact) = self.artifacts.get(id) {
            return Ok(Some(IndexedCoverage::Artifact(artifact)));
        }
        let cell = parse_strict_id(id)?;
        let Some(runs) = self.empty_by_row.get(&cell.i) else { return Ok(None) };
        let at = runs.partition_point(|(_, end)| *end < cell.j);
        Ok(runs.get(at).filter(|(start, end)| *start <= cell.j && cell.j <= *end).map(|_| IndexedCoverage::KnownEmpty))
    }
}

/// The cells an inclusive-row-run list covers.
pub(super) fn inclusive_run_count<'a>(runs: impl Iterator<Item = (&'a str, &'a str)>) -> Result<u32, String> {
    let mut total = 0u32;
    for (start, end) in runs {
        let start = parse_strict_id(start)?;
        let end = parse_strict_id(end)?;
        let width = end
            .j
            .checked_sub(start.j)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("known-empty run overflow")?;
        total = total.checked_add(width).ok_or("known-empty cell count exceeds u32")?;
    }
    Ok(total)
}
