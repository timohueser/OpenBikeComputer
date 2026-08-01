//! Mutable local state behind OBCC's compact known-empty row ranges.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use obc_pack::catalog::{CellSource, KnownEmptyRun};
use obc_pack::grid::{BandTable, CellId};
use serde::{Deserialize, Serialize};

const STATE_FILE: &str = ".known-empty.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmptyFact {
    pub(crate) built_at: String,
    pub(crate) sources: Vec<CellSource>,
}

#[derive(Clone, Debug)]
pub(crate) struct EmptyChange {
    pub(crate) id: CellId,
    pub(crate) fact: Option<EmptyFact>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownEmptyState {
    schema_revision: u32,
    band: String,
    known_empty: Vec<KnownEmptyRun>,
}

#[derive(Default)]
pub(crate) struct KnownEmptyIndex {
    by_band: BTreeMap<String, Vec<KnownEmptyRun>>,
}

impl KnownEmptyIndex {
    pub(crate) fn load(out: &Path, bands: &BandTable, revision: u32) -> Result<Self, String> {
        let mut by_band = BTreeMap::new();
        for band in &bands.bands {
            let path = out.join("cells").join(&band.id).join(STATE_FILE);
            if !path.is_file() {
                by_band.insert(band.id.clone(), Vec::new());
                continue;
            }
            let state: KnownEmptyState =
                serde_json::from_str(&std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            if state.schema_revision != revision || state.band != band.id {
                return Err(format!("{}: known-empty state does not match schema revision/band", path.display()));
            }
            by_band.insert(band.id.clone(), state.known_empty);
        }
        Ok(Self { by_band })
    }

    pub(crate) fn fact(&self, band: &str, id: CellId) -> Option<&KnownEmptyRun> {
        let runs = self.by_band.get(band)?;
        let position =
            runs.partition_point(|run| CellId::parse(&run.start).is_ok_and(|start| (start.i, start.j) <= (id.i, id.j)));
        let run = position.checked_sub(1).and_then(|index| runs.get(index))?;
        let start = CellId::parse(&run.start).ok()?;
        let end = CellId::parse(&run.end).ok()?;
        (start.i == id.i && start.j <= id.j && end.j >= id.j).then_some(run)
    }

    pub(crate) fn apply(&mut self, band: &str, changes: &[EmptyChange]) -> Result<(), String> {
        let runs = self.by_band.get_mut(band).ok_or_else(|| format!("unknown band `{band}`"))?;
        let mut by_row: BTreeMap<i64, BTreeMap<i64, (u32, Option<EmptyFact>)>> = BTreeMap::new();
        for change in changes {
            by_row.entry(change.id.i).or_default().insert(change.id.j, (change.id.log2, change.fact.clone()));
        }
        let mut survivors = Vec::new();
        for run in runs.iter() {
            let start = CellId::parse(&run.start)?;
            let end = CellId::parse(&run.end)?;
            let Some(row_changes) = by_row.get(&start.i) else {
                survivors.push(run.clone());
                continue;
            };
            let mut cursor = start.j;
            for (&j, _) in row_changes.range(start.j..=end.j) {
                if cursor < j {
                    survivors.push(run_from(
                        start.log2,
                        start.i,
                        cursor,
                        j - 1,
                        run.built_at.clone(),
                        run.sources.clone(),
                    )?);
                }
                cursor = j + 1;
            }
            if cursor <= end.j {
                survivors.push(run_from(
                    start.log2,
                    start.i,
                    cursor,
                    end.j,
                    run.built_at.clone(),
                    run.sources.clone(),
                )?);
            }
        }
        let mut additions = Vec::new();
        for (&i, row) in &by_row {
            for (&j, (log2, fact)) in row {
                if let Some(fact) = fact {
                    additions.push(run_from(*log2, i, j, j, fact.built_at.clone(), fact.sources.clone())?);
                }
            }
        }

        let mut survivors = survivors.into_iter().peekable();
        let mut additions = additions.into_iter().peekable();
        let mut ordered = Vec::with_capacity(survivors.len() + additions.len());
        while survivors.peek().is_some() || additions.peek().is_some() {
            let take_survivor = match (survivors.peek(), additions.peek()) {
                (Some(left), Some(right)) => left.start <= right.start,
                (Some(_), None) => true,
                _ => false,
            };
            ordered.push(if take_survivor {
                survivors.next().expect("peeked survivor")
            } else {
                additions.next().expect("peeked addition")
            });
        }
        let mut merged: Vec<KnownEmptyRun> = Vec::new();
        for run in ordered {
            if let Some(previous) = merged.last_mut() {
                let prev_end = CellId::parse(&previous.end)?;
                let start = CellId::parse(&run.start)?;
                if prev_end.i == start.i
                    && prev_end.j + 1 == start.j
                    && previous.built_at == run.built_at
                    && previous.sources == run.sources
                {
                    previous.end = run.end;
                    continue;
                }
            }
            merged.push(run);
        }
        *runs = merged;
        Ok(())
    }

    pub(crate) fn clear_cells(&mut self, cells: &BTreeMap<String, BTreeSet<CellId>>) -> Result<(), String> {
        for (band, ids) in cells {
            let changes: Vec<EmptyChange> = ids.iter().map(|id| EmptyChange { id: *id, fact: None }).collect();
            self.apply(band, &changes)?;
        }
        Ok(())
    }

    pub(crate) fn write_all(&self, out: &Path, revision: u32) -> Result<(), String> {
        for (band, known_empty) in &self.by_band {
            let path = out.join("cells").join(band).join(STATE_FILE);
            write_json(
                &path,
                &KnownEmptyState { schema_revision: revision, band: band.clone(), known_empty: known_empty.clone() },
            )?;
        }
        Ok(())
    }
}

fn run_from(
    log2: u32,
    i: i64,
    j0: i64,
    j1: i64,
    built_at: String,
    sources: Vec<CellSource>,
) -> Result<KnownEmptyRun, String> {
    Ok(KnownEmptyRun {
        start: CellId::new(log2, i, j0)?.to_string(),
        end: CellId::new(log2, i, j1)?.to_string(),
        built_at,
        sources,
    })
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(value).map_err(|e| format!("{}: {e}", path.display()))?;
    text.push('\n');
    let tmp = path.with_extension("json.part");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{} -> {}: {e}", tmp.display(), path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_split_remove_and_merge_row_ranges() {
        let mut index = KnownEmptyIndex { by_band: BTreeMap::from([("fine".into(), Vec::new())]) };
        let fact = EmptyFact {
            built_at: "2026-08-01T00:00:00Z".into(),
            sources: vec![CellSource { extract_id: "planet".into(), snapshot: "2026-08-01".into() }],
        };
        let id = |j| CellId::new(18, 1000, j).unwrap();
        index
            .apply(
                "fine",
                &(1000..=1004).map(|j| EmptyChange { id: id(j), fact: Some(fact.clone()) }).collect::<Vec<_>>(),
            )
            .unwrap();
        assert_eq!(index.by_band["fine"].len(), 1);
        index.apply("fine", &[EmptyChange { id: id(1002), fact: None }]).unwrap();
        assert_eq!(index.by_band["fine"].len(), 2, "removing the middle splits the range");
        index.apply("fine", &[EmptyChange { id: id(1002), fact: Some(fact) }]).unwrap();
        assert_eq!(index.by_band["fine"].len(), 1, "restoring identical provenance merges it again");
    }
}
