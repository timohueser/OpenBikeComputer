//! The producer of `wiki`: the Wikimedia facts, the article text and the photos of the OSM
//! subjects, in geographic shards (`specs/wikimedia-content.md`, "The `wiki` source"). It reads the
//! cached dumps in place and downloads missing inputs after review. The steps of `tools.wiki` do the
//! imports, ask Commons and write the shards; this producer selects the subjects and their photos
//! with the rules of the compilers.

mod commons;
mod downloads;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use obc_data::prepared::{Job, Manifest, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::regions::Area;
use obc_data::settings::Settings;
use obc_data::sources::Source;
use obc_data::store::{hash_file, write_atomic};
use obc_pack::landmarks::peaks::Summit;
use obc_pack::landmarks::shared::Landmarks;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::prepared_tiles;

/// The directory of the dumps on the machine that prepares.
const DUMPS: &str = "OBC_WIKI_DUMPS";
/// The shards of one move into the home.
const BATCH: usize = 64;

pub struct Wiki {
    /// `python -m tools.wiki`, as a command in the repository root.
    pub tools: fn(&Path) -> Result<Command, String>,
    /// The first wait after "Wikimedia is busy".
    pub wait: Duration,
}

pub const WIKI: Wiki = Wiki { tools, wait: Duration::from_secs(600) };

fn tools(root: &Path) -> Result<Command, String> {
    let mut command = Command::new(obc_data::engine::python_executable(root)?);
    command.args(["-m", "tools.wiki"]);
    Ok(command)
}

impl Prepared for Wiki {
    fn name(&self) -> &'static str {
        "wiki"
    }

    fn format(&self) -> u32 {
        1
    }

    fn revision(&self) -> u32 {
        1
    }

    fn reads(&self, source: &Source) -> bool {
        obc_data::sources::WIKI_INPUTS.contains(&source.id.as_str())
    }

    /// Reuse cached inputs and discover missing inputs from metadata. The review names each
    /// archive and its size; the number of output shards is unknown before the run.
    fn plan(
        &self,
        context: &obc_data::prepared::Planning,
        source: &Source,
        scope: Option<&Scope>,
        languages: &[String],
    ) -> Result<Plan, String> {
        if source.publish_text.is_none() {
            return Err("the row `wiki` names no `publish_text`: set it to `true` or `false`".into());
        }
        let inputs = downloads::plan(context, scope, languages)?;
        Ok(Plan { inputs, files: None, work_bytes: 0 })
    }

    fn languages(&self, requested: &[String]) -> Result<Vec<String>, String> {
        let supported: Vec<(String, String)> =
            serde_json::from_slice(obc_pack::landmarks::LANGUAGE_BYTES).map_err(|e| e.to_string())?;
        for language in requested {
            if !supported.iter().any(|(code, _)| code == language) {
                return Err(format!("unsupported wiki language `{language}`"));
            }
        }
        Ok(supported
            .into_iter()
            .map(|(code, _)| code)
            .filter(|code| requested.is_empty() || requested.contains(code))
            .collect())
    }

