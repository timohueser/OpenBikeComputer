//! `obc data serve [TARGET]`: follow the releases that a target names and run their planner
//! services. A dir target goes to the apps of this machine and of its network; an R2 target has
//! the API only, on this machine, for a proxy such as Caddy.

use std::future::IntoFuture;
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{print_json, Code, Error};
use crate::release;
use crate::serve::http::{self, Shared};
use crate::serve::services::{self, Origins, Processes, Programs};
use crate::serve::{look, Follower, Load, Seen, Served, PLANNER};
use crate::store::Store;
use crate::target::Target;

/// The port of `obc data serve`: on every IPv4 address of this machine for a dir target, and on
/// 127.0.0.1 for an R2 target.
const PORT: u16 = 8710;

/// How long the open requests may take after a signal stops `obc data serve`.
const GRACE: Duration = Duration::from_secs(10);

#[derive(Args)]
pub struct ServeArgs {
    /// The target, as `data/targets.toml` names it.
    #[arg(default_value = "local")]
    target: String,
}

/// What `serve` writes when it listens.
#[derive(Serialize, JsonSchema)]
pub struct Listening {
    pub target: String,
    /// The origin of `obc data serve` on this machine.
    pub url: String,
    /// Its origin on the network, for a phone; `null` without a network and for an R2 target.
    pub network: Option<String>,
}

pub fn command(root: &Path, args: ServeArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, &args.target, &store).map_err(|e| Code::Usage.error(e))?;
    let dir = match &target {
        Target::Dir(dir) => Some(dir.clone()),
        Target::R2(_) => None,
    };
    let programs = programs(root, dir.is_some())?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let address = if dir.is_some() { "0.0.0.0" } else { "127.0.0.1" };
    let listener = runtime.block_on(tokio::net::TcpListener::bind((address, PORT))).map_err(|e| {
        Code::Usage
            .error(format!("port {PORT}: {e}"))
            .fix("Stop the other `obc data serve`, or the program that uses the port, then run again.")
    })?;
    let listening = Listening {
        target: args.target.clone(),
        url: format!("http://127.0.0.1:{PORT}"),
        network: dir.as_ref().and_then(|_| network_address()).map(|address| format!("http://{address}:{PORT}")),
    };
    if json {
        print_json(&listening)?;
    } else if dir.is_some() {
        print_links(root, &listening);
    } else {
        println!("obc data serve: the planner API of {} at {}", listening.target, listening.url);
    }
    let work = store.root().join("serve").join(&args.target);
    // The work folder holds what a serve that did not stop left.
    let _ = std::fs::remove_dir_all(&work);
    // The objects of the releases that serve follows are in a store of their own, which it prunes.
    let objects = Store::open_at(store.root().join("follower").join(&args.target))?;
    let mut tile_server = Vec::new();
    let mut files = None;
    if let Some(dir) = &dir {
        // The tile server reads the folder of the target, also before the first update.
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tiles = services::free_port()?;
        let mut command = Command::new("node");
        command.arg("src/local.mjs").arg(dir).arg(tiles.to_string()).current_dir(root.join("planner/tiles"));
        tile_server.push(services::spawn(&mut command).map_err(|e| format!("cannot start the tile server: {e}"))?);
        files = Some((dir.as_path(), tiles));
    }

    let (shared, stop) = (Shared::default(), Arc::new(AtomicBool::new(false)));
    let loader = Loader {
        programs: Arc::new(programs),
        store: objects.root().to_path_buf(),
        target: target.clone(),
        work: work.clone(),
        public: dir.is_some().then(|| listening.network.clone().unwrap_or(listening.url.clone())),
    };
    let follower = {
        let (target, shared, stop) = (target.clone(), shared.clone(), stop.clone());
        let follower = std::thread::Builder::new().spawn(move || follow(&target, loader, &shared, &stop));
        follower.map_err(|e| format!("cannot start the follower: {e}"))?
    };
    let router = http::router(shared, files);
    let ended = runtime.block_on(async {
        let signalled = Arc::new(tokio::sync::Notify::new());
        let shutdown = {
            let signalled = signalled.clone();
            async move {
                signal().await;
                signalled.notify_one();
            }
        };
        let server = axum::serve(listener, router).with_graceful_shutdown(shutdown);
        // A follower that ended, such as on a panic, runs no service: serve ends, and its supervisor
        // starts it again.
        let follows = async {
            while !follower.is_finished() {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        };
        tokio::select! {
            served = server.into_future() => served.map_err(|e| Code::Failed.error(format!("port {PORT}: {e}"))),
            () = async { signalled.notified().await; tokio::time::sleep(GRACE).await } => Ok(()),
            () = follows => Err(Code::Failed.error("the follower of the target stopped").fix("Read the log above, then start `obc data serve` again.")),
        }
    });
    runtime.shutdown_background();
    stop.store(true, Ordering::Relaxed);
    // A download ends within one read; the follower waits for each before it ends.
    crate::fetch::http::stop();
    // The follower waits while a start installs files: past the grace, the process ends without it.
    let deadline = Instant::now() + GRACE;
    while !follower.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if follower.is_finished() {
        let _ = follower.join();
    }
    services::stop(&mut tile_server);
    let _ = std::fs::remove_dir_all(&work);
    ended
}

