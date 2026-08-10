//! The curated region list: what the bakery bakes.
//!
//! The list itself is [`regions.toml`](../regions.toml), compiled into the binary
//! with `include_str!` so an `obc-bake` copied onto a build box carries the list it
//! is supposed to bake, and overridable with `--regions <file>` (which is how the
//! tests get a two-region list without touching the real one).
//!
//! A region's `id` does double duty: it is the catalog's `region_id`
//! (`OBCC_Spec.md` §6) *and* the Geofabrik path the extract is downloaded from. One
//! string rather than two because the two can only ever disagree by mistake — a
//! separate `geofabrik = …` field would let a region be published under an id whose
//! extract came from somewhere else, which is exactly the confusion the manifest
//! exists to prevent.

use serde::Deserialize;

/// The list as it is checked in.
pub const BUILTIN_REGIONS_TOML: &str = include_str!("../regions.toml");

/// One curated region.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Region {
    /// Slash-separated Geofabrik path, e.g. `europe/germany/bayern`. Also the
    /// catalog's `region_id` and the selection-metadata path in the bake tree.
    pub id: String,
    /// Human-readable name, recorded verbatim in the region document.
    pub name: String,
}

impl Region {
    /// Directory-path segments below `regions/` in the bake tree.
    pub fn segments(&self) -> Vec<&str> {
        self.id.split('/').collect()
    }

    /// The extract URL under `base` (`https://download.geofabrik.de`, or a local
    /// directory / `file://` root in tests).
    pub fn extract_url(&self, base: &str) -> String {
        format!("{}/{}-latest.osm.pbf", base.trim_end_matches('/'), self.id)
    }

    /// Cache filename for the downloaded extract: the id flattened, so
    /// `europe/germany/bayern` and a hypothetical `europe/bayern` cannot collide.
    pub fn cache_name(&self) -> String {
        format!("{}-latest.osm.pbf", self.id.replace('/', "_"))
    }

    /// The region's Osmosis polygon under `base` — Geofabrik serves it beside the
    /// extract, at the same path with a `.poly` extension.
    pub fn poly_url(&self, base: &str) -> String {
        format!("{}/{}.poly", base.trim_end_matches('/'), self.id)
    }