    /// Each step resumes: a step whose output is in `state/` does not run again, and `tools.wiki`
    /// does not repeat an import that finished with the same input.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let before = earlier(job, self.format())?;
        let pin = pin(job)?;
        let state = job.state.clone();
        let (work, subjects, selection) =
            (state.join("work"), state.join("subjects.ndjson"), state.join("selection.json"));
        let mut paths = BTreeMap::new();
        paths.insert("languages", json!(job.languages));
        for dump in &pin.dumps {
            let name = dump.path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            job.input(&dump.source, &dump.day, [(name.to_string(), dump.sha256.clone())].into())?;
            let path = json!({"path": dump.path, "sha256": dump.sha256});
            match dump.key.split_once(':') {
                Some((kind, language)) => paths.entry(kind).or_insert_with(|| json!({}))[language] = path,
                None => {
                    paths.insert(dump.key.as_str(), path);
                }
            }
        }
        let inputs = state.join("inputs.json");
        write_atomic(&inputs, &serde_json::to_vec(&paths).expect("paths serialize"))?;
        let step = |name: &str| -> Result<Command, String> {
            let mut command = (self.tools)(job.root)?;
            command.arg(name).arg("--work").arg(&work);
            Ok(command)
        };
        if !subjects.exists() {
            let mut command = step("subjects")?;
            for dump in pin.osm_dumps() {
                command.arg("--osm").arg(&dump.path);
            }
            command.arg("--out").arg(&subjects);
            let box_scope = job.scope.as_ref().filter(|_| pin.dumps.iter().any(|dump| dump.source == "osm-planet"));
            if let Some([west, south, east, north]) = box_scope.map(bounds).transpose()? {
                command.arg(format!("--bbox={west},{south},{east},{north}"));
            }
            job.program(&mut command)?;
        }
        job.program(step("import")?.arg("--inputs").arg(&inputs).arg("--subjects").arg(&subjects))?;
        if !selection.exists() {
            let candidates = state.join("candidates.jsonl");
            job.program(step("candidates")?.arg("--out").arg(&candidates))?;
            let summits = commons::summits(&subjects)?;
            let chosen = select(&candidates, &summits)?;
            write_atomic(&selection, &serde_json::to_vec(&chosen).expect("a selection serializes"))?;
        }
        let chosen: commons::Selection = read_json(&selection)?;
        let selected = state.join("selected.json");
        let ids: BTreeSet<&String> = chosen.landmarks.iter().chain(&chosen.peaks).collect();
        write_atomic(&selected, &serde_json::to_vec(&ids).expect("ids serialize"))?;
        job.program(step("articles")?.arg("--inputs").arg(&inputs).arg("--selected").arg(&selected))?;
        let failed = commons::rounds(job, &step, self.wait, &pin)?;
        let counts = state.join("counts.json");
        write_atomic(&counts, json!({"photo_compile_failures": failed}).to_string().as_bytes())?;
        // The shards that the home has are not written again.
        let done: BTreeMap<&str, Value> =
            job.files().map(|file| (file.path.as_str(), json!({"sha256": file.sha256, "bytes": file.size}))).collect();
        let done_file = state.join("done.json");
        write_atomic(&done_file, &serde_json::to_vec(&done).expect("done serializes"))?;
        let mut command = step("shards")?;
        command.arg("--out").arg(&job.work).arg("--done").arg(&done_file).arg("--counts").arg(&counts);
        if let Some((_, keep)) = &before {
            let file = state.join("keep.json");
            write_atomic(&file, keep.to_string().as_bytes())?;
            command.arg("--keep").arg(file);
        }
        if !pin.publish_text {
            command.arg("--no-text");
        }
        job.program(&mut command)?;
        let index: Value = read_json(&job.work.join("index.json"))?;
        let shards = index["shards"].as_array().ok_or("the index of the shards lists no shard")?;
        if let Some((before, _)) = &before {
            reuse_unchanged(job, before, shards)?;
        }
        let shards: Vec<String> = shards.iter().filter_map(|shard| shard["path"].as_str()).map(String::from).collect();
        for batch in shards.iter().filter(|path| !job.done(path)).cloned().collect::<Vec<_>>().chunks(BATCH) {
            job.finish(batch)?;
        }
        job.finish(&["index.json".into()])
    }
}

/// A dump that the prepare reads in place: its key in `inputs.json` of `tools.wiki`, the source of
/// `data/sources.toml` that it is a version of, the day of the dump, and its file with the size,
/// the modification time and the SHA-256 that the first run of a version found.
#[derive(Serialize, Deserialize)]
struct Dump {
    key: String,
    source: String,
    day: String,
    path: PathBuf,
    size: u64,
    /// Nanoseconds since the Unix epoch.
    modified: u64,
    sha256: String,
    area: Option<String>,
}

impl Dump {
    fn wanted(&self) -> Wanted {
        let name = self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let mut params = Vec::new();
        if let Some(area) = &self.area {
            params.push(("area".into(), area.clone()));
        }
        params.push(("file".into(), name));
        Wanted { source: self.source.clone(), version: Some(self.day.clone()), params }
    }
}

/// What the first run of a version read, in `state/pin.json`: a resume reads exactly that again.
#[derive(Serialize, Deserialize)]
struct Pin {
    publish_text: bool,
    dumps: Vec<Dump>,
}

impl Pin {
    /// The OSM dumps: the planet, or the extract of each area.
    fn osm_dumps(&self) -> impl Iterator<Item = &Dump> {
        self.dumps.iter().filter(|dump| dump.key == "osm" || dump.key.starts_with("osm:"))
    }

