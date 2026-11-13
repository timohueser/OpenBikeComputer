//! The phone's routing provider and the installer of its offline areas. The host serializes the
//! calls on a handle; only `planner_router_cancel` may run during a call.
use crate::{error_body, respond, Engine};
use planner_router::{
    cells::Directory,
    data::RoutingData,
    install::{self, Object},
    Error, Router,
};
use serde_json::{json, Value};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    fs::File,
    io::{self, Read},
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, PoisonError,
    },
};

/// The workers of an install. Each holds a zstd window of up to 8 MiB and its buffers; the
/// install of an area takes seconds.
const INSTALL_THREADS: usize = 2;

pub struct Handle {
    router: Mutex<Engine>,
    cancel: AtomicBool,
}

fn reply(status: u16, body: Value) -> *mut c_char {
    let mut bytes = format!("{{\"status\":{status},\"body\":").into_bytes();
    bytes.extend_from_slice(&serde_json::to_vec(&body).expect("JSON value serializes"));
    bytes.push(b'}');
    self::body(bytes)
}

fn body(bytes: Vec<u8>) -> *mut c_char {
    CString::new(bytes).expect("Serialized JSON has no NUL bytes").into_raw()
}

/// # Safety
/// `response` is null or an unfreed pointer returned by a planner native response function.
#[no_mangle]
pub unsafe extern "C" fn planner_response_free(response: *mut c_char) {
    if !response.is_null() {
        // SAFETY: The caller transfers ownership of this response back to Rust.
        drop(unsafe { CString::from_raw(response) });
    }
}

/// # Safety
/// `root` is a NUL-terminated UTF-8 string. `error` points to writable pointer storage.
/// The budget is the search limit of one search; zero takes the engine's default.
/// The returned handle must be closed once, after all of its calls finish.
#[no_mangle]
pub unsafe extern "C" fn planner_router_open(
    root: *const c_char,
    memory_budget_bytes: usize,
    error: *mut *mut c_char,
) -> *mut Handle {
    if error.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = ptr::null_mut() };
    let open = || {
        if root.is_null() {
            return Err(Error::InvalidRequest("Missing routing directory".into()));
        }
        // SAFETY: The caller retains the NUL-terminated path for this call.
        let root = unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|_| Error::InvalidRequest("Routing directory must be UTF-8".into()))?;
        let package = planner_router::open(Path::new(root))?;
        let budget = if memory_budget_bytes == 0 { package.default_budget() } else { memory_budget_bytes };
        Ok(Router::new(package, budget))
    };
    let (status, body) = match catch_unwind(open) {
        Ok(Ok(router)) => {
            return Box::into_raw(Box::new(Handle { router: Mutex::new(router), cancel: AtomicBool::new(false) }))
        }
        Ok(Err(failure)) => error_body(failure),
        Err(_) => crate::internal("Opening the routing package panicked"),
    };
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = reply(status, body) };
    ptr::null_mut()
}

/// Answers `call` (`route` or `shape`) with a JSON body, as `POST /v1/{call}` does.
///
/// # Safety
/// `handle` is live with no other call in progress. `call` is a NUL-terminated string.
/// `body` points to `length` readable bytes. `status` points to writable storage.
/// The returned response must be freed with `planner_response_free`.
#[no_mangle]
pub unsafe extern "C" fn planner_router_call(
    handle: *const Handle,
    call: *const c_char,
    body: *const u8,
    length: usize,
    status: *mut u16,
) -> *mut c_char {
    if handle.is_null() || call.is_null() || body.is_null() || status.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The host retains the handle, the call name and the body for this call.
    let (handle, call, body) = unsafe { (&*handle, CStr::from_ptr(call), std::slice::from_raw_parts(body, length)) };
    let mut router = handle.router.lock().unwrap_or_else(PoisonError::into_inner);
    // A cancel that arrives before the call starts belongs to an earlier call.
    handle.cancel.store(false, Ordering::Relaxed);
    let (code, bytes) = respond(&mut router, call.to_str().unwrap_or_default(), body, &handle.cancel);
    // SAFETY: The caller provides writable status storage.
    unsafe { *status = code };
    self::body(bytes)
}

