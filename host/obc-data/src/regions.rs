//! `data/regions/`: one file per region, `<id>.toml`. A region is a Geofabrik area, a box, a
//! polygon file or a union of other regions.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Degrees, longitude first.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Bbox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Bbox {
    fn new([west, south, east, north]: [f64; 4]) -> Result<Self, String> {
        let lon = -180.0..=180.0;
        let lat = -90.0..=90.0;
        if lon.contains(&west)
            && lon.contains(&east)
            && lat.contains(&south)
            && lat.contains(&north)
            && west < east
            && south < north
        {
            Ok(Self { west, south, east, north })
        } else {
            Err("`box` is [west, south, east, north] in degrees, longitude first".into())
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            west: self.west.min(other.west),
            south: self.south.min(other.south),
            east: self.east.max(other.east),
            north: self.north.max(other.north),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Area {
    /// The Geofabrik area whose path is the region id.
    Geofabrik,
    Box {
        #[serde(rename = "box")]
        bbox: Bbox,
    },
    /// An Osmosis `.poly` file, relative to the region file.
    Polygon {
        polygon: String,
    },
    Union {
        union: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Region {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub area: Area,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionFile {
    name: String,
    kind: AreaKind,
    #[serde(rename = "box")]
    bbox: Option<[f64; 4]>,
    polygon: Option<String>,
    union: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum AreaKind {
    Geofabrik,
    Box,
    Polygon,
    Union,
}

/// Parse one region file. `id` is its path below `data/regions/` without `.toml`.
pub fn parse_region(id: &str, text: &str) -> Result<Region, String> {
    let fail = |why: String| format!("region `{id}`: {why}");
    if !id.split('/').all(crate::is_kebab) {
        return Err(fail("each part of the id is lowercase kebab-case".into()));
    }
    let file: RegionFile = toml::from_str(text).map_err(|e| fail(e.to_string()))?;
    if file.name.trim().is_empty() {
        return Err(fail("`name` is empty".into()));
    }
    let area = match (file.kind, file.bbox, file.polygon, file.union) {
        (AreaKind::Geofabrik, None, None, None) => Area::Geofabrik,
        (AreaKind::Box, Some(bbox), None, None) => Area::Box { bbox: Bbox::new(bbox).map_err(fail)? },
        (AreaKind::Polygon, None, Some(polygon), None) => Area::Polygon { polygon },
        (AreaKind::Union, None, None, Some(union)) if union.len() >= 2 => Area::Union { union },
        _ => {
            return Err(fail(
                "a region has the one key its kind names: `box`, `polygon` or `union` (two or more ids)".into(),
            ))
        }
    };
    Ok(Region { id: id.into(), name: file.name, area })
}

/// Every region, by id.
pub struct Regions(BTreeMap<String, Region>);

impl Regions {
    /// Check that every union member exists and no union contains itself.
    pub fn new(list: Vec<Region>) -> Result<Self, String> {
        let regions = Self(list.into_iter().map(|r| (r.id.clone(), r)).collect());
        for id in regions.0.keys() {
            regions.leaves(id)?;
        }
        Ok(regions)
    }

    /// Read `data/regions/` below `root`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let dir = root.join("data/regions");
        let mut files = Vec::new();
        collect(&dir, &mut files)?;
        let mut list = Vec::new();
        for path in files {
            let relative = path.strip_prefix(&dir).expect("collected below dir").with_extension("");
            let id = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let region = parse_region(&id, &text)?;
            if let Area::Polygon { polygon } = &region.area {
                if !path.parent().expect("a file has a parent").join(polygon).is_file() {
                    return Err(format!("region `{id}`: polygon file `{polygon}` is missing"));
                }
            }
            list.push(region);
        }
        Self::new(list)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Region> {
        self.0.values()
    }

    pub fn get(&self, id: &str) -> Option<&Region> {
        self.0.get(id)
    }

    /// The regions that are not unions, which a union of `id` resolves to, sorted.
    pub fn leaves(&self, id: &str) -> Result<Vec<&str>, String> {
        let mut leaves = Vec::new();
        self.walk(id, &mut Vec::new(), &mut leaves)?;
        leaves.sort_unstable();
        leaves.dedup();
        Ok(leaves)
    }

    fn walk<'a>(&'a self, id: &str, path: &mut Vec<String>, leaves: &mut Vec<&'a str>) -> Result<(), String> {
        let region = self.0.get(id).ok_or_else(|| match path.last() {
            Some(parent) => format!("region `{parent}`: member `{id}` does not exist"),
            None => format!("no region `{id}`"),
        })?;
        if path.iter().any(|seen| seen == id) {
            return Err(format!("region `{id}` contains itself: {} → {id}", path.join(" → ")));
        }
        match &region.area {
            Area::Union { union } => {
                path.push(id.into());
                for member in union {
                    self.walk(member, path, leaves)?;
                }
                path.pop();
            }
            _ => leaves.push(&region.id),
        }
        Ok(())
    }

    /// The box around `id` when every part of it is a box. A Geofabrik area or a polygon has
    /// no box until its outline is fetched.
    pub fn bounds(&self, id: &str) -> Option<Bbox> {
        let leaves = self.leaves(id).ok()?;
        let boxes = leaves.iter().map(|leaf| match self.0.get(*leaf)?.area {
            Area::Box { bbox } => Some(bbox),
            _ => None,
        });
        boxes.collect::<Option<Vec<_>>>()?.into_iter().reduce(Bbox::union)
    }
}

fn collect(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: &str, text: &str) -> Region {
        parse_region(id, text).unwrap()
    }

    fn boxed(id: &str, bbox: &str) -> Region {
        region(id, &format!("name = \"{id}\"\nkind = \"box\"\nbox = {bbox}\n"))
    }

    fn union(id: &str, members: &str) -> Region {
        region(id, &format!("name = \"{id}\"\nkind = \"union\"\nunion = {members}\n"))
    }

    #[test]
    fn a_box_reads_west_south_east_north_and_refuses_out_of_range_or_inverted_edges() {
        let grimsel = boxed("grimsel", "[8.15, 46.48, 8.46, 46.72]");
        let bbox = Bbox { west: 8.15, south: 46.48, east: 8.46, north: 46.72 };
        assert_eq!(grimsel.area, Area::Box { bbox });
        for bad in
            ["[8.0, 95.0, 9.0, 96.0]", "[9.0, 46.0, 8.0, 47.0]", "[8.0, 47.0, 9.0, 46.0]", "[170.0, 0.0, -170.0, 1.0]"]
        {
            let err = parse_region("x", &format!("name = \"x\"\nkind = \"box\"\nbox = {bad}\n")).unwrap_err();
            assert!(err.contains("longitude first"), "{bad}: {err}");
        }
    }

    #[test]
    fn an_unknown_field_or_a_missing_key_is_rejected() {
        assert!(parse_region("x", "name = \"x\"\nkind = \"geofabrik\"\nbounds = [1, 2, 3, 4]\n")
            .unwrap_err()
            .contains("bounds"));
        assert!(parse_region("x", "name = \"x\"\nkind = \"box\"\n").is_err());
        assert!(parse_region("Europe/x", "name = \"x\"\nkind = \"geofabrik\"\n").is_err());
    }

    #[test]
    fn a_union_resolves_to_its_leaves_and_their_box() {
        let regions = Regions::new(vec![
            region("europe/germany", "name = \"Germany\"\nkind = \"geofabrik\"\n"),
            boxed("a", "[1.0, 1.0, 2.0, 2.0]"),
            boxed("b", "[3.0, 0.0, 4.0, 1.5]"),
            union("ab", "[\"a\", \"b\"]"),
            union("all", "[\"ab\", \"a\", \"europe/germany\"]"),
        ])
        .unwrap();
        assert_eq!(regions.leaves("all").unwrap(), ["a", "b", "europe/germany"]);
        assert_eq!(regions.bounds("ab"), Some(Bbox { west: 1.0, south: 0.0, east: 4.0, north: 2.0 }));
        assert_eq!(regions.bounds("all"), None);
    }

    #[test]
    fn a_union_with_a_missing_member_or_a_cycle_is_rejected() {
        let missing = Regions::new(vec![boxed("a", "[1, 1, 2, 2]"), union("u", "[\"a\", \"gone\"]")]).err().unwrap();
        assert!(missing.contains("member `gone` does not exist"), "{missing}");
        let cycle = Regions::new(vec![union("u", "[\"v\", \"u\"]"), union("v", "[\"u\", \"u\"]")]).err().unwrap();
        assert!(cycle.contains("contains itself"), "{cycle}");
    }

    #[test]
    fn the_checked_in_regions_load() {
        let regions = Regions::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
        let dach = regions.leaves("dach").unwrap();
        assert_eq!(dach, ["europe/austria", "europe/germany", "europe/switzerland"]);
        assert_eq!(regions.get("europe/germany/baden-wuerttemberg").unwrap().area, Area::Geofabrik);
    }
}
