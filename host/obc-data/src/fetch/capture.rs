//! Fetchers that run a program: it writes the files of one request into a directory, and the
//! store takes them. A service answers with today's data, so another day comes from the store.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use super::{check_version, file_name, merge, snapshot_lock, Request};
use crate::date;
use crate::sources::{Refresh, Registry, Source};
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
        |_| &[0],
        |work, out| {
            // The pinned rasterio needs Python 3.12 or later.
            let mut command = python(
                &root,
                &["--no-project", "--python", ">=3.12", "--with-requirements", "tools/requirements-bake.txt"],
            );
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
        "wikidata" | "wikipedia" | "commons" => wiki(store, request, &root, SELECTOR.get().map(PathBuf::as_path)),
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
                |_| &[0],
                |_, out| {
                    let mut command = python(&root, &["--locked", "--group", "planner-snow"]);
                    command.args(["-m", "tools.planner_snow", "--source", kind, &format!("--bounds={bbox}")]);
                    command.args(["--first-season", first, "--last-season", last, "--fetch"]).arg(out);
                    command
                },
            )
        }
        "osm-trails" => {
            let [bbox] = values(request, ["bbox"])?;
            let bbox = parse_bbox(bbox)?;
            capture(
                store,
                request,
                &format!("bbox={bbox}"),
                &[source],
                |_| &[0],
                |_, out| {
                    let mut command = python(&root, &["--locked", "--group", "planner-snow"]);
                    command.args(["-m", "tools.planner_snow", &format!("--bounds={bbox}"), "--fetch-trails"]).arg(out);
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
                |_| &[0],
                |_, out| {
                    let mut command = python(&root, &["--locked", "--group", "planner-climate"]);
                    command.args(["-m", "tools.planner_climate", &format!("--bounds={bbox}"), "--first-year", first]);
                    command.arg("--fetch").arg(out);
                    command
                },
            )
        }
        _ => Err(format!("source `{}`: its kind of fetch has no fetcher yet", source.id)),
    }
}

/// The binary that answers the selection commands of the capture tool, such as
/// `landmark-content`: the `obc data` binary, which links the compiler. `obc-data-plumbing` has none.
static SELECTOR: OnceLock<PathBuf> = OnceLock::new();

/// Let the Wikimedia captures of this process select places with `binary`.
pub fn select_with(binary: PathBuf) {
    let _ = SELECTOR.set(binary);
}

/// One run of `tools/landmark_capture.py` captures Wikidata, Wikipedia and Commons for the
/// landmarks or the peaks of the region `area=<region id>`: `collection=landmarks|peaks`, and the files in the store of
/// the region's extract, `osm=sha256:<hex>`, and of its `.poly`, `poly=sha256:<hex>`. The program
/// finds the candidates in the extract itself and selects them with `selector`. [`wiki_owners`]
/// splits the files into the three records.
fn wiki(store: &Store, request: &Request, root: &Path, selector: Option<&Path>) -> Result<Snapshot, String> {
    let [collection, area, osm, poly] = values(request, ["collection", "area", "osm", "poly"])?;
    if !["landmarks", "peaks"].contains(&collection) {
        return Err(format!("`collection={collection}` is not `landmarks` or `peaks`"));
    }
    let (osm_file, poly_file) = (stored(store, "osm", osm)?, stored(store, "poly", poly)?);
    let selector = selector.ok_or(format!(
        "source `{}`: the capture selects places with the `obc data` binary; this binary has no step code",
        request.source.id
    ))?;
    let policy = root.join("host/obc-pack/src/landmarks/policy.json");
    let tool = root.join("tools/landmark_capture.py");
    // The tools and the files that decide what a capture asks for: a change is another capture.
    let mut digests = format!("{collection} {area} {osm} {poly} ");
    for file in [&policy, &root.join("specs/content-languages.json"), &tool, &root.join("tools/peak_capture.py")] {
        digests += &store::hash_file(file)?.0;
    }
    let query = format!("{collection}={}", &store::sha256_hex(digests.as_bytes())[..16]);
    let registry = Registry::load(root)?;
    let find = |id: &str| registry.sources.iter().find(|source| source.id == id).ok_or(format!("no source `{id}`"));
    let owners = [find("wikidata")?, find("wikipedia")?, find("commons")?];
    capture(store, request, &query, &owners, wiki_owners, |_, out| {
        // The capture needs no package beyond the standard library.
        let mut command = python(root, &[]);
        command.arg(&tool).arg("--poly").arg(&poly_file);
        match collection {
            "peaks" => command.arg("--peaks-osm").arg(&osm_file),
            _ => command.arg("--osm").arg(&osm_file).arg("--policy").arg(&policy),
        };
        // A failed run keeps its directory, and each run asks once more for what failed before.
        command.arg("--select-with").arg(selector).arg("--retry-failed").arg("--out").arg(out);
        command
    })
}

