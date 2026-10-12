//! The Commons rounds of `wiki`: the metadata and the 500 px thumbnails of the photos that the
//! compilers choose for the selected subjects, asked as `device/shared_content.rs` asks a region.
//! Round 0 names the categories of the subjects, round 1 the metadata of their photo candidates,
//! and round 2 the files that the compilers choose. A thumbnail that fails the checks of the
//! compilers makes them choose another file, so round 2 repeats for the subjects with new files
//! until a round asks no new file. `tools.wiki commons` gets the requests of all rounds so far,
//! and asks Commons only for what it has not answered.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use obc_data::prepared::Job;
use obc_data::store::{sha256_hex, write_atomic};
use obc_pack::landmarks::peaks::{self, Summit, SummitSource};
use obc_pack::landmarks::shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{read_json, Pin};

/// The subjects or peaks of one `facts` call.
const CHUNK: usize = 500;
/// The longest wait after "Wikimedia is busy".
const LONGEST: Duration = Duration::from_secs(2 * 3600);

/// What `tools.wiki commons` asks for one subject.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Request {
    pub categories: BTreeSet<String>,
    pub commons: BTreeSet<String>,
    pub files: BTreeSet<String>,
}

pub(super) type Requests = BTreeMap<String, Request>;

/// The subjects of the compilers, as round 0 selects them, and its requests.
#[derive(Default, Serialize, Deserialize)]
pub(super) struct Selection {
    pub landmarks: BTreeSet<String>,
    pub peaks: BTreeSet<String>,
    pub requests: Requests,
}

impl Selection {
    /// Ask for the Commons categories of a subject of the compilers.
    pub(super) fn categories(&mut self, id: &str, entity: &Value) {
        self.requests.entry(id.into()).or_default().categories.extend(shared::categories(entity));
    }
}

/// A command of a step of `tools.wiki`.
pub(super) type Step<'a> = dyn Fn(&str) -> Result<Command, String> + 'a;

/// The facts of one `facts` call, by name.
type Files = BTreeMap<String, PathBuf>;

/// The files that a compile chooses, as `(subject, file)`. `Err` is an I/O error, which ends the
/// run; `Ok(Err)` is a compile that refuses its subjects.
type Compiled = Result<Result<Vec<(String, String)>, String>, String>;

/// Run the rounds: `state/requests-<round>.json` keeps what each round asked, and
/// `state/failed.json` the subjects whose compile failed in it. `wait` is the first wait after
/// "Wikimedia is busy"; it doubles up to two hours. Gives the number of failed compiles.
pub(super) fn rounds(job: &Job, step: &Step, wait: Duration, pin: &Pin) -> Result<usize, String> {
    let state = &job.state;
    let selection: Selection = read_json(&state.join("selection.json"))?;
    let failed_file = state.join("failed.json");
    let mut failed: BTreeMap<u32, Vec<String>> =
        if failed_file.exists() { read_json(&failed_file)? } else { BTreeMap::new() };
    let peaks = summits(&state.join("subjects.ndjson"))?;
    let asking = Asking { job, step, pin, landmarks: &selection.landmarks, peaks: &peaks };
    let (mut before, mut requests) = (Requests::new(), selection.requests);
    for round in 0u32.. {
        let file = state.join(format!("requests-{round}.json"));
        if file.exists() {
            (before, requests) = (requests, read_json(&file)?);
        } else {
            let mut asked = requests.clone();
            if round > 0 {
                // A repeat of round 2 compiles only the subjects with new files.
                let none = BTreeSet::new();
                let again: Option<BTreeSet<String>> = (round > 2).then(|| {
                    let new = |(id, request): &(&String, &Request)| {
                        request.files != *before.get(*id).map_or(&none, |request| &request.files)
                    };
                    requests.iter().filter(new).map(|(id, _)| id.clone()).collect()
                });
                if again.as_ref().is_some_and(BTreeSet::is_empty) {
                    break;
                }
                failed.insert(round, asking.ask(round, again.as_ref(), &mut asked)?);
                write_atomic(&failed_file, &serde_json::to_vec(&failed).expect("failures serialize"))?;
                if round > 2 && asked == requests {
                    break;
                }
            }
            write_atomic(&file, &serde_json::to_vec(&asked).expect("requests serialize"))?;
            (before, requests) = (requests, asked);
        }
        commons(job, step, &file, wait)?;
    }
    Ok(failed.values().map(Vec::len).sum())
}