    /// Cache filename for that polygon, flattened like [`Region::cache_name`].
    pub fn poly_cache_name(&self) -> String {
        format!("{}.poly", self.id.replace('/', "_"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionsDoc {
    regions: Vec<Region>,
}

/// Parse and validate a region list.
///
/// Validation is deliberately strict and happens before any byte is downloaded: an
/// id the catalog generator would later reject (§8's lowercase-kebab path segments)
/// must fail in a second, not four hours into a bake.
pub fn parse(toml_text: &str) -> Result<Vec<Region>, String> {
    let doc: RegionsDoc = toml::from_str(toml_text).map_err(|e| format!("region list: {e}"))?;
    if doc.regions.is_empty() {
        return Err("region list: `regions` is empty — nothing to bake".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for r in &doc.regions {
        validate_id(&r.id)?;
        if r.name.trim().is_empty() {
            return Err(format!("region `{}`: name is empty", r.id));
        }
        if !seen.insert(r.id.as_str()) {
            return Err(format!("region `{}` is listed twice", r.id));
        }
    }
    Ok(doc.regions)
}

/// Load the built-in list, or one from a file.
pub fn load(path: Option<&std::path::Path>) -> Result<Vec<Region>, String> {
    match path {
        None => parse(BUILTIN_REGIONS_TOML),
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            parse(&text).map_err(|e| format!("{}: {e}", p.display()))
        }
    }
}

/// The id rules of `OBCC_Spec.md` §6/§8: slash-separated lowercase kebab-case
/// segments. The catalog generator enforces the same rules on the tree it walks;
/// checking here means the failure names the region list line, not a directory.
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("region id is empty".into());
    }
    for segment in id.split('/') {
        let ok = !segment.is_empty()
            && !segment.starts_with('-')
            && !segment.ends_with('-')
            && !segment.contains("--")
            && segment.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !ok {
            return Err(format!("region id `{id}`: segment `{segment}` is not lowercase kebab-case"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checked_in_list_is_the_curated_dach_coverage() {
        let regions = parse(BUILTIN_REGIONS_TOML).expect("the shipped region list parses");
        let ids: Vec<&str> = regions.iter().map(|r| r.id.as_str()).collect();
        // Germany + its sixteen Bundesländer + Austria + Switzerland (#898), plus
        // every Regierungsbezirk Geofabrik offers a boundary for: Baden-Württemberg's
        // four, Bayern's seven, Nordrhein-Westfalen's five. (Austria and Switzerland
        // have no Geofabrik subdivisions.) Regierungsbezirke are selection-only under
        // the maximal-source rule, so a new line costs a `.poly` fetch and a catalog
        // entry, never an extract. The count is pinned so a dropped line is a failed
        // test rather than a region that silently stops being offered.
        //
        // Plus exactly one non-DACH entry: Iowa, the basemap the weather event packs in
        // `host/obc-wx-bake/tests/events/` are rendered over. It is pinned by name and
        // by count for the same reason the rest are — and so that a second US state
        // arriving quietly fails here, which is the whole of the convention.
        assert_eq!(ids.len(), 36, "DACH's 35 plus Iowa for the weather event packs: {ids:?}");
        assert!(ids.contains(&"europe/germany"));
        assert!(ids.contains(&"europe/austria"));
        assert!(ids.contains(&"europe/switzerland"));
        assert_eq!(
            ids.iter().filter(|id| !id.starts_with("europe/")).copied().collect::<Vec<_>>(),
            vec!["north-america/us/iowa"],
            "the weather packs' basemap is the only non-European region"
        );
        assert!(ids.contains(&"europe/germany/baden-wuerttemberg/freiburg-regbez"));
        let regbez =
            ids.iter().filter(|id| id.strip_prefix("europe/germany/").is_some_and(|rest| rest.contains('/'))).count();
        assert_eq!(regbez, 16, "4 BW + 7 Bayern + 5 NRW Regierungsbezirke");
        let laender =
            ids.iter().filter_map(|id| id.strip_prefix("europe/germany/")).filter(|rest| !rest.contains('/')).count();
        assert_eq!(laender, 16, "all sixteen Bundesländer");
    }

    #[test]
    fn an_extract_url_is_the_id_plus_latest() {
        let r = Region { id: "europe/germany/bayern".into(), name: "Bayern".into() };
        assert_eq!(
            r.extract_url("https://download.geofabrik.de/"),
            "https://download.geofabrik.de/europe/germany/bayern-latest.osm.pbf"
        );
        assert_eq!(r.cache_name(), "europe_germany_bayern-latest.osm.pbf");
    }

    #[test]
    fn ids_the_catalog_would_reject_are_rejected_here() {
        for bad in ["Europe/Germany", "europe//germany", "europe/germany_bayern", "europe/-bayern", ""] {
            let toml = format!("regions = [ {{ id = \"{bad}\", name = \"x\" }} ]");
            assert!(parse(&toml).is_err(), "`{bad}` must not be accepted");
        }
    }

    #[test]
    fn a_duplicate_region_is_an_error_not_a_double_bake() {
        let toml = "regions = [ { id = \"europe/austria\", name = \"Austria\" }, \
                    { id = \"europe/austria\", name = \"Österreich\" } ]";
        assert!(parse(toml).unwrap_err().contains("listed twice"));
    }

    #[test]
    fn an_unknown_key_in_the_list_is_a_typo_not_metadata() {
        let toml = "regions = [ { id = \"europe/austria\", name = \"Austria\", styel = \"x\" } ]";
        assert!(parse(toml).is_err(), "a misspelled key must fail rather than silently bake everything");
    }
}
