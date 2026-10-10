//! Fetchers that run a program: it writes the files of one request into a directory, and the
//! store takes them. A service answers with today's data, so another day comes from the store.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{check_version, file_name, merge, snapshot_lock, Request};
use crate::date;
use crate::sources::{Refresh, Source};
use crate::store::{self, FileRecord, Snapshot, Store};

/// Archive tile `tile=<ti>-<tj>` of a national terrain model, from the adapter of the reference
/// ingest (`ingest.py fetch`): the pooled tile `<ti>-<tj>.tif`, or the empty `<ti>-<tj>.none` where
/// the model has no height. The raw rasters stay in the work directory, which the program removes.
pub(crate) fn dtm(
    root: &Path,
    store: &Store,
    request: &Request,
    checks: Option<crate::fetch::Checks<'_>>,
) -> Result<Snapshot, String> {
    let source = request.source;
    let [tile] = values(request, ["tile"])?;
    let digits = |part: &str| part.len() == 4 && part.bytes().all(|b| b.is_ascii_digit());
    if !tile.split_once('-').is_some_and(|(ti, tj)| digits(ti) && digits(tj)) {
        return Err(format!("`tile={tile}` is <ti>-<tj>, two four-digit archive tile indices"));
    }
    let key = source.id.strip_prefix("dtm-").unwrap_or(&source.id).to_string();
    capture(
        root,
        store,
        request,
        checks,
        &format!("tile={tile}"),
        &[source],
        |_| &[0],
        false,
        |work, out| {
            let mut command = python(root, Some("terrain-reference"))?;
            command.arg("host/obc-dem/reference/ingest.py");
            command.args(["fetch", &key, "--tile", tile, "--work"]).arg(work).arg("--out").arg(out);
            // The program stops when this process does, so no orphan writes into a later run.
            command.env("OBC_PARENT_PID", std::process::id().to_string());
            Ok(command)
        },
    )
}

