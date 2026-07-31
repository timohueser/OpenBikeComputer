//! Where an extract comes from, and how a cached one is known to still be current.
//!
//! The bakery downloads whole-country `.osm.pbf` extracts — Germany alone is
//! 4.8 GB — so "download or reuse the cached PBF" is not a detail: re-fetching one
//! costs more wall clock than packing several small regions. Two separate questions
//! live here, and keeping them separate is the whole design:
//!
//! - **Is the cached file still the current extract?** Answered with HTTP
//!   validators (`Last-Modified` + `Content-Length`) recorded beside the file. This
//!   is a *cache-freshness* decision and may use metadata, because being wrong only
//!   costs a re-download.
//! - **Did the input change since the last bake?** Answered with the file's SHA-256,
//!   in [`crate::bake`]. That is the *idempotency key* and it is never a timestamp:
//!   a mirror that rewrites `Last-Modified` without changing a byte must not
//!   trigger a twenty-hour re-bake, and a file mutated in place must not be missed.
//!
//! [`Extract::snapshot`] sits deliberately on the *far* side of that line. It is a
//! fact about the data that the manifest publishes (`source_snapshot`), so it must
//! not go stale — but it is derived from `Last-Modified`, so letting it force a
//! re-pack would reintroduce exactly the timestamp sensitivity the paragraph above
//! rules out. [`crate::bake`] therefore keeps it out of the pack key and compares it
//! separately: a re-dated but byte-identical extract rewrites the sidecar and
//! re-packs nothing.
//!
//! [`ExtractSource`] is a trait for one reason beyond tidiness: the tests must not
//! touch the network. [`LocalExtracts`] resolves the same regions against a
//! directory (or a `file://` URL), so every test in this crate runs offline against
//! the tiny fixtures already in the repo.

use std::path::{Path, PathBuf};

use obc_pack::progress::Progress;

use crate::regions::Region;

/// A resolved extract on local disk.
#[derive(Debug, Clone)]
pub struct Extract {
    /// Where the `.osm.pbf` is, ready to hand to the packer.
    pub path: PathBuf,
    /// `YYYY-MM-DD` of the extract itself — the manifest's `source_snapshot`, and a
    /// fact about the *data*, never about when we happened to fetch it.
    pub snapshot: String,
    /// Size in bytes.
    pub bytes: u64,
    /// Whether this run had to download it (a summary line, not a decision input).
    pub downloaded: bool,
}

/// Resolve a region to a local `.osm.pbf`.
pub trait ExtractSource: Sync {
    /// Human-readable description of where extracts come from, for the run header.
    fn describe(&self) -> String;
    /// Download or reuse the extract for `region`.
    fn fetch(&self, region: &Region, progress: &Progress) -> Result<Extract, String>;
    /// The region's Osmosis polygon (`<id>.poly`), as text.
    ///
    /// This is the extract's own statement of what ground it covers, and the cell
    /// bake needs it for two decisions a bbox cannot make: which cells a region
    /// selects, and whether a baked cell is canonical or `partial`
    /// ([`crate::coverage`]). It is also the file the catalog's drawable region
    /// outline is reduced from (`OBCC_Spec.md` §11.8), so both readings come from
    /// one download.
    ///
    /// Tiny (tens of KB) and unversioned by Geofabrik, so it is fetched fresh rather
    /// than validator-cached — but written into the same cache directory, which is
    /// what lets an offline re-bake work.
    fn fetch_poly(&self, region: &Region, progress: &Progress) -> Result<String, String>;
}

/// Geofabrik's public download server (or any mirror laid out the same way).
pub struct GeofabrikExtracts {
    base_url: String,
    cache_dir: PathBuf,
}

/// What we recorded about a cached download, used only to decide whether to fetch
/// it again. Never an input to the bake key — see the module note.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CachedMeta {
    url: String,
    last_modified: String,
    content_length: u64,
    snapshot: String,
}

impl GeofabrikExtracts {
    pub const DEFAULT_BASE_URL: &'static str = "https://download.geofabrik.de";

    pub fn new(base_url: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self { base_url: base_url.into(), cache_dir: cache_dir.into() }
    }

    fn meta_path(&self, region: &Region) -> PathBuf {
        self.cache_dir.join(format!("{}.meta.json", region.cache_name()))
    }
}

impl ExtractSource for GeofabrikExtracts {
    fn describe(&self) -> String {
        format!("{} (cache {})", self.base_url, self.cache_dir.display())
    }

    fn fetch(&self, region: &Region, progress: &Progress) -> Result<Extract, String> {
        let url = region.extract_url(&self.base_url);
        let dest = self.cache_dir.join(region.cache_name());
        let head = head(&url)?;
        let meta_path = self.meta_path(region);

        // Reuse only when the file is there *and* both validators still match what
        // was recorded for it. A missing meta file means "hashed by someone else" —
        // re-download rather than publish a snapshot date we cannot substantiate.
        if dest.is_file() {
            if let Ok(text) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<CachedMeta>(&text) {
                    let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                    if meta.url == url
                        && meta.last_modified == head.last_modified
                        && meta.content_length == head.content_length
                        && size == head.content_length
                    {
                        return Ok(Extract { path: dest, snapshot: meta.snapshot, bytes: size, downloaded: false });
                    }
                }
            }
        }

