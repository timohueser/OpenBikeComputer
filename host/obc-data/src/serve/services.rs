//! The services of a planner release: routing, search and downloads, as processes on this machine
//! that read the files of the release.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::{Fetched, Served};
use crate::release::{is_sha256, File as Named};
use crate::store::{hash_file, sha256_hex, Store};
use crate::target::Target;

pub struct Service {
    pub name: &'static str,
    /// The path of the service that the API path of a release gives after its name.
    pub path: &'static str,
    /// A GET that answers 200 once the service serves.
    ready: &'static str,
}

/// The services of each planner release, in the order of their ports.
pub const SERVICES: [Service; 3] = [
    Service { name: "routing", path: "", ready: "/health" },
    Service { name: "search", path: "/api/planner-search", ready: "/api/planner-search/status" },
    Service { name: "downloads", path: "", ready: "/catalog" },
];

/// The folders of a planner release that its services read.
const RUNTIME: [&str; 3] = ["routing", "search", "offline"];

/// The largest size of the prepared downloads of one planner release.
const DOWNLOADS_CACHE: u64 = 256 << 20;

/// The repository whose code runs the search and downloads services, and the routing service.
pub struct Programs {
    root: PathBuf,
    routing: PathBuf,
    /// What each service reads: the service, a layer and its format.
    reads: Vec<(&'static str, String, u32)>,
}

/// Where the clients of a planner release reach its API and the objects of its target.
pub struct Origins {
    pub api: String,
    /// The URL of `objects/`.
    pub objects: String,
}

impl Programs {
    /// The programs, with the formats that each service prints with `--formats`: a JSON object of
    /// layer and format.
    pub fn new(root: PathBuf, routing: PathBuf) -> Result<Self, String> {
        let mut all = Vec::new();
        for (mut command, service) in commands(&root, &routing).into_iter().zip(&SERVICES) {
            let fail = |e: String| format!("the {} service does not give its formats: {e}", service.name);
            let output = command.arg("--formats").output().map_err(|e| fail(e.to_string()))?;
            if !output.status.success() {
                return Err(fail(String::from_utf8_lossy(&output.stderr).trim().to_string()));
            }
            let reads: BTreeMap<String, u32> =
                serde_json::from_slice(&output.stdout).map_err(|e| fail(e.to_string()))?;
            all.extend(reads.into_iter().map(|(layer, format)| (service.name, layer, format)));
        }
        Ok(Self { root, routing, reads: all })
    }

    /// The format of each layer that the services read.
    pub fn formats(&self) -> BTreeMap<String, u32> {
        self.reads.iter().map(|(_, layer, format)| (layer.clone(), *format)).collect()
    }

