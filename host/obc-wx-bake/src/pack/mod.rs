//! Event packs (WX-step 2): a real past weather event, frozen on disk so the simulator and the
//! test suite can run against real radar instead of synthetic blobs.
//!
//! ```text
//! wx-events/<event-id>/
//!   event.json    the pack manifest: window, bake parameters and per-member provenance
//!   upstream/     raw archive bytes, byte-identical to what the archive served
//!   service/      the baked tree, produced by running the REAL baker over upstream/
//!   truth/        the OBSERVED frames at the truth offsets — ground truth for later scoring
//! ```
//!
//! The pack's central promise is **re-bakeability**, and it covers both halves: `service/` is what
//! [`crate::canonical::run_cycle`] emits when its `Upstream` is [`crate::fetch::FixtureUpstream`]
//! loaded from `upstream/` — the very seam the checked-in fixture cycles already use — and
//! `truth/` is the same deal through [`crate::source::mrms::bake_observation`]. Neither is a
//! hand-assembled artifact. A re-bake that differs by one byte is a bug in the baker, and
//! [`crate::pack::rebake`] is what CI runs to say so.
//!
//! A pack bakes the real cycle over a **smaller lattice** ([`crate::pack::window::sub_lattice`]),
//! not a crop of production objects: since #1246 the bakery publishes one global lattice in 24
//! shards of 6,144 x 4,608 cells, and a shard is not a thing a repository can hold.
//!
//! Two facts about historical archives shape the format:
//!
//! * **The archive is not the upstream.** The baker asks for
//!   `https://noaa-mrms-pds.s3.amazonaws.com/...` — a short-retention bucket that holds days, not
//!   years. A 2020 observation comes from Iowa State's MTArchive mirror instead. A member
//!   therefore carries *two* URLs: `url`, the canonical key the baker requests and the replay
//!   serves it under, and `archive_url`, where the bytes were actually retrieved. Confusing the
//!   two would make a pack unreplayable the moment the live bucket rolls over.
//! * **A shipped pack should depend on no archive at all.** A member may be recorded with full
//!   provenance and `stored: false` (see [`Member::stored`]), which keeps a genuinely oversized
//!   pack tractable — but the truth ladder's raw observations are *not* that case. They are the
//!   inputs `truth/` is a pure function of, so leaving them on a single free mirror would put a
//!   later lattice or quantization change one outage away from being unresolvable. Capture with
//!   `--store-truth-upstream`.
//!
//! Provenance discipline follows `tests/fixtures/README.md` exactly: exact retrieval URL, byte
//! range where one was used, length, sha256 and licence, for every member.

pub mod archive;
pub mod capture;
pub mod rebake;
pub mod window;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The pack-format tag `event.json` carries. Bump it when the on-disk shape changes; there is no
/// compatibility shim (the repo has no released packs to keep working).
pub const FORMAT: &str = "obc-wx-event/1";

/// The basemap every US event pack is rendered over — one line in
/// [`regions.toml`](../../../obc-bake/regions.toml), and deliberately only one.
///
/// The bakery is DACH-first; this is the single exception, and it exists because a frozen storm
/// with no map under it is not something the simulator can show. The condition attached to it is
/// a convention, not a mechanism: **US event packs crop to ground Iowa covers**, so one state map
/// serves all of them. A pack that wants Kansas is a conversation about a second basemap, not a
/// quiet second entry. Every pack records the ground it actually covers ([`Event::coverage_udeg`])
/// so drift is visible in the document rather than discovered when a map turns up blank.
pub const US_BASEMAP_REGION: &str = "north-america/us/iowa";

/// Iowa's bounding box (40.3755-43.5012 N, 96.6397-90.1401 W), the reference the convention above
/// is measured against.
///
/// **Hand-copied, and nothing ties it to the extract.** These are the published bounds of the
/// state, transcribed here because the Geofabrik `.poly` for `north-america/us/iowa` is not
/// fetched by anything in this crate — `obc-bake` downloads it at bake time, and this crate has no
/// business doing so to run a unit test. The consequence is honest to state: if Geofabrik ever
/// re-cuts the extract, this constant does not notice. It is a tripwire for "did a pack wander to
/// another state", not a survey marker, and it should not be treated as one.
///
/// A pack's coverage is *not* required to sit inside it. Two things push past the border, and the
/// test that enforces this ([`tests/event_pack.rs`]'s `the_pack_stays_on_the_basemap_it_names`)
/// budgets for both: the requested `--bbox` may already sit outside the state, and the crop then
/// aligns outward to whole tiles of each frame's own lattice, adding up to one more tile stride
/// (0.64 degrees on the 1 km observation).
pub const US_BASEMAP_BBOX: BboxUdeg =
    BboxUdeg { south_udeg: 40_375_501, west_udeg: -96_639_704, north_udeg: 43_501_196, east_udeg: -90_140_061 };

