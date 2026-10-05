//! The regions the bakery bakes: every Geofabrik region in `data/regions/`.
//!
//! A region's `id` does double duty: it is the catalog's `region_id` and the Geofabrik path the
//! extract is downloaded from. One string rather than two, because the two can only ever disagree
//! by mistake.

use std::path::{Path, PathBuf};

use obc_data::regions::{Area, Regions};

/// One Geofabrik region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Slash-separated Geofabrik path, e.g. `europe/germany/bayern`. Also the catalog's `region_id`
    /// and the selection-metadata path in the bake tree.
    pub id: String,
    /// Human-readable name, recorded verbatim in the region document.
    pub name: String,
}

impl Region {
    /// Directory-path segments below `regions/` in the bake tree.
    pub fn segments(&self) -> Vec<&str> {
        self.id.split('/').collect()
    }

    /// The file name of the extract in a flat directory of extracts: the id flattened, so
    /// `europe/germany/bayern` and a hypothetical `europe/bayern` cannot collide.
    pub fn cache_name(&self) -> String {
        format!("{}-latest.osm.pbf", self.id.replace('/', "_"))
    }

    /// Cache filename for that polygon, flattened like [`Region::cache_name`].
    pub fn poly_cache_name(&self) -> String {
        format!("{}.poly", self.id.replace('/', "_"))
    }
}

/// The Geofabrik regions in `dir`, a directory laid out like `data/regions/`, or in the
/// `data/regions/` of the repository above the current directory. A box, polygon or union region
/// has no Geofabrik extract, so the bakery leaves it out.
pub fn load(dir: Option<&Path>) -> Result<Vec<Region>, String> {
    let dir = match dir {
        Some(dir) => dir.to_path_buf(),
        None => repository_regions()?,
    };
    let regions = Regions::load_dir(&dir)?;
    let list: Vec<Region> = regions
        .iter()
        .filter(|r| r.area == Area::Geofabrik)
        .map(|r| Region { id: r.id.clone(), name: r.name.clone() })
        .collect();
    if list.is_empty() {
        return Err(format!("{}: no Geofabrik region — nothing to bake", dir.display()));
    }
    Ok(list)
}

fn repository_regions() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = obc_data::find_root(&cwd)
        .ok_or("no data/sources.toml above the current directory: run inside the repository or pass --regions DIR")?;
    Ok(root.join("data/regions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_list_is_the_curated_dach_coverage() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/regions");
        let regions = load(Some(&root)).expect("the checked-in regions load");
        let ids: Vec<&str> = regions.iter().map(|r| r.id.as_str()).collect();

        assert!(ids.contains(&"europe/germany"));
        assert!(ids.contains(&"europe/austria"));
        assert!(ids.contains(&"europe/switzerland"));
        assert!(!ids.contains(&"grimsel"), "a box region has no Geofabrik extract");

        assert!(ids.contains(&"europe/germany/baden-wuerttemberg/freiburg-regbez"));
        let regbez =
            ids.iter().filter(|id| id.strip_prefix("europe/germany/").is_some_and(|rest| rest.contains('/'))).count();
        assert_eq!(regbez, 16, "4 BW + 7 Bayern + 5 NRW Regierungsbezirke");
        let laender =
            ids.iter().filter_map(|id| id.strip_prefix("europe/germany/")).filter(|rest| !rest.contains('/')).count();
        assert_eq!(laender, 16, "all sixteen Bundesländer");
    }

    #[test]
    fn a_flat_extract_name_is_the_flattened_id_plus_latest() {
        let r = Region { id: "europe/germany/bayern".into(), name: "Bayern".into() };
        assert_eq!(r.cache_name(), "europe_germany_bayern-latest.osm.pbf");
    }
}