/// Call `commons` until it ends without "Wikimedia is busy".
fn commons(job: &Job, step: &Step, requests: &Path, mut wait: Duration) -> Result<(), String> {
    let busy = job.state.join("work/busy");
    loop {
        let _ = fs::remove_file(&busy);
        match job.program(step("commons")?.arg("--requests").arg(requests)) {
            Err(_) if busy.exists() => {
                eprintln!("obc data: Wikimedia is busy; `wiki` asks again in {} min", wait.as_secs() / 60);
                for _ in 0..wait.as_secs().div_ceil(10) {
                    std::thread::sleep(Duration::from_secs(10).min(wait));
                    job.stopped()?;
                }
                wait = (wait * 2).min(LONGEST);
            }
            done => return done,
        }
    }
}

/// What the rounds ask about: the selected landmarks and the summits of the planet.
struct Asking<'a, 'j> {
    job: &'a Job<'j>,
    step: &'a Step<'a>,
    pin: &'a Pin,
    landmarks: &'a BTreeSet<String>,
    peaks: &'a [Summit],
}

impl Asking<'_, '_> {
    /// Add the requests of `round` for the landmarks and the peaks, a chunk at a time; with
    /// `again`, for those subjects only. Gives each subject whose compile failed, with its error.
    fn ask(
        &self,
        round: u32,
        again: Option<&BTreeSet<String>>,
        requests: &mut Requests,
    ) -> Result<Vec<String>, String> {
        let state = &self.job.state;
        let (view, out, world) = (state.join("view"), state.join("view-out"), state.join("world.geojson"));
        let corners = [[-180.0, -90.0], [180.0, -90.0], [180.0, 90.0], [-180.0, 90.0], [-180.0, -90.0]];
        write_atomic(&world, json!({"type": "Polygon", "coordinates": [corners]}).to_string().as_bytes())?;
        let asked = |id: &String| again.is_none_or(|again| again.contains(id));
        let mut failed = Vec::new();
        let landmarks: Vec<String> = self.landmarks.iter().filter(|id| asked(id)).cloned().collect();
        for ids in landmarks.chunks(CHUNK) {
            let files = self.facts(ids, &[])?;
            match round {
                1 => candidates(&files, ids, requests)?,
                _ => chosen(ids, requests, &mut failed, String::clone, |ids| {
                    fresh(&[&view, &out])?;
                    Ok(landmark_photos(&files, [&view, &out, &world], ids))
                })?,
            }
        }
        let planet = &self.pin.dumps.iter().find(|dump| dump.key == "osm").expect("the dumps have a planet").sha256;
        for summits in self.peaks.chunks(CHUNK) {
            let links: Vec<[Option<&String>; 2]> =
                summits.iter().map(|summit| [summit.tags.get("wikidata"), summit.tags.get("wikipedia")]).collect();
            let files = self.facts(&[], &links)?;
            let facts = shared::facts(&files)?;
            let linked: Vec<(Summit, Vec<String>)> = summits.iter().map(|summit| linked(summit, &facts)).collect();
            if round == 1 {
                let ids: Vec<String> = linked.into_iter().flat_map(|(_, ids)| ids).collect();
                candidates(&files, &ids, requests)?;
                continue;
            }
            let summits: Vec<Summit> =
                linked.into_iter().filter(|(_, ids)| ids.iter().any(asked)).map(|(summit, _)| summit).collect();
            if summits.is_empty() {
                continue;
            }
            let name = |summit: &Summit| format!("OSM node {}", summit.node_id);
            chosen(&summits, requests, &mut failed, name, |summits| {
                fresh(&[&view, &out])?;
                let source = SummitSource { schema: 1, osm_sha256: planet.clone(), summits: summits.to_vec() };
                write_atomic(&view.join("summits.json"), &serde_json::to_vec(&source).expect("summits serialize"))?;
                Ok(peak_photos(&files, [&view, &out, &world]))
            })?;
        }
        Ok(failed)
    }