/// Stops the call in progress on `handle`; it answers `cancelled`. Any thread may call this.
///
/// # Safety
/// `handle` is null or live.
#[no_mangle]
pub unsafe extern "C" fn planner_router_cancel(handle: *const Handle) {
    // SAFETY: The caller keeps the handle live for this call.
    if let Some(handle) = unsafe { handle.as_ref() } {
        handle.cancel.store(true, Ordering::Relaxed);
    }
}

/// # Safety
/// `handle` is null or a live handle with no calls in progress.
#[no_mangle]
pub unsafe extern "C" fn planner_router_close(handle: *mut Handle) {
    if !handle.is_null() {
        // SAFETY: The caller transfers ownership after the final call.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// The progress callback of `planner_routing_install`: `(context, done, total)` in object bytes.
/// It returns false to stop the install.
pub type Progress = extern "C" fn(*mut c_void, u64, u64) -> bool;

type Failure = (u16, Value);

fn failure(status: u16, code: &str, message: impl std::fmt::Display) -> Failure {
    (status, json!({ "code": code, "message": message.to_string() }))
}

fn read_error(path: &Path, error: io::Error) -> Failure {
    failure(500, "internal", format!("{}: {error}", path.display()))
}

/// The request of `planner_routing_plan` and `planner_routing_install`.
struct Routing {
    package: Vec<u8>,
    directory: Vec<u8>,
    target: PathBuf,
    areas: Vec<[f64; 4]>,
    objects: Option<PathBuf>,
}

impl Routing {
    /// # Safety
    /// `request` is null or a NUL-terminated string.
    unsafe fn parse(request: *const c_char) -> Result<Self, Failure> {
        let invalid = |message: &str| failure(400, "invalid_request", message);
        if request.is_null() {
            return Err(invalid("Missing routing request"));
        }
        // SAFETY: The caller retains the NUL-terminated request for this call.
        let text = unsafe { CStr::from_ptr(request) }.to_str().map_err(|_| invalid("The request must be UTF-8"))?;
        let value: Value = serde_json::from_str(text).map_err(|_| invalid("The request must be JSON"))?;
        let path = |name: &str| value[name].as_str().map(PathBuf::from);
        let areas = value["areas"].as_array().ok_or_else(|| invalid("The request lists no areas"))?;
        let areas = areas
            .iter()
            .map(|area| {
                let sides: Option<Vec<f64>> = area.as_array()?.iter().map(Value::as_f64).collect();
                sides?.try_into().ok()
            })
            .collect::<Option<Vec<[f64; 4]>>>()
            .ok_or_else(|| invalid("Each area is [west, south, east, north] in degrees"))?;
        let (Some(package), Some(directory), Some(target)) = (path("package"), path("directory"), path("target"))
        else {
            return Err(invalid("The request needs package, directory and target paths"));
        };
        let read = |path: &Path| std::fs::read(path).map_err(|e| read_error(path, e));
        Ok(Routing { package: read(&package)?, directory: read(&directory)?, target, areas, objects: path("objects") })
    }

    /// The download cells of the areas.
    fn cells(&self) -> Result<Vec<u32>, Failure> {
        let directory = Directory::new(&self.directory).map_err(|e| failure(500, "invalid_data", e))?;
        install::area_cells(&directory, &self.areas).map_err(|e| failure(400, "invalid_request", e))
    }

    fn plan(&self, cells: &[u32]) -> Result<install::Plan, Failure> {
        install::plan(&self.directory, &self.package, Some(cells), None).map_err(|e| install_failure(e, false))
    }
}

fn install_failure(error: install::Error, stopped: bool) -> Failure {
    match error {
        _ if stopped => failure(408, "cancelled", "The install was cancelled"),
        install::Error::Locked(_) => failure(503, "busy", error),
        install::Error::Io { .. } => failure(500, "internal", error),
        install::Error::Object { .. } | install::Error::Invalid(_) => failure(500, "invalid_data", error),
    }
}

/// Writes `failure` into `error`.
///
/// # Safety
/// `error` points to writable pointer storage.
unsafe fn fail(error: *mut *mut c_char, (status, body): Failure) {
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = reply(status, body) };
}

/// Plans the install of `areas` into `target`. Answers `{"objects": [{"sha256", "bytes",
/// "part_bytes"}], "bytes"}`: each object that `target` lacks, and the installed bytes of the whole
/// install. The request is the JSON object `{"package", "directory", "target", "areas"}`.
///
/// # Safety
/// `request` is a NUL-terminated string. `error` points to writable pointer storage. The returned
/// response must be freed with `planner_response_free`.
#[no_mangle]
pub unsafe extern "C" fn planner_routing_plan(request: *const c_char, error: *mut *mut c_char) -> *mut c_char {
    if error.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = ptr::null_mut() };
    let plan = || {
        // SAFETY: The caller retains the request for this call.
        let routing = unsafe { Routing::parse(request) }?;
        let plan = routing.plan(&routing.cells()?)?;
        let hex = |hash: &[u8; 32]| hash.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let objects: Vec<Value> = plan
            .missing(&routing.target)
            .files
            .iter()
            .map(|item| json!({ "sha256": hex(&item.object), "bytes": item.object_bytes, "part_bytes": item.bytes }))
            .collect();
        let bytes = install::installed_bytes(&plan) + (routing.package.len() + routing.directory.len()) as u64;
        Ok(json!({ "objects": objects, "bytes": bytes }))
    };
    match catch_unwind(AssertUnwindSafe(plan)) {
        Ok(Ok(answer)) => body(serde_json::to_vec(&answer).expect("JSON value serializes")),
        Ok(Err(failed)) => {
            // SAFETY: The caller supplies writable error pointer storage.
            unsafe { fail(error, failed) };
            ptr::null_mut()
        }
        Err(_) => {
            // SAFETY: The caller supplies writable error pointer storage.
            unsafe { fail(error, failure(500, "internal", "Planning the routing install panicked")) };
            ptr::null_mut()
        }
    }
}

