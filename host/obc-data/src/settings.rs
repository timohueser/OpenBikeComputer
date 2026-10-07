//! Pending Live settings and the exact settings recorded by an applied release.

use std::collections::BTreeMap;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::env::Env;
use crate::live::Live;
use crate::regions::{Area, Regions};
use crate::sources::{Refresh, Source};
use crate::store::{write_atomic, Store};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub region: String,
    /// Full TOML definitions of the selection and its union members, keyed by region id.
    pub definitions: BTreeMap<String, String>,
    pub layers: Vec<String>,
    pub refresh: BTreeMap<String, Refresh>,
}

impl Default for Settings {
    fn default() -> Self {
        let refresh = [
            (7, &["osm-replication", "geofabrik-extracts"][..]),
            (
                90,
                &[
                    "osm-planet",
                    "geofabrik-index",
                    "geofabrik-poly",
                    "land-polygons",
                    "water-polygons",
                    "wikidata",
                    "wikipedia",
                    "commons",
                    "qrank",
                ][..],
            ),
            (365, &["osm-trails", "era5-land", "modis-snow", "hr-wsi"][..]),
        ]
        .into_iter()
        .flat_map(|(days, sources)| sources.iter().map(move |id| ((*id).into(), Refresh::Days(days))))
        .collect();
        Self { region: String::new(), definitions: BTreeMap::new(), layers: Vec::new(), refresh }
    }
}

fn path(store: &Store, name: &str) -> std::path::PathBuf {
    store.root().join("settings").join(format!("{name}.json"))
}

fn read(store: &Store, name: &str) -> Result<Option<Settings>, String> {
    match std::fs::read(path(store, name)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{name} settings: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub fn current(store: &Store) -> Result<Settings, String> {
    Ok(read(store, "pending")?.or(read(store, "applied")?).unwrap_or_default())
}

pub fn save(store: &Store, settings: &Settings) -> Result<(), String> {
    write_atomic(&path(store, "pending"), &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?)
}

pub fn edited(store: &Store) -> Result<bool, String> {
    let applied = read(store, "applied")?;
    Ok(read(store, "pending")?.is_some_and(|pending| Some(pending) != applied))
}

pub fn undo(store: &Store) -> Result<Settings, String> {
    let settings = read(store, "applied")?.unwrap_or_default();
    match std::fs::remove_file(path(store, "pending")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(settings)
}

/// Cache observed settings without changing pending edits.
pub fn observe(store: &Store, live: &Live) -> Result<(), String> {
    if let Some(settings) = live
        .products
        .iter()
        .filter_map(|product| Some((product.applied.as_deref(), product.release.as_ref()?.1.settings.as_ref()?)))
        .max_by_key(|(applied, _)| *applied)
        .map(|(_, settings)| settings)
    {
        write_atomic(&path(store, "applied"), &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?)?;
    }
    Ok(())
}

pub fn applied(store: &Store, settings: &Settings) -> Result<(), String> {
    write_atomic(&path(store, "applied"), &serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?)?;
    if read(store, "pending")?.as_ref() == Some(settings) {
        undo(store)?;
    }
    Ok(())
}

impl Settings {
    pub fn select(&mut self, regions: &Regions, id: &str) -> Result<(), String> {
        fn visit(regions: &Regions, id: &str, definitions: &mut BTreeMap<String, String>) -> Result<(), String> {
            let region = regions.get(id).ok_or_else(|| format!("no region `{id}`"))?;
            definitions.insert(id.into(), region.definition()?);
            if let Area::Union { union } = &region.area {
                for member in union {
                    visit(regions, member, definitions)?;
                }
            }
            Ok(())
        }
        regions.leaves(id)?;
        regions.get(id).expect("checked selection").selectable()?;
        let mut definitions = BTreeMap::new();
        visit(regions, id, &mut definitions)?;
        self.region = id.into();
        self.definitions = definitions;
        Ok(())
    }

    pub fn regions(&self) -> Result<Regions, String> {
        Regions::new(
            self.definitions
                .iter()
                .map(|(id, text)| crate::regions::parse_region(id, text))
                .collect::<Result<_, _>>()?,
        )
    }

    pub fn env(&self) -> Result<Env, String> {
        if self.region.is_empty() {
            return Err("Live has no region: choose one with `obc data region live REGION`".into());
        }
        let regions = self.regions()?;
        regions.get(&self.region).ok_or("Live settings lack the selected region definition")?;
        Ok(Env {
            name: "live".into(),
            region: self.region.clone(),
            layers: self.layers.clone(),
            settings: Some(self.clone()),
            ..Env::default()
        })
    }

    pub fn policies(&self, sources: &mut [Source]) -> Result<(), String> {
        for source in sources {
            source.refresh = self.refresh.get(&source.id).copied().unwrap_or(source.refresh);
            if matches!(source.refresh, Refresh::Days(_)) && source.version != crate::sources::VersionScheme::Date {
                return Err(format!("source `{}` needs a date version for a refresh in days", source.id));
            }
        }
        Ok(())
    }
}

/// Shipped region presets, saved store definitions and the applied selection.
pub fn regions(root: &Path, store: &Store) -> Result<Regions, String> {
    let presets = if root.join("data/regions").is_dir() { Regions::load(root)? } else { Regions::new(Vec::new())? };
    let mut found: BTreeMap<_, _> = presets.iter().map(|region| (region.id.clone(), region.clone())).collect();
    for region in current(store)?.regions()?.iter() {
        found.entry(region.id.clone()).or_insert_with(|| region.clone());
    }
    let saved = store.root().join("regions");
    if saved.is_dir() {
        found.extend(Regions::load_dir(&saved)?.iter().map(|region| (region.id.clone(), region.clone())));
    }
    Regions::new(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[test]
    fn a_selection_captures_union_members_and_applying_keeps_newer_edits() {
        let scratch = Scratch::new("settings");
        let store = Store::at(scratch.0.join("store"));
        let metadata = "countries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n";
        let definitions = [
            ("box", "name = \"Box\"\nkind = \"box\"\nbox = [7.0, 48.0, 8.0, 49.0]\n"),
            ("area", "name = \"Area\"\nkind = \"geofabrik\"\nareas = [\"europe/germany/baden-wuerttemberg\"]\n"),
            ("both", "name = \"Both\"\nkind = \"union\"\nunion = [\"box\", \"area\"]\n"),
        ];
        let regions = Regions::new(
            definitions
                .iter()
                .map(|(id, text)| crate::regions::parse_region(id, &format!("{text}{metadata}")).unwrap())
                .collect(),
        )
        .unwrap();
        let mut reviewed = Settings::default();
        reviewed.select(&regions, "both").unwrap();
        let restored = reviewed.regions().unwrap();
        for region in regions.iter() {
            assert_eq!(restored.get(&region.id), Some(region));
        }
        save(&store, &reviewed).unwrap();
        let mut newer = reviewed.clone();
        newer.refresh.insert("wikidata".into(), Refresh::Days(30));
        save(&store, &newer).unwrap();
        applied(&store, &reviewed).unwrap();
        assert_eq!(current(&store).unwrap(), newer);
        assert!(edited(&store).unwrap());
        assert_eq!(undo(&store).unwrap(), reviewed);
        save(&store, &newer).unwrap();
        applied(&store, &newer).unwrap();
        assert_eq!(current(&store).unwrap(), newer);
        assert!(!edited(&store).unwrap());
    }
}
