//! Where an extract comes from.
//!
//! The bakery bakes whole-country `.osm.pbf` extracts — Germany alone is 4.8 GB. Two separate
//! questions live here, and keeping them separate is the whole design:
//!
//! - Which extract is current? The store answers it: `obc_data` fetches the sources
//!   `geofabrik-extracts` and `geofabrik-poly` at their live pins, or else at the newest day
//!   Geofabrik has, and keeps each version once.
//! - Did the input change since the last bake? Answered with the file's SHA-256, in the cell
//!   bakery. That is the idempotency key and it is never a date.
//!
//! [`Extract::snapshot`] sits deliberately on the far side of that line. It is a fact about the
//! data that the manifest publishes, so it must not go stale, but letting it force a re-pack would
//! reintroduce date sensitivity. The bakery keeps it out of the pack key and compares it separately.
//!
//! [`ExtractSource`] is a trait because the tests must not touch the network. [`LocalExtracts`]
//! resolves the same regions against a directory or a `file://` URL, so every test in this crate
//! runs offline against the tiny fixtures already in the repo.

use std::path::PathBuf;

use obc_pack::progress::Progress;

use crate::regions::Region;

/// A resolved extract on local disk.
#[derive(Debug, Clone)]
pub struct Extract {
    /// Where the `.osm.pbf` is, ready to hand to the packer.
    pub path: PathBuf,
    /// `YYYY-MM-DD` of the extract itself — the manifest's `source_snapshot`, and a fact about the
    /// data, never about when it was fetched.
    pub snapshot: String,
    /// Size in bytes.
    pub bytes: u64,
}

/// Resolve a region to a local `.osm.pbf`.
pub trait ExtractSource: Sync {
    /// Human-readable description of where extracts come from, for the run header.
    fn describe(&self) -> String;
    /// Fetch or reuse the extract for `region`.
    fn fetch(&self, region: &Region, progress: &Progress) -> Result<Extract, String>;
    /// The region's Osmosis polygon (`<id>.poly`), as text.
    ///
    /// This is the extract's own statement of what ground it covers, and the cell bake needs it for
    /// two decisions a bbox cannot make: which cells a region selects, and whether a baked cell is
    /// canonical or `partial`. It is also the file the catalog's drawable region outline is reduced
    /// from, so both readings come from one download.
    fn fetch_poly(&self, region: &Region, progress: &Progress) -> Result<String, String>;
}

/// Geofabrik's extracts and polygons, from the store.
pub struct GeofabrikExtracts;

impl GeofabrikExtracts {
    fn get(source: &str, region: &Region) -> Result<obc_data::fetch::Fetched, String> {
        obc_data::fetch::live(source, None, vec![("area".into(), region.id.clone())])
    }
}

impl ExtractSource for GeofabrikExtracts {
    fn describe(&self) -> String {
        "Geofabrik, through the store".into()
    }

    fn fetch(&self, region: &Region, _progress: &Progress) -> Result<Extract, String> {
        let fetched = Self::get("geofabrik-extracts", region)?;
        let bytes = fetched.snapshot.files[0].size;
        Ok(Extract { path: fetched.paths[0].clone(), snapshot: fetched.snapshot.version, bytes })
    }

    fn fetch_poly(&self, region: &Region, _progress: &Progress) -> Result<String, String> {
        let path = &Self::get("geofabrik-poly", region)?.paths[0];
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Extracts already on disk: a directory of `.osm.pbf` files, or a `file://` URL.
///
/// Two layouts are accepted, because two callers want different ones: the nested
/// `europe/germany/bayern-latest.osm.pbf` mirror layout, which is what an rsync'd Geofabrik tree
/// looks like, and a flat directory of `<id-with-underscores>-latest.osm.pbf`, which is the
/// bakery's own download cache, so a workstation can re-bake from it with the network unplugged.
pub struct LocalExtracts {
    root: PathBuf,
    /// Overrides the mtime-derived snapshot date. Tests pin it so a manifest built from a
    /// checked-in fixture is reproducible.
    snapshot_override: Option<String>,
}

impl LocalExtracts {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), snapshot_override: None }
    }

    pub fn with_snapshot(mut self, snapshot: impl Into<String>) -> Self {
        self.snapshot_override = Some(snapshot.into());
        self
    }

    /// `file:///abs/path` → a local root; anything else is taken as a path.
    pub fn from_spec(spec: &str) -> Self {
        let path = spec.strip_prefix("file://").unwrap_or(spec);
        Self::new(path)
    }
}

