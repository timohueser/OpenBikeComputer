//! The OpenBikeComputer desktop app: the same web builder, in a Tauri shell, with
//! `obc-pack` linked in instead of a FastAPI server in front of a subprocess
//! (#906, part of #894's phase D).
//!
//! ## What this replaces
//!
//! The dev server is Python: six HTTP endpoints, a job queue, and a `subprocess`
//! whose stdout it reads a byte at a time to guess which stage the packer is in.
//! Every one of those is a Rust command here, and the guessing is gone — the
//! packer *names* its phases now ([`obc_pack::progress::Phase`]), so the progress
//! bar reads a value instead of a prefix. Nothing in the shipped app is Python;
//! nothing in it spawns the packer.
//!
//! | dev server | here |
//! |---|---|
//! | `GET /api/regions` | [`regions`] |
//! | `GET /api/presets` | [`presets`] |
//! | `GET /api/schema` (spawns `obc-pack schema`) | [`schema`] (the linked library) |
//! | `GET /api/palette` | [`palette`] |
//! | `POST /api/jobs` + SSE + `GET .../download` | [`build_start`] + a channel + a real path |
//! | — | [`storage_info`] / [`storage_clear`], because caches this large need a door |
//! | — | [`usb`], because the webview has no WebUSB and this tier is the universal USB path |
//! | — | [`rides`], because a durable copy of a ride is what a browser cannot promise |
//!
//! ## Why the frontend can't do any of it itself
//!
//! The window is granted `core:default` and nothing else: no filesystem, no shell,
//! no HTTP. Every one of those policies is written in Rust, where it can be read —
//! `storage_clear` takes an id from a fixed table rather than a path, the build
//! command writes only into the maps folder, and the only URLs the app fetches are
//! the Geofabrik index, the extracts that index names, and the catalog.

mod build_job;
mod catalog;
mod content;
mod http;
mod paths;
mod regions;
mod rides;
mod storage;
mod usb;

use std::sync::Arc;

use build_job::{BuildEvent, BuildRequest, JobSnapshot, Jobs};
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{Manager, State};

/// The Geofabrik download-region tree for the area picker.
#[tauri::command]
async fn regions() -> Result<Value, String> {
    // On the blocking pool: a cold index is an HTTP fetch plus a GEOS simplify
    // over a few hundred boundaries, and the window must stay alive through it.
    tauri::async_runtime::spawn_blocking(regions::regions).await.map_err(|e| e.to_string())?
}

/// The shipped style presets, default first.
#[tauri::command]
fn presets() -> Vec<content::Preset> {
    content::presets()
}

/// `obc-pack`'s config JSON Schema envelope, from the packer linked into this
/// binary — so the editor's capability cannot disagree with what packs.
#[tauri::command]
fn schema() -> Result<Value, String> {
    content::schema()
}

/// The device's color gamut, laid out for the picker grid.
#[tauri::command]
fn palette() -> Value {
    content::palette()
}

/// The published catalog of pre-baked maps, body and base URL.
#[tauri::command]
async fn catalog() -> Result<catalog::FetchedCatalog, String> {
    tauri::async_runtime::spawn_blocking(catalog::fetch).await.map_err(|e| e.to_string())?
}

/// Where built maps go. Shown in the UI, so it is a fact the user can act on.
fn maps_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    paths::maps_dir(app.path().document_dir().ok())
}

#[tauri::command]
fn build_start(
    app: tauri::AppHandle,
    jobs: State<'_, Arc<Jobs>>,
    request: BuildRequest,
    on_event: Channel<BuildEvent>,
) -> Result<String, String> {
    build_job::start(Arc::clone(&jobs), request, maps_dir(&app), on_event)
}

/// The active (or most recent) build, for a window that reloaded mid-build.
#[tauri::command]
fn build_active(jobs: State<'_, Arc<Jobs>>) -> Option<JobSnapshot> {
    jobs.snapshot()
}

/// Re-point a build's events at a new channel, replaying what it already said.
#[tauri::command]
fn build_attach(jobs: State<'_, Arc<Jobs>>, id: String, on_event: Channel<BuildEvent>) -> bool {
    jobs.attach(&id, on_event)
}

#[tauri::command]
fn build_cancel(jobs: State<'_, Arc<Jobs>>, id: String) -> bool {
    jobs.cancel(&id)
}

/// Every place the app has put bytes on this disk, with sizes.
#[tauri::command]
fn storage_info(app: tauri::AppHandle) -> Vec<storage::Place> {
    storage::places(&maps_dir(&app), &paths::ride_archive_dir(app.path().app_data_dir().ok()))
}

/// Delete one named cache. Returns the bytes freed.
#[tauri::command]
async fn storage_clear(id: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || storage::clear(&id)).await.map_err(|e| e.to_string())?
}

