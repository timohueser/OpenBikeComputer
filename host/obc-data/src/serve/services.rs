//! The services of a planner release: routing, search and downloads, as processes on this machine
//! that read the files of the release.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::Served;
use crate::release::{is_sha256, File as Named};
use crate::store::{hash_file, Store};
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
const RUNTIME: [&str; 3] = ["routing/", "search/", "offline/"];

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

/// The services of one planner release, and the folder of its files.
pub struct Processes {
    dir: PathBuf,
    children: Vec<Child>,
    ports: [u16; SERVICES.len()],
}

/// Install the files of the planner release of `served` (see [`install`]), and start its services
/// on free ports of this machine.
pub fn start(
    programs: &Programs,
    store: &Store,
    target: &Target,
    work: &Path,
    served: &Served,
    origins: &Origins,
) -> Result<Processes, String> {
    let (mut processes, region) = install(programs, store, target, work, served)?;
    let (dir, planner) = (processes.dir.clone(), &served.planner);
    processes.ports = [free_port()?, free_port()?, free_port()?];
    let [routing, search, downloads] = processes.ports.map(|port| port.to_string());
    let mut commands = commands(&programs.root, &programs.routing);
    commands[0].arg(dir.join("routing")).env("ROUTE_LISTEN", format!("127.0.0.1:{routing}")).env_remove("ROUTE_ORIGIN");
    commands[1]
        .env("OBC_SEARCH_DATA", dir.join("search"))
        .env("OBC_SEARCH_PORT", search)
        .env("OBC_SEARCH_REGIONS", region)
        .env_remove("OBC_SEARCH_ORIGINS");
    commands[2]
        .arg("--source")
        .arg(dir.join("offline"))
        .arg("--cache")
        .arg(dir.join("downloads"))
        .args(["--max-cache-bytes", &DOWNLOADS_CACHE.to_string(), "--port", &downloads])
        .args(["--public-url", &format!("{}/planner-api/releases/{planner}/downloads", origins.api)])
        .args(["--objects-url", &origins.objects]);
    for (mut command, service) in commands.into_iter().zip(&SERVICES) {
        let child = spawn(&mut command).map_err(|e| format!("cannot start the {} service: {e}", service.name))?;
        processes.children.push(child);
    }
    Ok(processes)
}