impl ExtractSource for LocalExtracts {
    fn describe(&self) -> String {
        format!("local extracts in {}", self.root.display())
    }

    fn fetch(&self, region: &Region, _progress: &Progress) -> Result<Extract, String> {
        let nested = self.root.join(format!("{}-latest.osm.pbf", region.id));
        let flat = self.root.join(region.cache_name());
        let path = if nested.is_file() {
            nested
        } else if flat.is_file() {
            flat
        } else {
            return Err(format!(
                "no extract for `{}` — looked for {} and {}",
                region.id,
                nested.display(),
                flat.display()
            ));
        };
        let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let snapshot = match &self.snapshot_override {
            Some(s) => s.clone(),
            None => {
                let secs = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .ok_or_else(|| format!("{}: no modification time to date the extract by", path.display()))?;
                obc_pack::catalog::format_timestamp(secs)[..10].to_string()
            }
        };
        Ok(Extract { path, snapshot, bytes: meta.len() })
    }

    fn fetch_poly(&self, region: &Region, _progress: &Progress) -> Result<String, String> {
        let nested = self.root.join(format!("{}.poly", region.id));
        let flat = self.root.join(region.poly_cache_name());
        for path in [&nested, &flat] {
            if path.is_file() {
                return std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
            }
        }
        Err(format!(
            "no coverage polygon for `{}` — looked for {} and {}. A cell bake needs it: it is what decides which \
             cells the region selects and whether a border cell is canonical (OBCA_Spec.md §3.7)",
            region.id,
            nested.display(),
            flat.display()
        ))
    }
}

/// The source a CLI `--source` spec asks for: the store without one, else a directory or a
/// `file://` URL of extracts.
pub fn from_spec(spec: Option<&str>) -> Result<Box<dyn ExtractSource>, String> {
    match spec {
        None => Ok(Box::new(GeofabrikExtracts)),
        Some(spec) if spec.starts_with("http://") || spec.starts_with("https://") => Err(format!(
            "--source {spec}: Geofabrik comes from the store, as `geofabrik-extracts` in data/sources.toml; \
             --source takes a directory of extracts"
        )),
        Some(spec) => Ok(Box::new(LocalExtracts::from_spec(spec))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_source_finds_both_layouts() {
        let dir = std::env::temp_dir().join(format!("obc-bake-src-{}", std::process::id()));
        let nested = dir.join("europe/germany");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("bayern-latest.osm.pbf"), b"x").unwrap();
        std::fs::write(dir.join("europe_austria-latest.osm.pbf"), b"y").unwrap();

        let src = LocalExtracts::new(&dir).with_snapshot("2026-07-28");
        let bayern = Region { id: "europe/germany/bayern".into(), name: "Bayern".into() };
        let austria = Region { id: "europe/austria".into(), name: "Austria".into() };
        let missing = Region { id: "europe/france".into(), name: "France".into() };
        let p = Progress::silent();
        assert_eq!(src.fetch(&bayern, &p).unwrap().snapshot, "2026-07-28");
        assert_eq!(src.fetch(&austria, &p).unwrap().bytes, 1);
        // Loud, and it names both paths it looked at.
        let err = src.fetch(&missing, &p).unwrap_err();
        assert!(err.contains("no extract for `europe/france`"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