    /// The SHA-256 of the one OSM dump, or of the SHA-256 of each extract.
    fn osm(&self) -> String {
        match self.osm_dumps().map(|dump| dump.sha256.as_str()).collect::<Vec<_>>()[..] {
            [one] => one.into(),
            ref several => obc_data::store::sha256_hex(several.join("\n").as_bytes()),
        }
    }
}

/// The pin of the version of `job`. The first run takes the newest dumps and hashes them once; a
/// resume refuses when a pinned dump is gone or changed, or `publish_text` changed. A dump of the
/// pinned size with another modification time, as after a copy, is hashed again and kept when its
/// SHA-256 is the pinned one.
fn pin(job: &Job) -> Result<Pin, String> {
    let file = job.state.join("pin.json");
    let publish_text = job.source.publish_text != Some(false);
    let dir = job.state.parent().unwrap_or(&job.state).display();
    let again = format!("Fix: delete {dir}; the next run starts a new version");
    if file.exists() {
        let mut pin: Pin = read_json(&file)?;
        if pin.publish_text != publish_text {
            return Err(format!("`publish_text` changed since the first run of the version. {again}"));
        }
        let mut touched = false;
        for dump in &mut pin.dumps {
            let changed =
                || format!("{} is gone or changed since the first run of the version. {again}", dump.path.display());
            match stat(&dump.path) {
                Ok((size, modified)) if (size, modified) == (dump.size, dump.modified) => {}
                Ok((size, modified)) if size == dump.size && hash_file(&dump.path)?.0 == dump.sha256 => {
                    (dump.modified, touched) = (modified, true);
                }
                _ => return Err(changed()),
            }
        }
        if touched {
            write_atomic(&file, &serde_json::to_vec(&pin).expect("a pin serializes"))?;
        }
        return Ok(pin);
    }
    let context = job.planning();
    let mut dumps =
        job.plan.inputs.iter().map(|input| downloads::acquire(&context, input)).collect::<Result<Vec<_>, _>>()?;
    for dump in &mut dumps {
        if dump.sha256.is_empty() {
            dump.sha256 = hash_file(&dump.path)?.0;
        }
        if stat(&dump.path)? != (dump.size, dump.modified) {
            return Err(format!("{} changed while the run read it; run the prepare again", dump.path.display()));
        }
    }
    let pin = Pin { publish_text, dumps };
    write_atomic(&file, &serde_json::to_vec(&pin).expect("a pin serializes"))?;
    Ok(pin)
}

/// The size and the modification time of a file.
fn stat(path: &Path) -> Result<(u64, u64), String> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let modified = metadata.modified().map_err(|e| format!("{}: {e}", path.display()))?;
    let nanos = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_nanos() as u64);
    Ok((metadata.len(), nanos))
}

/// The directory that `OBC_WIKI_DUMPS` names; `None` when it is unset or empty.
fn dumps_dir() -> Option<PathBuf> {
    std::env::var_os(DUMPS).filter(|dir| !dir.is_empty()).map(PathBuf::from)
}

/// The Geofabrik areas of the region of `scope`, each read from its extract; none for a box, which
/// clips the planet input.
fn regional_osm(scope: Option<&Scope>) -> Result<Vec<String>, String> {
    let Some(scope) = scope else { return Ok(Vec::new()) };
    let (regions, id) = prepared_tiles::regions(scope)?;
    let areas = regions.leaves(&id)?.into_iter().map(|leaf| match &regions.get(leaf)?.area {
        Area::Geofabrik { areas } => Some(areas.clone()),
        _ => None,
    });
    match areas.collect::<Option<Vec<_>>>() {
        Some(areas) => Ok(areas.concat().into_iter().collect::<BTreeSet<_>>().into_iter().collect()),
        None if regions.bounds(&id).is_some() => Ok(Vec::new()),
        None => Err("prepare wiki for Geofabrik regions (for DACH: --region europe/dach) or a box".into()),
    }
}