    /// Programs that read these formats, for a test that starts no service.
    #[cfg(test)]
    pub(crate) fn reading(reads: Vec<(&'static str, String, u32)>) -> Self {
        Self { root: PathBuf::new(), routing: PathBuf::new(), reads }
    }
}

/// The command of each of the [`SERVICES`], before the arguments of a start.
fn commands(root: &Path, routing: &Path) -> [Command; 3] {
    let (mut search, mut downloads) = (Command::new("node"), Command::new("python3"));
    search.arg("server.mjs").current_dir(root.join("planner/search"));
    downloads.args(["-m", "tools.planner_downloads"]).current_dir(root);
    [Command::new(routing), search, downloads]
}

/// Refuse a release with a layer that its service reads in another format.
fn readable(programs: &Programs, served: &Served) -> Result<(), String> {
    for (service, layer, reads) in &programs.reads {
        if let Some(format) = served.formats.get(layer).filter(|format| *format != reads) {
            return Err(format!("needs a {service} service that reads {layer} format {format}; it reads {reads}"));
        }
    }
    Ok(())
}

/// One service of a planner release: a process on this machine. It stops when it drops.
pub struct Process {
    child: Child,
    port: u16,
    /// Its place in [`SERVICES`].
    service: usize,
    /// When it was asked to stop.
    signalled: Option<Instant>,
}

/// How many objects download at once. It hides the request time of objects of a few MB; 8
/// requests fill one link.
const PARALLEL: usize = 8;

/// The file that an install writes last. It holds the planner release id.
const MARKER: &str = "installed";

/// The free bytes of a file system for this user, and its size.
pub struct Disk {
    pub free: u64,
    pub size: u64,
}

/// The [`Disk`] that holds `path`.
#[cfg(unix)]
// The fields of `statvfs` are narrower than 64 bits on some systems.
#[allow(clippy::useless_conversion)]
pub fn disk(path: &Path) -> Result<Disk, String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: a valid C string, and statvfs fills `stat` when it returns 0.
    if unsafe { libc::statvfs(c.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(format!("{}: {}", path.display(), std::io::Error::last_os_error()));
    }
    // SAFETY: statvfs returned 0.
    let stat = unsafe { stat.assume_init() };
    let block = u64::from(stat.f_frsize);
    Ok(Disk { free: u64::from(stat.f_bavail) * block, size: u64::from(stat.f_blocks) * block })
}

#[cfg(not(unix))]
pub fn disk(_: &Path) -> Result<Disk, String> {
    Ok(Disk { free: u64::MAX, size: 0 })
}

/// Check that the services read the layers of `served` in their formats, then install its files in
/// `dir`, `<store>/serve/<target>/<planner id>/`. An install with a marker stays as it is. Else the
/// folder loses its content, `store` gets the document of the planner release and its step indexes
/// from `target`, the disk check compares the bytes that the install needs with `disk` of `store`,
/// and the missing objects download before [`install`]. It stops between two files when `wanted`
/// is false or serve stops.
pub fn fetch(
    programs: &Programs,
    store: &Store,
    target: &Target,
    served: &Served,
    dir: &Path,
    wanted: &AtomicBool,
    disk: &dyn Fn(&Path) -> Result<Disk, String>,
) -> Fetched {
    let go = || match () {
        _ if !wanted.load(Ordering::Relaxed) => Err("no release of the target names it any more".to_string()),
        _ if crate::fetch::http::stopped() => Err("serve stops".to_string()),
        _ => Ok(()),
    };
    let fetched = || {
        readable(programs, served)?;
        if installed(dir) {
            return Ok(Fetched::Installed);
        }
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let document = &served.document;
        download(store, target, vec![(document.key.clone(), document.sha256.clone())], &go)?;
        let document = read(store, document)?;
        let indexes = indexes(&document, runs)?.into_iter();
        download(
            store,
            target,
            indexes.map(|index| (format!("objects/{}", index.sha256), index.sha256)).collect(),
            &go,
        )?;
        let runtime = runtime(store, &document)?;
        let missing: BTreeMap<&str, u64> = (runtime.iter())
            .filter(|(_, entry)| !store.object(&entry.transport.sha256).is_file())
            .map(|(_, entry)| (entry.transport.sha256.as_str(), entry.transport.bytes))
            .collect();
        let decoded = runtime.iter().filter(|(_, entry)| entry.transport.encoding != "identity");
        let Disk { free, size } = disk(store.root())?;
        // The reserve keeps room for the logs, the downloads cache and the file system itself.
        let need = missing.values().sum::<u64>() + decoded.map(|(_, entry)| entry.bytes).sum::<u64>() + size / 20;
        if need > free {
            return Ok(Fetched::NoRoom { need, free });
        }
        let missing = missing.keys().map(|sha256| (format!("objects/{sha256}"), sha256.to_string())).collect();
        download(store, target, missing, &go)?;
        install(store, dir, served, &go)?;
        Ok(Fetched::Installed)
    };
    fetched().unwrap_or_else(Fetched::Failed)
}

/// Give `store` each of `objects`, a key on `target` and its object, that it lacks, with
/// [`PARALLEL`] downloads at once. The first error stops the others at their next object.
fn download(
    store: &Store,
    target: &Target,
    objects: Vec<(String, String)>,
    go: &(dyn Fn() -> Result<(), String> + Sync),
) -> Result<(), String> {
    let missing: Vec<(String, String)> =
        objects.into_iter().filter(|(_, sha256)| !store.object(sha256).is_file()).collect();
    let (next, failed) = (AtomicUsize::new(0), Mutex::new(None));
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL.min(missing.len()) {
            scope.spawn(|| {
                while failed.lock().unwrap().is_none() {
                    let Some((key, sha256)) = missing.get(next.fetch_add(1, Ordering::Relaxed)) else { return };
                    let downloaded = go().and_then(|()| {
                        target.download(key, sha256, store)?.map(drop).ok_or_else(|| format!("the target has no {key}"))
                    });
                    if let Err(e) = downloaded {
                        failed.lock().unwrap().get_or_insert(e);
                    }
                }
            });
        }
    });
    failed.into_inner().unwrap().map_or(Ok(()), Err)
}

