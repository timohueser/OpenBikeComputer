//! The app's only network code, in one place so what it reaches is auditable:
//! the Geofabrik index, the `.pbf` extracts that index names, and the published
//! map catalog. Nothing else, and never on behalf of the webview — the frontend
//! has no HTTP capability at all (see `capabilities/default.json`).

use std::io::{Read, Write};
use std::path::Path;

use obc_pack::progress::Progress;

/// Small documents (the region index, the catalog manifest) — read whole, because
/// both are parsed as one document and a partial one is worthless.
pub fn get_text(url: &str) -> Result<String, String> {
    let mut resp = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    resp.body_mut().read_to_string().map_err(|e| format!("read {url}: {e}"))
}

/// Download `url` to `dest`, reporting percentage through `on_pct` and honouring
/// `progress`'s cancel token.
///
/// The write goes to a `.part` sibling and is renamed on completion, so an
/// interrupted download — cancelled, crashed, unplugged — can never be mistaken
/// for a cached extract on the next run. A region is hundreds of megabytes and
/// this is where a cancelled build usually is when the user changes their mind,
/// so the token is checked every chunk.
pub fn download(url: &str, dest: &Path, progress: &Progress, mut on_pct: impl FnMut(u8)) -> Result<u64, String> {
    let dir = dest.parent().ok_or("download destination has no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let part = dest.with_extension("part");

    let mut resp = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    let total: u64 =
        resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);

    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(&part).map_err(|e| format!("create {}: {e}", part.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done: u64 = 0;
    let mut last_pct = u8::MAX;
    loop {
        if progress.is_cancelled() {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err("build cancelled".into());
        }
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
    drop(file);
    std::fs::rename(&part, dest).map_err(|e| format!("install {}: {e}", dest.display()))?;
    Ok(done)
}
