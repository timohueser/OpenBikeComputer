//! The producer of `wiki`: the Wikimedia facts and the article text of the OSM subjects, in
//! geographic shards (`specs/wikimedia-content.md`, "The `wiki` source"). It reads the dumps in the
//! directory that `OBC_WIKI_DUMPS` names, in place. The steps of `tools.wiki` do the imports and
//! write the shards; this producer selects the subjects with the rule of the compilers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;

use obc_data::prepared::{Job, Plan, Prepared, Scope};
use obc_data::product::Wanted;
use obc_data::settings::Settings;
use obc_data::sources::Source;
use obc_data::store::{hash_file, write_atomic};
use obc_pack::landmarks::shared::Landmarks;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The directory of the dumps on the machine that prepares.
const DUMPS: &str = "OBC_WIKI_DUMPS";
/// The shards of one upload.
const BATCH: usize = 64;

pub struct Wiki {
    /// `python -m tools.wiki`, as a command in the repository root.
    pub tools: fn(&Path) -> Result<Command, String>,
}

pub const WIKI: Wiki = Wiki { tools };

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

    /// The newest dump of each kind, read in place: the review shows its day, its file and its
    /// size. The number of shards is not known before the run.
    fn plan(&self, _: &Source, _: Option<&Scope>) -> Result<Plan, String> {
        let Some(dir) = dumps_dir() else { return Ok(Plan { inputs: Vec::new(), files: None }) };
        let inputs = dumps(&dir)?.into_iter().map(|dump| (dump.wanted(), Some(dump.size))).collect();
        Ok(Plan { inputs, files: None })
    }

    /// Each step resumes: a step whose output is in `state/` does not run again, and `tools.wiki`
    /// does not repeat an import that finished with the same input.
    fn prepare(&self, job: &mut Job) -> Result<(), String> {
        let pin = pin(job)?;
        let state = job.state.clone();
        let (work, subjects, selected) =
            (state.join("work"), state.join("subjects.ndjson"), state.join("selected.json"));
        let mut paths = BTreeMap::new();
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
            let planet = &pin.dumps.iter().find(|dump| dump.key == "osm").expect("the dumps have a planet").path;
            let mut command = step("subjects")?;
            command.arg("--osm").arg(planet).arg("--out").arg(&subjects);
            if let Some([west, south, east, north]) = job.scope.as_ref().map(bounds).transpose()? {
                command.arg(format!("--bbox={west},{south},{east},{north}"));
            }
            job.program(&mut command)?;
        }
        job.program(step("import")?.arg("--inputs").arg(&inputs).arg("--subjects").arg(&subjects))?;
        if !selected.exists() {
            let candidates = state.join("candidates.jsonl");
            job.program(step("candidates")?.arg("--out").arg(&candidates))?;
            write_atomic(&selected, &serde_json::to_vec(&select(&candidates)?).expect("ids serialize"))?;
        }
        job.program(step("articles")?.arg("--inputs").arg(&inputs).arg("--selected").arg(&selected))?;
        // The shards that the target has are not written again.
        let done: BTreeMap<&str, Value> =
            job.files().map(|file| (file.path.as_str(), json!({"sha256": file.sha256, "bytes": file.size}))).collect();
        let done_file = state.join("done.json");
        write_atomic(&done_file, &serde_json::to_vec(&done).expect("done serializes"))?;
        let mut command = step("shards")?;
        command.arg("--out").arg(&job.work).arg("--done").arg(&done_file);
        if !pin.publish_text {
            command.arg("--no-text");
        }
        job.program(&mut command)?;
        let index: Value = read_json(&job.work.join("index.json"))?;
        let shards = index["shards"].as_array().ok_or("the index of the shards lists no shard")?;
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
}

impl Dump {
    fn wanted(&self) -> Wanted {
        let name = self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        Wanted { source: self.source.clone(), version: Some(self.day.clone()), params: vec![("file".into(), name)] }
    }
}

/// What the first run of a version read, in `state/pin.json`: a resume reads exactly that again.
#[derive(Serialize, Deserialize)]
struct Pin {
    publish_text: bool,
    dumps: Vec<Dump>,
}