    /// The facts that the compilers read of the subjects `ids` and of the peaks with the tags
    /// `links`, by name, from `tools.wiki facts`.
    fn facts(&self, ids: &[String], links: &[[Option<&String>; 2]]) -> Result<Files, String> {
        let state = &self.job.state;
        let dir = state.join("facts");
        fresh(&[&dir])?;
        let (subjects, peaks) = (state.join("facts-ids.json"), state.join("facts-links.json"));
        write_atomic(&subjects, &serde_json::to_vec(ids).expect("ids serialize"))?;
        write_atomic(&peaks, &serde_json::to_vec(links).expect("links serialize"))?;
        let mut command = (self.step)("facts")?;
        self.job
            .program(command.arg("--selected").arg(&subjects).arg("--requests").arg(&peaks).arg("--out").arg(&dir))?;
        let mut files = BTreeMap::new();
        for kind in fs::read_dir(dir.join("content")).into_iter().flatten().flatten() {
            for file in fs::read_dir(kind.path()).map_err(|e| e.to_string())?.flatten() {
                let name =
                    format!("content/{}/{}", kind.file_name().to_string_lossy(), file.file_name().to_string_lossy());
                files.insert(name, file.path());
            }
        }
        if !self.pin.publish_text {
            without_text(&mut files, &dir)?;
        }
        Ok(files)
    }
}

/// The files that the compilers choose for `items`, in `requests`. When the compile of the chunk
/// fails, each item compiles alone: an item that fails alone has no photo in this version, and
/// `failed` gets its `name` and error. When each item of a chunk of more than one fails, the run
/// stops, as for an I/O error.
fn chosen<T>(
    items: &[T],
    requests: &mut Requests,
    failed: &mut Vec<String>,
    name: impl Fn(&T) -> String,
    compile: impl Fn(&[T]) -> Compiled,
) -> Result<(), String> {
    let asked = match compile(items)? {
        Ok(asked) => asked,
        Err(error) => {
            let (mut asked, mut errors) = (Vec::new(), Vec::new());
            for item in items {
                match compile(std::slice::from_ref(item))? {
                    Ok(more) => asked.extend(more),
                    Err(error) => errors.push(format!("{}: {error}", name(item))),
                }
            }
            if items.len() > 1 && errors.len() == items.len() {
                return Err(format!("the compile of each subject of a chunk of {} fails: {error}", items.len()));
            }
            for error in &errors {
                eprintln!("obc data: `wiki` chooses no photo for {error}");
            }
            failed.extend(errors);
            asked
        }
    };
    for (id, filename) in asked {
        requests.entry(id).or_default().files.insert(filename);
    }
    Ok(())
}

/// The files that the landmark compiler chooses for `ids`, over `view` and `out`.
fn landmark_photos(
    files: &Files,
    [view, out, world]: [&Path; 3],
    ids: &[String],
) -> Result<Vec<(String, String)>, String> {
    shared::view(files, view, ids)?;
    let asked = obc_pack::landmarks::photo_requests(&view.join("manifest.json"), world, out, None)?;
    Ok(asked.requests.into_iter().map(|request| (request.qid, request.filename)).collect())
}

/// The files that the peak compiler chooses for the summits in `view/summits.json`.
fn peak_photos(files: &Files, [view, out, world]: [&Path; 3]) -> Result<Vec<(String, String)>, String> {
    shared::peak_view(files, view)?;
    let content = peaks::compile(&view.join("manifest.json"), world, out, true)?;
    Ok(content.photo_requests.into_iter().map(|request| (request.qid, request.filename)).collect())
}

