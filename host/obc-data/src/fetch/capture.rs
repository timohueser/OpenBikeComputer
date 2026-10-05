//! Fetchers that run a program: it writes the files of one request into a directory, and the
//! store takes them. A service answers with today's data, so another day comes from the store.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{check_version, record, Request};
use crate::date;
use crate::store::{self, FileRecord, Snapshot, Store};

/// The rasters of a national terrain model that cover `bbox=WEST,SOUTH,EAST,NORTH`, from the
/// adapter of the reference ingest (`ingest.py fetch`).
pub fn dtm(store: &Store, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let bbox = match request.params.as_slice() {
        [(name, value)] if name == "bbox" => parse_bbox(value).ok_or_else(|| {
            format!("`bbox={value}` is not WEST,SOUTH,EAST,NORTH in degrees, west < east, south < north")
        })?,
        _ => return Err(format!("source `{}` takes one bbox=WEST,SOUTH,EAST,NORTH", source.id)),
    };
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = crate::find_root(&cwd).ok_or("no data/sources.toml above the current directory")?;
    let key = source.id.strip_prefix("dtm-").unwrap_or(&source.id).to_string();
    capture(store, request, &format!("bbox={bbox}"), |work, out| {
        let mut command = python(&root.join("tools/requirements-bake.txt"));
        command.arg(root.join("host/obc-dem/reference/ingest.py"));
        command.args(["fetch", &key, &format!("--bbox={bbox}"), "--work"]).arg(work).arg("--out").arg(out);
        command
    })
}

/// The files of `request` that `query` names: from the store, or else from the program that
/// `command` makes, which writes them into its second directory and may keep downloads in its
/// first. A run that fails keeps the first directory, so the next run can reuse what it holds.
pub fn capture(
    store: &Store,
    request: &Request,
    query: &str,
    command: impl FnOnce(&Path, &Path) -> Command,
) -> Result<Snapshot, String> {
    let source = request.source;
    let today = date::format(date::today());
    let version = request.version.clone().unwrap_or_else(|| today.clone());
    check_version(source, &version)?;
    let prefix = format!("{}#{query}/", source.fetch.url.as_deref().unwrap_or_default());
    // Not the version: a run that failed one day resumes the next.
    let key = store::sha256_hex(prefix.as_bytes())[..32].to_string();
    let _lock = store.lock(&format!("capture-{key}"))?;
    let snapshot = |files| Snapshot { source: source.id.clone(), version: version.clone(), files };
    let stored: Vec<FileRecord> = store.snapshot(&source.id, &version)?.map_or_else(Vec::new, |snapshot| {
        snapshot.files.into_iter().filter(|file| file.url.starts_with(&prefix)).collect()
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
    if let Some(credential) = source.credential.as_ref().filter(|credential| !credential.present()) {
        return Err(format!("source `{}` is blocked: credential missing: {}", source.id, credential.describe()));
    }
    let staging = store.partial(&format!("capture-{key}"));
    let (work, out) = (staging.join("work"), staging.join("out"));
    let _ = fs::remove_dir_all(&out);
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
    let mut files = Vec::new();
    for path in walk(&out)? {
        let relative = path.strip_prefix(&out).unwrap_or(&path).components();
        let relative: Vec<_> = relative.map(|part| part.as_os_str().to_string_lossy()).collect();
        let (sha256, size) = store::hash_file(&path)?;
        store.insert(&path, &sha256)?;
        files.push(FileRecord {
            name: relative.last().map(|name| name.to_string()).unwrap_or_default(),
            url: format!("{prefix}{}", relative.join("/")),
            size,
            sha256,
            retrieved: date::timestamp(date::now()),
        });
    }
    if files.is_empty() {
        return Err(format!("source `{}`: the capture of {query} has no file", source.id));
    }
    record(store, &source.id, &version, &files)?;
    let _ = fs::remove_dir_all(&staging);
    Ok(snapshot(files))
}

/// `OBC_PYTHON`, or else a Python with the packages of `requirements` (`uv run`).
fn python(requirements: &Path) -> Command {
    if let Some(python) = std::env::var_os("OBC_PYTHON") {
        return Command::new(python);
    }
    let mut command = Command::new("uv");
    command.args(["run", "--with-requirements"]).arg(requirements).arg("python");
    command
}

/// `WEST,SOUTH,EAST,NORTH` in degrees, written back in one form, so one box is one request.
fn parse_bbox(text: &str) -> Option<String> {
    let numbers: Vec<f64> = text.split(',').map(|n| n.trim().parse().ok()).collect::<Option<_>>()?;
    let [west, south, east, north] = numbers[..] else { return None };
    let inside = (-180.0..=180.0).contains(&west) && (-180.0..=180.0).contains(&east);
    let inside = inside && (-90.0..=90.0).contains(&south) && (-90.0..=90.0).contains(&north);
    (inside && west < east && south < north).then(|| format!("{west},{south},{east},{north}"))
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
