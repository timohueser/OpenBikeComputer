//! Tool preparation may download pinned packages. The bake reads the prepared tool from the store.

use std::fs;
use std::path::Path;
use std::process::Stdio;

use super::{files, record, Request};
use crate::date;
use crate::fetch::http::Http;
use crate::store::{hash_file, sha256_hex, FileRecord, Snapshot, Store};

/// The jar of the current preparation recipe. Other jars of the commit stay unselected.
pub fn basemap_jar() -> String {
    let recipe = sha256_hex(include_bytes!("../../../../tools/basemap_tool.py"));
    format!("basemap-{recipe}.jar")
}

/// The basemaps source archive and its executable jar, including Planetiler and its dependencies.
pub(super) fn basemap(
    root: &Path,
    store: &Store,
    http: &Http,
    request: &Request,
    checks: Option<crate::fetch::Checks<'_>>,
) -> Result<Snapshot, String> {
    let version = request.version.as_deref().ok_or("protomaps-basemaps needs a commit: give SOURCE@VERSION")?;
    if version.len() != 40 || !version.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("protomaps-basemaps needs a full commit SHA".into());
    }
    if !request.params.is_empty() {
        return Err("protomaps-basemaps takes no parameters".into());
    }
    let name = basemap_jar();
    let archive_url =
        super::expand(request.source.fetch.url.as_deref().unwrap_or_default(), Some(version), &request.params)?
            .into_iter()
            .next()
            .ok_or("protomaps-basemaps has no archive URL")?;
    let url = format!("{archive_url}#{name}");
    let _lock = Http::lock(store, &url)?;
    if let Some(snapshot) = store.snapshot(&request.source.id, version)? {
        if [&archive_url, &url]
            .iter()
            .all(|url| snapshot.file(url).is_some_and(|file| store.object(&file.sha256).is_file()))
        {
            return Ok(snapshot);
        }
    }
    super::check_owner(root, request.source, checks)?;
    let code = super::owner_code(request.source).code;
    let before = code.files(root)?;
    let archive = files(store, http, request)?;
    let work = store.partial(&format!("{version}-{name}"));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let work = std::path::absolute(work).map_err(|e| e.to_string())?;
    let result = (|| {
        let input = std::path::absolute(store.object(&archive.files[0].sha256)).map_err(|e| e.to_string())?;
        let jar = work.join("basemap.jar");
        let status = super::capture::python(root, None)?
            .args(["-m", "tools.basemap_tool"])
            .arg(input)
            .arg(&jar)
            .current_dir(root)
            .stdout(Stdio::from(std::io::stderr()))
            .status()
            .map_err(|e| format!("basemap tool preparation: {e}"))?;
        if !status.success() {
            return Err(format!("basemap tool preparation failed: {status}"));
        }
        if code.files(root)? != before {
            return Err("tool preparation code or Python runtime changed; plan again".into());
        }
        let (sha256, size) = hash_file(&jar)?;
        store.insert(&jar, &sha256)?;
        let file = FileRecord { name, url, size, sha256, retrieved: date::timestamp(date::now()) };
        record(store, &archive.source, version, &[file])?;
        store.snapshot(&archive.source, version)?.ok_or("basemap snapshot disappeared".into())
    })();
    let _ = fs::remove_dir_all(work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::embedded;
    use crate::store::tests::Scratch;
    use crate::store::{sha256_hex, write_atomic};

    #[test]
    fn a_prepared_tool_needs_no_network_and_preserves_the_archive_identity() {
        let scratch = Scratch::new("basemap-tool");
        let store = Store::at(scratch.0.join("store"));
        let source = embedded("protomaps-basemaps");
        let version = "42ffaaa4a85a41bfcb23e43cc0f5b492a5eca123";
        let url = source.fetch.url.as_ref().unwrap().replace("{version}", version);
        let mut files = Vec::new();
        for (name, url, bytes) in [
            (version, url.clone(), b"archive".as_slice()),
            (basemap_jar().as_str(), format!("{url}#{}", basemap_jar()), b"jar"),
        ] {
            let path = store.partial("tool-file");
            write_atomic(&path, bytes).unwrap();
            let sha256 = sha256_hex(bytes);
            store.insert(&path, &sha256).unwrap();
            files.push(FileRecord {
                name: name.into(),
                url,
                size: bytes.len() as u64,
                sha256,
                retrieved: String::new(),
            });
        }
        let snapshot = Snapshot { source: source.id.clone(), version: version.into(), files };
        store.put_snapshot(&snapshot).unwrap();
        let request = Request { source, version: Some(version.into()), params: Vec::new() };
        assert_eq!(
            basemap(
                &scratch.0.join("absent-checkout"),
                &store,
                &Http::new(),
                &request,
                Some((&mut crate::engine::code::Context::default(), true))
            )
            .unwrap(),
            snapshot
        );
        assert!(!scratch.0.join("absent-checkout").exists());
        let request = Request { version: Some("main".into()), ..request };
        assert!(basemap(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").as_path(),
            &store,
            &Http::new(),
            &request,
            None
        )
        .unwrap_err()
        .contains("full commit SHA"));
    }

    #[test]
    fn cold_tool_admission_precedes_archive_download_and_snapshot_writes() {
        use crate::engine::tests::{fixture, repository, write};
        use crate::fetch::tests::{quick, serve, whole};
        let fixture = fixture("basemap-admission");
        let root = fixture.root();
        repository(&root, &[("obc-data", "")]);
        let (url, asked) = serve(|_, _| whole(b"archive"));
        let mut source = embedded("protomaps-basemaps").clone();
        source.fetch.url = Some(url.replace("file.bin", "{version}.tar.gz"));
        for path in super::super::owner_code(&source).code.paths {
            write(&root.join(path), "// fixture acquisition owner\n");
        }
        fixture.with_sources(std::slice::from_ref(&source));
        write(&root.join(".python-version"), "3.12\n");
        write(&root.join("pyproject.toml"), "[project]\nname = \"capture-fixture\"\nversion = \"0\"\nrequires-python = \">=3.12\"\ndependencies = []\n[tool.uv]\npackage = false\ndefault-groups = []\n");
        write(&root.join("uv.lock"), "version = 1\nrevision = 3\nrequires-python = \">=3.12\"\n[[package]]\nname = \"capture-fixture\"\nversion = \"0\"\nsource = { virtual = \".\" }\n");
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git").args(args).current_dir(&root).status().unwrap().success());
        };
        git(&["add", "."]);
        git(&["-c", "user.name=Fixture", "-c", "user.email=fixture@example.org", "commit", "-qm", "fixture"]);
        write(&root.join("tools/basemap_tool.py"), "# uncommitted preparation\n");
        let version = "42ffaaa4a85a41bfcb23e43cc0f5b492a5eca123";
        let request = Request { source: &source, version: Some(version.into()), params: Vec::new() };
        let mut checks = crate::engine::code::Context::default();
        let error = basemap(&root, &fixture.store, &quick(), &request, Some((&mut checks, true))).unwrap_err();
        assert!(error.contains("not committed"), "{error}");
        assert!(asked.lock().unwrap().is_empty(), "admission precedes the archive request");
        assert!(fixture.store.snapshot(&source.id, version).unwrap().is_none());
        assert!(!fixture.store.object(&sha256_hex(b"archive")).exists());
    }
}