pub const EVENT_FILE: &str = "event.json";
pub const UPSTREAM_DIR: &str = "upstream";
pub const SERVICE_DIR: &str = "service";
pub const TRUTH_DIR: &str = "truth";

/// Lowercase hex sha256 — the one digest the whole pack format speaks.
pub fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// An integer bounding box in microdegrees — the same units OBCG and the manifest use, so a
/// crop window can never be a subtly different number from the geometry it crops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BboxUdeg {
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub north_udeg: i64,
    pub east_udeg: i64,
}

/// The furthest a bbox edge may sit from the equator / prime meridian, in microdegrees.
pub const MAX_LAT_UDEG: i64 = 90_000_000;
pub const MAX_LON_UDEG: i64 = 180_000_000;

impl BboxUdeg {
    /// Parse `south,west,north,east` in decimal degrees.
    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(format!("--bbox wants south,west,north,east in degrees, got {text:?}"));
        }
        let mut udeg = [0i64; 4];
        for (slot, part) in udeg.iter_mut().zip(&parts) {
            let degrees: f64 = part.parse().map_err(|_| format!("--bbox: {part:?} is not a number"))?;
            if !degrees.is_finite() {
                return Err(format!("--bbox: {part:?} is not a finite number"));
            }
            // The cast is only safe *because* the magnitude is checked immediately below: an
            // out-of-range `f64 as i64` saturates to `i64::MAX` rather than failing.
            *slot = (degrees * 1e6).round() as i64;
        }
        let bbox = Self { south_udeg: udeg[0], west_udeg: udeg[1], north_udeg: udeg[2], east_udeg: udeg[3] };
        bbox.validate().map_err(|error| format!("--bbox {text:?}: {error}"))?;
        Ok(bbox)
    }

    /// A bbox that is on the planet and not empty.
    ///
    /// The magnitude half is not decoration. `(degrees * 1e6).round() as i64` **saturates** on an
    /// out-of-range float, so a fat-fingered `--bbox 40.5,-96.5,1e30,-90` reached the crop as
    /// `north_udeg = i64::MAX` — and against the MRMS lattice that clamps to rows 2048..3500 and
    /// crops half of CONUS, silently, with no error anywhere. The typo becomes a pack. That is the
    /// dangerous half of this defect, and it is a *validation* failure, not an arithmetic one:
    /// nothing overflowed.
    ///
    /// (The arithmetic could overflow too, on the other two extremes — `east = i64::MAX` and
    /// `south = i64::MIN` wrapped into a spurious "does not intersect" in a release build, where
    /// `overflow-checks` is off because the workspace sets no `[profile.release]`. That half is
    /// fail-safe, and [`crate::pack::crop::window`] is now independently total: it works in
    /// `i128`. See its `saturated_bbox_edges_cannot_wrap_the_axis_arithmetic` for the measured
    /// table.)
    ///
    /// So this is the outer of two defences, and it is the one that matters: it refuses nonsense
    /// at the boundary with a message a human can act on, and it covers a `BboxUdeg` arriving by
    /// any route — including `serde`, out of an `event.json` nobody parsed.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value, limit) in [
            ("south", self.south_udeg, MAX_LAT_UDEG),
            ("north", self.north_udeg, MAX_LAT_UDEG),
            ("west", self.west_udeg, MAX_LON_UDEG),
            ("east", self.east_udeg, MAX_LON_UDEG),
        ] {
            if !(-limit..=limit).contains(&value) {
                return Err(format!("{name} edge {} microdegrees is off the planet (limit +/-{limit})", value));
            }
        }
        if self.south_udeg >= self.north_udeg || self.west_udeg >= self.east_udeg {
            return Err("the window is empty (south<north and west<east are required)".into());
        }
        Ok(())
    }
}

/// What a member is to the pack. Service members are the baker's own ingress and are always
/// checked in; truth members feed the observed ladder and are recorded but usually not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Service,
    Truth,
}