/// A source of `kind = "capture"`: the program that captures it, with the `NAME=VALUE` it takes.
pub(crate) fn run(
    root: &Path,
    store: &Store,
    request: &Request,
    checks: Option<crate::fetch::Checks<'_>>,
) -> Result<Snapshot, String> {
    let source = request.source;
    match source.id.as_str() {
        "wikidata" | "wikipedia" | "commons" => {
            if request.params.iter().any(|(name, _)| name == "content") {
                super::wikimedia::run(root, store, request, checks)
            } else {
                legacy(store, request)
            }
        }
        "modis-snow" | "hr-wsi" => {
            let [bbox, seasons] = values(request, ["bbox", "seasons"])?;
            let bbox = parse_bbox(bbox)?;
            let years = seasons.split_once('-').filter(|(first, last)| year(first) && year(last) && first <= last);
            let (first, last) = years.ok_or_else(|| format!("`seasons={seasons}` is not FIRST-LAST, two years"))?;
            let kind = if source.id == "hr-wsi" { "copernicus-hr-wsi" } else { "nasa-modis" };
            capture(
                root,
                store,
                request,
                checks,
                &format!("bbox={bbox}&seasons={seasons}"),
                &[source],
                |_| &[0],
                // HR-WSI has no product in parts of its extent.
                source.id == "hr-wsi",
                |_, out| {
                    let mut command = python(root, Some("planner-snow"))?;
                    command.args(["-m", "tools.planner_snow", "--source", kind, &format!("--bounds={bbox}")]);
                    command.args(["--first-season", first, "--last-season", last, "--fetch"]).arg(out);
                    Ok(command)
                },
            )
        }
        "osm-trails" => {
            let [bbox] = values(request, ["bbox"])?;
            let bbox = parse_bbox(bbox)?;
            capture(
                root,
                store,
                request,
                checks,
                &format!("bbox={bbox}"),
                &[source],
                |_| &[0],
                false,
                |_, out| {
                    let mut command = python(root, Some("planner-snow"))?;
                    command.args(["-m", "tools.planner_snow", &format!("--bounds={bbox}"), "--fetch-trails"]).arg(out);
                    Ok(command)
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
                root,
                store,
                request,
                checks,
                &format!("bbox={bbox}&first-year={first}"),
                &[source],
                |_| &[0],
                false,
                |_, out| {
                    let mut command = python(root, Some("planner-climate"))?;
                    command.args(["-m", "tools.planner_climate", &format!("--bounds={bbox}"), "--first-year", first]);
                    command.arg("--fetch").arg(out);
                    Ok(command)
                },
            )
        }
        _ => Err(format!("source `{}`: its kind of fetch has no fetcher yet", source.id)),
    }
}

/// Retained regional captures are immutable compiler inputs. New facts use shared acquisition.
fn legacy(store: &Store, request: &Request) -> Result<Snapshot, String> {
    let snapshots = if let Some(version) = &request.version {
        store.snapshot(&request.source.id, version)?.into_iter().collect()
    } else {
        store.snapshots(&request.source.id)?
    };
    for snapshot in snapshots.into_iter().rev() {
        if let Some(names) = store.requested(&request.source.id, &snapshot.version, &request.params)? {
            let files: Vec<_> = snapshot.files.into_iter().filter(|file| names.contains(&file.name)).collect();
            if files.len() == names.len() && files.iter().all(|file| store.object(&file.sha256).is_file()) {
                return Ok(Snapshot { source: snapshot.source, version: snapshot.version, files });
            }
        }
    }
    Err("regional Wikimedia capture is read-only; prepare shared content to acquire missing facts".into())
}

/// The files of `request` that `query` names: from the store, or else from the program that
/// `command` makes. The program writes them into its second directory, and may keep downloads in
/// its first. `owners` gives the indexes in `sources` of the sources whose records take a file,
/// from its path. A run that fails keeps both directories, so the next run can resume, unless
/// they are older than the `refresh` of the source or the source is manual. With `empty`, a run
/// that writes no file gives a fetch without files; else it fails.
#[allow(clippy::too_many_arguments)]
pub(super) fn capture(
    root: &Path,
    store: &Store,
    request: &Request,
    checks: Option<crate::fetch::Checks<'_>>,
    query: &str,
    sources: &[&Source],
    owners: impl Fn(&str) -> &'static [usize],
    empty: bool,
    command: impl FnOnce(&Path, &Path) -> Result<Command, String>,
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
    // A fetch without files has only the record of its request.
    if store.requested(&source.id, &version, &request.params)?.is_some_and(|files| files.is_empty()) {
        return Ok(snapshot(Vec::new()));
    }
    // A manual source moves only by `--move`, so a request that a started version lacks joins it:
    // a national model is fetched tile by tile over days. A version that the store never started
    // is a day the service cannot answer for.
    let joins = source.refresh == Refresh::Manual && store.snapshot(&source.id, &version)?.is_some();
    if version != today && !joins {
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
    super::check_owner(root, source, checks)?;
    let staging = store.partial(&format!("capture-{key}"));
    let staging = std::path::absolute(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    if let Ok(modified) = fs::metadata(&staging).and_then(|m| m.modified()) {
        // Data that old would mix with today's, so the capture starts again. Nothing bounds the age
        // of what a failed run of a manual source kept, so it never resumes.
        let stale = match source.refresh {
            Refresh::Days(days) => modified.elapsed().is_ok_and(|age| age.as_secs() > u64::from(days) * 86_400),
            Refresh::Manual => true,
        };
        if stale {
            fs::remove_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        }
    }
    let (work, out) = (staging.join("work"), staging.join("out"));
    for dir in [&work, &out] {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let code = super::owner_code(source).code;
    let before = code.files(root)?;
    let mut command = command(&work, &out)?;
    eprintln!("obc data: capturing {}#{query}", source.id);
    // Standard output is the answer of `obc data`, so the program writes its progress to standard error.
    let status = command.stdout(std::io::stderr()).status();
    if source.refresh == Refresh::Manual && !status.as_ref().is_ok_and(|status| status.success()) {
        // A manual run never resumes, so the raw downloads of a failed one go now.
        let _ = fs::remove_dir_all(&staging);
    }
    let status = status.map_err(|e| format!("source `{}`: {:?}: {e}", source.id, command.get_program()))?;
    if !status.success() {
        return Err(format!("source `{}`: {:?} failed with {status}", source.id, command.get_program()));
    }
    if code.files(root)? != before {
        return Err("capture code or Python runtime changed; plan again".into());
    }
    let mut files: Vec<Vec<FileRecord>> = vec![Vec::new(); sources.len()];
    for path in walk(&out)? {
        let parts = path.strip_prefix(&out).unwrap_or(&path).components();
        let relative: Vec<_> = parts.map(|part| part.as_os_str().to_string_lossy()).collect();
        let relative = relative.join("/");
        let indexes = owners(&relative);
        if indexes.is_empty() {
            continue;
        }
        let (sha256, size) = store::hash_file(&path)?;
        store.insert(&path, &sha256)?;
        for &index in indexes {
            let url = format!("{}{relative}", prefix(sources[index]));
            files[index].push(FileRecord {
                name: file_name(sources[index], Some(&version), &url),
                url,
                size,
                sha256: sha256.clone(),
                retrieved: date::timestamp(date::now()),
            });
        }
    }
    if files.iter().all(Vec::is_empty) && !empty {
        return Err(format!("source `{}`: the capture of {query} has no file", source.id));
    }
    // Every record is checked before one is written, so a conflict in one leaves all as they were.
    let mut locks = Vec::new();
    let mut merged = Vec::new();
    for (owner, files) in sources.iter().zip(&files) {
        locks.push(store.lock(&snapshot_lock(&owner.id, &version))?);
        merged.push(merge(store, &owner.id, &version, files)?);
    }
    for snapshot in merged.into_iter().flatten() {
        store.put_snapshot(&snapshot)?;
    }
    drop(locks);
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

/// Use the same selected interpreter as the capture's code identity.
pub(crate) fn python(root: &Path, group: Option<&str>) -> Result<Command, String> {
    let executable = crate::engine::code::python_executable(root)?;
    let mut command = match group {
        None => Command::new(&executable),
        Some(group) => {
            let mut command = Command::new("uv");
            command.args([
                "run",
                "--locked",
                "--offline",
                "--no-default-groups",
                "--no-python-downloads",
                "--group",
                group,
                "python",
            ]);
            command.env("UV_PYTHON", &executable).env("UV_NO_SYNC", "0");
            command
        }
    };
    command.current_dir(root);
    Ok(command)
}

/// Every file under `dir`, sorted, but hidden, `.part` and `.tmp` files: a killed run leaves
/// those behind, and the next run resumes in the same directory.
fn walk(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name.ends_with(".part") || name.ends_with(".tmp") {
            continue;
        }
        if path.is_dir() {
            files.extend(walk(&path)?);
        } else {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retained_empty_capture_needs_no_runtime_or_owner_checkout() {
        let scratch = crate::store::tests::Scratch::new("capture-retained-no-runtime");
        let store = Store::at(scratch.0.join("store"));
        let source = crate::sources::embedded("hr-wsi");
        let version = date::format(date::today());
        let params = vec![("bbox".into(), "7,47,8,48".into()), ("seasons".into(), "2024-2025".into())];
        store
            .put_requested(
                &source.id,
                &store::Requested { version: version.clone(), params: params.clone(), files: Vec::new() },
            )
            .unwrap();
        let mut context = crate::engine::code::Context::default();
        let request = Request { refresh: false, source, version: Some(version.clone()), params };
        let snapshot = run(&scratch.0.join("absent-checkout"), &store, &request, Some(&mut context)).unwrap();
        assert_eq!(snapshot.version, version);
        assert!(snapshot.files.is_empty());
        assert!(!scratch.0.join("absent-checkout").exists());
    }

    #[test]
    fn capture_commands_use_the_identity_interpreter_and_locked_group() {
        use crate::engine::tests::write;
        let fixture = crate::engine::tests::fixture("capture-interpreter");
        let root = fixture.root();
        write(&root.join(".python-version"), "3.12\n");
        write(&root.join("pyproject.toml"), "[project]\nname = \"capture-fixture\"\nversion = \"0\"\nrequires-python = \">=3.12\"\ndependencies = []\n[dependency-groups]\ncapture = []\n[tool.uv]\npackage = false\ndefault-groups = []\n");
        let lock = Command::new("uv")
            .args(["lock", "--offline", "--no-python-downloads"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(lock.status.success(), "{}", String::from_utf8_lossy(&lock.stderr));
        let selected = crate::engine::code::python_executable(&root).unwrap();
        let code = crate::engine::Code {
            python: Some(crate::engine::Python { group: Some("capture".into()) }),
            ..Default::default()
        };
        code.identity(&root).unwrap();
        let mut direct = python(&root, None).unwrap();
        assert_eq!(direct.get_program(), selected.as_os_str());
        let output = direct.args(["-c", "import sys; print(sys.executable)"]).output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).canonicalize().unwrap(),
            selected.canonicalize().unwrap()
        );
        let mut grouped = python(&root, Some("capture")).unwrap();
        assert!(grouped.get_args().any(|arg| arg == "--no-default-groups"));
        assert!(grouped.get_envs().any(|(name, value)| name == "UV_PYTHON" && value == Some(selected.as_os_str())));
        assert!(!root.join(".venv").exists(), "selection neither syncs nor installs");
        let output = grouped
            .env_remove("UV_PROJECT_ENVIRONMENT")
            .env_remove("VIRTUAL_ENV")
            .args(["-c", "import sys; print(sys._base_executable)"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            PathBuf::from(String::from_utf8(output.stdout).unwrap().trim()).canonicalize().unwrap(),
            selected.canonicalize().unwrap()
        );
    }
}
