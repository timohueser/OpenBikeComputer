//! `obc data store import`: move the cache directories of the older bake tools into the store.
//! Each file becomes an object, so the same bytes in two directories become one object. An
//! import record keeps the old path of each file.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{create_parent, hash_file, Store};
use crate::date;

/// The cache directories of the older bake tools, relative to the home directory.
pub const OLD_DIRS: [&str; 5] =
    [".cache/obcm", ".cache/obc/planner", ".cache/openbikecomputer", "obc-bake", "obc-reference"];

/// What `store import` moves, or moved, and what stays.
#[derive(Debug, Default, Serialize, JsonSchema)]
#[schemars(rename = "ImportPlan")]
pub struct Plan {
    pub dirs: Vec<DirPlan>,
    /// The size of every file.
    pub bytes: u64,
    /// How much the store grows: the size of each content that is not an object yet, once.
    pub new_bytes: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ImportDir")]
pub struct DirPlan {
    /// The directory, without symbolic links when it is present.
    pub dir: PathBuf,
    pub present: bool,
    /// The regular files that it moves, or moved, and their size.
    pub files: u64,
    pub bytes: u64,
    /// What stays in the directory: symbolic links and other entries that are not regular files,
    /// and after an import each file that changed while it was read.
    pub left: Vec<PathBuf>,
}

/// One line of an import record: one file, with its old place and its object.
#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedFile {
    pub dir: PathBuf,
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
    let store_root = store_root(store)?;
    let mut plan = Plan::default();
    let mut seen = HashSet::new();
    for dir in dirs {
        let (dir_plan, files) = scan(dir, &store_root)?;
        for (path, _) in files {
            let (sha256, size) = hash_file(&path)?;
            if seen.insert(sha256.clone()) && !store.object(&sha256).is_file() {
                plan.new_bytes += size;
            }
        }
        plan.bytes += dir_plan.bytes;
        plan.dirs.push(dir_plan);
    }
    Ok(plan)
}

/// Move every regular file of `dirs` into the store, add a line for it to the import record, and
/// then delete the directories that are empty. One import runs at a time; a run that stopped
/// keeps its record, and the next run imports the rest.
pub fn apply(store: &Store, dirs: &[PathBuf]) -> Result<Plan, String> {
    let store_root = store_root(store)?;
    let _alone = store.lock("import")?;
    let _using = store.using()?;
    clean_partials(store)?;
    let name = date::timestamp(date::now()).replace([':', '-'], "");
    let mut record: Option<File> = None;
    let mut plan = Plan::default();
    for dir in dirs {
        let (mut dir_plan, files) = scan(dir, &store_root)?;
        dir_plan.bytes = 0;
        for (path, relative) in files {
            // The line is on the disk before the file moves. A line of a file that then stays is
            // only one more root of a collection.
            let mut write_line = |sha256: &str, size: u64| -> Result<(), String> {
                let record = match &mut record {
                    Some(record) => record,
                    None => record.insert(open_record(store, &name)?),
                };
                let line =
                    ImportedFile { dir: dir_plan.dir.clone(), path: relative.clone(), size, sha256: sha256.into() };
                let mut text = serde_json::to_string(&line).map_err(|e| e.to_string())?;
                text.push('\n');
                record.write_all(text.as_bytes()).and_then(|()| record.sync_data()).map_err(|e| e.to_string())
            };
            match move_in(store, &store_root, &path, &mut write_line)? {
                Moved::Yes { size, new } => {
                    dir_plan.bytes += size;
                    plan.new_bytes += if new { size } else { 0 };
                }
                Moved::No(stays) => {
                    dir_plan.files -= 1;
                    dir_plan.left.push(stays);
                }
            }
        }
        if dir_plan.present {
            remove_empty(&dir_plan.dir, &store_root)?;
            // A directory reached through a symbolic link: the link goes with its target.
            if !dir_plan.dir.exists() && fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
                fs::remove_file(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
        }
        plan.bytes += dir_plan.bytes;
        plan.dirs.push(dir_plan);
    }
    Ok(plan)
}

/// The store root, which exists after this, without symbolic links.
fn store_root(store: &Store) -> Result<PathBuf, String> {
    let root = store.root();
    fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))
}