/// Largest style export this command will write.
///
/// A working config with every feature type spelled out is tens of kilobytes; a
/// megabyte is two orders of magnitude of headroom and still a bound. It is here
/// because "the window may write a file" needs one — not because anything
/// legitimate approaches it.
const MAX_STYLE_BYTES: usize = 1024 * 1024;

/// Write an exported style config where the user can find it (E3 #913).
///
/// **The webview cannot save a file itself, and its usual trick does not work
/// here.** A browser exports by clicking an `<a download>` at a blob URL; inside
/// this app that is silently a no-op, because wry only installs a download
/// delegate when the embedder supplies a handler and Tauri supplies one only if
/// the application asks. So the export is a command, and it is the same shape as
/// every other filesystem policy in this crate: a fixed folder, a sanitised
/// basename, a size ceiling, and a path handed back so the UI can say where the
/// file went and offer to reveal it. The frontend names a file; it never names a
/// place.
#[tauri::command]
fn save_style(app: tauri::AppHandle, name: String, body: String) -> Result<String, String> {
    if body.len() > MAX_STYLE_BYTES {
        return Err(format!("that config is {} bytes; the limit is {MAX_STYLE_BYTES}", body.len()));
    }
    let dir = paths::styles_dir(app.path().document_dir().ok());
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = paths::unique_in(&dir, &paths::sanitize_basename(&name, ".json", "style"));
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Show a produced file in the platform's file manager.
///
/// Scoped to folders this app owns on purpose: this is a command a webview can
/// call, and "reveal any path" is a wider door than the features that need it.
/// The ride library is listed separately from the maps folder rather than
/// assumed to be inside it, because a relocated library (E2 #912) is not.
#[tauri::command]
fn reveal_file(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let path = std::path::PathBuf::from(path);
    let roots = [maps_dir(&app), ride_library(&app).0.root().to_path_buf()];
    if !roots.iter().any(|root| path.starts_with(root)) {
        let names: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
        return Err(format!("only files under {} can be revealed", names.join(" or ")));
    }
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| format!("reveal {}: {e}", path.display()))
}

// ============================ the ride library (E2 #912) ============================

/// The library, and whether its visible GPX folder is the default one.
///
/// Resolved per call rather than held in state: the folder can move while the app is running, and
/// a cached root is the fact that would then be wrong exactly when a rider is looking at it. The
/// archive (the `.obcride` objects and the index) lives in app data and never moves — see
/// `rides.rs`'s module docs for the split.
fn ride_library(app: &tauri::AppHandle) -> (rides::Library, bool, Option<String>) {
    let default = paths::rides_dir(app.path().document_dir().ok());
    let archive = paths::ride_archive_dir(app.path().app_data_dir().ok());
    let (root, is_default) = match app.path().app_config_dir().ok().and_then(|dir| rides::configured(&dir)) {
        Some(chosen) if chosen != default => (chosen, false),
        _ => (default, true),
    };
    let library = rides::Library::new(root, archive);
    // The one-time move of pre-split folders (then a cheap no-op). A failure is not fatal — it
    // leaves the old files where they were and the library reading the safe, empty direction, so
    // nothing is acked from a half-moved state — but it is *surfaced*: `rides_index` puts the
    // warning on screen, and `rides_choose_folder` refuses to move a folder that still holds
    // unmigrated files (relocating past them would orphan them permanently).
    let warning = library.migrate().err().map(|e| {
        eprintln!("ride library migration: {e}");
        format!(
            "Rides from an older version of this app could not be moved into the app's own storage \
             ({e}). They are unchanged in {}, but they will not appear here or sync until this is \
             fixed — is that folder read-only?",
            library.root().display()
        )
    });
    (library, is_default, warning)
}

/// The library folder and everything in it.
#[tauri::command]
fn rides_index(app: tauri::AppHandle) -> rides::IndexView {
    let (library, is_default, warning) = ride_library(&app);
    let mut view = library.view(is_default);
    view.migration_warning = warning;
    view
}

/// Land one pulled ride durably, and **only then** resolve.
///
/// Everything about this command is the interface spec's §4.4 rule that the desktop app acks
/// *after* `fsync`, never on transfer completion: the frontend awaits this, and sends `ackRides`
/// afterwards. A ride whose write failed rejects here, is absent from
/// [`rides_ack_set`](rides_ack_set), and is therefore never flagged on the device — so the worst a
/// power cut can cost is re-downloading a ride, which is the direction that does not lose one.
///
/// On the blocking pool because `fsync` genuinely blocks — that is the whole point of calling it.
#[tauri::command]
async fn rides_import(app: tauri::AppHandle, request: rides::ImportRequest) -> Result<rides::Imported, String> {
    let (library, _, _) = ride_library(&app);
    tauri::async_runtime::spawn_blocking(move || library.import(&request)).await.map_err(|e| e.to_string())?
}

