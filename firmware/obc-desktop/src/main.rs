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
    storage::places(&maps_dir(&app))
}

/// Delete one named cache. Returns the bytes freed.
#[tauri::command]
async fn storage_clear(id: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || storage::clear(&id)).await.map_err(|e| e.to_string())?
}

/// Show a produced file in the platform's file manager.
///
/// Scoped to the maps folder on purpose: this is a command a webview can call, and
/// "reveal any path" is a wider door than the one feature that needs it.
#[tauri::command]
fn reveal_file(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let path = std::path::PathBuf::from(path);
    let maps = maps_dir(&app);
    if !path.starts_with(&maps) {
        return Err(format!("only files under {} can be revealed", maps.display()));
    }
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| format!("reveal {}: {e}", path.display()))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            reveal_file,
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