fn open_record(store: &Store, name: &str) -> Result<File, String> {
    let path = store.root().join("imports").join(format!("{name}.jsonl"));
    create_parent(&path)?;
    OpenOptions::new().create(true).append(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Delete the copies that an import on another file system left when it stopped.
fn clean_partials(store: &Store) -> Result<(), String> {
    let dir = store.partial("");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("import-") {
            fs::remove_file(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// The size and the modification time: a file that a process still writes changes them.
fn stat(path: &Path) -> Result<(u64, SystemTime), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((metadata.len(), metadata.modified().map_err(|e| format!("{}: {e}", path.display()))?))
}

enum Moved {
    /// The size, and whether the object is new.
    Yes { size: u64, new: bool },
    /// The file changed while it was read; this path holds it now.
    No(PathBuf),
}

/// Make `file` an object, and call `record` with its SHA-256 and size before it moves. A file that
/// changes while it is read stays where it is.
///
/// On the file system of the store the file is hashed where it is and renamed into the objects,
/// so no byte is copied. On another file system it is copied into `partial/`, and the copy is
/// hashed. A process that still writes the file after the last check can change an object, so
/// the bakes, the planner and every fetch stop before an import.
fn move_in(
    store: &Store,
    store_root: &Path,
    file: &Path,
    record: &mut dyn FnMut(&str, u64) -> Result<(), String>,
) -> Result<Moved, String> {
    let before = stat(file)?;
    let rename = |from: &Path, to: &Path| {
        fs::rename(from, to).map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))
    };
    if same_file_system(file, store_root)? {
        let (sha256, size) = hash_file(file)?;
        if stat(file)? != before {
            return Ok(Moved::No(file.to_path_buf()));
        }
        record(&sha256, size)?;
        let object = store.object(&sha256);
        if object.is_file() {
            fs::remove_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
            return Ok(Moved::Yes { size, new: false });
        }
        create_parent(&object)?;
        rename(file, &object)?;
        if stat(&object)? != before {
            // Its bytes are not its name, so it is no object. A new file at the old path stays,
            // and this one goes beside it.
            let back = match fs::symlink_metadata(file) {
                Err(e) if e.kind() == ErrorKind::NotFound => file.to_path_buf(),
                _ => file.with_file_name(format!(
                    "{}.changed-{}",
                    file.file_name().unwrap_or_default().to_string_lossy(),
                    std::process::id()
                )),
            };
            rename(&object, &back)?;
            return Ok(Moved::No(back));
        }
        let mut permissions = fs::metadata(&object).map_err(|e| format!("{}: {e}", object.display()))?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&object, permissions).map_err(|e| format!("{}: {e}", object.display()))?;
        return Ok(Moved::Yes { size, new: true });
    }
    let part = store.partial(&format!("import-{}", std::process::id()));
    create_parent(&part)?;
    fs::copy(file, &part).map_err(|e| format!("{} -> {}: {e}", file.display(), part.display()))?;
    let (sha256, size) = hash_file(&part)?;
    if stat(file)? != before || size != before.0 {
        let _ = fs::remove_file(&part);
        return Ok(Moved::No(file.to_path_buf()));
    }
    let new = !store.object(&sha256).is_file();
    store.insert(&part, &sha256)?;
    record(&sha256, size)?;
    fs::remove_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(Moved::Yes { size, new })
}

#[cfg(unix)]
fn same_file_system(file: &Path, store_root: &Path) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    let device = |path: &Path| fs::metadata(path).map(|m| m.dev()).map_err(|e| format!("{}: {e}", path.display()));
    Ok(device(file)? == device(store_root)?)
}

#[cfg(not(unix))]
fn same_file_system(_: &Path, _: &Path) -> Result<bool, String> {
    Ok(false)
}

