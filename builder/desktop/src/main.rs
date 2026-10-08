//! The OpenBikeComputer desktop app: the same published-cell map builder as the website, in a
//! Tauri shell with native storage and device access.
//!
//! It adds three things the webview cannot do itself: [`catalog`] fetches the published map
//! catalog, [`usb`] drives the device because the webview has no WebUSB, and [`rides`] keeps a
//! durable copy of a ride.
//!
//! The window is granted `core:default` and nothing else: no filesystem, no shell, no HTTP. Every
//! one of those policies is written in Rust, where it can be read, and catalog object reads are
//! restricted to the configured catalog origin.

mod catalog;
mod http;
mod map_output;
mod paths;
mod rides;
mod usb;

use std::sync::Arc;

use tauri::Manager;

/// The published cell catalog, body and base URL.
#[tauri::command]
async fn catalog() -> Result<catalog::FetchedCatalog, String> {
    tauri::async_runtime::spawn_blocking(catalog::fetch).await.map_err(|e| e.to_string())?
}

/// One catalog satellite or cell, restricted to the catalog root's origin.
#[tauri::command]
async fn catalog_get(url: String) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || http::get_catalog_object(&url))
        .await
        .map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn map_output_begin(
    app: tauri::AppHandle,
    outputs: tauri::State<'_, Arc<map_output::Outputs>>,
    name: String,
) -> Result<map_output::Opened, String> {
    outputs.begin(&maps_dir(&app), &name)
}

#[tauri::command]
async fn map_output_write(
    outputs: tauri::State<'_, Arc<map_output::Outputs>>,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| format!("a map output write is missing its `{name}` header"))
    };
    let id: u64 = header("output-id")?.parse().map_err(|_| "a map output write has an invalid id".to_string())?;
    let name = header("filename")?.to_owned();
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("a map output write must carry raw bytes".into());
    };
    let bytes = bytes.clone();
    let outputs = outputs.inner().clone();
    tauri::async_runtime::spawn_blocking(move || outputs.write(id, &name, &bytes)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn map_output_finish(outputs: tauri::State<'_, Arc<map_output::Outputs>>, id: u64) -> Result<(), String> {
    let outputs = outputs.inner().clone();
    tauri::async_runtime::spawn_blocking(move || outputs.finish(id)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn map_output_discard(outputs: tauri::State<'_, Arc<map_output::Outputs>>, id: u64) -> Result<(), String> {
    let outputs = outputs.inner().clone();
    tauri::async_runtime::spawn_blocking(move || outputs.discard(id)).await.map_err(|e| e.to_string())?
}

/// Where built maps go. Shown in the UI, so the user can act on it.
fn maps_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    paths::maps_dir(app.path().document_dir().ok())
}

/// Show a produced file in the platform's file manager.
///
/// Scoped to the folders this app owns: a webview can call this command, and revealing any path is
/// a wider door than the features need. The ride library is listed separately from the maps folder,
/// because a relocated library is not inside it.
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

/// The library, and whether its visible GPX folder is the default one.
///
/// Resolved per call rather than held in state: the folder can move while the app is running, and a
/// cached root would then be wrong exactly when a rider is looking at it. The archive lives in app
/// data and never moves.
fn ride_library(app: &tauri::AppHandle) -> (rides::Library, bool) {
    let default = paths::rides_dir(app.path().document_dir().ok());
    let archive = paths::ride_archive_dir(app.path().app_data_dir().ok());
    let (root, is_default) = match app.path().app_config_dir().ok().and_then(|dir| rides::configured(&dir)) {
        Some(chosen) if chosen != default => (chosen, false),
        _ => (default, true),
    };
    (rides::Library::new(root, archive), is_default)
}

/// The library folder and everything in it.
#[tauri::command]
fn rides_index(app: tauri::AppHandle) -> rides::IndexView {
    let (library, is_default) = ride_library(&app);
    library.view(is_default)
}

/// Land one pulled ride durably, and only then resolve.
///
/// It lands the bytes with `fsync` before it resolves, so a ride the frontend believes it has is a
/// ride on this disk. A ride whose write failed rejects here and is absent from the durable index,
/// so a power cut costs at worst a re-download.
///
/// It runs on the blocking pool, because `fsync` blocks.
#[tauri::command]
async fn rides_import(app: tauri::AppHandle, request: rides::ImportRequest) -> Result<rides::Imported, String> {
    let (library, _) = ride_library(&app);
    tauri::async_runtime::spawn_blocking(move || library.import(&request)).await.map_err(|e| e.to_string())?
}

/// The stored ride object of one key, which a GPX re-export decodes.
#[tauri::command]
async fn rides_read(app: tauri::AppHandle, key: String) -> Result<tauri::ipc::Response, String> {
    let (library, _) = ride_library(&app);
    let bytes =
        tauri::async_runtime::spawn_blocking(move || library.read_object(&key)).await.map_err(|e| e.to_string())??;
    // The raw path, like `usb_read`: a ride object is hundreds of kilobytes and a JSON number
    // array would be four times that in text.
    Ok(tauri::ipc::Response::new(bytes))
}

/// Write one ride's GPX into the visible folder: the repair for a GPX somebody deleted, and the
/// fallback when the file to show is not there yet.
#[tauri::command]
async fn rides_write_gpx(app: tauri::AppHandle, key: String, gpx: String) -> Result<String, String> {
    let (library, _) = ride_library(&app);
    tauri::async_runtime::spawn_blocking(move || library.write_gpx(&key, &gpx)).await.map_err(|e| e.to_string())?
}

/// Let the rider pick a new home for the library, and move it there.
///
/// The native chooser is opened from Rust, because the frontend names a file and never a place, and
/// the OS's own directory picker is the one way a person can name a place without the webview being
/// handed a filesystem. It answers `None` when the chooser was dismissed, which is an ordinary
/// outcome.
#[tauri::command]
async fn rides_choose_folder(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let (library, _) = ride_library(&app);
    let from = library.root().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().set_title("Where should pulled rides be kept?").pick_folder(move |picked| {
        let _ = tx.send(picked);
    });
    // The chooser answers on the UI thread, so parking a blocking-pool thread on the reply keeps
    // the event loop free to run it.
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
        // Registered for its Rust API only, which `rides_choose_folder` calls. No JS permission is
        // granted, so the webview cannot open a picker of its own.
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(usb::UsbState::default()))
        .manage(Arc::new(map_output::Outputs::default()))
        .invoke_handler(tauri::generate_handler![
            catalog,
            catalog_get,
            map_output_begin,
            map_output_write,
            map_output_finish,
            map_output_discard,
            reveal_file,
            // The library is a folder plus an index, and the ack follows `rides_import`.
            rides_index,
            rides_import,
            rides_read,
            rides_write_gpx,
            rides_choose_folder,
            // Bytes only: the protocol lives once, in TypeScript, over these.
            usb::usb_watch,
            usb::usb_list,
            usb::usb_open,
            usb::usb_close,
            usb::usb_read,
            usb::usb_write,
            usb::usb_cancel,
            usb::usb_reset,
        ])
        .run(tauri::generate_context!())
        .expect("run the OpenBikeComputer app");
}