/// The key on the target and the object of each file that the services of `served` read: its
/// document, once `store` has it the step indexes that it names for [`RUNTIME`], and once `store`
/// has those each file under [`RUNTIME`].
fn objects(store: &Store, served: &Served) -> Result<Vec<(String, String)>, String> {
    let document = &served.document;
    let mut objects = vec![(document.key.clone(), document.sha256.clone())];
    if !store.object(&document.sha256).is_file() {
        return Ok(objects);
    }
    let document = read(store, document)?;
    let indexes = indexes(&document, runs)?;
    let complete = indexes.iter().all(|index| store.object(&index.sha256).is_file());
    objects.extend(indexes.into_iter().map(|index| (format!("objects/{}", index.sha256), index.sha256)));
    if complete {
        let runtime = runtime(store, &document)?.into_iter().map(|(_, entry)| entry.transport.sha256);
        objects.extend(runtime.map(|sha256| (format!("objects/{sha256}"), sha256)));
    }
    Ok(objects)
}

/// Delete from `store`, the store of a follower of `target`, each object and each unfinished
/// download that the services of none of `wanted` read. A `gzip` object of a release with an
/// install in `work` goes too: its install holds the decoded file. An `identity` object is a link
/// that takes no space, and the next release can need it.
pub fn prune(store: &Store, target: &Target, work: &Path, wanted: &[&Served]) -> Result<(), String> {
    let mut keep = Vec::new();
    for served in wanted {
        let mut objects = objects(store, served)?;
        if installed(&work.join(&served.planner)) {
            let runtime = runtime(store, &read(store, &served.document)?)?;
            let decoded: Vec<String> = (runtime.into_iter())
                .filter(|(_, entry)| entry.transport.encoding != "identity")
                .map(|(_, entry)| entry.transport.sha256)
                .collect();
            objects.retain(|(_, sha256)| !decoded.contains(sha256));
        }
        keep.extend(objects);
    }
    // A download names its file in `partial/` by the URL of an R2 target, or by the object.
    let part = |(key, sha256): &(String, String)| match target {
        Target::R2(remote) => crate::fetch::http::key(&remote.url(key)),
        Target::Dir(_) => sha256.clone(),
    };
    let parts: Vec<String> = keep.iter().map(part).collect();
    crate::store::gc::keep_only(store, &keep.into_iter().map(|(_, sha256)| sha256).collect(), &parts)
}