/// The ride ids of one `(serial, epoch)` whose bytes are on this disk right now — the exact list
/// the frontend acks. See [`rides::Library::durable_ids`] for why it is computed here and not from
/// what the caller thinks it just wrote.
#[tauri::command]
fn rides_ack_set(app: tauri::AppHandle, serial: String, epoch: u32) -> Vec<u16> {
    ride_library(&app).0.durable_ids(&serial, epoch)
}

/// The stored ride object of one key — what a GPX re-export decodes.
#[tauri::command]
async fn rides_read(app: tauri::AppHandle, key: String) -> Result<tauri::ipc::Response, String> {
    let (library, _, _) = ride_library(&app);
    let bytes =
        tauri::async_runtime::spawn_blocking(move || library.read_object(&key)).await.map_err(|e| e.to_string())??;
    // The raw path, like `usb_read`: a ride object is hundreds of kilobytes and a JSON number
    // array would be four times that in text.
    Ok(tauri::ipc::Response::new(bytes))
}

/// (Re-)write one ride's GPX into the visible folder — the automatic repair for a GPX somebody
/// deleted, and the "Show in folder" fallback when the file to show is not there yet.
#[tauri::command]
async fn rides_write_gpx(app: tauri::AppHandle, key: String, gpx: String) -> Result<String, String> {
    let (library, _, _) = ride_library(&app);
    tauri::async_runtime::spawn_blocking(move || library.write_gpx(&key, &gpx)).await.map_err(|e| e.to_string())?
}

/// Let the rider pick a new home for the library, and move it there.
///
/// The native chooser, opened from Rust. That is not ceremony: the crate's rule is that the
/// frontend names a file and never a place, and the OS's own directory picker is the one way a
/// person can name a place without the webview being handed a filesystem. Resolves to the new
/// folder, or to `None` when the chooser was dismissed — which is an ordinary outcome, not an
/// error.
#[tauri::command]
async fn rides_choose_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let (library, _, migration_warning) = ride_library(&app);
    // A folder that still holds pre-split files must not be moved away from: the relocation moves
    // only GPX, so re-pointing the root would strand the old index and archives somewhere the app
    // never looks again — silently, permanently, and repeatably on a read-only folder.
    if migration_warning.is_some() || library.has_unmigrated() {
        return Err(format!(
            "{} still holds ride files from an older version of this app that could not be moved \
             into the app's own storage. Moving the library now would leave them behind for good — \
             fix that folder first (is it read-only?) and reopen the ride library.",
            library.root().display()
        ));
    }
    let from = library.root().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().set_title("Where should pulled rides be kept?").pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    // The chooser answers on the UI thread; parking a blocking-pool thread on the reply keeps the
    // event loop free to run it.
    let picked =
        tauri::async_runtime::spawn_blocking(move || rx.recv().ok().flatten()).await.map_err(|e| e.to_string())?;
    let Some(picked) = picked else { return Ok(None) };
    let to = picked.into_path().map_err(|e| format!("that folder could not be used: {e}"))?;

    let config = app.path().app_config_dir().map_err(|e| format!("no config directory: {e}"))?;
    let moved = to.clone();
    tauri::async_runtime::spawn_blocking(move || {
        rides::relocate(&from, &moved)?;
        rides::remember(&config, &moved)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(Some(to.display().to_string()))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Registered for its Rust API only — `rides_choose_folder` calls it. No JS permission is
        // granted, so the webview cannot open a picker of its own.
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Jobs::default()))
        .manage(Arc::new(usb::UsbState::default()))
        .invoke_handler(tauri::generate_handler![
            regions,
            presets,
            schema,
            palette,
            catalog,
            build_start,
            build_active,
            build_attach,
            build_cancel,
            storage_info,
            storage_clear,
            save_style,
            reveal_file,
            // E2 (#912). The library is a folder plus an index; the ack follows `rides_import`.
            rides_index,
            rides_import,
            rides_ack_set,
            rides_read,
            rides_write_gpx,
            rides_choose_folder,
            // D4 (#909). Bytes only — the protocol lives once, in TypeScript, over these.
            usb::usb_watch,
            usb::usb_list,
            usb::usb_open,
            usb::usb_close,
            usb::usb_read,
            usb::usb_write,
            usb::usb_cancel,
            usb::usb_reset,
            usb::usb_file_digest,
            usb::usb_send_file,
        ])
        .run(tauri::generate_context!())
        .expect("run the OpenBikeComputer app");
}
