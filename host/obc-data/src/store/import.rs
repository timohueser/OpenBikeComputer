//! `obc data store import`: move the cache directories of the older bake tools into the store.
//! Each file becomes an object, so the same bytes in two directories become one object. An
//! import record keeps the old path of each file.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{hash_file, write_record, Store};
use crate::date;

/// The cache directories of the older bake tools, relative to the home directory.
pub const OLD_DIRS: [&str; 5] =
    [".cache/obcm", ".cache/obc/planner", ".cache/openbikecomputer", "obc-bake", "obc-reference"];

#[derive(Debug, Default, Serialize)]
pub struct Plan {
    pub dirs: Vec<DirPlan>,
    /// The size of every file.
    pub bytes: u64,
    /// How much the store grows: the size of each content that is not an object yet, once.
    pub new_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct DirPlan {
    pub dir: PathBuf,
    pub present: bool,
    pub files: u64,
    pub bytes: u64,
    /// Symbolic links, which are not followed and are deleted with the directory.
    pub links: u64,
}

/// One import run: for each directory, the old path of each file and its object.
#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRecord {
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub imported: String,
    pub dirs: Vec<ImportedDir>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedDir {
    pub dir: PathBuf,
    pub files: Vec<ImportedFile>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedFile {
    /// The path below `dir`, with `/`.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// The old directories below `home`.
pub fn old_dirs(home: &Path) -> Vec<PathBuf> {
    OLD_DIRS.iter().map(|dir| home.join(dir)).collect()
}

/// What an import of `dirs` moves, and how much the store grows. It hashes every file and
/// changes nothing.
pub fn plan(store: &Store, dirs: &[PathBuf]) -> Result<Plan, String> {
    run(store, dirs, false)
}

/// Move every file of `dirs` into the store, write the import record, and delete each
/// directory. A directory that holds the store keeps the store.
pub fn apply(store: &Store, dirs: &[PathBuf]) -> Result<Plan, String> {
    run(store, dirs, true)
}

fn run(store: &Store, dirs: &[PathBuf], apply: bool) -> Result<Plan, String> {
    let store_root = fs::canonicalize(store.root()).unwrap_or_else(|_| store.root().to_path_buf());
    let mut plan = Plan::default();
    let mut seen = HashSet::new();
    let mut record = ImportRecord { imported: date::timestamp(date::now()), dirs: Vec::new() };
    let result = (|| {
        for dir in dirs {
            let (files, links) = walk(dir, &store_root)?;
            let mut dir_plan = DirPlan { dir: dir.clone(), present: dir.is_dir(), files: 0, bytes: 0, links };
            // A failed import still records the files it moved.
            record.dirs.push(ImportedDir { dir: dir.clone(), files: Vec::new() });
            for (path, relative) in files {
                let (sha256, size, new) = if apply {
                    move_in(store, &path)?
                } else {
                    let (sha256, size) = hash_file(&path)?;
                    let new = seen.insert(sha256.clone()) && !store.object(&sha256).is_file();
                    (sha256, size, new)
                };
                if new {
                    plan.new_bytes += size;
                }
                dir_plan.files += 1;
                dir_plan.bytes += size;
                if apply {
                    let imported = &mut record.dirs.last_mut().expect("pushed above").files;
                    imported.push(ImportedFile { path: relative, size, sha256 });
                }
            }
            plan.bytes += dir_plan.bytes;
            plan.dirs.push(dir_plan);
            if apply {
                remove_except(dir, &store_root)?;
            }
        }
        Ok(())
    })();
    if apply && record.dirs.iter().any(|dir| !dir.files.is_empty()) {
        let name = record.imported.replace([':', '-'], "");
        write_record(&store.root().join("imports").join(format!("{name}.json")), &record)?;
    }
    result.map(|()| plan)
}

/// Move `file` to a part file in the store, hash it there, and make it an object; `true` when the
/// object is new. The digest is of the bytes in the store, so a file that changes during the import
/// is still whole.
fn move_in(store: &Store, file: &Path) -> Result<(String, u64, bool), String> {
    let part = store.partial(&format!("import-{}", std::process::id()));
    super::create_parent(&part)?;
    if let Err(error) = fs::rename(file, &part) {
        // Another file system: copy, then delete the original.
        fs::copy(file, &part).map_err(|e| format!("{}: {error}; copy: {e}", file.display()))?;
        fs::remove_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let (sha256, size) = hash_file(&part)?;
    let existed = store.object(&sha256).is_file();
    store.insert(&part, &sha256)?;
    Ok((sha256, size, !existed))
}

/// The regular files below `dir` with their paths relative to it, sorted, and the number of
/// symbolic links. The store and everything below it are skipped.
fn walk(dir: &Path, store_root: &Path) -> Result<(Vec<(PathBuf, String)>, u64), String> {
    let mut files = Vec::new();
    let mut links = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        if fs::canonicalize(&current).is_ok_and(|path| path == store_root) {
            continue;
        }
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound && current == dir => continue,
            Err(e) => return Err(format!("{}: {e}", current.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
            let kind = entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?;
            let path = entry.path();
            if kind.is_symlink() {
                links += 1;
            } else if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path.strip_prefix(dir).expect("below dir").components();
                let relative = relative.map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                files.push((path, relative));
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok((files, links))
}

/// Delete `dir` and what remains in it, but not the store.
fn remove_except(dir: &Path, store_root: &Path) -> Result<(), String> {
    let canonical = match fs::canonicalize(dir) {
        Ok(path) => path,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    if !store_root.starts_with(&canonical) {
        return fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()));
    }
    if canonical == store_root {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let kind = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.file_type();
        if kind.is_dir() {
            remove_except(&path, store_root)?;
        } else {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sha256_hex;
    use crate::store::tests::Scratch;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn an_import_moves_each_content_once_and_keeps_the_store() {
        let scratch = Scratch::new("import");
        let home = &scratch.0;
        // The store lives in one of the old directories, and already has one content.
        let store = Store::at(home.join(".cache/openbikecomputer/store"));
        let stored = store.partial("stored");
        write(&stored, b"in the store");
        store.insert(&stored, &sha256_hex(b"in the store")).unwrap();
        write(&home.join(".cache/obcm/geofabrik/monaco.osm.pbf"), b"monaco");
        write(&home.join(".cache/obc/planner/sources/downloads/monaco.osm.pbf"), b"monaco");
        write(&home.join(".cache/openbikecomputer/fixtures/a.tar.gz"), b"in the store");
        write(&home.join("obc-bake/cells/1.obcm"), b"cell");
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.join("obc-bake/cells/1.obcm"), home.join("obc-bake/latest")).unwrap();
        let dirs = old_dirs(home);

        let plan = plan(&store, &dirs).unwrap();
        assert_eq!(plan.dirs.iter().map(|dir| dir.files).collect::<Vec<_>>(), [1, 1, 1, 1, 0]);
        assert!(!plan.dirs[4].present, "~/obc-reference is not there");
        assert_eq!(
            (plan.bytes, plan.new_bytes),
            (6 + 6 + 12 + 4, 6 + 4),
            "a duplicate and a stored content are not new"
        );
        assert!(home.join("obc-bake/cells/1.obcm").is_file(), "a plan moves nothing");

        let applied = apply(&store, &dirs).unwrap();
        assert_eq!((applied.bytes, applied.new_bytes), (plan.bytes, plan.new_bytes));
        for bytes in [&b"monaco"[..], b"cell", b"in the store"] {
            assert!(store.object(&sha256_hex(bytes)).is_file());
        }
        for dir in [".cache/obcm", ".cache/obc/planner", "obc-bake"] {
            assert!(!home.join(dir).exists(), "{dir} is deleted");
        }
        assert!(!home.join(".cache/openbikecomputer/fixtures").exists());
        assert!(store.root().join("objects").is_dir(), "the store stays");
        let records: Vec<_> = fs::read_dir(store.root().join("imports")).unwrap().flatten().collect();
        assert_eq!(records.len(), 1);
        let record: ImportRecord = serde_json::from_slice(&fs::read(records[0].path()).unwrap()).unwrap();
        let bake = record.dirs.iter().find(|dir| dir.dir == home.join("obc-bake")).unwrap();
        assert_eq!(bake.files, [ImportedFile { path: "cells/1.obcm".into(), size: 4, sha256: sha256_hex(b"cell") }]);

        let again = super::plan(&store, &dirs).unwrap();
        assert_eq!((again.bytes, again.new_bytes), (0, 0), "the store is not imported into itself");
    }
}