/// The command that opens each app on the target, with absolute paths.
fn print_links(root: &Path, listening: &Listening) {
    let Listening { target, url, network } = listening;
    let (planner, device) = (format!("{url}/{PLANNER}"), format!("{url}/{}", release::POINTERS[0]));
    let web = root.join("builder/web");
    println!("obc data serve: {target} at {url}");
    println!("  web planner  cd '{}' && VITE_PLANNER_CATALOG={planner} npm run dev -- --mode planner", web.display());
    println!("               then open /planner.html; once before: npm ci && npm run build:wasm");
    println!("  map builder  VITE_PLANNER_CATALOG={planner} obc site");
    println!("  desktop app  OBC_CATALOG_URL={device} obc desktop");
    println!("  simulator    obc sim MAP.obcm, with a map that the map builder or the desktop app assembles");
    if let Some(network) = network {
        println!("  iPhone       the Debug app with the launch argument -OBCPlannerCatalog {network}/{PLANNER}");
    }
    println!("Other machines on the local network can reach port {PORT} while it runs. Ctrl-C stops it.");
}

/// Follow `target` until `stop`: each second the follower checks the services, and gives `shared`
/// what clients get. It reads what a dir target names each second, and an R2 target, through its
/// public origin, every ten seconds.
fn follow(target: &Target, mut loader: Loader, shared: &Shared, stop: &AtomicBool) {
    let every = match target {
        Target::Dir(_) => 1,
        Target::R2(_) => 10,
    };
    let (mut follower, mut seen) = (Follower::default(), Seen::default());
    for tick in 0u64.. {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if tick % every == 0 {
            look(&mut follower, target, &mut seen, &mut loader, &Instant::now);
        } else {
            follower.poll(&mut loader, &Instant::now);
        }
        *shared.write().unwrap() = follower.view();
        std::thread::sleep(Duration::from_secs(1));
    }
    follower.finish();
}

/// How serve loads a planner release: a thread downloads its objects into the store, then it
/// installs its files from the store and starts its services.
struct Loader {
    programs: Arc<Programs>,
    store: PathBuf,
    target: Target,
    work: PathBuf,
    /// For a dir target, the origin at which clients reach serve. An R2 target uses the origins of
    /// the planner pointer.
    public: Option<String>,
}

impl Load for Loader {
    type Services = Processes;

    fn fetch(&mut self, served: &Served, wanted: Arc<AtomicBool>) -> Result<JoinHandle<Result<(), String>>, String> {
        let (programs, store) = (self.programs.clone(), Store::at(self.store.clone()));
        let (target, served) = (self.target.clone(), served.clone());
        let fetch = move || services::fetch(&programs, &store, &target, &served, &wanted);
        let worker = std::thread::Builder::new().spawn(fetch);
        worker.map_err(|e| format!("cannot start a download: {e}"))
    }