/// The object of `<name>=sha256:<hex>`, a file of the store.
fn stored(store: &Store, name: &str, value: &str) -> Result<PathBuf, String> {
    let hex = value
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let hex = hex.ok_or(format!("`{name}={value}` is not sha256:<64 lowercase hex digits>"))?;
    let object = store.object(hex);
    if !object.is_file() {
        return Err(format!("`{name}={value}`: the store has no such file"));
    }
    // The program runs in the repository root.
    std::path::absolute(&object).map_err(|e| format!("{}: {e}", object.display()))
}

/// The records of `wikidata` (0), `wikipedia` (1) and `commons` (2) that take a file of a
/// Wikimedia capture, by its path, so each file has the licence of its source. Every record has
/// the recipe, which links the three. The copies of the inputs are OSM data, and the archived
/// failures are no source data, so no record takes them.
fn wiki_owners(path: &str) -> &'static [usize] {
    match path.split('/').next().unwrap_or_default() {
        "recipe.json" => &[0, 1, 2],
        "boundary.geojson" | "candidates.json" | "summits.json" | "policy.json" | "attempts" => &[],
        "articles" => &[1],
        "links" if path.starts_with("links/wikipedia-") => &[1],
        "images" | "categories" => &[2],
        _ => &[0],
    }
}

/// The files of `request` that `query` names: from the store, or else from the program that
/// `command` makes. The program writes them into its second directory, and may keep downloads in
/// its first. `owners` gives the indexes in `sources` of the sources whose records take a file,
/// from its path. A run that fails keeps both directories, so the next run can resume, unless
/// they are older than the `refresh` of the source.
pub fn capture(
    store: &Store,
    request: &Request,
    query: &str,
    sources: &[&Source],
    owners: impl Fn(&str) -> &'static [usize],
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
    if let (Refresh::Days(days), Ok(modified)) = (source.refresh, fs::metadata(&staging).and_then(|m| m.modified())) {
        // Data that old would mix with today's, so the capture starts again.
        if modified.elapsed().is_ok_and(|age| age.as_secs() > u64::from(days) * 86_400) {
            fs::remove_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        }
    }
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
    if files.iter().all(Vec::is_empty) {
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

/// The repository root, where the capture programs are.
fn root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    crate::find_root(&cwd).ok_or_else(|| "no data/sources.toml above the current directory".into())
}

/// `OBC_PYTHON`, or else Python under `uv run` with the arguments `packages`, which name its
/// packages, or `python3` without; in the repository root.
fn python(root: &Path, packages: &[&str]) -> Command {
    let mut command = match (std::env::var_os("OBC_PYTHON"), packages) {
        (Some(python), _) => Command::new(python),
        (None, []) => Command::new("python3"),
        (None, packages) => {
            let mut command = Command::new("uv");
            command.arg("run").args(packages).arg("python");
            command
        }
    };
    command.current_dir(root);
    command
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
    use super::wiki_owners;

    #[test]
    fn a_wikimedia_file_goes_to_the_record_of_its_licence() {
        for (path, owners) in [
            ("recipe.json", &[0, 1, 2][..]),
            ("entities/batch-1.json", &[0]),
            ("manifest.json", &[0]),
            ("articles/Q1-de.json", &[1]),
            ("images/Q1.json", &[2]),
            ("categories/Foo.json", &[2]),
            ("boundary.geojson", &[]),
            ("candidates.json", &[]),
            ("summits.json", &[]),
            ("policy.json", &[]),
            ("attempts/abc.response", &[]),
            ("links/wikidata-Q1.json", &[0]),
            ("links/wikipedia-abc.json", &[1]),
        ] {
            assert_eq!(wiki_owners(path), owners, "{path}");
        }
    }
}
