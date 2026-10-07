//! Reading a small document over HTTPS, and unpacking a `.zip`, in process, with no `curl` and no
//! `unzip`.
//!
//! `unzip` is on essentially no Windows box, and doing it in process buys two things a subprocess
//! could not give. Cancellation: the token is checked every archive entry, where a
//! `Command::status()` blocks until the child exits. Zip-slip safety:
//! [`ZipFile::enclosed_name`] refuses a hostile `../../etc/whatever` entry.
//!
//! Bake data comes from the store of `obc-data`. The one read here is the live catalog that the
//! guard reads.
//!
//! [`ZipFile::enclosed_name`]: zip::read::ZipFile::enclosed_name

use std::io::Write;
use std::path::Path;

use obc_map_core::progress::Progress;

/// Small documents, such as a region index or a catalog manifest, are read whole: each is parsed as
/// one document and a partial one is worthless.
pub fn get_text(url: &str) -> Result<String, String> {
    let mut resp = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    resp.body_mut().read_to_string().map_err(|e| format!("read {url}: {e}"))
}

/// Extract every entry of the zip at `archive` beneath `dest_dir`, creating it.
///
/// Entry names are resolved with [`zip::read::ZipFile::enclosed_name`], which returns `None` for
/// anything that would escape the destination. Such an entry is a hard error and not a skip: a land
/// dataset that contains one is not a land dataset.
pub fn extract_zip(archive: &Path, dest_dir: &Path, progress: &Progress) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
    let mut zip =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| format!("read {}: {e}", archive.display()))?;

    for i in 0..zip.len() {
        progress.check()?;
        let mut entry = zip.by_index(i).map_err(|e| format!("entry {i} of {}: {e}", archive.display()))?;
        let name = entry.enclosed_name().ok_or_else(|| {
            format!("{}: entry {:?} escapes the destination directory", archive.display(), entry.name())
        })?;
        let out = dest_dir.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut sink =
            std::io::BufWriter::new(std::fs::File::create(&out).map_err(|e| format!("create {}: {e}", out.display()))?);
        std::io::copy(&mut entry, &mut sink).map_err(|e| format!("write {}: {e}", out.display()))?;
        sink.flush().map_err(|e| format!("flush {}: {e}", out.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("obc-pack-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// Build a deflate-compressed zip in memory, the way the land dataset is shipped: one directory
    /// with files under it.
    fn sample_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut w = zip::ZipWriter::new(&mut buf);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in entries {
            w.start_file(*name, opts).expect("start entry");
            w.write_all(body).expect("write entry");
        }
        w.finish().expect("finish zip");
        buf.into_inner()
    }

    /// A nested archive unpacks with its directory structure and its bytes intact.
    #[test]
    fn a_zip_unpacks_without_the_unzip_binary() {
        let dir = tmp("extract");
        let archive = dir.join("dataset.zip");
        std::fs::write(
            &archive,
            sample_zip(&[
                ("land-polygons-split-3857/land_polygons.shp", b"shapefile bytes"),
                ("land-polygons-split-3857/land_polygons.prj", b"PROJCS[...]"),
            ]),
        )
        .expect("write archive");

        let out = dir.join("out");
        extract_zip(&archive, &out, &Progress::silent()).expect("extract");
        assert_eq!(
            std::fs::read(out.join("land-polygons-split-3857/land_polygons.shp")).expect("shp"),
            b"shapefile bytes"
        );
        assert!(out.join("land-polygons-split-3857/land_polygons.prj").exists(), "every entry is written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Zip-slip: refusing is the default here, and it refuses loudly.
    #[test]
    fn an_entry_that_escapes_the_destination_is_refused() {
        let dir = tmp("slip");
        let archive = dir.join("hostile.zip");
        std::fs::write(&archive, sample_zip(&[("../escaped.txt", b"nope")])).expect("write archive");

        let out = dir.join("out");
        let err = extract_zip(&archive, &out, &Progress::silent()).expect_err("must refuse");
        assert!(err.contains("escapes"), "the refusal must say why: {err}");
        assert!(!dir.join("escaped.txt").exists(), "nothing may be written outside the destination");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cancelled run stops the unpack, which a stop button could not do to a subprocess.
    #[test]
    fn a_cancelled_run_stops_the_unpack() {
        let dir = tmp("cancel");
        let archive = dir.join("dataset.zip");
        std::fs::write(&archive, sample_zip(&[("a.txt", b"a"), ("b.txt", b"b")])).expect("write archive");

        let cancel = obc_map_core::progress::CancelToken::new();
        cancel.cancel();
        let progress = Progress::new(cancel, |_, _| {});
        let out = dir.join("out");
        extract_zip(&archive, &out, &progress).expect_err("a cancelled unpack must not finish");
        assert!(!out.join("b.txt").exists(), "the unpack stopped before the last entry");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