    fn keep(&mut self, wanted: &[&Served]) {
        if let Err(e) = services::prune(&Store::at(self.store.clone()), &self.target, wanted) {
            eprintln!("obc data serve: the store of serve keeps files that no release reads: {e}");
        }
    }

    fn start(&mut self, served: &Served) -> Result<Processes, String> {
        let origins = match &self.public {
            Some(public) => Origins { api: public.clone(), objects: format!("{public}/objects") },
            None => served.origins()?,
        };
        services::start(&self.programs, &Store::at(self.store.clone()), &self.work, served, &origins)
    }
}

/// The programs of the services. For a dir target it builds the routing service with cargo; for an
/// R2 target the routing service is beside this program, as `tools/vps/deploy-services.sh` builds it.
fn programs(root: &Path, dir: bool) -> Result<Programs, Error> {
    let setup = "Run `python3 planner/search/setup.py`: it installs the packages of the search service.";
    let mut needed = vec![("planner/search/node_modules", setup), ("planner/search/.venv", setup)];
    if dir {
        needed.push(("planner/tiles/node_modules", "Run `npm ci --prefix planner/tiles`."));
    }
    for (path, fix) in needed {
        if !root.join(path).is_dir() {
            return Err(Code::Blocked.error(format!("{path} is missing")).fix(fix));
        }
    }
    for tool in ["node", "python3"] {
        if Command::new(tool).arg("--version").stdout(Stdio::null()).status().is_err() {
            return Err(Code::Blocked
                .error(format!("`{tool}` is not on PATH"))
                .fix("Install the Node and Python versions of `data/planner-runtime.toml`, then run again."));
        }
    }
    let routing = match dir {
        true => build_routing(root)?,
        false => {
            let routing = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("planner-service");
            if !routing.is_file() {
                return Err(Code::Blocked
                    .error(format!("{} is missing", routing.display()))
                    .fix("Deploy the services as tools/vps/README.md says."));
            }
            routing
        }
    };
    Programs::new(root.to_path_buf(), routing).map_err(|e| {
        Code::Blocked.error(e).fix("Prepare the services as host/obc-data/README.md says, then run again.")
    })
}

/// Build the routing service with cargo, and give its program.
fn build_routing(root: &Path) -> Result<PathBuf, Error> {
    let cargo = std::env::var_os("CARGO").unwrap_or("cargo".into());
    let args = ["build", "--release", "--locked", "-p", "planner-service", "--message-format=json-render-diagnostics"];
    eprintln!("obc data serve: cargo {}", args[..5].join(" "));
    let built = Command::new(cargo).args(args).current_dir(root).stderr(Stdio::inherit()).output();
    let built = built.map_err(|e| format!("cargo: {e}"))?;
    let fix = "Correct what cargo reports, then run again.";
    if !built.status.success() {
        return Err(Code::Failed.error("the routing service does not build").fix(fix));
    }
    let routing = String::from_utf8_lossy(&built.stdout).lines().find_map(|line| {
        let message: Value = serde_json::from_str(line).ok()?;
        let artifact = message["reason"] == "compiler-artifact" && message["target"]["name"] == "planner-service";
        artifact.then(|| message["executable"].as_str().map(PathBuf::from)).flatten()
    });
    routing.ok_or_else(|| Code::Failed.error("cargo built no `planner-service` program").fix(fix))
}

/// The address of this machine on the network of its default route. A UDP connect sends nothing.
fn network_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|address| !address.is_loopback() && !address.is_unspecified())
}

/// Ctrl-C, SIGTERM, or SIGHUP when the terminal closes.
async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal as unix, SignalKind};
        let (mut terminate, mut hangup) = (unix(SignalKind::terminate()).unwrap(), unix(SignalKind::hangup()).unwrap());
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
            _ = hangup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
