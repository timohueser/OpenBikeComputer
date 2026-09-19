//! What `obc inspect` must do: name a format, print what is inside, and report damage instead of
//! failing. The formats themselves are pinned by the crates that own them, so nothing here
//! re-asserts a byte layout.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    Store, StoreId,
};
use obcm_testkit::scratch::{scratch_dir, scratch_path};
use serde_json::Value;

const DEMO_MAP: &str = "../../apps/obc-sim/assets/grimsel-demo.obcm";

fn inspect(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_obc-inspect")).args(args).output().expect("the inspector runs")
}

/// The report, with the process proved to have exited on its own rather than through a panic.
fn report(path: &Path, json: bool) -> (String, bool) {
    let mut args = vec![path.to_str().expect("a printable path")];
    if json {
        args.push("--json");
    }
    let output = inspect(&args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked at"), "{} panicked: {stderr}", path.display());
    let code = output.status.code().unwrap_or_else(|| panic!("{} killed by a signal", path.display()));
    assert!(code <= 1, "{} exited {code}", path.display());
    (String::from_utf8_lossy(&output.stdout).into_owned(), code == 0)
}

fn json_report(path: &Path) -> Value {
    let (text, ok) = report(path, true);
    assert!(ok, "expected a clean read of {}:\n{text}", path.display());
    serde_json::from_str(&text).expect("the report is valid JSON")
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).expect("the directory is readable") {
        let path = entry.expect("a readable entry").path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[test]
fn a_map_reports_its_ladder_its_sections_and_its_terrain() {
    let report = json_report(Path::new(DEMO_MAP));
    assert_eq!(report["format"], "obcm");
    assert_eq!(report["version"], obc_formats::obcm::VERSION);

    let lods = report["lods"].as_array().expect("the LOD ladder is a table");
    assert!(lods.len() > 1, "the ladder has every level");
    assert_eq!(lods[0]["max_mpp"], "unbounded", "the coarsest level is unbounded");
    assert!(lods.iter().all(|lod| lod["index_offset"].as_u64().is_some()), "every level says where it sits");

    assert!(report["poi"]["categories"].as_array().is_some_and(|rows| !rows.is_empty()));
    assert_eq!(report["nav"]["present"], true);
    assert_eq!(report["terrain"]["present"], true);
    assert!(report["terrain"]["container"]["levels"].as_u64().is_some(), "the embedded container is read too");
}

#[test]
fn a_route_reports_its_name_its_points_and_its_waypoints() {
    let report = json_report(Path::new("../../specs/vectors/route-waypoints.obcr"));
    assert_eq!(report["format"], "obcr");
    assert_eq!(report["version"], obc_formats::obcr::VERSION);
    assert_eq!(report["name"], "Vector Loop");
    assert_eq!(report["points"], 9);
    let waypoints = report["waypoints"].as_array().expect("the waypoints are a table");
    assert_eq!(waypoints.len(), report["waypoint_count"].as_u64().expect("a count") as usize);
    assert_eq!(waypoints[0]["name"], "Brunnen");
}

/// The trip object and the settings blob carry no magic. Both are still named.
#[test]
fn the_two_headerless_formats_are_named_by_content() {
    let trip = json_report(Path::new("../../specs/vectors/trip-v2.bin"));
    assert_eq!(trip["format"], "obt");
    assert_eq!(trip["stage_count"], 3);
    assert_eq!(trip["stages"].as_array().expect("the stage ids").len(), 3);

    let dir = scratch_dir("obc-inspect", "settings");
    let path = dir.join("obc-settings.bin");
    std::fs::write(&path, obc_app::settings::encode(&obc_app::settings::Settings::DEFAULT)).expect("write the blob");
    let settings = json_report(&path);
    assert_eq!(settings["format"], "settings");
    assert_eq!(settings["version"], obc_app::settings::VERSION);
    assert_eq!(settings["fields"]["units"], "Metric", "a field prints under its declared name");
    assert!(settings["fields"]["fix_interval_s"].is_string(), "every declared field is listed");
}

#[test]
fn a_card_image_reports_its_store_and_the_objects_on_it() {
    let path = write_card();
    let report = json_report(&path);
    assert_eq!(report["format"], "card");
    assert_eq!(report["store"]["mode"], "ReadWrite");
    assert_eq!(report["catalog"]["entries"], 1);
    assert_eq!(report["listing_complete"], true);

    let objects = report["objects"].as_array().expect("the objects are a table");
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["name"], "Grimsel Loop");
    assert_eq!(objects[0]["kind"], "Route");
    assert_eq!(objects[0]["payload"], "512");
    assert_eq!(report["recovered_ride"]["present"], false);
}

/// Damage is a report, not a crash: the format is still named, and the exit is non-zero.
#[test]
fn a_truncated_map_reports_the_damage_and_exits_non_zero() {
    let dir = scratch_dir("obc-inspect", "damage");
    let whole = std::fs::read(DEMO_MAP).expect("the demo map is readable");
    let path = dir.join("truncated.obcm");
    std::fs::write(&path, &whole[..100_000]).expect("write the truncated map");

    let (text, ok) = report(&path, true);
    assert!(!ok, "a truncated map is not a clean read");
    let parsed: Value = serde_json::from_str(&text).expect("a damage report is still valid JSON");
    assert_eq!(parsed["format"], "obcm", "the format is named before the file is read");
    assert!(parsed["damage"].as_str().is_some_and(|text| text.contains("OBCM")), "the damage names the parse: {text}");

    let empty = dir.join("empty.obcm");
    std::fs::write(&empty, []).expect("write the empty file");
    let (_, ok) = report(&empty, false);
    assert!(!ok, "an empty file is not a clean read");
}

/// Every shared vector, every route beside it, and the demo map: read or refused, never a panic.
#[test]
fn every_committed_artefact_is_read_or_refused_without_a_panic() {
    let mut named = 0;
    let mut paths = files_under(Path::new("../../specs/vectors"));
    paths.push(PathBuf::from(DEMO_MAP));
    paths.extend(files_under(Path::new("../../fixtures/sources/sim-grimsel/routes")));
    assert!(paths.len() > 30, "the vector set is there");

    for path in &paths {
        let (text, ok) = report(path, false);
        assert!(!text.is_empty(), "{} printed nothing", path.display());
        let (json, json_ok) = report(path, true);
        serde_json::from_str::<Value>(&json).expect("every report is valid JSON");
        assert_eq!(ok, json_ok, "{} disagrees between the two renderings", path.display());
        named += usize::from(ok);
    }
    assert!(named >= 8, "the map, the routes, the trip and the terrain shard are all named");
}

// ── a card to read ──────────────────────────────────────────────────────────

/// 64 MiB, the smallest card the geometry rule gives an ordinary 1 MiB extent.
const CARD_BYTES: u64 = 64 * 1024 * 1024;

/// The writable twin of the tool's read-only file device. It exists to *make* a card; the tool
/// itself never opens a file for writing.
struct CardFile(RefCell<File>);

impl BlockDevice for &CardFile {
    type Error = ();

    fn block_count(&self) -> Result<u64, ()> {
        Ok(CARD_BYTES / 512)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        let mut file = self.0.borrow_mut();
        file.seek(SeekFrom::Start(lba * 512)).map_err(|_| ())?;
        file.read_exact(buf).map_err(|_| ())
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        let mut file = self.0.borrow_mut();
        file.seek(SeekFrom::Start(lba * 512)).map_err(|_| ())?;
        file.write_all(buf).map_err(|_| ())
    }

    fn sync(&self) -> Result<(), ()> {
        self.0.borrow().sync_all().map_err(|_| ())
    }
}

/// A formatted card carrying one committed route object.
fn write_card() -> PathBuf {
    let path = scratch_path("obc-inspect", "card.img");
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).expect("the card");
    file.set_len(CARD_BYTES).expect("size the card");
    let card = CardFile(RefCell::new(file));

    let store = FlatStore::initialize(&card, StoreId([0x42; 16])).expect("the card formats");
    let mut allocation = store.allocate(512).expect("reserve one payload");
    store.write(&mut allocation, &[7u8; 512]).expect("write the payload");
    let meta = EntryMeta {
        added_at_utc: 0,
        id: ObjectId(1),
        revision: Revision(1),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: 512,
        payload_crc: store.allocation_crc(&allocation).expect("the payload CRC"),
        name: DisplayName::new("Grimsel Loop").expect("a legal name"),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("publish the object");
    store.sync_media().expect("make the card durable");
    path
}