/// The callback and its context, which the workers call one at a time.
struct Callback(Progress, *mut c_void);

// SAFETY: The host passes a context that any thread may use for the callback; the mutex of
// `Counting` serializes the calls.
unsafe impl Send for Callback {}

/// An object file that counts its bytes into the progress of the install.
struct Counting<'a> {
    file: File,
    done: &'a AtomicU64,
    total: u64,
    callback: &'a Option<Mutex<Callback>>,
    stopped: &'a AtomicBool,
}

impl Read for Counting<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.file.read(buf)?;
        let done = self.done.fetch_add(read as u64, Ordering::Relaxed) + read as u64;
        if let Some(callback) = self.callback {
            let callback = callback.lock().unwrap_or_else(PoisonError::into_inner);
            if read > 0 && !(callback.0)(callback.1, done, self.total) {
                self.stopped.store(true, Ordering::Relaxed);
            }
        }
        if self.stopped.load(Ordering::Relaxed) {
            return Err(io::Error::other("cancelled"));
        }
        Ok(read)
    }
}

impl Object for Counting<'_> {
    fn file(&self) -> &File {
        &self.file
    }
}

/// Installs `areas` into `target` from `objects`, a folder of objects named by their SHA-256.
/// With no area, it deletes `target`. The request is that of `planner_routing_plan` with
/// `objects`. `progress`, when given, runs on the install workers, one call at a time, with the
/// object bytes read and those of all missing objects; when it returns false, the install stops
/// with `cancelled`, and a later call goes on.
///
/// # Safety
/// `request` is a NUL-terminated string. `context` is valid for `progress` on any thread until the
/// call returns. `error` points to writable pointer storage.
#[no_mangle]
pub unsafe extern "C" fn planner_routing_install(
    request: *const c_char,
    progress: Option<Progress>,
    context: *mut c_void,
    error: *mut *mut c_char,
) -> bool {
    if error.is_null() {
        return false;
    }
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { *error = ptr::null_mut() };
    let callback = progress.map(|progress| Mutex::new(Callback(progress, context)));
    let (done, stopped) = (AtomicU64::new(0), AtomicBool::new(false));
    let run = || {
        // SAFETY: The caller retains the request for this call.
        let routing = unsafe { Routing::parse(request) }?;
        if routing.areas.is_empty() {
            return install::remove(&routing.target).map_err(|e| install_failure(e, false));
        }
        let objects = routing
            .objects
            .as_deref()
            .ok_or_else(|| failure(400, "invalid_request", "The request names no objects"))?;
        let cells = routing.cells()?;
        let total = install::object_bytes(&routing.plan(&cells)?.missing(&routing.target));
        let fetch = |name: &str| {
            let file = File::open(objects.join(name))?;
            Ok(Counting { file, done: &done, total, callback: &callback, stopped: &stopped })
        };
        let selection = (Some(&cells[..]), None);
        install::install(&routing.target, (&routing.directory, &routing.package), selection, INSTALL_THREADS, fetch)
            .map(drop)
            .map_err(|e| install_failure(e, stopped.load(Ordering::Relaxed)))
    };
    let failed = match catch_unwind(AssertUnwindSafe(run)) {
        Ok(Ok(())) => return true,
        Ok(Err(failed)) => failed,
        Err(_) => failure(500, "internal", "The routing install panicked"),
    };
    // SAFETY: The caller supplies writable error pointer storage.
    unsafe { fail(error, failed) };
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::{Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION};
    use planner_router_build::cells::{write, Info, Input};
    use planner_router_build::source::{Builder, Tags, Ways};
    use planner_router_build::Graph;

    /// Two roads east along the equator, from longitude 0 and 1, each of 0.01 degrees.
    fn two_roads() -> Graph {
        let points: Vec<Point> =
            [0, 10_000, 1_000_000, 1_010_000].map(|lon| Point { lon, lat: 0, elevation: NO_ELEVATION }).into();
        let mut ways = Builder::<Ways>::default();
        let roads: Vec<Road> = [(0, 1), (2, 3)]
            .map(|(from, to)| {
                let way = i64::from(from) + 1;
                ways.push(way, &[i64::from(from), i64::from(to)], &Tags::default());
                Road {
                    from,
                    to,
                    way,
                    length_m: 1112,
                    surface: Surface::Paved,
                    class: 1,
                    access: BIKE | FOOT,
                    shape: vec![points[from as usize], points[to as usize]],
                    ..Road::default()
                }
            })
            .into();
        let mut graph = Graph {
            signals: [0; 4].into_iter().collect(),
            node_doubts: [0; 4].into_iter().collect(),
            node_closures: [u32::MAX; 4].into_iter().collect(),
            node_ids: (0..4).collect(),
            node_access: [BIKE | FOOT; 4].into_iter().collect(),
            points: points.iter().copied().collect(),
            roads: roads.into_iter().collect(),
            ..Default::default()
        };
        graph.osm.ways = ways.finish().0;
        graph.link_ways();
        planner_router_build::layout::order(&mut graph, &[2, 8], None).unwrap();
        graph
    }

    /// Writes the package of `two_roads` with a download cell for each road into `objects`.
    fn publish(objects: &Path) {
        let graph = two_roads();
        let profiles = &Profile::presets()[..1];
        let topology = planner_router_build::base::Topology::new(&graph.roads, graph.points.len()).unwrap();
        let costs = planner_router_build::costs(&graph, &topology, profiles).unwrap();
        let (first, head) = (topology.first.values(), topology.head.values());
        let tables = planner_router_build::tables::build(&graph.cells, first, head, &costs.columns(), 16).unwrap();
        let info = Info {
            region: "test".into(),
            bounds: [0.0, 0.0, 1.01, 0.0],
            source_sha256: vec![],
            attribution: "test".into(),
        };
        let input = Input {
            graph: &graph,
            topology: &topology,
            costs: &costs,
            tables: &tables,
            sizes: &[2, 8],
            download_level: 1,
            info,
            level: 3,
        };
        write(&input, objects).unwrap();
    }

    fn text(response: *mut c_char) -> Value {
        assert!(!response.is_null());
        // SAFETY: Each response of the test is read and freed once.
        let value = serde_json::from_slice(unsafe { CStr::from_ptr(response) }.to_bytes()).unwrap();
        // SAFETY: As above.
        unsafe { planner_response_free(response) };
        value
    }

    fn plan(request: &Value) -> Value {
        let request = CString::new(request.to_string()).unwrap();
        let mut error = ptr::null_mut();
        // SAFETY: The request and the error storage live for the call.
        let answer = unsafe { planner_routing_plan(request.as_ptr(), &mut error) };
        assert!(error.is_null());
        text(answer)
    }

    fn install(request: &Value, progress: Option<Progress>) -> Result<(), Value> {
        let request = CString::new(request.to_string()).unwrap();
        let mut error = ptr::null_mut();
        // SAFETY: The request and the error storage live for the call; the callback ignores its
        // context.
        let done = unsafe { planner_routing_install(request.as_ptr(), progress, ptr::null_mut(), &mut error) };
        if done {
            assert!(error.is_null());
            Ok(())
        } else {
            Err(text(error))
        }
    }

    fn route(target: &Path, lon: f64) -> (u16, Value) {
        let root = CString::new(target.to_str().unwrap()).unwrap();
        let mut error = ptr::null_mut();
        // SAFETY: The path and the error storage live for the call.
        let handle = unsafe { planner_router_open(root.as_ptr(), 0, &mut error) };
        assert!(!handle.is_null() && error.is_null());
        let body = json!({ "points": [[lon + 0.002, 0.0], [lon + 0.008, 0.0]], "profile": "touring" }).to_string();
        let mut status = 0;
        // SAFETY: The handle is live, with one call at a time, and closed once after it.
        let answer = unsafe {
            let answer = planner_router_call(handle, c"route".as_ptr(), body.as_ptr(), body.len(), &mut status);
            planner_router_close(handle);
            answer
        };
        (status, text(answer))
    }

    extern "C" fn stop(_: *mut c_void, _: u64, _: u64) -> bool {
        false
    }

    #[test]
    fn areas_of_one_package_share_an_install_and_the_last_one_takes_it() {
        let scratch = std::env::temp_dir().join(format!("planner-native-areas-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let (objects, target) = (scratch.join("objects"), scratch.join("routing"));
        publish(&objects);
        let (a, b) = ([-0.001, -0.001, 0.011, 0.001], [0.999, -0.001, 1.011, 0.001]);
        let request = |areas: &[[f64; 4]]| {
            json!({
                "package": objects.join("package.json"),
                "directory": objects.join("directory.bin"),
                "target": target,
                "areas": areas,
                "objects": objects,
            })
        };
        assert!(!plan(&request(&[a]))["objects"].as_array().unwrap().is_empty());
        install(&request(&[a]), None).unwrap();
        assert!(plan(&request(&[a]))["objects"].as_array().unwrap().is_empty());
        assert!(!plan(&request(&[a, b]))["objects"].as_array().unwrap().is_empty());

        let stopped = install(&request(&[a, b]), Some(stop)).unwrap_err();
        assert_eq!((stopped["status"].as_u64(), stopped["body"]["code"].as_str()), (Some(408), Some("cancelled")));
        install(&request(&[a, b]), None).unwrap();
        assert!(plan(&request(&[a, b]))["objects"].as_array().unwrap().is_empty());
        let directory = std::fs::read(objects.join("directory.bin")).unwrap();
        let (status, answer) = route(&target, 0.0);
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["routes"][0]["package"], planner_router::package::digest(&directory));
        assert_eq!(route(&target, 1.0).0, 200);

        install(&request(&[a]), None).unwrap();
        let (status, answer) = route(&target, 1.0);
        assert_eq!((status, answer["code"].as_str()), (422, Some("missing_region")));
        let only_b = install::area_cells(&Directory::new(&directory).unwrap(), &[b]).unwrap();
        let names: Vec<String> = std::fs::read_dir(target.join(install::CELLS))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(!names.is_empty());
        assert!(names.iter().all(|name| only_b.iter().all(|id| !name.starts_with(&format!("1-{id:08x}-")))));

        install(&request(&[]), None).unwrap();
        assert!(!target.exists());
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
