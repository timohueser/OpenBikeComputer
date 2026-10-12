//! `obc data serve [TARGET]`: serve a dir target to the apps of this machine and of its network,
//! and follow the release that the target serves.

use std::future::IntoFuture;
use std::net::{IpAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::{print_json, Code, Error};
use crate::release;
use crate::serve::http::{self, Shared, View};
use crate::serve::services::{self, Processes, Programs};
use crate::serve::{wanted, Follower, Served, PLANNER};
use crate::store::Store;
use crate::target::Target;

/// The port of `obc data serve` on every IPv4 address of this machine.
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
    /// Its origin on the network of this machine, for a phone; `null` without a network.
    pub network: Option<String>,
}

pub fn command(root: &Path, args: ServeArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, &args.target, &store).map_err(|e| Code::Usage.error(e))?;
    let Target::Dir(dir) = target.clone() else {
        return Err(Code::Blocked
            .error(format!("`obc data serve` serves a dir target, and `{}` is an R2 bucket", args.target))
            .fix("Serve a dir target, such as `obc data serve local`."));
    };
    let programs = programs(root)?;
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let listener = runtime.block_on(tokio::net::TcpListener::bind(("0.0.0.0", PORT))).map_err(|e| {
        Code::Usage
            .error(format!("port {PORT}: {e}"))
            .fix("Stop the other `obc data serve`, or the program that uses the port, then run again.")
    })?;
    let listening = Listening {
        target: args.target.clone(),
        url: format!("http://127.0.0.1:{PORT}"),
        network: network_address().map(|address| format!("http://{address}:{PORT}")),
    };
    if json {
        print_json(&listening)?;
    } else {
        print_links(root, &listening);
    }
    let work = store.root().join("serve").join(&args.target);
    // The work folder holds what a serve that did not stop left.
    let _ = std::fs::remove_dir_all(&work);
    // The tile server reads the folder of the target, also before the first update.
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tiles = services::free_port()?;
    let mut tile_server = Command::new("node");
    tile_server.arg("src/local.mjs").arg(&dir).arg(tiles.to_string()).current_dir(root.join("planner/tiles"));
    let mut tile_server =
        [services::spawn(&mut tile_server).map_err(|e| format!("cannot start the tile server: {e}"))?];

    let (shared, stop) = (Shared::default(), Arc::new(AtomicBool::new(false)));
    let public = listening.network.clone().unwrap_or(listening.url.clone());
    let follower = {
        let (target, shared, stop, dir, work) =
            (target.clone(), shared.clone(), stop.clone(), dir.clone(), work.clone());
        let start = move |served: &Served| services::start(&programs, &dir, &work, served, &public);
        std::thread::spawn(move || follow(&target, start, &shared, &stop))
    };
    let router = http::router(shared, &dir, tiles);
    let served = runtime.block_on(async {
        let signalled = Arc::new(tokio::sync::Notify::new());
        let shutdown = {
            let signalled = signalled.clone();
            async move {
                signal().await;
                signalled.notify_one();
            }
        };
        let server = axum::serve(listener, router).with_graceful_shutdown(shutdown);
        tokio::select! {
            served = server.into_future() => served,
            () = async { signalled.notified().await; tokio::time::sleep(GRACE).await } => Ok(()),
        }
    });
    runtime.shutdown_background();
    stop.store(true, Ordering::Relaxed);
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
    served.map_err(|e| Code::Failed.error(format!("port {PORT}: {e}")))
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

/// Follow `target` until `stop`: a change of its `release.json` starts the services of the
/// releases that it names, and each second the follower checks them and gives `shared` what
/// clients get.
fn follow(
    target: &Target,
    mut start: impl FnMut(&Served) -> Result<Processes, String>,
    shared: &Shared,
    stop: &AtomicBool,
) {
    let mut follower = Follower::default();
    let mut seen = None;
    while !stop.load(Ordering::Relaxed) {
        let read = release::serving(target);
        let tag = read.as_ref().map(|roles| roles.as_ref().map(|(_, tag)| tag.clone())).map_err(Clone::clone);
        if seen.as_ref() == Some(&tag) {
            follower.poll(Instant::now());
        } else {
            match read.and_then(|roles| roles.map(|(roles, _)| wanted(target, &roles)).transpose()) {
                Ok(None) => eprintln!("obc data serve: the target serves no release yet; `obc data update` makes one"),
                Ok(Some(wanted)) => follower.want(wanted, &mut start, &Instant::now),
                Err(e) => eprintln!("obc data serve: {e}; the current release stays"),
            }
            seen = Some(tag);
        }
        let ports = follower.ready().map(|(id, processes)| (id.to_string(), processes.ports)).collect();
        *shared.write().unwrap() = View { current: follower.current().cloned(), ports };
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The programs of the services. It builds the routing service with cargo.
fn programs(root: &Path) -> Result<Programs, Error> {
    let setup = "Run `python3 planner/search/setup.py`: it installs the packages of the search service.";
    for (path, fix) in [
        ("planner/search/node_modules", setup),
        ("planner/search/.venv", setup),
        ("planner/tiles/node_modules", "Run `npm ci --prefix planner/tiles`."),
    ] {
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
    let routing = routing.ok_or_else(|| Code::Failed.error("cargo built no `planner-service` program").fix(fix))?;
    Ok(Programs { root: root.to_path_buf(), routing })
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