/// The directory without symbolic links, its regular files with their paths below it, sorted,
/// and what an import leaves in it. The store and everything below it are skipped; a directory
/// inside the store is refused.
fn scan(dir: &Path, store_root: &Path) -> Result<(DirPlan, Vec<(PathBuf, String)>), String> {
    let dir = match fs::canonicalize(dir) {
        Ok(dir) => dir,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let plan = DirPlan { dir: dir.to_path_buf(), present: false, files: 0, bytes: 0, left: Vec::new() };
            return Ok((plan, Vec::new()));
        }
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    if dir.starts_with(store_root) {
        return Err(format!("{} is in the store {}", dir.display(), store_root.display()));
    }
    let mut plan = DirPlan { dir: dir.clone(), present: true, files: 0, bytes: 0, left: Vec::new() };
    let mut files = Vec::new();
    let mut pending = vec![dir.clone()];
    while let Some(current) = pending.pop() {
        if current == store_root {
            continue;
        }
        for entry in fs::read_dir(&current).map_err(|e| format!("{}: {e}", current.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
            let kind = entry.file_type().map_err(|e| format!("{}: {e}", entry.path().display()))?;
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let size = entry.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
                let relative = path.strip_prefix(&dir).expect("below dir").components();
                let relative = relative.map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                plan.files += 1;
                plan.bytes += size;
                files.push((path, relative));
            } else {
                plan.left.push(path);
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    plan.left.sort();
    Ok((plan, files))
}

/// Delete each directory below `dir`, and `dir`, that is empty now; never the store or a
/// directory above it.
fn remove_empty(dir: &Path, store_root: &Path) -> Result<(), String> {
    if store_root.starts_with(dir) && store_root != dir {
        for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) && entry.path() != store_root {
                remove_empty(&entry.path(), store_root)?;
            }
        }
        return Ok(());
    }
    if dir == store_root {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            remove_empty(&entry.path(), store_root)?;
        }
    }
    match fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        // Not empty: it holds what the import left.
        Err(_) if fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some()) => Ok(()),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
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

    fn record(store: &Store) -> Vec<ImportedFile> {
        let records: Vec<_> = fs::read_dir(store.root().join("imports")).unwrap().flatten().collect();
        assert_eq!(records.len(), 1);
        let text = fs::read_to_string(records[0].path()).unwrap();
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[test]
    fn an_import_moves_each_content_once_and_keeps_the_store() {
        let scratch = Scratch::new("import");
        let home = fs::canonicalize(&scratch.0).unwrap();
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
        let dirs = old_dirs(&home);

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
        for dir in [".cache/obcm", ".cache/obc/planner"] {
            assert!(!home.join(dir).exists(), "{dir} is deleted");
        }
        #[cfg(unix)]
        {
            assert_eq!(applied.dirs[3].left, [home.join("obc-bake/latest")], "an import deletes only what it moved");
            assert!(fs::symlink_metadata(home.join("obc-bake/latest")).is_ok());
            assert!(!home.join("obc-bake/cells").exists(), "an empty directory is deleted");
        }
        assert!(!home.join(".cache/openbikecomputer/fixtures").exists());
        assert!(store.root().join("objects").is_dir(), "the store stays");
        let cell = ImportedFile {
            dir: home.join("obc-bake"),
            path: "cells/1.obcm".into(),
            size: 4,
            sha256: sha256_hex(b"cell"),
        };
        assert!(record(&store).contains(&cell));

        let again = super::plan(&store, &dirs).unwrap();
        assert_eq!((again.bytes, again.new_bytes), (0, 0), "the store is not imported into itself");
    }

    /// A home behind a symbolic link, and a store that does not exist yet: the store is still
    /// found inside the old directory and stays.
    #[cfg(unix)]
    #[test]
    fn a_store_behind_a_symbolic_link_stays() {
        let scratch = Scratch::new("import-link");
        let real = scratch.0.join("real");
        let home = scratch.0.join("home");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &home).unwrap();
        write(&home.join(".cache/openbikecomputer/fixtures/a.tar.gz"), b"fixture");
        let store = Store::at(home.join(".cache/openbikecomputer/store"));

        let elsewhere = scratch.0.join("elsewhere/obcm");
        write(&elsewhere.join("land/a.zip"), b"land");
        std::os::unix::fs::symlink(&elsewhere, home.join(".cache/obcm")).unwrap();

        apply(&store, &old_dirs(&home)).unwrap();
        assert!(store.object(&sha256_hex(b"fixture")).is_file());
        assert!(
            !elsewhere.exists() && fs::symlink_metadata(home.join(".cache/obcm")).is_err(),
            "the link goes with its target"
        );
        assert_eq!(record(&store).len(), 2);
        assert!(!real.join(".cache/openbikecomputer/fixtures").exists());
    }
}
