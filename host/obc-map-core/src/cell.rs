//! Cell job options and output layout shared by independent producers.

use crate::grid::{id_width, Band, BandTable, CellId};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub struct Job {
    pub band: Band,
    pub leaf: CellId,
    pub cells: Vec<CellId>,
    pub partial: BTreeSet<String>,
}

pub struct SourceFiles {
    pub pbf: PathBuf,
    pub terrain: BTreeMap<String, PathBuf>,
}

impl SourceFiles {
    pub fn parse(layers: &BTreeMap<String, BTreeMap<String, PathBuf>>) -> Result<Self, String> {
        let files: BTreeMap<_, _> = layers.values().flatten().collect();
        let pbfs: Vec<_> = files.iter().filter(|(name, _)| name.ends_with(".osm.pbf")).map(|(_, path)| *path).collect();
        let [pbf] = pbfs.as_slice() else {
            return Err(format!("the step reads {} .osm.pbf files, not one", pbfs.len()));
        };
        let terrain = files
            .iter()
            .filter(|(name, _)| name.ends_with(".obcd"))
            .map(|(name, path)| ((*name).clone(), (*path).clone()))
            .collect();
        Ok(Self { pbf: (*pbf).clone(), terrain })
    }
}

impl Job {
    pub fn parse(options: &Value) -> Result<Self, String> {
        let band = options["band"].as_str().ok_or("option `band` is not a string")?;
        let band =
            BandTable::recommended().bands.into_iter().find(|b| b.id == band).ok_or(format!("no band `{band}`"))?;
        let numbers = |value: &Value| value.as_array()?.iter().map(Value::as_i64).collect::<Option<Vec<i64>>>();
        let leaf = match numbers(&options["leaf"]).as_deref() {
            Some(&[log2, i, j]) => u32::try_from(log2).ok().and_then(|log2| CellId::new(log2, i, j).ok()),
            _ => None,
        }
        .ok_or("option `leaf` is not [log2, i, j]")?;
        let mut cells = options["cells"]
            .as_array()
            .ok_or("option `cells` is not a list")?
            .iter()
            .map(|cell| match numbers(cell).as_deref() {
                Some(&[i, j]) => CellId::new(band.cell_log2, i, j).ok(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or("a cell is not [i, j] of the band")?;
        cells.sort_unstable();
        cells.dedup();
        let partial: BTreeSet<String> = serde_json::from_value::<Vec<String>>(options["partial_cells"].clone())
            .map_err(|e| format!("option `partial_cells`: {e}"))?
            .into_iter()
            .collect();
        let selected = cells.iter().map(ToString::to_string).collect();
        if !partial.is_subset(&selected) {
            return Err("partial cell is not selected by this step".into());
        }
        Ok(Job { band, leaf, cells, partial })
    }

    /// A fully covered empty cell has no payload; partial empty coverage keeps the actual bytes.
    pub fn finish(&self, output: &Path, files: &[(CellId, PathBuf, bool)]) -> Result<(), String> {
        let mut produced: Vec<_> = files.iter().map(|(id, _, _)| *id).collect();
        let mut requested = self.cells.clone();
        produced.sort_unstable();
        requested.sort_unstable();
        requested.dedup();
        if produced != requested {
            return Err(format!(
                "the cut wrote other cells than the cells of the step ({} written, {} asked)",
                produced.len(),
                requested.len()
            ));
        }
        std::fs::create_dir_all(output.join("cells").join(&self.band.id)).map_err(|e| e.to_string())?;
        let mut empty = Vec::new();
        for (id, file, no_content) in files {
            if *no_content && !self.partial.contains(&id.to_string()) {
                empty.push(id.to_string());
                continue;
            }
            let path = output.join(cell_path(&self.band, id));
            std::fs::create_dir_all(path.parent().expect("cell path has a parent")).map_err(|e| e.to_string())?;
            std::fs::rename(file, &path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let metadata = output.join("metadata");
        std::fs::create_dir_all(&metadata).map_err(|e| e.to_string())?;
        let path = metadata.join("empty.json");
        std::fs::write(&path, serde_json::to_string(&empty).expect("strings serialize"))
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

pub fn cell_path(band: &Band, cell: &CellId) -> String {
    let w = id_width(cell.log2);
    format!("cells/{}/{:0w$}/{:0w$}.obcm", band.id, cell.i, cell.j, w = w)
}

/// Whether a selected drawing level can carry generated contour geometry.
pub fn has_contours(config: &crate::config::Config, band: &Band) -> bool {
    let classes = [crate::config::ContourClass::Major, crate::config::ContourClass::Index];
    let min_lod = classes.into_iter().filter_map(|class| config.contour_style(class)).map(|style| style.min_lod).min();
    config.contours.enabled && min_lod.is_some_and(|min| band.lods.iter().any(|&lod| lod >= min))
}
