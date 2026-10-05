//! Fetchers that run a program: it writes the files of one request into a directory, and the
//! store takes them. A service answers with today's data, so another day comes from the store.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{check_version, record, Request};
use crate::date;
use crate::sources::{Registry, Source};
use crate::store::{self, FileRecord, Snapshot, Store};

/// The rasters of a national terrain model that cover `bbox=WEST,SOUTH,EAST,NORTH`, from the
/// adapter of the reference ingest (`ingest.py fetch`).
pub fn dtm(store: &Store, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let [bbox] = values(request, ["bbox"])?;
    let bbox = parse_bbox(bbox)?;
    let root = root()?;
    let key = source.id.strip_prefix("dtm-").unwrap_or(&source.id).to_string();
    capture(
        store,
        request,
        &format!("bbox={bbox}"),
        &[source],
        |_| 0,
        |work, out| {
            let mut command = python(&root, Some("tools/requirements-bake.txt"));
            command.arg("host/obc-dem/reference/ingest.py");
            command.args(["fetch", &key, &format!("--bbox={bbox}"), "--work"]).arg(work).arg("--out").arg(out);
            command
        },
    )
}

/// A source of `kind = "capture"`: the program that captures it, with the `NAME=VALUE` it takes.
pub fn run(store: &Store, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let root = root()?;
    match source.id.as_str() {
        "wikidata" | "wikipedia" | "commons" => landmarks(store, request, &root),
        "modis-snow" | "hr-wsi" => {
            let [bbox, seasons] = values(request, ["bbox", "seasons"])?;
            let bbox = parse_bbox(bbox)?;
            let years = seasons.split_once('-').filter(|(first, last)| year(first) && year(last) && first <= last);
            let (first, last) = years.ok_or_else(|| format!("`seasons={seasons}` is not FIRST-LAST, two years"))?;
            let kind = if source.id == "hr-wsi" { "copernicus-hr-wsi" } else { "nasa-modis" };
            capture(
                store,
                request,
                &format!("bbox={bbox}&seasons={seasons}"),
                &[source],
                |_| 0,
                |_, out| {
                    let mut command = python(&root, Some("tools/requirements-planner-snow.txt"));
                    command.args(["-m", "tools.planner_snow", "--source", kind, &format!("--bounds={bbox}")]);
                    command.args(["--first-season", first, "--last-season", last, "--fetch"]).arg(out);
                    command
                },
            )
        }
        "era5-land" => {
            let [bbox, first] = values(request, ["bbox", "first-year"])?;
            let bbox = parse_bbox(bbox)?;
            if !year(first) {
                return Err(format!("`first-year={first}` is not a year"));
            }
            capture(
                store,
                request,
                &format!("bbox={bbox}&first-year={first}"),
                &[source],
                |_| 0,
                |_, out| {
                    let mut command = python(&root, Some("tools/requirements-planner-climate.txt"));
                    command.args(["-m", "tools.planner_climate", &format!("--bounds={bbox}"), "--first-year", first]);
                    command.arg("--fetch").arg(out);
                    command
                },
            )
        }
        _ => Err(format!("source `{}`: its kind of fetch has no fetcher yet", source.id)),
    }
}

/// One run of `tools/landmark_capture.py` captures Wikidata, Wikipedia and Commons for a region:
/// `boundary=` and `candidates=` are its files, and `select-with=` is the `obc-bake` that selects
/// the places. Articles go to the record of `wikipedia`, images and categories to `commons`, and
/// the rest to `wikidata`.
fn landmarks(store: &Store, request: &Request, root: &Path) -> Result<Snapshot, String> {
    let [boundary, candidates, compiler] = values(request, ["boundary", "candidates", "select-with"])?;
    // The program runs in the repository root, so a path names its file from here.
    let absolute = |path: &str| std::path::absolute(path).map_err(|e| format!("{path}: {e}"));
    let (boundary, candidates, compiler) = (absolute(boundary)?, absolute(candidates)?, absolute(compiler)?);
    let policy = root.join("host/obc-pack/src/landmarks/policy.json");
    // The tool refuses another boundary, candidate list or policy in the same directory, so the three name the request.
    let mut digests = String::new();
    for file in [&boundary, &candidates, &policy] {
        digests += &store::hash_file(file)?.0;
    }
    let query = format!("recipe={}", &store::sha256_hex(digests.as_bytes())[..16]);
    let registry = Registry::load(root)?;
    let find = |id: &str| registry.sources.iter().find(|source| source.id == id).ok_or(format!("no source `{id}`"));
    let owners = [find("wikidata")?, find("wikipedia")?, find("commons")?];
    let owner = |path: &str| match path.split('/').next() {
        Some("articles") => 1,
        Some("images" | "categories") => 2,
        _ => 0,
    };
    capture(store, request, &query, &owners, owner, |_, out| {
        // The capture needs no package beyond the standard library.
        let mut command = python(root, None);
        command.arg("tools/landmark_capture.py");
        command.arg("--boundary").arg(&boundary).arg("--candidates").arg(&candidates).arg("--policy").arg(&policy);
        // A failed run keeps its directory, and each run asks once more for what failed before.
        command.arg("--select-with").arg(&compiler).arg("--retry-failed").arg("--out").arg(out);
        command
    })
}