/// Delete each folder in `work` whose name `keep` does not take.
pub fn uninstall(work: &Path, keep: impl Fn(&str) -> bool) -> Result<(), String> {
    let Ok(folders) = fs::read_dir(work) else { return Ok(()) };
    for folder in folders {
        let path = folder.map_err(|e| format!("{}: {e}", work.display()))?.path();
        if !path.file_name().and_then(|name| name.to_str()).is_some_and(&keep) {
            fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Delete the folder `dir`; whether it was there.
pub fn remove(dir: &Path) -> Result<bool, String> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// Start the service `service` of [`SERVICES`] of the planner release `served`, installed in `dir`,
/// on a free port of this machine.
pub fn start(
    programs: &Programs,
    dir: &Path,
    served: &Served,
    origins: &Origins,
    service: usize,
) -> Result<Process, String> {
    let port = free_port()?;
    let mut command = commands(&programs.root, &programs.routing).into_iter().nth(service).expect("a service");
    match SERVICES[service].name {
        "routing" => {
            command.arg(dir.join("routing")).env("ROUTE_LISTEN", format!("127.0.0.1:{port}")).env_remove("ROUTE_ORIGIN")
        }
        "search" => {
            let document = dir.join("release.json");
            let read = fs::read(&document).map_err(|e| format!("{}: {e}", document.display()))?;
            let document: Value = serde_json::from_slice(&read).map_err(|e| format!("{}: {e}", document.display()))?;
            let region = document["region"].as_str().ok_or("the planner release has no region")?;
            command
                .env("OBC_SEARCH_DATA", dir.join("search"))
                .env("OBC_SEARCH_PORT", port.to_string())
                .env("OBC_SEARCH_REGIONS", region)
                .env_remove("OBC_SEARCH_ORIGINS")
        }
        _ => command
            .arg("--source")
            .arg(dir.join("offline"))
            .arg("--cache")
            .arg(dir.join("downloads"))
            .args(["--max-cache-bytes", &DOWNLOADS_CACHE.to_string(), "--port", &port.to_string()])
            .args(["--public-url", &format!("{}/planner-api/releases/{}/downloads", origins.api, served.planner)])
            .args(["--objects-url", &origins.objects]),
    };
    let name = SERVICES[service].name;
    let child = spawn(&mut command).map_err(|e| format!("cannot start the {name} service: {e}"))?;
    Ok(Process { child, port, service, signalled: None })
}

/// Install the files of the planner release of `served` in `dir` from the objects that [`fetch`]
/// gave `store`: each file, then the document as `release.json`, then the marker, after the data of
/// the file system is on the disk. It stops between two files when `go` fails.
fn install(store: &Store, dir: &Path, served: &Served, go: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", dir.display());
    fs::create_dir_all(dir).map_err(fail)?;
    materialize(store, &served.document, dir, go)?;
    crate::target::link(&store.object(&served.document.sha256), &dir.join("release.json")).map_err(fail)?;
    let folder = File::open(dir).map_err(fail)?;
    sync(&folder).map_err(fail)?;
    let part = crate::target::part(&dir.join(MARKER));
    let mut marker = File::options().write(true).create_new(true).open(&part).map_err(fail)?;
    std::io::Write::write_all(&mut marker, served.planner.as_bytes()).map_err(fail)?;
    marker.sync_all().map_err(fail)?;
    fs::rename(&part, dir.join(MARKER)).map_err(fail)?;
    folder.sync_all().map_err(fail)
}

/// Write the data of the file system of `folder` to the disk.
fn sync(folder: &File) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: an open descriptor.
        if unsafe { libc::syncfs(folder.as_raw_fd()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let _ = folder;
        // SAFETY: no arguments.
        unsafe { libc::sync() };
    }
    #[cfg(not(unix))]
    let _ = folder;
    Ok(())
}

/// Whether `dir` holds a complete install: a marker that names the folder.
pub fn installed(dir: &Path) -> bool {
    let name = dir.file_name().and_then(|name| name.to_str());
    fs::read_to_string(dir.join(MARKER)).is_ok_and(|marker| Some(marker.as_str()) == name)
}

/// Start `command` in a process group of its own, which [`stop`] stops. It writes its log to
/// standard error, as `obc data` does.
pub fn spawn(command: &mut Command) -> std::io::Result<Child> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    // A child stops when `obc data serve` ends without stopping it, such as on SIGKILL. The signal
    // comes when the thread that spawned the child ends, and the threads that spawn live until then.
    #[cfg(target_os = "linux")]
    // SAFETY: prctl is async-signal-safe, and the closure allocates nothing.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, || {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    command.stdout(std::io::stderr()).spawn()
}

impl super::Process for Process {
    fn ready(&mut self) -> Result<bool, String> {
        if let Some(stopped) = self.stopped() {
            return Err(stopped);
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .into();
        let service = &SERVICES[self.service];
        let Ok(mut response) = agent.get(format!("http://127.0.0.1:{}{}", self.port, service.ready)).call() else {
            return Ok(false);
        };
        let body = response.body_mut().read_to_string().unwrap_or_default();
        // Search answers before its query model loads.
        let model = serde_json::from_str::<Value>(&body).is_ok_and(|status| status["parser"]["ready"] == true);
        Ok(response.status() == 200 && (service.name != "search" || model))
    }

    fn port(&self) -> u16 {
        self.port
    }

    fn stopped(&mut self) -> Option<String> {
        let name = SERVICES[self.service].name;
        match self.child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(format!("the {name} service stopped: {status}")),
            Err(e) => Some(format!("the {name} service: {e}")),
        }
    }

    fn signal(&mut self) {
        if self.signalled.is_none() && running(&mut self.child) {
            signal(&mut self.child, false);
        }
        self.signalled.get_or_insert_with(Instant::now);
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        super::Process::signal(self);
        end(&mut self.child, self.signalled.unwrap_or_else(Instant::now));
    }
}

/// Ask the process group of each child to stop, and kill those that still run after five seconds.
pub fn stop(children: &mut [Child]) {
    for child in children.iter_mut() {
        if running(child) {
            signal(child, false);
        }
    }
    let signalled = Instant::now();
    for child in children {
        end(child, signalled);
    }
}

/// A child that was waited for may have a new process at its id: only a running child gets a signal.
fn running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

/// Wait for `child`, asked to stop at `signalled`, and kill it when it still runs five seconds later.
fn end(child: &mut Child, signalled: Instant) {
    let deadline = signalled + Duration::from_secs(5);
    while Instant::now() < deadline && running(child) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if running(child) {
        signal(child, true);
    }
    let _ = child.wait();
}

#[cfg(unix)]
fn signal(child: &mut Child, kill: bool) {
    // SAFETY: the group of a running child that leads it.
    unsafe { libc::kill(-(child.id() as libc::pid_t), if kill { libc::SIGKILL } else { libc::SIGTERM }) };
}

#[cfg(not(unix))]
fn signal(child: &mut Child, _kill: bool) {
    let _ = child.kill();
}

/// A port of this machine that is free now.
pub fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no free port: {e}"))?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

#[derive(Deserialize)]
struct Entry {
    bytes: u64,
    sha256: String,
    transport: Transport,
}

#[derive(Deserialize)]
struct Transport {
    bytes: u64,
    sha256: String,
    encoding: String,
}

/// The document of a planner release, from `store`.
fn read(store: &Store, document: &Named) -> Result<Value, String> {
    if !is_sha256(&document.sha256) {
        return Err(format!("{} names no object", document.key));
    }
    let path = store.object(&document.sha256);
    let read = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&read).map_err(|e| format!("{}: {e}", document.key))
}

/// Whether the services read the files of the folder `folder` of a planner release.
fn runs(folder: &str) -> bool {
    RUNTIME.contains(&folder)
}

/// A step index that the document of a planner release of format 2 names in `indexes`.
pub struct Index {
    pub step: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The step indexes of the planner release `document` that list a file of a folder that `wanted`
/// takes. A document of format 1 names none: it lists each file in `files`.
pub fn indexes(document: &Value, wanted: impl Fn(&str) -> bool) -> Result<Vec<Index>, String> {
    #[derive(Deserialize)]
    struct Named {
        bytes: u64,
        sha256: String,
        folders: Vec<String>,
    }
    match document["format"].as_u64() {
        Some(1) => Ok(Vec::new()),
        Some(2) => {
            let named: BTreeMap<String, Named> =
                serde_json::from_value(document["indexes"].clone()).map_err(|e| format!("indexes: {e}"))?;
            let named = named.into_iter().filter(|(_, index)| index.folders.iter().any(|folder| wanted(folder)));
            named
                .map(|(step, Named { bytes, sha256, .. })| match is_sha256(&sha256) {
                    true => Ok(Index { step, bytes, sha256 }),
                    false => Err(format!("the index of {step} names no object")),
                })
                .collect()
        }
        _ => Err(format!("the planner release has the format {}, not 1 or 2", document["format"])),
    }
}

/// The entry of each file of the planner release `document` whose first folder `wanted` takes:
/// from the step [`indexes`], which `read` gives, and from `files`. Each index must have the size
/// and the SHA-256 that the document gives.
pub fn files(
    document: &Value,
    wanted: impl Fn(&str) -> bool,
    read: impl Fn(&Index) -> Result<Vec<u8>, String>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut files = BTreeMap::new();
    let mut add = |listed: &Value, of: &str| {
        let listed = listed.as_object().ok_or_else(|| format!("{of} has no files"))?;
        let taken = listed.iter().filter(|(name, _)| wanted(name.split('/').next().unwrap_or_default()));
        files.extend(taken.map(|(name, entry)| (name.clone(), entry.clone())));
        Ok::<_, String>(())
    };
    for index in indexes(document, &wanted)? {
        let bytes = read(&index)?;
        if bytes.len() as u64 != index.bytes || sha256_hex(&bytes) != index.sha256 {
            return Err(format!("the index of {} is not the one of the planner release", index.step));
        }
        let parsed: Value = serde_json::from_slice(&bytes).map_err(|e| format!("the index of {}: {e}", index.step))?;
        add(&parsed["files"], &format!("the index of {}", index.step))?;
    }
    add(&document["files"], "the planner release")?;
    Ok(files)
}

/// Each file under [`RUNTIME`] of the planner release whose document is `document`, with its step
/// indexes from `store`.
fn runtime(store: &Store, document: &Value) -> Result<Vec<(String, Entry)>, String> {
    let read = |index: &Index| {
        let path = store.object(&index.sha256);
        fs::read(&path).map_err(|e| format!("the index of {}: {}: {e}", index.step, path.display()))
    };
    files(document, runs, read)?
        .into_iter()
        .map(|(name, entry)| {
            crate::r2::check_key(&name)?;
            let entry: Entry = serde_json::from_value(entry).map_err(|e| format!("{name}: {e}"))?;
            match is_sha256(&entry.transport.sha256) {
                true => Ok((name, entry)),
                false => Err(format!("{name} names no object")),
            }
        })
        .collect()
}

/// Install each file under [`RUNTIME`] of the planner release whose document is `document` in
/// `dir`, from the objects of `store`, while `go` passes. A gzip object is decoded; each file must
/// have the SHA-256 of the release.
fn materialize(store: &Store, document: &Named, dir: &Path, go: &dyn Fn() -> Result<(), String>) -> Result<(), String> {
    let document = read(store, document)?;
    for (name, entry) in runtime(store, &document)? {
        go()?;
        let (object, path) = (store.object(&entry.transport.sha256), dir.join(&name));
        fs::create_dir_all(path.parent().expect("a file has a folder")).map_err(|e| e.to_string())?;
        let written = match entry.transport.encoding.as_str() {
            "identity" => crate::target::link(&object, &path),
            "gzip" => decode(&object, &path, entry.bytes),
            other => return Err(format!("{name} has the encoding `{other}`")),
        };
        written.map_err(|e| format!("{name} from {}: {e}", object.display()))?;
        if hash_file(&path)? != (entry.sha256.clone(), entry.bytes) {
            return Err(format!("{name} from {} is not the file of the planner release", object.display()));
        }
    }
    Ok(())
}

/// Decode the gzip object `object` to a new file under a [`crate::target::part`] name, then
/// rename it to `path`: nothing opens an existing file for writing.
fn decode(object: &Path, path: &Path, bytes: u64) -> std::io::Result<()> {
    let part = crate::target::part(path);
    let written = File::open(object).and_then(|object| {
        let mut file = File::options().write(true).create_new(true).open(&part)?;
        // A larger output is not the file of the release.
        std::io::copy(&mut std::io::Read::take(flate2::read::GzDecoder::new(object), bytes + 1), &mut file).map(drop)
    });
    let renamed = written.and_then(|()| fs::rename(&part, path));
    if renamed.is_err() {
        let _ = fs::remove_file(&part);
    }
    renamed
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;

    use super::*;
    use crate::store::sha256_hex;
    use crate::store::tests::Scratch;

    #[test]
    fn a_release_in_a_format_that_a_service_does_not_read_starts_nothing() {
        let scratch = Scratch::new("serve-formats");
        let reads = vec![
            ("routing", "planner/routing".into(), 8),
            ("search", "planner/index".into(), 3),
            ("downloads", "planner/index".into(), 3),
        ];
        let programs = Programs::reading(reads);
        let served = |routing, index| Served {
            id: "b".repeat(64),
            planner: "a".repeat(64),
            document: Named {
                key: format!("releases/{}/release.json", "a".repeat(64)),
                size: 0,
                sha256: "c".repeat(64),
            },
            pointers: BTreeMap::new(),
            formats: [("planner/routing".into(), routing), ("planner/index".into(), index)].into(),
        };
        let store = Store::at(scratch.0.join("store"));
        let target = Target::Dir(scratch.0.join("target"));
        let dir = scratch.0.join("work").join("a".repeat(64));
        let Fetched::Failed(refused) =
            fetch(&programs, &store, &target, &served(9, 3), &dir, &AtomicBool::new(true), &|_| unreachable!())
        else {
            panic!("the release starts")
        };
        assert_eq!(refused, "needs a routing service that reads planner/routing format 9; it reads 8");
        assert!(!store.root().exists(), "nothing downloads");
        let index = readable(&programs, &served(8, 4)).unwrap_err();
        assert_eq!(index, "needs a search service that reads planner/index format 4; it reads 3");
        assert!(readable(&programs, &served(8, 3)).is_ok());
    }

    /// The planner catalog reads the previous release of a target with `files` too: before its
    /// first update after format 2, that release has format 1.
    #[test]
    fn the_files_of_a_planner_release_of_format_1_are_its_files() {
        let entry = json!({"bytes": 1});
        let document = json!({"format": 1, "files": {"maps/basemap.json": entry, "routing/blocks.json": entry}});
        let unread = |index: &Index| -> Result<Vec<u8>, String> { panic!("{} is read", index.step) };
        let all = files(&document, |_| true, unread).unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["maps/basemap.json", "routing/blocks.json"]);
        assert_eq!(all["maps/basemap.json"], entry);
        let refused = files(&json!({"format": 3, "files": {}}), |_| true, unread).unwrap_err();
        assert_eq!(refused, "the planner release has the format 3, not 1 or 2");
    }

    #[test]
    fn the_files_of_a_planner_release_come_from_its_objects() {
        let scratch = Scratch::new("serve-materialize");
        let target = scratch.0.join("target");
        let object = |bytes: &[u8]| {
            let sha256 = sha256_hex(bytes);
            fs::create_dir_all(target.join("objects")).unwrap();
            fs::write(target.join("objects").join(&sha256), bytes).unwrap();
            sha256
        };
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"places").unwrap();
        let (pages, places) = (object(b"pages"), object(&encoder.finish().unwrap()));
        let entry = |bytes: &[u8], transport: &str, encoding: &str| {
            let (sha256, size) =
                (sha256_hex(bytes), fs::metadata(target.join("objects").join(transport)).map_or(0, |m| m.len()));
            json!({"bytes": bytes.len(), "sha256": sha256, "transport": {"bytes": size, "sha256": transport, "encoding": encoding}})
        };
        let mut document = json!({"format": 1, "region": "test", "files": {
            "routing/pages.bin": entry(b"pages", &pages, "identity"),
            "search/tiles/pois/9-1-1.sqlite": entry(b"places", &places, "gzip"),
            "maps/basemap.json": entry(b"basemap", &"0".repeat(64), "identity"),
        }});
        let release = |document: &Value| {
            let bytes = serde_json::to_vec(document).unwrap();
            let key = format!("releases/{}/release.json", sha256_hex(&bytes));
            fs::create_dir_all(target.join(&key).parent().unwrap()).unwrap();
            fs::write(target.join(&key), &bytes).unwrap();
            Named { key, size: bytes.len() as u64, sha256: sha256_hex(&bytes) }
        };
        let (store, dir_target) = (Store::at(scratch.0.join("store")), Target::Dir(target.clone()));
        let served = |document: &Named| Served {
            id: String::new(),
            planner: String::new(),
            document: document.clone(),
            pointers: BTreeMap::new(),
            formats: BTreeMap::new(),
        };
        let disk = |_: &Path| Ok(Disk { free: u64::MAX, size: 0 });
        let fetched = |document: &Named, dir: &Path, wanted: bool| {
            let programs = Programs::reading(Vec::new());
            match fetch(&programs, &store, &dir_target, &served(document), dir, &AtomicBool::new(wanted), &disk) {
                Fetched::Installed => Ok(()),
                Fetched::Failed(e) => Err(e),
                Fetched::NoRoom { .. } => panic!("the disk has room"),
            }
        };
        let materialize = |document: &Named, dir: &Path| fetched(document, dir, true);
        let named = release(&document);
        let unwanted = fetched(&named, &scratch.0.join("unwanted"), false);
        assert_eq!(unwanted.unwrap_err(), "no release of the target names it any more");
        assert!(!store.object(&named.sha256).exists(), "a download that no release wants stops at its next object");
        let dir = scratch.0.join("work");
        materialize(&named, &dir).unwrap();
        assert_eq!(fs::read(dir.join("routing/pages.bin")).unwrap(), b"pages");
        assert_eq!(fs::read(dir.join("search/tiles/pois/9-1-1.sqlite")).unwrap(), b"places");
        assert!(!dir.join("maps").exists(), "no service reads the maps");

        // Format 2 names the step indexes. The follower reads only those of the folders of its services:
        // the target has no index of the maps.
        let index = serde_json::to_vec(&json!({"files": {"routing/pages.bin": entry(b"pages", &pages, "identity")}}));
        let index = index.unwrap();
        let routing = object(&index);
        let step = |bytes: usize, sha256: &str, folders: &[&str]| json!({"bytes": bytes, "sha256": sha256, "folders": folders});
        let mut indexed = json!({"format": 2, "region": "test", "indexes": {
            "planner/routing/grid": step(index.len(), &routing, &["routing"]),
            "planner/maps/grid": step(1, &"d".repeat(64), &["maps"])},
            "files": {"search/tiles/pois/9-1-1.sqlite": entry(b"places", &places, "gzip")}});
        let (indexed_release, dir) = (release(&indexed), scratch.0.join("indexed"));
        materialize(&indexed_release, &dir).unwrap();
        assert_eq!(fs::read(dir.join("routing/pages.bin")).unwrap(), b"pages");
        assert_eq!(fs::read(dir.join("search/tiles/pois/9-1-1.sqlite")).unwrap(), b"places");
        let kept = objects(&store, &served(&indexed_release)).unwrap();
        assert!(kept.contains(&(format!("objects/{routing}"), routing.clone())), "prune keeps the index");
        indexed["indexes"]["planner/routing/grid"]["bytes"] = json!(index.len() + 1);
        let refused = materialize(&release(&indexed), &scratch.0.join("changed")).unwrap_err();
        assert!(refused.contains("not the one of the planner release"), "{refused}");

        // An existing file is never opened for writing: it can be a link to an object of the store.
        let (object, again) = (scratch.0.join("object"), scratch.0.join("again"));
        fs::write(&object, b"store object").unwrap();
        for name in ["routing/pages.bin", "search/tiles/pois/9-1-1.sqlite"] {
            fs::create_dir_all(again.join(name).parent().unwrap()).unwrap();
            fs::hard_link(&object, again.join(name)).unwrap();
        }
        assert!(super::materialize(&store, &named, &again, &|| Ok(())).unwrap_err().contains("exists"));
        fs::remove_file(again.join("routing/pages.bin")).unwrap();
        super::materialize(&store, &named, &again, &|| Ok(())).unwrap();
        assert_eq!(fs::read(again.join("search/tiles/pois/9-1-1.sqlite")).unwrap(), b"places");
        assert_eq!(fs::read(&object).unwrap(), b"store object");

        document["files"]["routing/pages.bin"]["sha256"] = json!("b".repeat(64));
        let refused = materialize(&release(&document), &scratch.0.join("other")).unwrap_err();
        assert!(refused.contains("not the file of the planner release"), "{refused}");
        document["files"]["routing/pages.bin"]["sha256"] = json!(sha256_hex(b"pages"));
        document["files"]["routing/../x"] = document["files"]["routing/pages.bin"].clone();
        let refused = materialize(&release(&document), &scratch.0.join("third")).unwrap_err();
        assert!(refused.contains("does not name"), "{refused}");
    }
}