/// How the baker retrieved the member — which is also how the replay must serve it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Retrieval {
    /// A HEAD probe. No bytes; the recorded length is what the range arithmetic was bounded by
    /// (`None` records a genuine 404, which is part of what the baker's discovery proved).
    Probe { object_length: Option<u64> },
    /// A whole object body.
    Body,
    /// One inclusive byte range of an object far too large to store whole (the NOAA `.idx` fast
    /// path). `object_length` is the whole object's upstream `Content-Length`.
    Range { object_length: u64, start: u64, end_inclusive: u64 },
}

/// One retrieved (or probed) upstream object, with the provenance `tests/fixtures/README.md`
/// demands: exact URL, byte range where one was used, length, sha256 and licence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub role: Role,
    /// The canonical key **the baker requests**. The replay serves the bytes under this URL.
    pub url: String,
    /// Where the bytes were actually retrieved. Equal to `url` when the live source is itself a
    /// full archive (NOAA's HRRR bucket); different when it is not (MRMS, via MTArchive).
    pub archive_url: String,
    #[serde(flatten)]
    pub retrieval: Retrieval,
    /// Pack-relative path of the stored bytes, e.g. `upstream/mrms/....grib2.gz`. Absent for a
    /// probe, which has no body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Length of the retrieved bytes (the range length for a range member). Absent for a probe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Are the bytes checked in at `path`? A `false` member is fully provenanced but must be
    /// materialized by `obc-wx-pack fetch` before it can be replayed. A [`Retrieval::Probe`] has
    /// no bytes at all, so it is always `false` and the flag means nothing for it.
    pub stored: bool,
    pub licence: String,
    pub attribution_url: String,
}

impl Member {
    pub fn is_body_like(&self) -> bool {
        !matches!(self.retrieval, Retrieval::Probe { .. })
    }
}

/// One published object of the baked `service/` tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceObject {
    /// The object's key inside the service tree, e.g. `wx/v1/us/20200810T1852Z/f0.obcg`.
    pub key: String,
    pub bytes: u64,
    pub sha256: String,
}

/// One observed ground-truth frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruthFrame {
    /// The offset the ladder asked for.
    pub requested_offset_min: u32,
    /// The offset actually available: upstream observations land on their own cadence, so the
    /// request is floored to the newest observation at or before it. The frame's own OBCG header
    /// carries this same instant — the label never overrides the bytes.
    pub offset_min: u32,
    pub valid_at: String,
    /// Pack-relative path, e.g. `truth/f14.obcg`.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The bake parameters a re-bake must reproduce exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BakeParams {
    /// The sources the pack bakes, in [`crate::source::MOSAIC_PRIORITY`] order (`mrms`, `hrrr`
    /// today — see [`archive::SUPPORTED_SOURCES`]). A pack is a **subset** mosaic: it names the
    /// sources whose upstream bytes it carries, and a replay builds the mosaic from exactly those.
    pub sources: Vec<String>,
    /// The wall clock injected into the cycle. Discovery and run selection both read it.
    pub now: String,
    /// The ground the pack's own lattice covers. Required: a global cycle is not a thing a
    /// repository can hold. The raw `upstream/` bytes are never cropped — they must stay
    /// byte-identical to the archive.
    pub bbox_udeg: BboxUdeg,
}

/// The observed ladder's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruthParams {
    pub requested_offsets_min: Vec<u32>,
    /// The upstream observation cadence a request is floored to, in seconds.
    pub cadence_seconds: i64,
}

/// `event.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub format: String,
    pub id: String,
    pub title: String,
    /// Free-form region label for humans (`conus`, `dach`, ...).
    pub region: String,
    /// The event's UTC window: the cycle anchor through the last truth frame.
    pub window_start: String,
    pub window_end: String,
    pub bake: BakeParams,
    /// The ground the pack's baked frames actually answer for — its lattice's own extent, which
    /// is `bake.bbox_udeg` aligned outward to whole tiles. Stated rather than inferred so a later
    /// pack drifting off the basemap is visible in the document.
    pub coverage_udeg: BboxUdeg,
    /// The `regions.toml` id of the map a simulator needs under this pack. See
    /// [`US_BASEMAP_REGION`].
    pub basemap_region: String,
    pub truth: TruthParams,
    pub members: Vec<Member>,
    /// The manifest key inside `service/`.
    pub manifest_key: String,
    pub service: Vec<ServiceObject>,
    pub truth_frames: Vec<TruthFrame>,
}

