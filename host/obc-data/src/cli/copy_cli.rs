//! `obc data source copy ID --from TARGET`: copy the versions of a prepared source from the legacy
//! folder `sources/<id>/` of a target into its home in the archive. It only reads the target. It
//! goes once no target has such a folder.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::prepare_cli;
use super::update_cli::{self, Downloads};
use super::{print_json, print_table, Code, Error};
use crate::engine::runs::{self, Run};
use crate::fetch::http::Http;
use crate::prepared::{self, Manifest, MANIFEST};
use crate::sources::{FetchKind, Registry};
use crate::store::{sha256_hex, Store};
use crate::target::Condition;

#[derive(Args)]
pub struct CopyArgs {
    /// The prepared source, as `data/sources.toml` names it.
    id: String,
    /// The target whose folder `sources/<id>/` holds the versions.
    #[arg(long, value_name = "TARGET")]
    from: String,
    /// Show the plan, and change nothing.
    #[arg(long)]
    dry_run: bool,
}

/// What a copy does.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CopyPlan {
    pub source: String,
    pub from: String,
    /// The archive root of the home of the source.
    pub root: PathBuf,
    /// Each version of the target, oldest first.
    pub versions: Vec<CopiedVersion>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CopiedVersion {
    pub version: String,
    pub manifest_sha256: String,
    /// The files that the home lacks.
    pub copy: Downloads,
    /// The files that the home holds.
    pub held: Downloads,
}

/// A version of the plan, with what the run needs to copy it.
struct Planned {
    shown: CopiedVersion,
    manifest: Vec<u8>,
    /// The keys `<version>/<path>` that the home lacks, with their SHA-256.
    missing: Vec<(String, String)>,
    /// The keys of the version that the home holds, with their SHA-256: the run reads each again,
    /// unless the home lists the version, whose copy checked them.
    held: Vec<(String, String)>,
    /// The keys `<origin>/<path>` of the files that the version reuses, with their size.
    reused: Vec<(String, u64)>,
}

pub fn command(root: &Path, args: CopyArgs, json: bool) -> Result<(), Error> {
    let (store, http) = (Store::open()?, Http::new());
    let print = |plan: &CopyPlan| {
        let copied: u64 = plan.versions.iter().map(|version| version.copy.files).sum();
        println!(
            "{} versions of {} in {}; this run copied {copied} files",
            plan.versions.len(),
            plan.source,
            plan.root.display()
        );
        Ok(())
    };
    let work = |run: &mut Run| copy(root, &store, &http, &args, run);
    if let Some(done) = super::api::detached(&store, work, print) {
        return done;
    }
    let (plan, _) = plan(root, &store, &http, &args)?;
    if args.dry_run {
        return print_plan(&plan, json);
    }
    let run = runs::detach(&store, &["source".into(), "copy".into(), args.id.clone(), "--from".into(), args.from])?;
    if json {
        return print_json(&super::api::Detached { run });
    }
    eprintln!("obc data: run {run}");
    super::runs_cli::follow(&store, &run, false)
}