/// The metadata that round 1 asks for: the P18 files, the lead images of the articles and the
/// members of the categories of each subject `ids`, as the compilers rank them.
fn candidates(files: &Files, ids: &[String], requests: &mut Requests) -> Result<(), String> {
    let facts = shared::facts(files)?;
    let languages: Vec<(String, String)> =
        serde_json::from_slice(obc_pack::landmarks::LANGUAGE_BYTES).map_err(|e| e.to_string())?;
    let present = |kind: &str, key: String| facts.get(&(kind.into(), key)).filter(|fact| fact["status"] == "present");
    for id in ids {
        let Some(entity) = shared::subject(&facts, id) else { continue };
        let request = requests.entry(id.clone()).or_default();
        let claims = entity["claims"]["P18"].as_array().into_iter().flatten();
        request
            .commons
            .extend(claims.filter_map(|claim| claim["mainsnak"]["datavalue"]["value"].as_str()).map(String::from));
        for (language, _) in &languages {
            let Some(title) = entity["sitelinks"][format!("{language}wiki")]["title"].as_str() else { continue };
            request.commons.extend(present("article", format!("{language}:{title}")).and_then(shared::lead_image));
        }
        for category in shared::categories(&entity) {
            let members = present("category", category).map(|fact| fact["members"].clone()).unwrap_or_default();
            let titles = members.as_array().into_iter().flatten().filter_map(|member| member["title"].as_str());
            request.commons.extend(titles.filter_map(|title| title.strip_prefix("File:")).map(String::from));
        }
    }
    Ok(())
}

/// The summits of `subjects.ndjson`: each node with `natural=peak`, a point and the tags that the
/// map takes as a summit, as the regional discovery of the peak compiler finds them.
pub(super) fn summits(subjects: &Path) -> Result<Vec<Summit>, String> {
    let text = fs::read_to_string(subjects).map_err(|e| format!("{}: {e}", subjects.display()))?;
    let mut summits = Vec::new();
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let node = value["osm"].as_str().and_then(|osm| osm.strip_prefix('n')?.parse().ok());
        let (Some(node_id), Some(true), Some(tags)) = (node, value["peak"].as_bool(), value.get("tags")) else {
            continue;
        };
        let (Some(longitude), Some(latitude)) = (value["box"][0].as_f64(), value["box"][1].as_f64()) else { continue };
        let tags = serde_json::from_value(tags.clone()).map_err(|e| e.to_string())?;
        if peaks::is_summit(&tags) {
            summits.push(Summit { node_id, latitude, longitude, tags });
        }
    }
    Ok(summits)
}

/// `summit` with the link tags that have a fact only, and the identities that they link.
fn linked(summit: &Summit, facts: &BTreeMap<(String, String), Value>) -> (Summit, Vec<String>) {
    let mut tags = summit.tags.clone();
    let mut ids = Vec::new();
    for kind in ["wikidata", "wikipedia"] {
        let Some(tag) = tags.get(kind) else { continue };
        match facts.get(&("link".to_string(), format!("{kind}:{tag}"))) {
            None => drop(tags.remove(kind)),
            Some(fact) if fact["status"] == "present" => ids.extend(fact["identity"].as_str().map(String::from)),
            Some(_) => {}
        }
    }
    (Summit { tags, ..*summit }, ids)
}

/// Without published text, the compilers see each article as missing, as a version without text
/// holds no article: they choose no lead image that a reader cannot check.
fn without_text(files: &mut Files, dir: &Path) -> Result<(), String> {
    let articles: Vec<String> = files.keys().filter(|name| name.starts_with("content/article/")).cloned().collect();
    for name in articles {
        let fact: Value = read_json(&files.remove(&name).expect("a listed file"))?;
        let missing = json!({"kind": "article", "key": fact["key"], "status": "missing"});
        let bytes = serde_json::to_vec(&missing).expect("a fact serializes");
        let name = format!("content/article/{}.json", sha256_hex(&bytes));
        write_atomic(&dir.join(&name), &bytes)?;
        files.insert(name.clone(), dir.join(name));
    }
    Ok(())
}

