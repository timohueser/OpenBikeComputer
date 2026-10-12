//! The Commons rounds of `wiki`: the metadata and the 500 px thumbnails of the photos that the
//! compilers choose for the selected subjects, asked as `device/shared_content.rs` asks a region.
//! Round 0 names the categories of the subjects, round 1 the metadata of their photo candidates,
//! and round 2 the files that the compilers choose. `tools.wiki commons` gets the requests of all
//! rounds so far, and asks Commons only for what it has not answered.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use obc_data::prepared::Job;
use obc_data::store::write_atomic;
use obc_pack::landmarks::peaks::{Summit, SummitSource};
use obc_pack::landmarks::shared;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The subjects or peaks of one `facts` call.
const CHUNK: usize = 500;
/// The longest wait after "Wikimedia is busy".
const LONGEST: Duration = Duration::from_secs(2 * 3600);

/// What `tools.wiki commons` asks for one subject.
#[derive(Default, Serialize, Deserialize)]
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

/// Run the rounds after round 0, each once: `state/requests-<round>.json` keeps what it asked.
/// `wait` is the first wait after "Wikimedia is busy"; it doubles up to two hours.
pub(super) fn rounds(job: &Job, step: &Step, wait: Duration, planet: &str) -> Result<(), String> {
    let state = job.state.clone();
    let selection: Selection = super::read_json(&state.join("selection.json"))?;
    let peaks = summits(&state.join("subjects.ndjson"))?;
    let mut requests = selection.requests;
    for round in 0..3 {
        let file = state.join(format!("requests-{round}.json"));
        if file.exists() {
            requests = super::read_json(&file)?;
        } else {
            if round > 0 {
                ask(job, step, (&selection.landmarks, &peaks), planet, round, &mut requests)?;
            }
            write_atomic(&file, &serde_json::to_vec(&requests).expect("requests serialize"))?;
        }
        commons(job, step, &file, wait)?;
    }
    Ok(())
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

/// Add the requests of `round` for the landmarks and the peaks, a chunk at a time.
fn ask(
    job: &Job,
    step: &Step,
    (landmarks, peaks): (&BTreeSet<String>, &[Summit]),
    planet: &str,
    round: u32,
    requests: &mut Requests,
) -> Result<(), String> {
    let (view, out) = (job.state.join("view"), job.state.join("view-out"));
    let world = job.state.join("world.geojson");
    let corners = [[-180.0, -90.0], [180.0, -90.0], [180.0, 90.0], [-180.0, 90.0], [-180.0, -90.0]];
    let square = json!({"type": "Polygon", "coordinates": [corners]});
    write_atomic(&world, square.to_string().as_bytes())?;
    let landmarks: Vec<String> = landmarks.iter().cloned().collect();
    for ids in landmarks.chunks(CHUNK) {
        let files = facts(job, step, ids, &[])?;
        match round {
            1 => candidates(&files, ids, requests)?,
            _ => chosen(ids, requests, |ids| {
                fresh(&[&view, &out])?;
                shared::view(&files, &view, ids)?;
                let asked = obc_pack::landmarks::photo_requests(&view.join("manifest.json"), &world, &out, None)?;
                Ok(asked.requests.into_iter().map(|request| (request.qid, request.filename)).collect())
            })?,
        }
    }
    for summits in peaks.chunks(CHUNK) {
        let links: Vec<[Option<&String>; 2]> =
            summits.iter().map(|summit| [summit.tags.get("wikidata"), summit.tags.get("wikipedia")]).collect();
        let files = facts(job, step, &[], &links)?;
        let present =
            shared::facts(&files)?.into_values().filter(|fact| fact["kind"] == "link" && fact["status"] == "present");
        let ids: Vec<String> = present.filter_map(|fact| fact["identity"].as_str().map(String::from)).collect();
        match round {
            1 => candidates(&files, &ids, requests)?,
            _ => chosen(summits, requests, |summits| {
                fresh(&[&view, &out])?;
                fs::create_dir_all(&view).map_err(|e| e.to_string())?;
                let source =
                    SummitSource { schema: 1, osm_sha256: planet.into(), summits: summits.iter().map(copy).collect() };
                fs::write(view.join("summits.json"), serde_json::to_vec(&source).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                shared::peak_view(&files, &view)?;
                let content = obc_pack::landmarks::peaks::compile(&view.join("manifest.json"), &world, &out, true)?;
                Ok(content.photo_requests.into_iter().map(|request| (request.qid, request.filename)).collect())
            })?,
        }
    }
    Ok(())
}

/// The files that the compilers choose in `items`; when the compile of the chunk fails, the
/// compile of each item, and an item that fails alone has no photo in this version.
fn chosen<T>(
    items: &[T],
    requests: &mut Requests,
    compile: impl Fn(&[T]) -> Result<Vec<(String, String)>, String>,
) -> Result<(), String> {
    let asked = compile(items).or_else(|_| {
        let each = items.chunks(1).filter_map(|item| compile(item).ok());
        Ok::<_, String>(each.flatten().collect())
    })?;
    for (id, filename) in asked {
        requests.entry(id).or_default().files.insert(filename);
    }
    Ok(())
}

/// The metadata that round 1 asks for: the P18 files, the lead images of the articles and the
/// members of the categories of each subject `ids`, as the compilers rank them.
fn candidates(files: &BTreeMap<String, PathBuf>, ids: &[String], requests: &mut Requests) -> Result<(), String> {
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

/// The facts that the compilers read of the subjects `ids` and of the peaks with the tags
/// `links`, by name, from `tools.wiki facts`.
fn facts(
    job: &Job,
    step: &Step,
    ids: &[String],
    links: &[[Option<&String>; 2]],
) -> Result<BTreeMap<String, PathBuf>, String> {
    let dir = job.state.join("facts");
    fresh(&[&dir])?;
    let (subjects, peaks) = (job.state.join("facts-ids.json"), job.state.join("facts-links.json"));
    write_atomic(&subjects, &serde_json::to_vec(ids).expect("ids serialize"))?;
    write_atomic(&peaks, &serde_json::to_vec(links).expect("links serialize"))?;
    job.program(step("facts")?.arg("--selected").arg(&subjects).arg("--requests").arg(&peaks).arg("--out").arg(&dir))?;
    let mut files = BTreeMap::new();
    for kind in fs::read_dir(dir.join("content")).into_iter().flatten().flatten() {
        for file in fs::read_dir(kind.path()).map_err(|e| e.to_string())?.flatten() {
            let name = format!("content/{}/{}", kind.file_name().to_string_lossy(), file.file_name().to_string_lossy());
            files.insert(name, file.path());
        }
    }
    Ok(files)
}

/// The peaks of `subjects.ndjson`: each node with `natural=peak` and its tags.
fn summits(subjects: &Path) -> Result<Vec<Summit>, String> {
    let text = fs::read_to_string(subjects).map_err(|e| format!("{}: {e}", subjects.display()))?;
    let mut summits = Vec::new();
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let node = value["osm"].as_str().and_then(|osm| osm.strip_prefix('n')?.parse().ok());
        let (Some(node_id), Some(true), Some(tags)) = (node, value["peak"].as_bool(), value.get("tags")) else {
            continue;
        };
        let [longitude, latitude] =
            [&value["box"][0], &value["box"][1]].map(|value| value.as_f64().unwrap_or(f64::NAN));
        let tags = serde_json::from_value(tags.clone()).map_err(|e| e.to_string())?;
        summits.push(Summit { node_id, latitude, longitude, tags });
    }
    Ok(summits)
}

fn copy(summit: &Summit) -> Summit {
    Summit { tags: summit.tags.clone(), ..*summit }
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
    }
}