        std::fs::create_dir_all(&self.cache_dir).map_err(|e| format!("{}: {e}", self.cache_dir.display()))?;
        let bytes = obc_pack::net::download(&url, &dest, progress, |pct| {
            if pct % 10 == 0 {
                progress.log(format!("  {} extract {pct}%", region.id));
            }
        })?;
        let meta = CachedMeta {
            url,
            last_modified: head.last_modified,
            content_length: head.content_length,
            snapshot: head.snapshot.clone(),
        };
        let text = serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?;
        std::fs::write(&meta_path, text).map_err(|e| format!("{}: {e}", meta_path.display()))?;
        Ok(Extract { path: dest, snapshot: head.snapshot, bytes, downloaded: true })
    }

    fn fetch_poly(&self, region: &Region, progress: &Progress) -> Result<String, String> {
        let url = region.poly_url(&self.base_url);
        let cached = self.cache_dir.join(region.poly_cache_name());
        match obc_pack::net::get_text(&url) {
            Ok(text) => {
                std::fs::create_dir_all(&self.cache_dir).map_err(|e| format!("{}: {e}", self.cache_dir.display()))?;
                std::fs::write(&cached, &text).map_err(|e| format!("{}: {e}", cached.display()))?;
                Ok(text)
            }
            // A cached copy is a better answer than a failed bake: the polygon
            // changes about as often as a country's borders do.
            Err(e) => match std::fs::read_to_string(&cached) {
                Ok(text) => {
                    progress.warn(format!("{}: {e} — using the cached {}", region.id, cached.display()));
                    Ok(text)
                }
                Err(_) => Err(format!("{url}: {e}")),
            },
        }
    }
}

/// Extracts already on disk: a directory of `.osm.pbf` files, or a `file://` URL.
///
/// Two layouts are accepted, because two callers want different ones: the nested
/// `europe/germany/bayern-latest.osm.pbf` mirror layout (what a rsync'd Geofabrik
/// tree looks like), and a flat directory of `<id-with-underscores>-latest.osm.pbf`
/// — the bakery's own download cache, so a workstation can re-bake from it with the
/// network unplugged.
pub struct LocalExtracts {
    root: PathBuf,
    /// Overrides the mtime-derived snapshot date. Tests pin it so a manifest built
    /// from a checked-in fixture is reproducible.
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
        Ok(Extract { path, snapshot, bytes: meta.len(), downloaded: false })
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

/// Build the source a CLI `--source` spec asks for.
pub fn from_spec(spec: &str, cache_dir: &Path) -> Box<dyn ExtractSource> {
    if spec.starts_with("http://") || spec.starts_with("https://") {
        Box::new(GeofabrikExtracts::new(spec, cache_dir))
    } else {
        Box::new(LocalExtracts::from_spec(spec))
    }
}

struct Head {
    last_modified: String,
    content_length: u64,
    snapshot: String,
}

/// One `HEAD` for the validators and the extract's date.
///
/// The snapshot date comes from `Last-Modified` rather than from the redirect
/// target's `…-260728.osm.pbf` filename: `-latest` for some regions redirects to a
/// mirror that keeps no date in the name, and a `source_snapshot` guessed wrong is
/// worse than a failed bake — the manifest is trusted.
fn head(url: &str) -> Result<Head, String> {
    let resp = ureq::head(url).call().map_err(|e| format!("HEAD {url}: {e}"))?;
    let get = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let last_modified = get("last-modified")
        .ok_or_else(|| format!("{url}: no Last-Modified header — cannot date the extract, refusing to guess"))?;
    let content_length: u64 = get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let snapshot = http_date_to_iso(&last_modified)
        .ok_or_else(|| format!("{url}: unparseable Last-Modified `{last_modified}`"))?;
    Ok(Head { last_modified, content_length, snapshot })
}

/// `Tue, 28 Jul 2026 23:24:16 GMT` → `2026-07-28`.
fn http_date_to_iso(value: &str) -> Option<String> {
    let mut parts = value.split_whitespace();
    let _weekday = parts.next()?;
    let day: u32 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: u32 = parts.next()?.parse().ok()?;
    let iso = format!("{year:04}-{month:02}-{day:02}");
    obc_pack::catalog::validate_date(&iso).ok()?;
    Some(iso)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_http_date_becomes_the_snapshot_date() {
        assert_eq!(http_date_to_iso("Tue, 28 Jul 2026 23:24:16 GMT").as_deref(), Some("2026-07-28"));
        assert_eq!(http_date_to_iso("Sun, 01 Feb 2026 00:00:00 GMT").as_deref(), Some("2026-02-01"));
        // Rejected rather than silently turned into a plausible-looking date.
        assert_eq!(http_date_to_iso("yesterday"), None);
        assert_eq!(http_date_to_iso("Thu, 30 Feb 2026 00:00:00 GMT"), None);
    }

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