/// Remove each folder, so that it is new and empty.
fn fresh(dirs: &[&Path]) -> Result<(), String> {
    for dir in dirs {
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::tests::temp;

    #[test]
    fn round_1_asks_for_the_photo_candidates_of_the_selected_subjects_only() {
        let temp = temp("wiki-candidates");
        let claim = |value: Value| json!([{"rank": "normal", "mainsnak": {"datavalue": {"value": value}}}]);
        let entity = |key: &str, claims: Value| {
            let sitelinks = json!({"enwiki": {"title": "Castle"}});
            json!({"kind": "entity", "key": key, "status": "present", "identity": key,
                   "entity": {"id": key, "claims": claims, "sitelinks": sitelinks}})
        };
        let lead = r#"<a class="mw-file-description" href="./File:Lead.jpg"><img src="//upload.wikimedia.org/wikipedia/commons/a/ab/Lead.jpg"></a>"#;
        let facts = [
            entity("Q1", json!({"P18": claim(json!("Hill.jpg")), "P373": claim(json!("Castles"))})),
            entity("Q2", json!({"P18": claim(json!("Other.jpg"))})),
            json!({"kind": "article", "key": "en:Castle", "status": "present", "lead_html": lead}),
            json!({"kind": "category", "key": "Category:Castles", "status": "present",
                   "members": [{"title": "File:Member.jpg"}, {"title": "Category:Sub"}]}),
        ];
        let mut files = BTreeMap::new();
        for fact in facts {
            let bytes = serde_json::to_vec(&fact).unwrap();
            let name =
                format!("content/{}/{}.json", fact["kind"].as_str().unwrap(), obc_data::store::sha256_hex(&bytes));
            let path = temp.0.join(&name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            files.insert(name, path);
        }
        let mut requests = Requests::new();
        candidates(&files, &["Q1".into()], &mut requests).unwrap();
        assert_eq!(requests.keys().collect::<Vec<_>>(), ["Q1"]);
        let asked: Vec<&str> = requests["Q1"].commons.iter().map(String::as_str).collect();
        assert_eq!(asked, ["Hill.jpg", "Lead.jpg", "Member.jpg"]);

        // Without published text, no lead image.
        without_text(&mut files, &temp.0).unwrap();
        let mut requests = Requests::new();
        candidates(&files, &["Q1".into()], &mut requests).unwrap();
        let asked: Vec<&str> = requests["Q1"].commons.iter().map(String::as_str).collect();
        assert_eq!(asked, ["Hill.jpg", "Member.jpg"]);
    }

    #[test]
    fn a_subject_whose_compile_fails_alone_is_counted_and_a_chunk_where_each_fails_stops_the_run() {
        let compile = |ids: &[&str]| -> Compiled {
            match ids {
                ["Q2"] => Ok(Err("invalid source summit".into())),
                [id] => Ok(Ok(vec![(id.to_string(), format!("{id}.jpg"))])),
                _ => Ok(Err("the chunk fails".into())),
            }
        };
        let (mut requests, mut failed) = (Requests::new(), Vec::new());
        chosen(&["Q1", "Q2", "Q3"], &mut requests, &mut failed, |id| id.to_string(), compile).unwrap();
        assert_eq!(requests.keys().collect::<Vec<_>>(), ["Q1", "Q3"]);
        assert_eq!(failed, ["Q2: invalid source summit"]);

        let stopped = chosen(&["Q2", "Q2"], &mut requests, &mut failed, |id| id.to_string(), compile).unwrap_err();
        assert!(stopped.contains("each subject of a chunk of 2 fails"), "{stopped}");
        let io = |_: &[&str]| -> Compiled { Err("No space left on device".into()) };
        let stopped = chosen(&["Q1"], &mut requests, &mut failed, |id| id.to_string(), io).unwrap_err();
        assert_eq!((stopped.as_str(), failed.len()), ("No space left on device", 1));
    }
}
