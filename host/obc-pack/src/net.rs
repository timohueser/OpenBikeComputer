//! Fetching bytes over HTTPS, and unpacking a `.zip` — **in process**, with no
//! `curl` and no `unzip`.
//!
//! This module exists because of what the packer became. As a developer's CLI it
//! could shell out to whatever a developer's machine happens to have; as the
//! engine inside a shipped desktop app (#906) it cannot. `curl` is not on every
//! Windows box and `unzip` is on essentially none of them — the two land-dataset
//! shell-outs would have failed on the very platform D2 (#907) exists to support,
//! *after* a user had already picked a region and started a build.
//!
//! Three things fall out of doing it in-process that the subprocesses could not
//! give us:
//!
//! - **Cancellation.** A `Command::status()` blocks until the child exits; a
//!   950 MB download through `curl` could not be stopped by the app's cancel
//!   token at all. Here the token is checked every chunk and every archive entry.
//! - **Progress.** `curl`'s meter went to the *app's* stderr, i.e. nowhere a user
//!   could see, which made a first build look hung for several minutes. The
//!   percentage now arrives through [`Progress`] like every other stage.
//! - **Zip-slip safety.** `unzip` will happily write `../../etc/whatever` from a
//!   hostile archive; [`ZipFile::enclosed_name`] refuses to.
//!
//! It is also the *only* downloader in the tree: the desktop app's `http.rs`
//! delegates here rather than keeping its own copy, so there is one retry policy,
//! one `.part`-then-rename rule, and one cancellation contract.
//!
//! [`ZipFile::enclosed_name`]: zip::read::ZipFile::enclosed_name

use std::io::{Read, Write};
use std::path::Path;

use crate::progress::Progress;

/// Read size for the download loop. Big enough that the syscall overhead is
/// irrelevant on a 950 MB body, small enough that a cancel lands promptly.
const CHUNK: usize = 1 << 16;

/// How many times a download is attempted before giving up. Matches the
/// `curl --retry 3` this replaced — and, like it, an attempt restarts from zero
/// rather than resuming, because the servers involved are not guaranteed to honour
/// a `Range` request and a silently truncated dataset is far worse than a slow one.
const ATTEMPTS: usize = 3;

/// Small documents (a region index, a catalog manifest) — read whole, because both
/// are parsed as one document and a partial one is worthless.
pub fn get_text(url: &str) -> Result<String, String> {
    let mut resp = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    resp.body_mut().read_to_string().map_err(|e| format!("read {url}: {e}"))
}

/// Download `url` to `dest`, reporting percentage through `on_pct` and honouring
/// `progress`'s cancel token. Returns the number of bytes written.
///
/// The write goes to a `.part` sibling and is renamed on completion, so an
/// interrupted download — cancelled, crashed, unplugged — can never be mistaken
/// for a cached extract on the next run. A region is hundreds of megabytes and
/// this is where a cancelled build usually is when the user changes their mind, so
/// the token is checked every chunk.
///
/// A failed attempt is retried up to [`ATTEMPTS`] times *unless* it failed because
/// the run was cancelled: retrying a cancellation would be the one way to make the
/// stop button do nothing.
pub fn download(url: &str, dest: &Path, progress: &Progress, mut on_pct: impl FnMut(u8)) -> Result<u64, String> {
    let dir = dest.parent().ok_or("download destination has no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let part = dest.with_extension("part");

    let mut last_err = String::new();
    for attempt in 1..=ATTEMPTS {
        match download_once(url, &part, progress, &mut on_pct) {
            Ok(done) => {
                std::fs::rename(&part, dest).map_err(|e| format!("install {}: {e}", dest.display()))?;
                return Ok(done);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                if progress.is_cancelled() {
                    return Err(e);
                }
                last_err = e;
                if attempt < ATTEMPTS {
                    progress.warn(format!("{last_err} — retrying ({attempt}/{})", ATTEMPTS - 1));
                }
            }
        }
    }
    Err(last_err)
}

/// One attempt: the whole body to `part`, or an error and whatever is on disk.
fn download_once(url: &str, part: &Path, progress: &Progress, on_pct: &mut impl FnMut(u8)) -> Result<u64, String> {
    let mut resp = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    let total: u64 =
        resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(part).map_err(|e| format!("create {}: {e}", part.display()))?;
    let mut buf = vec![0u8; CHUNK];
    let mut done: u64 = 0;
    let mut last_pct = u8::MAX;
    loop {
        progress.check()?;
        let n = reader.read(&mut buf).map_err(|e| format!("read {url}: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("write {}: {e}", part.display()))?;
        done += n as u64;
        // No Content-Length ⇒ no percentage to report. The bar stays where the
        // phase put it rather than inventing motion.
        if let Some(pct) = (done * 100).checked_div(total) {
            let pct = (pct as u8).min(100);
            if pct != last_pct {
                last_pct = pct;
                on_pct(pct);
            }
        }
    }
    file.flush().map_err(|e| format!("flush {}: {e}", part.display()))?;
    Ok(done)
}

/// Extract every entry of the zip at `archive` beneath `dest_dir`, creating it.
///
/// Entry names are resolved with [`zip::read::ZipFile::enclosed_name`], which
/// returns `None` for anything that would escape the destination (`..`, an
/// absolute path, a Windows drive letter). Such an entry is a **hard error**, not
/// a skip: a land dataset that contains one is not a land dataset, and quietly
/// unpacking the rest of it would hand the packer a half-archive to puzzle over.
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

    /// Build a deflate-compressed zip in memory, the way the land dataset is
    /// shipped: one directory with files under it.
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

    /// The `unzip` replacement, doing the job `unzip` used to do: a nested archive
    /// unpacks with its directory structure and its bytes intact.
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

    /// Zip-slip. `unzip` needed `-j` or a careful invocation to be safe about this;
    /// refusing is the default here, and it refuses *loudly*.
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

    /// A cancelled run stops the unpack, which is the half of the story `unzip`
    /// could not do at all — the app's stop button could not reach into a
    /// subprocess.
    #[test]
    fn a_cancelled_run_stops_the_unpack() {
        let dir = tmp("cancel");
        let archive = dir.join("dataset.zip");
        std::fs::write(&archive, sample_zip(&[("a.txt", b"a"), ("b.txt", b"b")])).expect("write archive");

        let cancel = crate::progress::CancelToken::new();
        cancel.cancel();
        let progress = Progress::new(cancel, |_, _| {});
        let out = dir.join("out");
        extract_zip(&archive, &out, &progress).expect_err("a cancelled unpack must not finish");
        assert!(!out.join("b.txt").exists(), "the unpack stopped before the last entry");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