/// The files of `request` that `query` names: from the store, or else from the program that
/// `command` makes. The program writes them into its second directory, and may keep downloads in
/// its first. `owner` gives the index in `sources` of the source whose record takes a file, from
/// its path. A run that fails keeps both directories, so the next run can resume.
pub fn capture(
    store: &Store,
    request: &Request,
    query: &str,
    sources: &[&Source],
    owner: impl Fn(&str) -> usize,
    command: impl FnOnce(&Path, &Path) -> Command,
) -> Result<Snapshot, String> {
    let source = request.source;
    let today = date::format(date::today());
    let version = request.version.clone().unwrap_or_else(|| today.clone());
    check_version(source, &version)?;
    let prefix = |source: &Source| format!("{}#{query}/", source.fetch.url.as_deref().unwrap_or_default());
    let ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
    // Not the version: a run that failed one day resumes the next.
    let key = store::sha256_hex(format!("{query} {}", ids.join(" ")).as_bytes())[..32].to_string();
    let _lock = store.lock(&format!("capture-{key}"))?;
    let snapshot = |files| Snapshot { source: source.id.clone(), version: version.clone(), files };
    let stored: Vec<FileRecord> = store.snapshot(&source.id, &version)?.map_or_else(Vec::new, |snapshot| {
        snapshot.files.into_iter().filter(|file| file.url.starts_with(&prefix(source))).collect()
    });
    if !stored.is_empty() && stored.iter().all(|file| store.object(&file.sha256).is_file()) {
        return Ok(snapshot(stored));
    }
    if version != today {
        return Err(format!(
            "source `{}`: {version} with {query} is not in the store, and the service answers with today's data",
            source.id
        ));
    }
    for source in sources {
        if let Some(credential) = source.credential.as_ref().filter(|credential| !credential.present()) {
            return Err(format!("source `{}` is blocked: credential missing: {}", source.id, credential.describe()));
        }
    }
    let staging = store.partial(&format!("capture-{key}"));
    let staging = std::path::absolute(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let (work, out) = (staging.join("work"), staging.join("out"));
    for dir in [&work, &out] {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut command = command(&work, &out);
    eprintln!("obc data: capturing {}#{query}", source.id);
    // Standard output is the answer of `obc data`, so the program writes its progress to standard error.
    let status = command.stdout(std::io::stderr()).status();
    let status = status.map_err(|e| format!("source `{}`: {:?}: {e}", source.id, command.get_program()))?;
    if !status.success() {
        return Err(format!("source `{}`: {:?} failed with {status}", source.id, command.get_program()));
    }
    let mut files: Vec<Vec<FileRecord>> = vec![Vec::new(); sources.len()];
    for path in walk(&out)? {
        let parts = path.strip_prefix(&out).unwrap_or(&path).components();
        let relative: Vec<_> = parts.map(|part| part.as_os_str().to_string_lossy()).collect();
        let relative = relative.join("/");
        let index = owner(&relative);
        let (sha256, size) = store::hash_file(&path)?;
        store.insert(&path, &sha256)?;
        files[index].push(FileRecord {
            name: relative.rsplit('/').next().unwrap_or_default().to_string(),
            url: format!("{}{relative}", prefix(sources[index])),
            size,
            sha256,
            retrieved: date::timestamp(date::now()),
        });
    }
    if files.iter().all(Vec::is_empty) {
        return Err(format!("source `{}`: the capture of {query} has no file", source.id));
    }
    for (owner, files) in sources.iter().zip(&files) {
        record(store, &owner.id, &version, files)?;
    }
    let _ = fs::remove_dir_all(&staging);
    let mine = sources.iter().position(|owner| owner.id == source.id);
    Ok(snapshot(mine.map(|index| files.swap_remove(index)).unwrap_or_default()))
}

/// The value of each of `names`, given once, and no other `NAME=VALUE`.
fn values<'a, const N: usize>(request: &'a Request, names: [&str; N]) -> Result<[&'a str; N], String> {
    let mut found = [""; N];
    for (slot, name) in found.iter_mut().zip(names) {
        let mut given = request.params.iter().filter(|(n, _)| n == name);
        if let (Some((_, value)), None) = (given.next(), given.next()) {
            *slot = value;
        }
    }
    if request.params.len() == N && found.iter().all(|value| !value.is_empty()) {
        return Ok(found);
    }
    let wanted: Vec<_> = names.iter().map(|name| format!("{name}=…")).collect();
    Err(format!("source `{}` takes {}", request.source.id, wanted.join(" ")))
}

fn year(text: &str) -> bool {
    text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())
}

/// `WEST,SOUTH,EAST,NORTH` in degrees, written back in one form, so one box is one request.
fn parse_bbox(text: &str) -> Result<String, String> {
    let numbers: Option<Vec<f64>> = text.split(',').map(|n| n.trim().parse().ok()).collect();
    if let Some(&[west, south, east, north]) = numbers.as_deref() {
        let inside = (-180.0..=180.0).contains(&west) && (-180.0..=180.0).contains(&east);
        if inside && (-90.0..=90.0).contains(&south) && (-90.0..=90.0).contains(&north) && west < east && south < north
        {
            return Ok(format!("{west},{south},{east},{north}"));
        }
    }
    Err(format!("`bbox={text}` is not WEST,SOUTH,EAST,NORTH in degrees, west < east, south < north"))
}

/// The repository root, where the capture programs are.
fn root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    crate::find_root(&cwd).ok_or_else(|| "no data/sources.toml above the current directory".into())
}

/// `OBC_PYTHON`, or else Python with the packages of `requirements` (`uv run`), in the
/// repository root.
fn python(root: &Path, requirements: Option<&str>) -> Command {
    let mut command = match (std::env::var_os("OBC_PYTHON"), requirements) {
        (Some(python), _) => Command::new(python),
        (None, Some(requirements)) => {
            let mut command = Command::new("uv");
            command.args(["run", "--with-requirements", requirements, "python"]);
            command
        }
        (None, None) => Command::new("python3"),
    };
    command.current_dir(root);
    command
}

/// Every file under `dir`, sorted.
fn walk(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() {
            files.extend(walk(&path)?);
        } else {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