impl Event {
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("event serializes");
        text.push('\n');
        text
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let event: Event = serde_json::from_slice(bytes).map_err(|error| format!("event.json: {error}"))?;
        if event.format != FORMAT {
            return Err(format!("event.json declares format {:?}, expected {FORMAT}", event.format));
        }
        // A pack document is untrusted input like any other: the bbox it carries goes straight
        // into the lattice window's integer arithmetic, so it is checked here rather than where it
        // lands.
        event.bake.bbox_udeg.validate().map_err(|error| format!("event.json: bake.bbox_udeg: {error}"))?;
        event.coverage_udeg.validate().map_err(|error| format!("event.json: coverage_udeg: {error}"))?;
        Ok(event)
    }

    pub fn read(root: &Path) -> Result<Self, String> {
        let path = root.join(EVENT_FILE);
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::from_json(&bytes)
    }

    pub fn write(&self, root: &Path) -> Result<(), String> {
        write_file(&root.join(EVENT_FILE), self.to_json().as_bytes())
    }

    /// Members whose bytes the replay needs: everything the baker actually read.
    pub fn service_members(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|member| member.role == Role::Service)
    }
}

/// Create parents and write `bytes` to `path`.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Read every file under `root` as `relative/slash/path` → bytes. The service tree comparison and
/// the pack writer both speak this shape.
pub fn read_tree(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if !dir.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|error| format!("{}: {error}", dir.display()))?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let key = path
                .strip_prefix(root)
                .map_err(|_| format!("{} escaped {}", path.display(), root.display()))?
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            files.insert(key, bytes);
        }
    }
    Ok(files)
}

/// Resolve a pack-relative path against `root`, refusing anything that would escape the pack.
/// Member paths come out of a JSON document, so they are untrusted input.
pub fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() || relative.starts_with('/') {
        return Err(format!("pack path {relative:?} is not pack-relative"));
    }
    let mut path = root.to_path_buf();
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("pack path {relative:?} is not a plain relative path"));
        }
        path.push(segment);
    }
    Ok(path)
}

/// Prove every stored member and every baked object still hashes to what `event.json` swears.
/// Members with `stored: false` are reported as missing rather than failed — they are recorded,
/// not checked in.
pub fn verify_digests(root: &Path, event: &Event) -> Result<VerifyReport, String> {
    let mut report = VerifyReport::default();
    for member in &event.members {
        let (Some(path), Some(expected)) = (member.path.as_deref(), member.sha256.as_deref()) else {
            continue;
        };
        if !member.stored {
            report.unmaterialized.push(path.to_string());
            continue;
        }
        let bytes = std::fs::read(resolve(root, path)?).map_err(|error| format!("{path}: {error}"))?;
        if Some(bytes.len() as u64) != member.length {
            return Err(format!("{path}: {} bytes on disk, {:?} in event.json", bytes.len(), member.length));
        }
        let actual = sha256(&bytes);
        if actual != expected {
            return Err(format!("{path}: sha256 {actual} != {expected}"));
        }
        report.verified += 1;
    }
    for object in &event.service {
        let path = format!("{SERVICE_DIR}/{}", object.key);
        let bytes = std::fs::read(resolve(root, &path)?).map_err(|error| format!("{path}: {error}"))?;
        if bytes.len() as u64 != object.bytes || sha256(&bytes) != object.sha256 {
            return Err(format!("{path}: baked object does not match event.json"));
        }
        report.verified += 1;
    }
    for frame in &event.truth_frames {
        let bytes = std::fs::read(resolve(root, &frame.path)?).map_err(|error| format!("{}: {error}", frame.path))?;
        if bytes.len() as u64 != frame.bytes || sha256(&bytes) != frame.sha256 {
            return Err(format!("{}: truth frame does not match event.json", frame.path));
        }
        report.verified += 1;
    }
    Ok(report)
}

#[derive(Debug, Default)]
pub struct VerifyReport {
    pub verified: usize,
    pub unmaterialized: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bbox_is_integer_microdegrees_and_never_empty() {
        let bbox = BboxUdeg::parse("40.5,-96.5,43.5,-90.0").expect("a real crop window");
        assert_eq!(
            bbox,
            BboxUdeg { south_udeg: 40_500_000, west_udeg: -96_500_000, north_udeg: 43_500_000, east_udeg: -90_000_000 }
        );
        for bad in ["1,2,3", "40,-96,40,-90", "40,-96,43,-96", "x,2,3,4"] {
            assert!(BboxUdeg::parse(bad).is_err(), "{bad} must be refused");
        }
    }