/// The folder of the source on the target, the home, and each version of the target with the files
/// that the home lacks. Refused before any write when a version of the home or a manifest differs.
fn plan(root: &Path, store: &Store, http: &Http, args: &CopyArgs) -> Result<(CopyPlan, Vec<Planned>), Error> {
    let id = args.id.as_str();
    let sources = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?.sources;
    if !sources.iter().any(|source| source.id == id && source.fetch.kind == FetchKind::Prepared) {
        return Err(Code::Usage.error(format!("data/sources.toml has no prepared source `{id}`")));
    }
    let target = crate::target::find(root, &args.from, store).map_err(|e| Code::Usage.error(e))?;
    let from = target.within(&format!("sources/{id}"));
    let home = prepare_cli::home(http, id)?;
    let folder = prepared::folder(&home, id);
    let ours = prepared::versions(&folder, id)?;
    let mut theirs = prepared::versions(&from, id)?;
    theirs.sort_by_key(|entry| prepared::order(&entry.version));
    let refused = |text: String| Code::Failed.error(format!("{text}; nothing changed"));
    let mut versions = Vec::new();
    for entry in theirs {
        let (version, sha256) = (&entry.version, &entry.manifest_sha256);
        let key = format!("{version}/{MANIFEST}");
        let bytes = from.get(&key)?.ok_or_else(|| refused(format!("`{}` has no sources/{id}/{key}", args.from)))?;
        if sha256_hex(&bytes) != *sha256 {
            return Err(refused(format!("sources/{id}/{key} of `{}` is not the manifest that it lists", args.from)));
        }
        let listed = ours.iter().find(|ours| ours.version == *version);
        let held = folder.get(&key)?;
        if listed.is_some_and(|ours| ours.manifest_sha256 != *sha256) || held.is_some_and(|held| held != bytes) {
            return Err(refused(format!("the home has {id}@{version} with another manifest")));
        }
        let manifest = Manifest::parse(id, version, &bytes)?;
        let own: Vec<_> = manifest.files.iter().filter(|file| !manifest.reused.contains_key(&file.path)).collect();
        let sizes: Vec<(String, u64)> =
            own.iter().map(|file| (format!("{version}/{}", file.path), file.size)).collect();
        let lacks = folder.missing(&sizes).map_err(|e| {
            Code::Failed.error(format!("{id}/{e}")).fix("Delete that file of the home, then run the copy again.")
        })?;
        let (mut copy, mut kept) = (Downloads::default(), Downloads::default());
        let (mut missing, mut held) = (Vec::new(), Vec::new());
        for (file, (key, size)) in own.iter().zip(sizes) {
            let (tally, list) = if lacks.contains(&key) { (&mut copy, &mut missing) } else { (&mut kept, &mut held) };
            (tally.files, tally.bytes) = (tally.files + 1, tally.bytes + size);
            list.push((key, file.sha256.clone()));
        }
        let reused = manifest.reused.iter().map(|(path, origin)| {
            let size = manifest.files.iter().find(|file| &file.path == path).map_or(0, |file| file.size);
            (format!("{origin}/{path}"), size)
        });
        let shown = CopiedVersion { version: version.clone(), manifest_sha256: sha256.clone(), copy, held: kept };
        let held = if listed.is_some() { Vec::new() } else { held };
        versions.push(Planned { shown, manifest: bytes, missing, held, reused: reused.collect() });
    }
    if versions.is_empty() {
        return Err(Code::Usage.error(format!("`{}` lists no version of `{id}`; nothing changed", args.from)));
    }
    let shown = versions.iter().map(|planned| planned.shown.clone()).collect();
    Ok((CopyPlan { source: id.into(), from: args.from.clone(), root: home, versions: shown }, versions))
}

/// In the run, under the lock of the source: plan again, then copy each version oldest first, its
/// files, then its manifest, then its entry of `versions.json`.
fn copy(root: &Path, store: &Store, http: &Http, args: &CopyArgs, run: &mut Run) -> Result<CopyPlan, Error> {
    let id = args.id.as_str();
    let home = prepare_cli::home(http, id)?;
    let folder = prepared::folder(&home, id);
    let dir = prepared::path(&home, id);
    prepare_cli::locked(store, (&home, id), run, |run, lock| {
        let (plan, versions) = plan(root, store, http, args)?;
        let bytes: u64 = plan.versions.iter().map(|version| version.copy.bytes).sum();
        // The files go into the store, then into the home.
        let room = match one_disk(&home, store.root()) {
            true => crate::store::check_free(&home, bytes.saturating_mul(2)),
            false => {
                crate::store::check_free(&home, bytes).and_then(|()| crate::store::check_free(store.root(), bytes))
            }
        };
        room.map_err(|e| Code::Blocked.error(format!("{e}; nothing changed")))?;
        let target = crate::target::find(root, &args.from, store).map_err(|e| Code::Usage.error(e))?;
        let from = target.within(&format!("sources/{id}"));
        let other = |key: &str| {
            Code::Failed
                .error(format!("{id}/{key} of the home has other bytes than its manifest; the version is not listed"))
                .fix("Delete that file of the home, then run the copy again.")
        };
        for Planned { shown, manifest, missing, held, reused } in versions {
            let version = &shown.version;
            run.line(&format!("{id}@{version}: {} files, {}", shown.copy.files, runs::bytes(shown.copy.bytes)))?;
            for (at, (key, sha256)) in missing.iter().chain(&held).enumerate() {
                if at % 100 == 0 && !lock.beat(None, None, crate::date::now())? {
                    return Err(update_cli::lost(&prepare_cli::lock_names(id).0));
                }
                run.check_stop(store)?;
                if at >= missing.len() {
                    // A file that an earlier run copied, or that a person put there.
                    if crate::store::hash_file(&dir.join(key))?.0 != *sha256 {
                        return Err(other(key));
                    }
                    continue;
                }
                let object = from.download(key, sha256, store).map_err(|e| format!("sources/{id}/{e}"))?;
                let object = object.ok_or_else(|| format!("`{}` has no sources/{id}/{key}", args.from))?;
                place(&object, &dir.join(key))?;
            }
            if let Some(key) = folder.missing(&reused)?.first() {
                return Err(Code::Failed.error(format!("{id}/{key}, which {id}@{version} reuses, is not in the home")));
            }
            folder.put(&format!("{version}/{MANIFEST}"), &manifest, &Condition::Absent)?;
            prepared::list(&folder, id, version)?;
            leftovers(&dir.join(version));
        }
        Ok(plan)
    })
}

