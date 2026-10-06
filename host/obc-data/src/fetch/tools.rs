//! Tool preparation may download pinned packages. The bake reads the prepared tool from the store.

use std::fs;
use std::process::{Command, Stdio};

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
pub(super) fn basemap(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let version = request.version.as_deref().ok_or("protomaps-basemaps needs a commit: give SOURCE@VERSION")?;
    if version.len() != 40 || !version.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("protomaps-basemaps needs a full commit SHA".into());
    }
    if !request.params.is_empty() {
        return Err("protomaps-basemaps takes no parameters".into());
    }
    let archive = files(store, http, request)?;
    let name = basemap_jar();
    let url = format!("{}#{name}", archive.files[0].url);
    let _lock = Http::lock(store, &url)?;
    if let Some(snapshot) = store.snapshot(&archive.source, version)? {
        if snapshot.file(&url).is_some_and(|jar| store.object(&jar.sha256).is_file()) {
            return Ok(snapshot);
        }
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = crate::find_root(&cwd).ok_or("no data/sources.toml above the current directory")?;
    let work = store.partial(&format!("{version}-{name}"));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let work = std::path::absolute(work).map_err(|e| e.to_string())?;
    let result = (|| {
        let input = std::path::absolute(store.object(&archive.files[0].sha256)).map_err(|e| e.to_string())?;
        let jar = work.join("basemap.jar");
        let status = Command::new("uv")
            .args(["run", "--locked", "--offline", "python", "-m", "tools.basemap_tool"])
            .arg(input)
            .arg(&jar)
            .current_dir(root)
            .stdout(Stdio::from(std::io::stderr()))
            .status()
            .map_err(|e| format!("basemap tool preparation: {e}"))?;
        if !status.success() {
            return Err(format!("basemap tool preparation failed: {status}"));
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
        assert_eq!(basemap(&store, &Http::new(), &request).unwrap(), snapshot);
        let request = Request { version: Some("main".into()), ..request };
        assert!(basemap(&store, &Http::new(), &request).unwrap_err().contains("full commit SHA"));
    }
}