/// The version before that a version of a region extends, and `keep.json` for `shards --keep`:
/// each shard of that version with its index entry and its file in the store.
fn earlier(job: &mut Job, format: u32) -> Result<Option<(Manifest, Value)>, String> {
    if job.scope.is_none() {
        return Ok(None);
    }
    let Some(before) = job.extend(format)? else { return Ok(None) };
    if let (Some(was), Some(now)) = (before.publish_text, job.source.publish_text) {
        if was != now {
            return Err(format!(
                "wiki@{} has `publish_text = {was}` and the row has `{now}`: a version that extends it keeps its articles, so its manifest would state the text wrongly. Fix: set `publish_text = {was}`, or prepare a version of everything",
                before.version
            ));
        }
    }
    let index: Value = read_json(&job.earlier_file(&before, "index.json")?)?;
    let mut shards = index["shards"].as_array().ok_or("the index of the version before lists no shard")?.clone();
    for shard in &mut shards {
        let path = shard["path"].as_str().ok_or("a shard of the version before has no path")?;
        shard["file"] = json!(job.earlier_file(&before, path)?);
    }
    Ok(Some((before, json!({"before": shards}))))
}

/// Keep each shard of `shards` that has its bytes in the version before, without a move.
fn reuse_unchanged(job: &mut Job, before: &Manifest, shards: &[Value]) -> Result<(), String> {
    for shard in shards {
        let (path, sha256) = (shard["path"].as_str(), shard["sha256"].as_str());
        if before.files.iter().any(|file| Some(file.path.as_str()) == path && Some(file.sha256.as_str()) == sha256) {
            job.reuse(before, path.expect("a held path"))?;
        }
    }
    Ok(())
}

/// The subjects of the compilers, whose articles and photos the version holds: each subject that
/// the landmark rule of the compilers selects, and each peak that a link of one of `summits`
/// names, with the Commons categories of each. The classes come first in `candidates`.
fn select(candidates: &Path, summits: &[Summit]) -> Result<commons::Selection, String> {
    let tags = summits.iter().flat_map(|summit| ["wikidata", "wikipedia"].map(|kind| (kind, summit.tags.get(kind))));
    let links: BTreeSet<String> = tags.filter_map(|(kind, tag)| Some(format!("{kind}:{}", tag?))).collect();
    let file = fs::File::open(candidates).map_err(|e| format!("{}: {e}", candidates.display()))?;
    let (mut landmarks, mut selection) = (Landmarks::new(), commons::Selection::default());
    for line in std::io::BufReader::new(file).lines() {
        let value: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if let Some(class) = value.get("class") {
            landmarks.class(class);
        } else if let Some(subject) = value.get("subject").filter(|subject| landmarks.selects(subject)) {
            let Some(id) = subject["key"].as_str() else { continue };
            selection.landmarks.insert(id.into());
            selection.categories(id, &subject["entity"]);
        } else if let Some(id) = value["peak"].as_str().filter(|_| {
            value["links"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|link| links.contains(link))
        }) {
            selection.peaks.insert(id.into());
            selection.categories(id, &value["entity"]["entity"]);
        }
    }
    Ok(selection)
}