/// The pin of the version of `job`. The first run takes the newest dumps and hashes them once; a
/// resume refuses when a pinned dump is gone or changed, or `publish_text` changed.
fn pin(job: &Job) -> Result<Pin, String> {
    let file = job.state.join("pin.json");
    let publish_text = job.source.publish_text != Some(false);
    let dir = job.state.parent().unwrap_or(&job.state).display();
    let again = format!("Fix: delete {dir}; the next run starts a new version");
    if file.exists() {
        let pin: Pin = read_json(&file)?;
        if pin.publish_text != publish_text {
            return Err(format!("`publish_text` changed since the first run of the version. {again}"));
        }
        if let Some(dump) = pin.dumps.iter().find(|dump| stat(&dump.path).ok() != Some((dump.size, dump.modified))) {
            return Err(format!(
                "{} is gone or changed since the first run of the version. {again}",
                dump.path.display()
            ));
        }
        return Ok(pin);
    }
    let dir = dumps_dir().ok_or_else(|| format!("{DUMPS} names no directory of dumps"))?;
    let mut dumps = dumps(&dir)?;
    for dump in &mut dumps {
        dump.sha256 = hash_file(&dump.path)?.0;
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

/// The newest dump of each kind in `dir`, by the upstream names of the files. A kind without a
/// file refuses, and names the file that it needs.
fn dumps(dir: &Path) -> Result<Vec<Dump>, String> {
    let languages: Vec<(String, String)> =
        serde_json::from_slice(obc_pack::landmarks::LANGUAGE_BYTES).map_err(|e| e.to_string())?;
    let mut kinds = vec![
        ("wikidata".to_string(), "wikidata", "wikidata-".to_string(), "-all.json.bz2".to_string()),
        ("wikidata_pages".into(), "wikidata", "wikidatawiki-".into(), "-page.sql.gz".into()),
        ("wikidata_redirects".into(), "wikidata", "wikidatawiki-".into(), "-redirect.sql.gz".into()),
        ("osm".into(), "osm-planet", "planet-".into(), ".osm.pbf".into()),
    ];
    for (language, _) in &languages {
        kinds.push((
            format!("langlinks:{language}"),
            "wikipedia",
            format!("{language}wiki-"),
            "-langlinks.sql.gz".into(),
        ));
        let html = "-ENTERPRISE-HTML.json.tar.gz".to_string();
        kinds.push((format!("wikipedia:{language}"), "wikipedia", format!("{language}wiki-NS0-"), html));
    }
    let entries = fs::read_dir(dir).map_err(|e| format!("{DUMPS} {}: {e}", dir.display()))?;
    let names: Vec<String> = entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
    let mut found = Vec::new();
    for (key, source, prefix, suffix) in kinds {
        // A day of a dump is `YYYYMMDD`; the day of a planet is `YYMMDD`.
        let day = |name: &String| {
            let digits = name.strip_prefix(&prefix)?.strip_suffix(&suffix)?;
            let digits = if key == "osm" { format!("20{digits}") } else { digits.to_string() };
            let day = format!("{}-{}-{}", digits.get(..4)?, digits.get(4..6)?, digits.get(6..)?);
            (digits.len() == 8 && obc_data::date::parse(&day).is_some()).then_some(day)
        };
        let newest = names.iter().filter_map(|name| Some((day(name)?, name))).max();
        let (day, name) = newest.ok_or_else(|| format!("{DUMPS} {} has no `{prefix}<day>{suffix}`", dir.display()))?;
        let path = dir.join(name);
        let (size, modified) = stat(&path)?;
        found.push(Dump { key, source: source.into(), day, path, size, modified, sha256: String::new() });
    }
    Ok(found)
}

/// The ids whose articles the compilers read: each subject that the landmark rule of the
/// compilers selects, and each peak. The classes come first in `candidates`.
fn select(candidates: &Path) -> Result<BTreeSet<String>, String> {
    let file = fs::File::open(candidates).map_err(|e| format!("{}: {e}", candidates.display()))?;
    let (mut landmarks, mut selected) = (Landmarks::new(), BTreeSet::new());
    for line in std::io::BufReader::new(file).lines() {
        let value: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if let Some(class) = value.get("class") {
            landmarks.class(class);
        } else if let Some(subject) = value.get("subject").filter(|subject| landmarks.selects(subject)) {
            selected.extend(subject["key"].as_str().map(String::from));
        } else {
            selected.extend(value["peak"].as_str().map(String::from));
        }
    }
    Ok(selected)
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
    use obc_data::fetch::http::Http;
    use obc_data::prepared::Opening;
    use obc_data::sources::Registry;
    use obc_data::store::Store;
    use obc_data::target::Target;

    #[test]
    fn the_selection_is_each_subject_of_a_landmark_category_and_each_peak() {
        let temp = temp("wiki-select");
        fs::create_dir_all(&temp.0).unwrap();
        let fact = |key: &str, classes: &[(&str, &str)]| {
            let mut claims = serde_json::Map::new();
            for (property, id) in classes {
                let claim = json!({"rank": "normal", "mainsnak": {"datavalue": {"value": {"id": id}}}});
                claims.entry(property.to_string()).or_insert_with(|| json!([])).as_array_mut().unwrap().push(claim);
            }
            json!({"kind": "entity", "key": key, "status": "present", "identity": key, "entity": {"claims": claims}})
        };
        // A castle, and a subject whose class has no category.
        let lines = [
            json!({"class": fact("Q23413", &[])}),
            json!({"class": fact("Q5", &[])}),
            json!({"subject": fact("Q1", &[("P31", "Q23413")])}),
            json!({"subject": fact("Q2", &[("P31", "Q5")])}),
            json!({"peak": "wiki-en-7"}),
        ];
        let candidates = temp.0.join("candidates.jsonl");
        fs::write(&candidates, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        assert_eq!(select(&candidates).unwrap(), ["Q1".to_string(), "wiki-en-7".into()].into());
    }

    const SOURCES: &str = r#"
        [[source]]
        id = "wiki"
        kind = "data"
        licence = "LicenseRef-Wikimedia-per-record"
        fetch = { kind = "prepared" }
        version = "date"
        redistribute = true
        publish_text = false
    "#;

    /// `tools.wiki` as a script that logs each step to `calls`: `fail-<step>` fails a step, `import`
    /// keeps its inputs, `candidates` writes one castle, and `shards` writes two shards that
    /// `done.json` does not name, and the index.
    const TOOLS: &str = r#"step=$1; shift; echo "$step" >> calls
while [ $# -gt 1 ]; do case "$1" in --out) out=$2;; --done) done=$2;; --selected) selected=$2;; --inputs) inputs=$2;; esac; shift; done
[ -e "fail-$step" ] && exit 1
case "$step" in
subjects) echo '{"osm":"n1","wikidata":"Q1"}' > "$out" ;;
import) cp "$inputs" inputs.seen ;;
candidates) printf '%s\n' '{"class":{"kind":"entity","key":"Q23413","status":"present","entity":{"claims":{}}}}' \
    '{"subject":{"kind":"entity","key":"Q1","status":"present","entity":{"claims":{"P31":[{"rank":"normal","mainsnak":{"datavalue":{"value":{"id":"Q23413"}}}}]}}}}' > "$out" ;;
articles) cp "$selected" selected.seen ;;
shards)
    cp "$done" done.seen; echo "$*" > shards.args; mkdir -p "$out/cells"
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
    fn a_resume_runs_no_step_again_whose_output_it_has_and_uploads_only_new_shards() {
        let temp = temp("wiki-resume");
        let (root, dumps) = (temp.0.join("repository"), temp.0.join("dumps"));
        fs::create_dir_all(root.join("data")).unwrap();
        fs::create_dir_all(&dumps).unwrap();
        fs::write(root.join("data/sources.toml"), SOURCES).unwrap();
        fs::write(root.join("tools.sh"), TOOLS).unwrap();
        let mut names = vec![
            "wikidata-20260901-all.json.bz2".to_string(),
            "wikidata-20260915-all.json.bz2".into(),
            "wikidatawiki-20260915-page.sql.gz".into(),
            "wikidatawiki-20260915-redirect.sql.gz".into(),
            "planet-260914.osm.pbf".into(),
        ];
        for language in ["en", "de", "fr", "es"] {
            names.push(format!("{language}wiki-20260920-langlinks.sql.gz"));
            names.push(format!("{language}wiki-NS0-20260920-ENTERPRISE-HTML.json.tar.gz"));
        }
        for name in &names {
            fs::write(dumps.join(name), name).unwrap();
        }
        std::env::set_var(DUMPS, &dumps);
        let (store, target) = (Store::at(temp.0.join("store")), Target::Dir(temp.0.join("target")));
        let sources = Registry::load(&root).unwrap().sources;
        let (producer, http) = (Wiki { tools: script }, Http::new());
        let plan = producer.plan(&sources[0], None).unwrap();
        let read: Vec<String> = plan.inputs.iter().map(|(wanted, _)| wanted.params[0].1.clone()).collect();
        assert_eq!((read.len(), read[0].as_str()), (12, "wikidata-20260915-all.json.bz2"), "the newest of each kind");
        let prepare = |sources: &[Source]| {
            let _ = fs::remove_file(root.join("calls"));
            let mut run = Run::create(&store, ["source", "prepare", "wiki"]).unwrap();
            let opening = Opening { root: &root, store: &store, http: &http, sources, target: ("t", &target) };
            let done = Job::open(opening, ("wiki", "2026-10-01"), None, &mut run)
                .and_then(|mut job| producer.prepare(&mut job));
            (done, fs::read_to_string(root.join("calls")).unwrap_or_default())
        };

        fs::write(root.join("fail-articles"), "").unwrap();
        let (done, calls) = prepare(&sources);
        assert!(done.is_err());
        assert_eq!(calls, "subjects\nimport\ncandidates\narticles\n");

        // A newer dump lands: the version keeps the dumps of its first run.
        fs::write(dumps.join("wikidata-20260929-all.json.bz2"), "newer").unwrap();
        fs::remove_file(root.join("fail-articles")).unwrap();
        let (done, calls) = prepare(&sources);
        done.unwrap();
        assert_eq!(calls, "import\narticles\nshards\n", "the subjects and the selection stay");
        let inputs: Value = read_json(&root.join("inputs.seen")).unwrap();
        let wikidata = dumps.join("wikidata-20260915-all.json.bz2");
        assert_eq!(inputs["wikidata"]["path"], json!(wikidata));
        assert_eq!(inputs["wikidata"]["sha256"], json!(hash_file(&wikidata).unwrap().0));
        assert_eq!(fs::read_to_string(root.join("selected.seen")).unwrap(), r#"["Q1"]"#);
        assert!(fs::read_to_string(root.join("shards.args")).unwrap().contains("--no-text"));
        let Target::Dir(dir) = &target else { unreachable!() };
        let version = dir.join("sources/wiki/2026-10-01");
        for path in ["common.tar.zst", "cells/0300-0262.tar.zst", "index.json"] {
            assert!(version.join(path).is_file(), "{path}");
        }

        // A run after the shards, as after a stop before the manifest: no shard is written again.
        prepare(&sources).0.unwrap();
        let done: BTreeMap<String, Value> = read_json(&root.join("done.seen")).unwrap();
        assert_eq!(done.keys().collect::<Vec<_>>(), ["cells/0300-0262.tar.zst", "common.tar.zst", "index.json"]);

        let mut published = sources.clone();
        published[0].publish_text = Some(true);
        let refused = prepare(&published).0.unwrap_err();
        assert!(refused.contains("`publish_text` changed") && refused.contains("Fix: delete "), "{refused}");
        fs::write(dumps.join("planet-260914.osm.pbf"), "another planet").unwrap();
        let refused = prepare(&sources).0.unwrap_err();
        assert!(refused.contains("planet-260914.osm.pbf is gone or changed"), "{refused}");
        assert!(refused.contains("; the next run starts a new version"), "{refused}");
    }
}