/// Check that the services read the layers of `served` in their formats, then install the files
/// of its planner release in `work/<planner release id>/` from the objects of `target`, through
/// `store`. Give the processes, which run nothing yet and remove the folder when they drop, and
/// the region of the release.
pub(crate) fn install(
    programs: &Programs,
    store: &Store,
    target: &Target,
    work: &Path,
    served: &Served,
) -> Result<(Processes, String), String> {
    readable(programs, served)?;
    // Dropped on an error, it removes what the install wrote.
    let processes = Processes { dir: work.join(&served.planner), children: Vec::new(), ports: [0; 3] };
    let region = materialize(store, target, &served.document, &processes.dir)?;
    Ok((processes, region))
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

impl super::Services for Processes {
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
        for (port, service) in self.ports.iter().zip(&SERVICES) {
            let Ok(mut response) = agent.get(format!("http://127.0.0.1:{port}{}", service.ready)).call() else {
                return Ok(false);
            };
            let body = response.body_mut().read_to_string().unwrap_or_default();
            // Search answers before its query model loads.
            let model = serde_json::from_str::<Value>(&body).is_ok_and(|status| status["parser"]["ready"] == true);
            if response.status() != 200 || service.name == "search" && !model {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn ports(&self) -> [u16; SERVICES.len()] {
        self.ports
    }

    fn stopped(&mut self) -> Option<String> {
        self.children.iter_mut().zip(&SERVICES).find_map(|(child, service)| match child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => Some(format!("the {} service stopped: {status}", service.name)),
            Err(e) => Some(format!("the {} service: {e}", service.name)),
        })
    }
}

impl Drop for Processes {
    fn drop(&mut self) {
        stop(&mut self.children);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Ask the process group of each child to stop, and kill those that still run after five seconds.
pub fn stop(children: &mut [Child]) {
    // A child that was waited for may have a new process at its id: only a running child gets a signal.
    let running = |child: &mut Child| matches!(child.try_wait(), Ok(None));
    for child in children.iter_mut() {
        if running(child) {
            signal(child, false);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    for child in children {
        while Instant::now() < deadline && running(child) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if running(child) {
            signal(child, true);
        }
        let _ = child.wait();
    }
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
    sha256: String,
    encoding: String,
}

/// Install each file under [`RUNTIME`] of the planner release whose document is `document` in
/// `dir`, and give the region of the release. Each file comes from an object of `store`, which
/// downloads the objects that it lacks from `target`. A gzip object is decoded; each file must have
/// the SHA-256 of the release.
fn materialize(store: &Store, target: &Target, document: &Named, dir: &Path) -> Result<String, String> {
    let object = |key: &str, sha256: &str| {
        if !is_sha256(sha256) {
            return Err(format!("{key} names no object"));
        }
        match store.object(sha256) {
            object if object.is_file() => Ok(object),
            _ => target.download(key, sha256, store)?.ok_or_else(|| format!("the target has no {key}")),
        }
    };
    let path = object(&document.key, &document.sha256)?;
    let read = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let document: Value = serde_json::from_slice(&read).map_err(|e| format!("{}: {e}", document.key))?;
    let files = document["files"].as_object().ok_or("the planner release has no files")?;
    for (name, entry) in files.iter().filter(|(name, _)| RUNTIME.iter().any(|folder| name.starts_with(folder))) {
        crate::r2::check_key(name)?;
        let entry: Entry = serde_json::from_value(entry.clone()).map_err(|e| format!("{name}: {e}"))?;
        let sha256 = &entry.transport.sha256;
        let (object, path) = (object(&format!("objects/{sha256}"), sha256)?, dir.join(name));
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
    document["region"].as_str().map(String::from).ok_or_else(|| "the planner release has no region".into())
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
        let (store, work) = (Store::at(scratch.0.join("store")), scratch.0.join("work"));
        let target = Target::Dir(scratch.0.join("target"));
        let refused = install(&programs, &store, &target, &work, &served(9, 3)).err().unwrap();
        assert_eq!(refused, "needs a routing service that reads planner/routing format 9; it reads 8");
        assert!(!work.exists(), "nothing is installed");
        let index = readable(&programs, &served(8, 4)).unwrap_err();
        assert_eq!(index, "needs a search service that reads planner/index format 4; it reads 3");
        assert!(readable(&programs, &served(8, 3)).is_ok());
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
            let sha256 = sha256_hex(bytes);
            json!({"bytes": bytes.len(), "sha256": sha256, "transport": {"sha256": transport, "encoding": encoding}})
        };
        let mut document = json!({"region": "test", "files": {
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
        let materialize = |document: &Named, dir: &Path| materialize(&store, &dir_target, document, dir);
        let named = release(&document);
        let dir = scratch.0.join("work");
        assert_eq!(materialize(&named, &dir).unwrap(), "test");
        assert_eq!(fs::read(dir.join("routing/pages.bin")).unwrap(), b"pages");
        assert_eq!(fs::read(dir.join("search/tiles/pois/9-1-1.sqlite")).unwrap(), b"places");
        assert!(!dir.join("maps").exists(), "no service reads the maps");

        // An existing file is never opened for writing: it can be a link to an object of the store.
        let (object, again) = (scratch.0.join("object"), scratch.0.join("again"));
        fs::write(&object, b"store object").unwrap();
        for name in ["routing/pages.bin", "search/tiles/pois/9-1-1.sqlite"] {
            fs::create_dir_all(again.join(name).parent().unwrap()).unwrap();
            fs::hard_link(&object, again.join(name)).unwrap();
        }
        assert!(materialize(&named, &again).unwrap_err().contains("exists"));
        fs::remove_file(again.join("routing/pages.bin")).unwrap();
        materialize(&named, &again).unwrap();
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