/// The box of the region of `scope`: a version of a region keeps the subjects in its box.
fn bounds(scope: &Scope) -> Result<[f64; 4], String> {
    let settings =
        Settings { region: scope.region.clone(), definitions: scope.definitions.clone(), ..Settings::default() };
    let bbox = settings.regions()?.bounds(&scope.region).ok_or_else(|| {
        format!("the region `{}` has no box: prepare a version of a region with `--box`", scope.region)
    })?;
    Ok([bbox.west, bbox.south, bbox.east, bbox.north])
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::temp;
    use obc_data::engine::runs::Run;
    use obc_data::engine::LayerFile;
    use obc_data::fetch::http::Http;
    use obc_data::prepared::Opening;
    use obc_data::sources::Registry;
    use obc_data::store::Store;

    #[test]
    fn the_selection_is_each_subject_of_a_landmark_category_and_each_summit_with_its_categories() {
        let temp = temp("wiki-select");
        fs::create_dir_all(&temp.0).unwrap();
        let fact = |key: &str, claims: &[(&str, Value)]| {
            let mut map = serde_json::Map::new();
            for (property, value) in claims {
                let claim = json!({"rank": "normal", "mainsnak": {"datavalue": {"value": value}}});
                map.entry(property.to_string()).or_insert_with(|| json!([])).as_array_mut().unwrap().push(claim);
            }
            json!({"kind": "entity", "key": key, "status": "present", "identity": key, "entity": {"claims": map}})
        };
        // A castle with a Commons category, a subject whose class has no category, and a peak.
        let lines = [
            json!({"class": fact("Q23413", &[])}),
            json!({"class": fact("Q5", &[])}),
            json!({"subject": fact("Q1", &[("P31", json!({"id": "Q23413"})), ("P373", json!("Castles"))])}),
            json!({"subject": fact("Q2", &[("P31", json!({"id": "Q5"})), ("P373", json!("Other"))])}),
            json!({"peak": "Q7", "links": ["wikidata:Q7"], "entity": fact("Q7", &[("P373", json!("Summits"))])}),
            json!({"peak": "Q8", "links": ["wikidata:Q8"], "entity": fact("Q8", &[("P373", json!("Hills"))])}),
        ];
        let candidates = temp.0.join("candidates.jsonl");
        fs::write(&candidates, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        // The map takes a named peak as a summit, and an unnamed one not: no round asks for it.
        let subjects = temp.0.join("subjects.ndjson");
        let peak = |osm: &str, tags: Value| json!({"osm": osm, "wikidata": tags["wikidata"], "peak": true, "box": [8.3, 46.5, 8.3, 46.5], "tags": tags});
        // `subjects` keeps no `wikipedia` link without a language: a summit takes the links it kept.
        let named = peak("n7", json!({"natural": "peak", "name": "Testhorn", "wikidata": "Q7", "wikipedia": "Horn"}));
        let unnamed = peak("n8", json!({"natural": "peak", "wikidata": "Q8"}));
        fs::write(&subjects, format!("{named}\n{unnamed}\n")).unwrap();
        let summits = commons::summits(&subjects).unwrap();
        assert_eq!(summits.iter().map(|summit| summit.node_id).collect::<Vec<_>>(), [7]);
        assert_eq!(summits[0].tags.get("wikipedia"), None);
        let selection = select(&candidates, &summits).unwrap();
        assert_eq!((selection.landmarks, selection.peaks), (["Q1".into()].into(), ["Q7".into()].into()));
        let categories: Vec<(&String, &BTreeSet<String>)> =
            selection.requests.iter().map(|(id, request)| (id, &request.categories)).collect();
        assert_eq!(categories.len(), 2, "no request for an unselected subject");
        assert_eq!(categories[0].1, &BTreeSet::from(["Category:Castles".to_string()]));
        assert_eq!(categories[1].1, &BTreeSet::from(["Category:Summits".to_string()]));
    }

    #[test]
    fn a_version_reads_each_shard_of_the_version_before_and_keeps_those_it_does_not_change() {
        let temp = temp("wiki-keep");
        let root = temp.0.join("repository");
        fs::create_dir_all(root.join("data")).unwrap();
        fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        let (store, archive) = (Store::at(temp.0.join("store")), temp.0.join("archive"));
        let (folder, dir) = (obc_data::prepared::folder(&archive, "wiki"), archive.join("prepared/wiki"));
        let (sources, http) = (Registry::load(&root).unwrap().sources, Http::new());
        let scope = |id: &str, [west, south, east, north]: [f64; 4]| {
            let text = format!("name = \"{id}\"\nkind = \"box\"\nbox = [{west}, {south}, {east}, {north}]\n");
            Scope { region: id.into(), definitions: [(id.into(), text)].into() }
        };
        // wiki@2026-10-01 holds a cell in Colorado; wiki@2026-10-02 adds the two Swiss cells 0300-0262
        // and 0300-0263, and keeps the cell of Colorado.
        let (far, west, east) = ("cells/0293-0155.tar.zst", "cells/0300-0262.tar.zst", "cells/0300-0263.tar.zst");
        let put = |version: &str, path: &str, bytes: &[u8]| {
            let at = dir.join(version).join(path);
            fs::create_dir_all(at.parent().unwrap()).unwrap();
            fs::write(at, bytes).unwrap();
            LayerFile { path: path.into(), size: bytes.len() as u64, sha256: obc_data::store::sha256_hex(bytes) }
        };
        let complete = |version: &str, scope: Scope, files: Vec<LayerFile>, extends: Vec<Option<Scope>>| {
            // The cell of Colorado is the one file that a later version keeps.
            let reused = files.iter().filter(|file| file.path == far && version != "2026-10-01");
            let reused = reused.map(|file| (file.path.clone(), "2026-10-01".to_string())).collect();
            let manifest = Manifest {
                source: "wiki".into(),
                version: version.into(),
                format: 1,
                revision: 1,
                inputs: Vec::new(),
                files,
                reused,
                scope: Some(scope),
                extends,
                languages: Vec::new(),
                publish_text: Some(false),
                licence: None,
                attribution: None,
                machine: "test".into(),
                time: format!("{version}T00:00:00Z"),
                cost: obc_data::prepared::Cost { wall_ms: 0, peak_rss_bytes: None },
            };
            obc_data::prepared::complete(&folder, &manifest).unwrap();
            manifest.scope.unwrap()
        };
        let colorado = put("2026-10-01", far, b"colorado");
        let first =
            complete("2026-10-01", scope("colorado", [-105.5, 39.5, -105.0, 40.0]), vec![colorado.clone()], vec![]);
        let entry =
            |file: &LayerFile| json!({"path": file.path, "sha256": file.sha256, "bytes": file.size, "facts": 1});
        let (west, east) = (put("2026-10-02", west, b"west"), put("2026-10-02", east, b"east"));
        let index = json!({"shards": [entry(&colorado), entry(&west), entry(&east)], "counts": {}}).to_string();
        let files = vec![
            colorado.clone(),
            west,
            east,
            put("2026-10-02", "common.tar.zst", b"common"),
            put("2026-10-02", "index.json", index.as_bytes()),
        ];
        let second = complete("2026-10-02", scope("alps", [7.0, 46.5, 8.0, 46.6]), files, vec![Some(first.clone())]);
        // A region next to the one before, and inside neither.
        let mut run = Run::create(&store, ["source", "prepare", "wiki"]).unwrap();
        let opening = Opening { root: &root, store: &store, http: &http, sources: &sources, home: &archive };
        let region = scope("west", [6.0, 46.5, 7.2, 46.6]);
        let mut job = Job::open(opening, ("wiki", "2026-10-03"), Some(region), &[], &mut run).unwrap();
        let (before, keep) = earlier(&mut job, 1).unwrap().unwrap();
        let shards = keep["before"].as_array().unwrap();
        let files: Vec<Vec<u8>> =
            shards.iter().map(|shard| fs::read(shard["file"].as_str().unwrap()).unwrap()).collect();
        assert_eq!(files, [&b"colorado"[..], b"west", b"east"]);
        assert_eq!(job.extends(), [Some(first), Some(second)]);
        // `shards` gives Colorado as it was and the west cell with the facts of this run.
        let changed = json!({"path": "cells/0300-0262.tar.zst", "sha256": "ab".repeat(32)});
        reuse_unchanged(&mut job, &before, &[entry(&colorado), changed]).unwrap();
        assert_eq!(job.reused(), &[(far.to_string(), "2026-10-01".to_string())].into());
        drop(job);
        // A version that would publish text over one that kept it local refuses.
        let mut flipped = sources.clone();
        flipped[0].publish_text = Some(true);
        let opening = Opening { root: &root, store: &store, http: &http, sources: &flipped, home: &archive };
        let region = scope("west", [6.0, 46.5, 7.2, 46.6]);
        let mut job = Job::open(opening, ("wiki", "2026-10-03"), Some(region), &[], &mut run).unwrap();
        let refused = earlier(&mut job, 1).unwrap_err();
        assert!(refused.starts_with("wiki@2026-10-02 has `publish_text = false`"), "{refused}");
    }

    const SOURCES: &str = r#"
        [[source]]
        id = "wiki"
        kind = "data"
        licence = "LicenseRef-Wikimedia-per-record"
        fetch = { kind = "prepared" }
        version = "date"
        keep = "prepared"
        redistribute = true
        publish_text = false
    "#;

    /// `tools.wiki` as a script that logs each step to `calls`: `fail-<step>` fails a step, `import`
    /// keeps its inputs, `candidates` writes one castle, `articles` writes the file `imports` as its
    /// finished imports, `commons` is busy once after `busy-once`,
    /// `facts` gives no fact, so the compile of the castle fails, and `shards` writes two shards
    /// that `done.json` does not name, and the index.
    const TOOLS: &str = r#"step=$1; shift; echo "$step" >> calls
while [ $# -gt 1 ]; do case "$1" in --out) out=$2;; --done) done=$2;; --counts) counts=$2;; --selected) selected=$2;; --inputs) inputs=$2;; --work) work=$2;; esac; shift; done
[ -e "fail-$step" ] && exit 1
case "$step" in
facts) mkdir -p "$out"; echo "$*" > facts.args ;;
commons) if [ -e busy-once ]; then rm busy-once; mkdir -p "$work"; touch "$work/busy"; exit 1; fi ;;
subjects) echo '{"osm":"n1","wikidata":"Q1"}' > "$out" ;;
import) cp "$inputs" inputs.seen ;;
candidates) printf '%s\n' '{"class":{"kind":"entity","key":"Q23413","status":"present","entity":{"claims":{}}}}' \
    '{"subject":{"kind":"entity","key":"Q1","status":"present","entity":{"claims":{"P31":[{"rank":"normal","mainsnak":{"datavalue":{"value":{"id":"Q23413"}}}}]}}}}' > "$out" ;;