    /// A magnitude the planet does not have must be refused **at the boundary**.
    ///
    /// `(degrees * 1e6).round() as i64` saturates rather than failing, so a fat-fingered decimal
    /// used to reach the crop as `i64::MAX` and wrap its arithmetic in a release build — coming
    /// back as a confident, wrong, non-empty window over the wrong ground.
    #[test]
    fn a_bbox_off_the_planet_is_refused_before_it_can_saturate() {
        for bad in [
            "40.5,-96.5,1e30,-90",  // the fat-fingered decimal, verbatim
            "-1e30,-96.5,43.5,-90", // …southward
            "40.5,-1e30,43.5,-90",  // …westward
            "40.5,-96.5,43.5,1e30", // …eastward
            "40.5,-96.5,91,-90",    // just past the pole
            "40.5,-181,43.5,-90",   // just past the antimeridian
            "1e300,1e300,1e301,1e301",
        ] {
            let error = BboxUdeg::parse(bad).unwrap_err();
            assert!(error.contains("off the planet"), "{bad}: {error}");
        }
        // The limits themselves are legal.
        assert!(BboxUdeg::parse("-90,-180,90,180").is_ok());
        // And the saturation really is what would have happened without the check.
        assert_eq!((1e30f64 * 1e6).round() as i64, i64::MAX);
    }

    /// The same check covers a bbox that arrives through `serde` rather than the CLI — a pack
    /// document is untrusted input like any other.
    #[test]
    fn an_event_document_carrying_an_impossible_bbox_is_refused() {
        let mut event = sample_event();
        event.bake.bbox_udeg = BboxUdeg { south_udeg: 0, west_udeg: 0, north_udeg: i64::MAX, east_udeg: 1_000_000 };
        let error = Event::from_json(event.to_json().as_bytes()).unwrap_err();
        assert!(error.contains("bake.bbox_udeg") && error.contains("off the planet"), "{error}");

        let mut event = sample_event();
        event.coverage_udeg.west_udeg = i64::MIN;
        let error = Event::from_json(event.to_json().as_bytes()).unwrap_err();
        assert!(error.contains("coverage_udeg") && error.contains("off the planet"), "{error}");
    }

    /// Member paths are untrusted JSON: nothing may reach outside the pack directory.
    #[test]
    fn member_paths_can_never_escape_the_pack() {
        let root = Path::new("/packs/event");
        assert_eq!(resolve(root, "upstream/a.bin").unwrap(), Path::new("/packs/event/upstream/a.bin"));
        for bad in ["", "/etc/passwd", "../../etc/passwd", "upstream/../../x", "upstream//x", "./x"] {
            assert!(resolve(root, bad).is_err(), "{bad} must be refused");
        }
    }

    fn sample_event() -> Event {
        Event {
            format: FORMAT.to_string(),
            id: "x".into(),
            title: "X".into(),
            region: "conus".into(),
            window_start: "2020-08-10T18:52:00Z".into(),
            window_end: "2020-08-10T20:52:00Z".into(),
            bake: BakeParams {
                sources: vec!["mrms".into(), "hrrr".into()],
                now: "2020-08-10T18:52:00Z".into(),
                bbox_udeg: BboxUdeg {
                    south_udeg: 40_500_000,
                    west_udeg: -96_500_000,
                    north_udeg: 43_500_000,
                    east_udeg: -90_000_000,
                },
            },
            coverage_udeg: US_BASEMAP_BBOX,
            basemap_region: US_BASEMAP_REGION.into(),
            truth: TruthParams { requested_offsets_min: vec![15, 30], cadence_seconds: 120 },
            members: vec![Member {
                role: Role::Service,
                url: "https://example.invalid/a".into(),
                archive_url: "https://archive.invalid/a".into(),
                retrieval: Retrieval::Range { object_length: 100, start: 10, end_inclusive: 19 },
                path: Some("upstream/a.bin".into()),
                length: Some(10),
                sha256: Some(sha256(b"0123456789")),
                stored: true,
                licence: "NOAA".into(),
                attribution_url: "https://noaa.invalid".into(),
            }],
            manifest_key: "wx/v1/manifest.json".into(),
            service: vec![],
            truth_frames: vec![],
        }
    }

    #[test]
    fn the_event_document_round_trips_and_pins_its_format_tag() {
        let event = sample_event();
        let json = event.to_json();
        assert_eq!(Event::from_json(json.as_bytes()).unwrap(), event);
        // The retrieval discriminant is flattened, so a member reads as one flat record.
        assert!(json.contains("\"kind\": \"range\""), "{json}");
        let wrong = json.replace(FORMAT, "obc-wx-event/99");
        assert!(Event::from_json(wrong.as_bytes()).is_err(), "a foreign format tag must be refused");
    }
}
