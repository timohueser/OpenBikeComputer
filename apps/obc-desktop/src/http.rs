//! The app's network surface, in one place so what it reaches is auditable: the
//! Geofabrik index, the `.pbf` extracts that index names, and the published map
//! catalog. Never on behalf of the webview — the frontend has no HTTP capability
//! at all (see `capabilities/default.json`).
//!
//! There is a fourth host, and it is worth naming here rather than leaving it to
//! be discovered: `obc-pack` fetches the ~950 MB land-polygon dataset from
//! `osmdata.openstreetmap.de` on the first build that needs land. That download has
//! always happened — it used to shell out to `curl`, which is exactly why it was
//! invisible from this file — and it now runs through the same code the functions
//! below do (`obc_pack::net`, #907). One downloader, one retry policy, one
//! cancellation contract; this module is the app's list of *what* it reaches, not a
//! second implementation of *how*.

use std::path::Path;

use obc_pack::progress::Progress;

/// Small documents (the region index, the catalog manifest) — read whole, because
/// both are parsed as one document and a partial one is worthless.
pub fn get_text(url: &str) -> Result<String, String> {
    obc_pack::net::get_text(url)
}

/// Download `url` to `dest`, reporting percentage through `on_pct` and honouring
/// `progress`'s cancel token. Returns the number of bytes written.
///
/// The write goes to a `.part` sibling and is renamed on completion, so an
/// interrupted download — cancelled, crashed, unplugged — can never be mistaken
/// for a cached extract on the next run. A region is hundreds of megabytes and
/// this is where a cancelled build usually is when the user changes their mind,
/// so the token is checked every chunk.
pub fn download(url: &str, dest: &Path, progress: &Progress, on_pct: impl FnMut(u8)) -> Result<u64, String> {
    obc_pack::net::download(url, dest, progress, on_pct)
}