/// Whether `a` and `b` are on one file system.
fn one_disk(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let device = |path: &Path| path.ancestors().find_map(|path| fs::metadata(path).ok()).map(|found| found.dev());
        device(a).is_some() && device(a) == device(b)
    }
    #[cfg(not(unix))]
    {
        let _ = (a, b);
        false
    }
}

/// Delete the hidden `.part` files that a stopped copy left in the folder of a complete version.
fn leftovers(folder: &Path) {
    let mut pending = vec![folder.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let (path, name) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
            if path.is_dir() {
                pending.push(path);
            } else if name.starts_with('.') && name.ends_with(".part") {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Give `path` a copy of `object`, never a link: a store object is not a file of the archive.
fn place(object: &Path, path: &Path) -> Result<(), String> {
    let parent = path.parent().expect("a file of a version has a folder");
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let part = crate::target::part(path);
    let copied =
        fs::copy(object, &part).and_then(|_| fs::File::open(&part)?.sync_all()).and_then(|()| fs::rename(&part, path));
    copied.map_err(|e| {
        let _ = fs::remove_file(&part);
        format!("{}: {e}", path.display())
    })
}

fn print_plan(plan: &CopyPlan, json: bool) -> Result<(), Error> {
    if json {
        return print_json(plan);
    }
    println!("copy of {} from {} into {}", plan.source, plan.from, plan.root.display());
    let mut rows = Vec::new();
    for version in &plan.versions {
        let name = format!("  {}", prepared::show(&format!("{}/{}", version.version, version.manifest_sha256)));
        let text = format!(
            "{} files, {} to copy; {} files, {} in place",
            version.copy.files,
            runs::bytes(version.copy.bytes),
            version.held.files,
            runs::bytes(version.held.bytes)
        );
        rows.push(vec![name, text]);
    }
    rows.push(vec!["  start".into(), format!("obc data source copy {} --from {}", plan.source, plan.from)]);
    print_table(&rows);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::cli::prepare_cli::tests::{prepare, setup};
    use crate::cli::prepare_cli::Asked;
    use crate::engine::tests::{write, Fixture};
    use crate::target::Target;

    /// Copy `sample` from the target `prod` into the archive of `fixture`.
    fn copied(fixture: &Fixture, dry_run: bool) -> Result<CopyPlan, Error> {
        let (root, http) = (fixture.root(), crate::cli::prepare_cli::tests::http(fixture));
        let args = CopyArgs { id: "sample".into(), from: "prod".into(), dry_run };
        if dry_run {
            return plan(&root, &fixture.store, &http, &args).map(|(plan, _)| plan);
        }
        let mut run = Run::create(&fixture.store, ["source", "copy", "sample"]).unwrap();
        let done = copy(&root, &fixture.store, &http, &args, &mut run);
        crate::cli::api::finish_run(run, done)
    }

    /// The SHA-256 of each file of the folder `dir` by path, without the files of the lock.
    fn tree(dir: &Path) -> BTreeMap<String, String> {
        let listed = Target::Dir(dir.to_path_buf()).list().unwrap().into_iter().map(|listed| listed.key);
        let kept = listed.filter(|key| !key.starts_with('.') && key != crate::lock::LOCK);
        kept.map(|key| (key.clone(), crate::store::hash_file(&dir.join(&key)).unwrap().0)).collect()
    }

    /// Two versions of `sample` in the legacy folder of `prod`, the second with a file of the first:
    /// the fixture, the home and that folder.
    fn legacy(name: &str) -> (Fixture, PathBuf, PathBuf) {
        let (fixture, archive, prod) = setup(name);
        prepare(&fixture, "2026-10-01", Asked::default()).unwrap();
        write(&fixture.root().join("reuse-a.txt"), "");
        prepare(&fixture, "2026-10-02", Asked::default()).unwrap();
        let (home, legacy) = (prepared::path(&archive, "sample"), prod.join("sources/sample"));
        fs::create_dir_all(prod.join("sources")).unwrap();
        fs::rename(&home, &legacy).unwrap();
        (fixture, home, legacy)
    }

    #[test]
    fn a_copy_takes_each_version_with_its_bytes_resumes_and_comes_before_a_prepare() {
        let (fixture, home, legacy) = legacy("copy");
        let refused = prepare(&fixture, "2026-10-03", Asked::default()).unwrap_err();
        assert!(refused.fix.contains("`obc data source copy sample --from prod`"), "{}", refused.fix);
        assert!(!home.exists(), "a refused prepare leaves no home");
        // A copy that stopped left one file of the first version.
        fs::create_dir_all(home.join("2026-10-01")).unwrap();
        fs::copy(legacy.join("2026-10-01/a.txt"), home.join("2026-10-01/a.txt")).unwrap();
        let before = tree(&legacy);

        let planned = copied(&fixture, true).unwrap();
        let shown: Vec<_> = planned.versions.iter().map(|v| (v.version.as_str(), v.copy.files, v.held.files)).collect();
        assert_eq!(shown, [("2026-10-01", 2, 1), ("2026-10-02", 2, 0)], "a reused file is copied with its version");
        copied(&fixture, false).unwrap();
        assert_eq!(tree(&home), before, "the same bytes and versions.json");
        assert_eq!(tree(&legacy), before, "the target stays");
        let again = copied(&fixture, false).unwrap();
        assert!(again.versions.iter().all(|version| version.copy.files == 0), "a second copy copies nothing");
        assert_eq!(prepare(&fixture, "2026-10-03", Asked::default()).unwrap().version, "2026-10-03");
    }

    #[test]
    fn a_copy_refuses_other_bytes_another_manifest_and_too_little_room() {
        let (fixture, home, legacy) = legacy("copy-refuse");
        let (a, right) = ("2026-10-01/a.txt", fs::read(legacy.join("2026-10-01/a.txt")).unwrap());
        write(&home.join(a), "other size");
        let refused = copied(&fixture, true).unwrap_err();
        assert!(refused.message.contains("holds 10 bytes"), "{}", refused.message);
        assert!(refused.fix.starts_with("Delete that file"), "{}", refused.fix);
        let mut wrong = right.clone();
        wrong[0] ^= 1;
        fs::write(home.join(a), &wrong).unwrap();
        let refused = copied(&fixture, false).unwrap_err();
        assert!(refused.message.contains("2026-10-01/a.txt of the home has other bytes"), "{}", refused.message);
        assert!(prepared::versions(&Target::Dir(home.clone()), "sample").unwrap().is_empty(), "nothing is listed");
        fs::write(home.join(a), &right).unwrap();
        copied(&fixture, false).unwrap();

        let (ours, theirs) = (home.join("versions.json"), legacy.join("versions.json"));
        let listed = fs::read_to_string(&ours).unwrap();
        let sha256 = prepared::versions(&Target::Dir(home.clone()), "sample").unwrap()[0].manifest_sha256.clone();
        fs::write(&ours, listed.replace(&sha256, &"0".repeat(64))).unwrap();
        let refused = copied(&fixture, true).unwrap_err();
        assert!(
            refused.message.contains("the home has sample@2026-10-01 with another manifest"),
            "{}",
            refused.message
        );
        fs::write(&ours, &listed).unwrap();
        let tampered = fs::read_to_string(&theirs).unwrap().replace(&sha256, &"0".repeat(64));
        fs::write(&theirs, &tampered).unwrap();
        let refused = copied(&fixture, true).unwrap_err();
        assert!(refused.message.contains("is not the manifest that it lists"), "{}", refused.message);
        fs::write(&theirs, tampered.replace(&"0".repeat(64), &sha256)).unwrap();

        // A version whose file no disk holds.
        let source = Target::Dir(legacy.clone());
        let bytes = fs::read(legacy.join("2026-10-02/manifest.json")).unwrap();
        let mut huge = Manifest::parse("sample", "2026-10-02", &bytes).unwrap();
        huge.version = "2026-10-03".into();
        huge.reused.clear();
        huge.files = vec![crate::engine::LayerFile { path: "huge.bin".into(), size: 1 << 60, sha256: "0".repeat(64) }];
        write(&legacy.join("2026-10-03/manifest.json"), &String::from_utf8(huge.canonical()).unwrap());
        prepared::list(&source, "sample", "2026-10-03").unwrap();
        let refused = copied(&fixture, false).unwrap_err();
        assert!(refused.message.contains("nothing changed") && refused.message.contains("free"), "{}", refused.message);
        assert!(!home.join("2026-10-03").exists());
    }
}