articles) cp "$selected" selected.seen; mkdir -p "$work"; cp imports "$work/imports.json" 2>/dev/null || : > "$work/imports.json" ;;
shards)
    cp "$done" done.seen; cp "$counts" counts.seen; echo "$*" > shards.args; mkdir -p "$out/cells"
    for shard in common.tar.zst cells/0300-0262.tar.zst; do grep -q "$shard" "$done" || echo "$shard" > "$out/$shard"; done
    echo '{"shards":[{"path":"common.tar.zst"},{"path":"cells/0300-0262.tar.zst"}],"counts":{}}' > "$out/index.json" ;;
esac
"#;

    fn script(_: &Path) -> Result<Command, String> {
        let mut command = Command::new("sh");
        command.arg("tools.sh");
        Ok(command)
    }

    #[test]
    fn a_resume_runs_no_step_again_whose_output_it_has_and_moves_only_new_shards() {
        let temp = temp("wiki-resume");
        let (root, dumps) = (temp.0.join("repository"), temp.0.join("dumps"));
        fs::create_dir_all(root.join("data")).unwrap();
        fs::create_dir_all(dumps.join("europe")).unwrap();
        fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        fs::write(root.join("tools.sh"), TOOLS).unwrap();
        let mut names = vec![
            "wikidata-20260901-all.json.bz2".to_string(),
            "wikidata-20260915-all.json.bz2".into(),
            "wikidatawiki-20260915-page.sql.gz".into(),
            "wikidatawiki-20260915-redirect.sql.gz".into(),
            "europe/dach-260914.osm.pbf".into(),
        ];
        for language in ["en", "de"] {
            names.push(format!("{language}wiki-20260920-langlinks.sql.gz"));
            names.push(format!("{language}wiki-NS0-20260920-ENTERPRISE-HTML.json.tar.gz"));
        }
        for name in &names {
            fs::write(dumps.join(name), name).unwrap();
        }
        std::env::set_var(DUMPS, &dumps);
        let (store, archive) = (Store::at(temp.0.join("store")), temp.0.join("archive"));
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (Wiki { tools: script, wait: Duration::ZERO }, Http::new());
        let languages = producer.languages(&["de".into(), "en".into(), "de".into()]).unwrap();
        assert_eq!(languages, ["en", "de"]);
        assert!(producer.languages(&["xx".into()]).is_err());
        let scope = Scope {
            region: "europe/dach".into(),
            definitions: [(
                "europe/dach".into(),
                "name = \"DACH\"\nkind = \"geofabrik\"\nareas = [\"europe/dach\"]\n".into(),
            )]
            .into(),
        };
        let plan = producer
            .plan(
                &obc_data::prepared::Planning { store: &store, http: &http, sources: &sources },
                &sources[0],
                Some(&scope),
                &languages,
            )
            .unwrap();
        assert!(plan.inputs.iter().all(|input| input.wanted.source != "osm-planet"));
        let read: Vec<String> = plan
            .inputs
            .iter()
            .map(|input| input.wanted.params.iter().find(|(key, _)| key == "file").unwrap().1.clone())
            .collect();
        assert_eq!((read.len(), read[0].as_str()), (8, "wikidata-20260915-all.json.bz2"), "the newest of each kind");
        let prepare = |sources: &[Source]| {
            let _ = fs::remove_file(root.join("calls"));
            let mut run = Run::create(&store, ["source", "prepare", "wiki"]).unwrap();
            let opening = Opening { root: &root, store: &store, http: &http, sources, home: &archive };
            let done = Job::open(opening, ("wiki", "2026-10-01"), Some(scope.clone()), &languages, &mut run).and_then(
                |mut job| {
                    job.plan = plan.clone();
                    producer.prepare(&mut job)
                },
            );
            (done, fs::read_to_string(root.join("calls")).unwrap_or_default())
        };

        fs::write(root.join("fail-articles"), "").unwrap();
        let (done, calls) = prepare(&sources);
        assert!(done.unwrap_err().ends_with("`sh tools.sh articles` failed: exit status: 1"));
        assert_eq!(calls, "subjects\nimport\ncandidates\narticles\n");

        // A newer dump lands: the version keeps the dumps of its first run.
        fs::write(dumps.join("wikidata-20260929-all.json.bz2"), "newer").unwrap();
        fs::remove_file(root.join("fail-articles")).unwrap();
        fs::write(root.join("busy-once"), "").unwrap();
        let (done, calls) = prepare(&sources);
        done.unwrap();
        let rounds = "commons\ncommons\nfacts\ncommons\nfacts\ncommons\n";
        assert_eq!(calls, format!("import\narticles\n{rounds}shards\n"), "a busy Commons is asked again");
        let inputs: Value = read_json(&root.join("inputs.seen")).unwrap();
        let wikidata = dumps.join("wikidata-20260915-all.json.bz2");
        assert_eq!(inputs["wikidata"]["path"], json!(wikidata));
        assert_eq!(inputs["languages"], json!(["en", "de"]));
        assert_eq!(inputs["osm"]["europe/dach"]["path"], json!(dumps.join("europe/dach-260914.osm.pbf")));
        assert_eq!(inputs["wikidata"]["sha256"], json!(hash_file(&wikidata).unwrap().0));
        assert_eq!(fs::read_to_string(root.join("selected.seen")).unwrap(), r#"["Q1"]"#);
        for step in ["facts", "shards"] {
            assert!(fs::read_to_string(root.join(format!("{step}.args"))).unwrap().contains("--no-text"), "{step}");
        }
        let counts: Value = read_json(&root.join("counts.seen")).unwrap();
        assert_eq!(counts, json!({"photo_compile_failures": 1}), "the castle has no facts");
        let version = archive.join("prepared/wiki/2026-10-01");
        for path in ["common.tar.zst", "cells/0300-0262.tar.zst", "index.json"] {
            assert!(version.join(path).is_file(), "{path}");
        }

        // A run after the shards, as after a stop before the manifest: no shard is written again,
        // and no round asks the compilers again.
        let (done, calls) = prepare(&sources);
        done.unwrap();
        assert_eq!(calls, "import\narticles\ncommons\ncommons\ncommons\nshards\n");
        let done: BTreeMap<String, Value> = read_json(&root.join("done.seen")).unwrap();
        assert_eq!(done.keys().collect::<Vec<_>>(), ["cells/0300-0262.tar.zst", "common.tar.zst", "index.json"]);

        // An import that reads its input again, as after a parser fix, starts the rounds again.
        fs::write(root.join("imports"), r#"[["wikipedia:en","ab",2]]"#).unwrap();
        let (done, calls) = prepare(&sources);
        done.unwrap();
        assert_eq!(calls, "import\narticles\ncommons\nfacts\ncommons\nfacts\ncommons\nshards\n");

        // A copy of a dump with another modification time is the same dump.
        let pages = fs::File::options().write(true).open(dumps.join("wikidatawiki-20260915-page.sql.gz")).unwrap();
        pages.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000)).unwrap();
        prepare(&sources).0.unwrap();
        let mut published = sources.clone();
        published[0].publish_text = Some(true);
        let refused = prepare(&published).0.unwrap_err();
        assert!(refused.contains("`publish_text` changed") && refused.contains("Fix: delete "), "{refused}");
        fs::write(dumps.join("europe/dach-260914.osm.pbf"), "another planet").unwrap();
        let refused = prepare(&sources).0.unwrap_err();
        assert!(refused.contains("dach-260914.osm.pbf is gone or changed"), "{refused}");
        assert!(refused.contains("; the next run starts a new version"), "{refused}");
    }
}
