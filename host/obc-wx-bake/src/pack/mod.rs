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
//! The pack's central promise is **re-bakeability**: `service/` is not a hand-assembled artifact,
//! it is what [`crate::cycle::run_cycle`] emits when its `Upstream` is
//! [`crate::fetch::FixtureUpstream`] loaded from `upstream/` — the very seam the checked-in
//! fixture cycles already use. A re-bake that differs by one byte is a bug in the baker, and
//! [`crate::pack::rebake`] is what CI runs to say so.
//!
//! Two facts about historical archives shape the format, and both are recorded per member:
//!
//! * **The archive is not the upstream.** The baker asks for
//!   `https://noaa-mrms-pds.s3.amazonaws.com/...` — a short-retention bucket that holds days, not
//!   years. A 2020 observation comes from Iowa State's MTArchive mirror instead. A member
//!   therefore carries *two* URLs: `url`, the canonical key the baker requests and the replay
//!   serves it under, and `archive_url`, where the bytes were actually retrieved. Confusing the
//!   two would make a pack unreplayable the moment the live bucket rolls over.
//! * **Not every member is checked in.** The truth ladder's raw observations are ~400 KB each and
//!   CI never decodes them, so they are recorded with full provenance and `stored: false`. See
//!   [`Member::stored`]; `obc-wx-pack fetch` materializes them and `verify` proves the sha256.
//!
//! Provenance discipline follows `tests/fixtures/README.md` exactly: exact retrieval URL, byte
//! range where one was used, length, sha256 and licence, for every member.

pub mod archive;
pub mod capture;
pub mod crop;
pub mod rebake;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The pack-format tag `event.json` carries. Bump it when the on-disk shape changes; there is no
/// compatibility shim (the repo has no released packs to keep working).
pub const FORMAT: &str = "obc-wx-event/1";

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
            *slot = (degrees * 1e6).round() as i64;
        }
        let bbox = Self { south_udeg: udeg[0], west_udeg: udeg[1], north_udeg: udeg[2], east_udeg: udeg[3] };
        if bbox.south_udeg >= bbox.north_udeg || bbox.west_udeg >= bbox.east_udeg {
            return Err(format!("--bbox {text:?} is empty (south<north and west<east are required)"));
        }
        Ok(bbox)
    }
}

/// What a member is to the pack. Service members are the baker's own ingress and are always
/// checked in; truth members feed the observed ladder and are recorded but usually not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The adapter the pack captures (`us` today — see `archive::supported_adapters`).
    pub adapter: String,
    /// The wall clock injected into the cycle. Discovery, run selection and staleness all read it.
    pub now: String,
    /// The crop applied to the **baked** output. `null` is a full-domain pack (hundreds of MB).
    /// The raw `upstream/` bytes are never cropped — they must stay byte-identical to the archive.
    pub bbox_udeg: Option<BboxUdeg>,
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

    /// Member paths are untrusted JSON: nothing may reach outside the pack directory.
    #[test]
    fn member_paths_can_never_escape_the_pack() {
        let root = Path::new("/packs/event");
        assert_eq!(resolve(root, "upstream/a.bin").unwrap(), Path::new("/packs/event/upstream/a.bin"));
        for bad in ["", "/etc/passwd", "../../etc/passwd", "upstream/../../x", "upstream//x", "./x"] {
            assert!(resolve(root, bad).is_err(), "{bad} must be refused");
        }
    }

    #[test]
    fn the_event_document_round_trips_and_pins_its_format_tag() {
        let event = Event {
            format: FORMAT.to_string(),
            id: "x".into(),
            title: "X".into(),
            region: "conus".into(),
            window_start: "2020-08-10T18:52:00Z".into(),
            window_end: "2020-08-10T20:52:00Z".into(),
            bake: BakeParams {
                adapter: "us".into(),
                now: "2020-08-10T18:52:00Z".into(),
                bbox_udeg: Some(BboxUdeg {
                    south_udeg: 40_500_000,
                    west_udeg: -96_500_000,
                    north_udeg: 43_500_000,
                    east_udeg: -90_000_000,
                }),
            },
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
        };
        let json = event.to_json();
        assert_eq!(Event::from_json(json.as_bytes()).unwrap(), event);
        // The retrieval discriminant is flattened, so a member reads as one flat record.
        assert!(json.contains("\"kind\": \"range\""), "{json}");
        let wrong = json.replace(FORMAT, "obc-wx-event/99");
        assert!(Event::from_json(wrong.as_bytes()).is_err(), "a foreign format tag must be refused");
    }
}
