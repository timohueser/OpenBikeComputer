//! The settings of a target: its region, the optional layers that are on and the refresh policy of
//! each source that differs from `data/sources.toml`. The release of a target holds them, so the
//! next update of the target starts from them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::regions::{Area, Regions};
use crate::sources::{Refresh, Source};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The id of the region; empty until an update names one.
    pub region: String,
    /// Full TOML definitions of the region and its union members, keyed by region id.
    pub definitions: BTreeMap<String, String>,
    /// The optional layers that are on, sorted.
    pub layers: Vec<String>,
    /// Source id to the policy that replaces the policy of `data/sources.toml`.
    pub refresh: BTreeMap<String, Refresh>,
}

impl Settings {
    /// Select the region `id` of `regions`: its definition and those of its union members.
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
        let mut definitions = BTreeMap::new();
        visit(regions, id, &mut definitions)?;
        self.region = id.into();
        self.definitions = definitions;
        Ok(())
    }

    /// The regions of the definitions.
    pub fn regions(&self) -> Result<Regions, String> {
        Regions::new(
            self.definitions
                .iter()
                .map(|(id, text)| crate::regions::parse_region(id, text))
                .collect::<Result<_, _>>()?,
        )
    }

    /// Give each of `sources` the policy of these settings, where they name one.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_copies_the_definitions_of_its_union_members() {
        let definitions = [
            ("box", "name = \"Box\"\nkind = \"box\"\nbox = [7.0, 48.0, 8.0, 49.0]\n"),
            ("area", "name = \"Area\"\nkind = \"geofabrik\"\nareas = [\"europe/germany/baden-wuerttemberg\"]\n"),
            ("both", "name = \"Both\"\nkind = \"union\"\nunion = [\"box\", \"area\"]\n"),
        ];
        let regions = Regions::new(
            definitions.iter().map(|(id, text)| crate::regions::parse_region(id, text).unwrap()).collect(),
        )
        .unwrap();
        let mut settings = Settings::default();
        settings.select(&regions, "both").unwrap();
        let copied = settings.regions().unwrap();
        for region in regions.iter() {
            assert_eq!(copied.get(&region.id), Some(region));
        }
        assert!(settings.select(&regions, "absent").is_err());
        assert_eq!(settings.region, "both", "a refused selection changes nothing");
    }
}
