//! `obc data serve`: run the planner services of the releases that a target names, and give the
//! apps a dir target. The services of a new release start beside those of the current one, and
//! clients get the new release once its services answer. An install stays on disk across restarts.
//! The services of two planner releases run, so a rollback, and a switch that waited for the next
//! release, are instant.

pub mod http;
pub mod services;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::release::{File, Release, Role, Serving, NEXT, POINTERS, SERVING};
use crate::store::sha256_hex;
use crate::target::Target;

/// The pointer that names the planner release of a release.
pub const PLANNER: &str = POINTERS[1];

/// How long the services of a planner release may take to answer after they start.
pub const READY: Duration = Duration::from_secs(120);

/// How long a failed planner release or service waits before it starts again. Each failure in a
/// row doubles the wait, up to [`BACKOFF_MAX`]: a download that broke soon tries again, and a
/// release that cannot start, such as one in a format that the services do not read, logs at most
/// twice an hour. A service that stops this long after its first answer starts again at once.
const BACKOFF: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

/// A release of the target as its clients read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// The id of its manifest.
    pub id: String,
    /// The planner release that its planner pointer names. Its services answer the API.
    pub planner: String,
    /// The document of the planner release, `releases/<planner>/release.json`.
    pub document: File,
    /// The bytes of each pointer key.
    pub pointers: BTreeMap<String, Vec<u8>>,
    /// The format of each layer of the release.
    pub formats: BTreeMap<String, u32>,
}

/// The bytes of `release.json` and of `planner/next.json` of a target: the releases that it names.
type Names = [Option<Vec<u8>>; 2];

/// The releases that a target names.
#[derive(Default, PartialEq)]
pub struct Wanted {
    /// The release that it serves, then its rollback.
    pub serving: Vec<Served>,
    /// The release of `planner/next.json`, unless it is one of `serving`. No client gets it.
    pub next: Option<Served>,
}

fn names(target: &Target) -> Result<Names, String> {
    Ok([target.get_public(SERVING)?, target.get_public(NEXT)?])
}

/// The releases that `names` of `target` name, and why the release of `planner/next.json` cannot
/// be read: it is then no release, and never holds back the others. `known` gives a release that
/// the follower read before, so a rollback reads nothing but `release.json`.
fn wanted(
    target: &Target,
    names: &Names,
    known: impl Fn(&str) -> Option<Served>,
) -> Result<(Wanted, Option<String>), String> {
    let read = |id: &str| known(id).map_or_else(|| served(target, id), Ok);
    let mut wanted = Wanted::default();
    if let Some(bytes) = &names[0] {
        let roles: Serving = serde_json::from_slice(bytes).map_err(|e| format!("{SERVING}: {e}"))?;
        let ids = std::iter::once(&roles.serving).chain(&roles.rollback);
        wanted.serving = ids.map(|role| read(&role.id)).collect::<Result<_, _>>()?;
    }
    let next = names[1].as_ref().map(|bytes| {
        let next: Role = serde_json::from_slice(bytes).map_err(|e| format!("{NEXT}: {e}"))?;
        match wanted.serving.iter().any(|served| served.id == next.id) {
            true => Ok(None),
            false => read(&next.id).map(Some).map_err(|e| format!("{NEXT}: {e}")),
        }
    });
    let problem = match next.transpose() {
        Ok(next) => {
            wanted.next = next.flatten();
            None
        }
        Err(e) => Some(e),
    };
    Ok((wanted, problem))
}

/// The release `id` of `target`, as its clients read it.
pub(crate) fn served(target: &Target, id: &str) -> Result<Served, String> {
    let key = crate::release::manifest_key(id);
    let bytes = target.get_public(&key)?.ok_or_else(|| format!("the target has no {key}"))?;
    let release = Release::parse(id, &bytes)?;
    let mut pointers = BTreeMap::new();
    for pointer in &release.pointers {
        let key = format!("objects/{}", pointer.sha256);
        let bytes = target.get_public(&key)?.filter(|bytes| sha256_hex(bytes) == pointer.sha256);
        pointers.insert(pointer.key.clone(), bytes.ok_or_else(|| format!("{key} is missing or holds other bytes"))?);
    }
    let catalog: Value = serde_json::from_slice(&pointers[PLANNER]).map_err(|e| format!("{PLANNER} of {id}: {e}"))?;
    let planner = catalog["active"]["id"].as_str().filter(|planner| crate::release::is_sha256(planner));
    let planner = planner.ok_or_else(|| format!("{PLANNER} of release {id} names no planner release"))?;
    let key = format!("releases/{planner}/release.json");
    let named = release.named_files(|list| {
        target.get_public(&list.key())?.ok_or_else(|| format!("the target has no {}", list.key()))
    })?;
    let document = named.into_iter().find(|file| file.key == key);
    let document = document.ok_or_else(|| format!("release {id} has no {key}"))?;
    let formats = release.outputs.iter().map(|output| (output.layer.clone(), output.format)).collect();
    Ok(Served { id: id.into(), planner: planner.into(), document, pointers, formats })
}

impl Served {
    /// The origins that its planner pointer names: those of a published target.
    pub fn origins(&self) -> Result<services::Origins, String> {
        let pointer: Value = serde_json::from_slice(&self.pointers[PLANNER]).map_err(|e| format!("{PLANNER}: {e}"))?;
        let origin = |name: &str| {
            pointer["origins"][name].as_str().map(String::from).ok_or_else(|| format!("{PLANNER} has no {name}"))
        };
        Ok(services::Origins { api: origin("api_origin")?, objects: format!("{}/objects", origin("objects_origin")?) })
    }
}

/// What the follower read of its target last.
#[derive(Default)]
pub struct Seen {
    names: Option<Names>,
    /// Whether it read each release that they name.
    whole: bool,
    error: Option<String>,
}

/// Read what `target` names. When it named other releases since `seen`, the follower wants them;
/// else it polls. An error leaves the releases as they are, and the next look reads again: also a
/// release of `planner/next.json` that cannot be read.
pub fn look<P: Process>(
    follower: &mut Follower<P>,
    target: &Target,
    seen: &mut Seen,
    load: &mut dyn Load<Process = P>,
    now: &dyn Fn() -> Instant,
) {
    let read = names(target).and_then(|names| match seen.whole && seen.names.as_ref() == Some(&names) {
        true => Ok(None),
        false => wanted(target, &names, |id| follower.known(id)).map(|wanted| Some((names, wanted))),
    });
    let problem = match read {
        Ok(Some((names, (wanted, problem)))) => {
            if names[0].is_none() && seen.names.as_ref().is_none_or(|seen| seen[0].is_some()) {
                say("the target names no release yet".into());
            }
            (seen.names, seen.whole) = (Some(names), problem.is_none());
            if follower.wanted != wanted {
                follower.want(wanted, load, now);
            } else {
                follower.poll(load, now);
            }
            problem
        }
        Ok(None) => {
            follower.poll(load, now);
            None
        }
        Err(e) => {
            follower.poll(load, now);
            Some(format!("{e}; the current release stays"))
        }
    };
    if problem != seen.error {
        if let Some(problem) = &problem {
            say(problem.clone());
        }
        seen.error = problem;
    }
}

/// One service of a planner release. It stops when it drops.
pub trait Process {
    /// Whether it answers; an error when it stopped.
    fn ready(&mut self) -> Result<bool, String>;
    /// Why it stopped; `None` while it runs.
    fn stopped(&mut self) -> Option<String>;
    /// Its port on this machine.
    fn port(&self) -> u16;
}

/// How a fetch of a planner release ended.
pub enum Fetched {
    Installed,
    /// The disk has `free` bytes, and the install needs `need`.
    NoRoom {
        need: u64,
        free: u64,
    },
    Failed(String),
}

/// How the follower loads the planner release of a release.
pub trait Load {
    type Process: Process;
    /// Install the files of `served`, on a thread of its own: a download can take long, and the
    /// follower keeps polling. It stops between two files once `wanted` is false. An error when the
    /// system gives no thread.
    fn fetch(&mut self, served: &Served, wanted: Arc<AtomicBool>) -> Result<JoinHandle<Fetched>, String>;
    /// Start the service `service` of [`services::SERVICES`] of the installed `served`.
    fn start(&mut self, served: &Served, service: usize) -> Result<Self::Process, String>;
    /// Delete from the store what the services of none of `wanted` read, and each install whose
    /// planner release `installs` does not hold.
    fn keep(&mut self, wanted: &[&Served], installs: &BTreeSet<String>);
    /// Delete the files of the planner release `planner`; whether it had files.
    fn delete(&mut self, planner: &str) -> bool;
}

enum State<P> {
    /// Its files download and install while `wanted`. `backoff` is the wait after a failure.
    Fetching { worker: JoinHandle<Fetched>, backoff: Duration, wanted: Arc<AtomicBool> },
    /// Its files are on disk. Each of the [`services::SERVICES`] has its own state.
    Installed { services: [Service<P>; 3] },
    /// Its fetch failed, for this reason. It fetches again at `retry`, or at once when the target
    /// names other releases.
    Failed { reason: String, retry: Instant, backoff: Duration },
}

struct Service<P> {
    run: Run<P>,
    /// The wait after its next failure.
    backoff: Duration,
}

enum Run<P> {
    Stopped,
    Starting {
        process: P,
        since: Instant,
    },
    /// It answers since `since`.
    Ready {
        process: P,
        since: Instant,
    },
    /// It stopped, for this reason, and starts again at `retry`.
    Failed {
        reason: String,
        retry: Instant,
    },
}

impl<P: Process> Service<P> {
    fn new() -> Self {
        Self { run: Run::Stopped, backoff: BACKOFF }
    }

    /// Follow a process that starts or runs at `now`.
    fn check(&mut self, planner: &str, service: usize, now: Instant) {
        let name = services::SERVICES[service].name;
        match &mut self.run {
            Run::Starting { process, since } => match process.ready() {
                Ok(true) => {
                    let Run::Starting { process, .. } = std::mem::replace(&mut self.run, Run::Stopped) else {
                        unreachable!("the service starts")
                    };
                    self.run = Run::Ready { process, since: now };
                }
                Ok(false) if now.duration_since(*since) < READY => {}
                Ok(false) => {
                    self.fail(planner, format!("the {name} service does not answer after {} s", READY.as_secs()), now)
                }
                Err(e) => self.fail(planner, e, now),
            },
            Run::Ready { process, since } => {
                let since = *since;
                if let Some(e) = process.stopped() {
                    match now.duration_since(since) >= BACKOFF {
                        true => {
                            say(format!("planner release {}: {e}; it starts again", short(planner)));
                            (self.run, self.backoff) = (Run::Stopped, BACKOFF);
                        }
                        false => self.fail(planner, e, now),
                    }
                }
            }
            Run::Stopped | Run::Failed { .. } => {}
        }
    }

    fn fail(&mut self, planner: &str, reason: String, now: Instant) {
        say(format!("planner release {}: {reason}; it starts again in {} s", short(planner), self.backoff.as_secs()));
        self.run = Run::Failed { reason, retry: now + self.backoff };
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }
}

/// The planner releases whose services run, and the release that clients get.
pub struct Follower<P> {
    /// By planner release id: those of the wanted releases and of the current one.
    planners: BTreeMap<String, State<P>>,
    wanted: Wanted,
    current: Option<Served>,
    /// The fetches of planner releases that no release wants now.
    abandoned: Vec<JoinHandle<Fetched>>,
    /// Whether the store can hold what no wanted release reads: the wanted releases changed or a
    /// fetch ran since the store was pruned.
    dirty: bool,
}

impl<P: Process> Default for Follower<P> {
    fn default() -> Self {
        Self {
            planners: BTreeMap::new(),
            wanted: Wanted::default(),
            current: None,
            abandoned: Vec::new(),
            dirty: false,
        }
    }
}

impl<P: Process> Follower<P> {
    /// Follow `wanted`, then [`Self::poll`]. The planner releases whose fetch failed fetch again at
    /// once.
    pub fn want(&mut self, wanted: Wanted, load: &mut dyn Load<Process = P>, now: &dyn Fn() -> Instant) {
        self.planners.retain(|_, state| !matches!(state, State::Failed { .. }));
        (self.wanted, self.dirty) = (wanted, true);
        self.poll(load, now);
        if let Some(current) = self.current.as_ref().filter(|current| !self.wanted.serving.contains(current)) {
            say(format!(
                "WARNING: clients get release {}, which release.json no longer names. An update can delete its \
                 files. Fix: run `obc data update` until a release loads.",
                short(&current.id)
            ));
        }
    }

    /// The releases in the order of their claim: current, serving, next, rollback. While a next
    /// release is named, the rollback fetches nothing.
    fn order(&self, fetch: bool) -> Vec<&Served> {
        let rollback = self.wanted.serving.get(1).filter(|_| !fetch || self.wanted.next.is_none());
        let all = self.current.iter().chain(self.wanted.serving.first()).chain(&self.wanted.next).chain(rollback);
        let mut order: Vec<&Served> = Vec::new();
        for served in all {
            if !order.iter().any(|seen| seen.planner == served.planner) {
                order.push(served);
            }
        }
        order
    }

    /// Take the fetch that ended. Start one fetch, for the first release of [`Self::order`] without
    /// an install that does not wait for its back-off, when no fetch runs. Run the services of the
    /// first two installed planner releases of the order, stop the others, and check them. Clients
    /// get the serving release of the target once its services answer; until then they keep the
    /// current release. Stop what no release needs, and prune.
    pub fn poll(&mut self, load: &mut dyn Load<Process = P>, now: &dyn Fn() -> Instant) {
        self.abandoned.retain(|worker| !worker.is_finished());
        self.prune(load);
        let mut later = false;
        let ended: Vec<String> = (self.planners.iter())
            .filter(|(_, state)| matches!(state, State::Fetching { worker, .. } if worker.is_finished()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ended {
            let Some(State::Fetching { worker, backoff, .. }) = self.planners.remove(&id) else {
                unreachable!("a fetch ended")
            };
            match worker.join().unwrap_or_else(|_| Fetched::Failed("the download stopped".into())) {
                Fetched::Installed => {
                    let services = [Service::new(), Service::new(), Service::new()];
                    self.planners.insert(id, State::Installed { services });
                }
                Fetched::Failed(e) => {
                    self.planners.insert(id.clone(), failed(&id, e, now(), backoff));
                }
                Fetched::NoRoom { need, free } => {
                    let (need, free) = (gb(need), gb(free));
                    match self.drop_rollback(&id, load) {
                        Some(rollback) => {
                            say(format!(
                                "WARNING: no rollback during the install of {}: deleted the files of planner release \
                                 {}; the disk has {free} GB free and the install needs {need} GB",
                                short(&id),
                                short(&rollback)
                            ));
                            later = true;
                        }
                        None => {
                            let reason = format!("the disk has {free} GB free; the install needs {need} GB");
                            self.planners.insert(id.clone(), failed(&id, reason, now(), backoff));
                        }
                    }
                }
            }
        }
        if !later
            && self.abandoned.is_empty()
            && !self.planners.values().any(|state| matches!(state, State::Fetching { .. }))
        {
            let waits = |served: &&Served| match self.planners.get(&served.planner) {
                None => Some(BACKOFF),
                Some(State::Failed { retry, backoff, .. }) if *retry <= now() => Some(*backoff),
                Some(_) => None,
            };
            let next =
                self.order(true).into_iter().find_map(|served| waits(&served).map(|backoff| (served.clone(), backoff)));
            if let Some((served, backoff)) = next {
                let wanted = Arc::new(AtomicBool::new(true));
                self.dirty = true;
                let state = match load.fetch(&served, wanted.clone()) {
                    Ok(worker) => State::Fetching { worker, backoff, wanted },
                    Err(e) => failed(&served.planner, e, now(), backoff),
                };
                self.planners.insert(served.planner.clone(), state);
            }
        }
        let order: Vec<Served> = self.order(false).into_iter().cloned().collect();
        let installed =
            |id: &str, planners: &BTreeMap<String, State<P>>| matches!(planners.get(id), Some(State::Installed { .. }));
        let running: Vec<&Served> =
            order.iter().filter(|served| installed(&served.planner, &self.planners)).take(2).collect();
        for (id, state) in &mut self.planners {
            let State::Installed { services } = state else { continue };
            let served = running.iter().find(|served| served.planner == *id);
            for (at, service) in services.iter_mut().enumerate() {
                let Some(served) = served else {
                    service.run = Run::Stopped;
                    continue;
                };
                let due = match &service.run {
                    Run::Stopped => true,
                    Run::Failed { retry, .. } => *retry <= now(),
                    Run::Starting { .. } | Run::Ready { .. } => false,
                };
                if due {
                    match load.start(served, at) {
                        Ok(process) => service.run = Run::Starting { process, since: now() },
                        Err(e) => service.fail(id, e, now()),
                    }
                }
            }
        }
        for (id, state) in &mut self.planners {
            if let State::Installed { services } = state {
                for (at, service) in services.iter_mut().enumerate() {
                    service.check(id, at, now());
                }
            }
        }
        if let Some(next) = self.wanted.serving.first().filter(|next| self.current.as_ref() != Some(*next)) {
            if self.ready(&next.planner) {
                say(format!("release {} with planner release {}", short(&next.id), short(&next.planner)));
                self.current = Some(next.clone());
            }
        }
        let fetchable: BTreeSet<&str> = self.order(true).into_iter().map(|served| served.planner.as_str()).collect();
        let needed: BTreeSet<&str> = order.iter().map(|served| served.planner.as_str()).collect();
        let gone: Vec<String> = (self.planners.iter())
            .filter(|(id, state)| match state {
                State::Fetching { .. } => !fetchable.contains(id.as_str()),
                _ => !needed.contains(id.as_str()),
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            if let Some(State::Fetching { worker, wanted, .. }) = self.planners.remove(&id) {
                wanted.store(false, Ordering::Relaxed);
                self.abandoned.push(worker);
            }
        }
        self.prune(load);
    }

    /// Whether each service of the planner release `planner` answers.
    fn ready(&self, planner: &str) -> bool {
        match self.planners.get(planner) {
            Some(State::Installed { services }) => {
                services.iter().all(|service| matches!(service.run, Run::Ready { .. }))
            }
            _ => false,
        }
    }

    /// For the planner release `planner`, which has no room: stop the services of the rollback and
    /// delete its files, when no other release names its planner release. Give it when it had files.
    fn drop_rollback(&mut self, planner: &str, load: &mut dyn Load<Process = P>) -> Option<String> {
        let rollback = self.wanted.serving.get(1)?.planner.clone();
        let others = self.current.iter().chain(self.wanted.serving.first()).chain(&self.wanted.next);
        if rollback == planner || others.into_iter().any(|served| served.planner == rollback) {
            return None;
        }
        self.planners.remove(&rollback);
        load.delete(&rollback).then_some(rollback)
    }

    /// Delete from the store what no wanted release reads, and the installs of the planner releases
    /// that neither the follower nor a wanted release has, once no fetch runs: a fetch writes in
    /// `partial/` and its folder meanwhile.
    fn prune(&mut self, load: &mut dyn Load<Process = P>) {
        let fetching = self.planners.values().any(|state| matches!(state, State::Fetching { .. }));
        if self.dirty && !fetching && self.abandoned.is_empty() {
            let wanted: Vec<&Served> = self.wanted.serving.iter().chain(&self.wanted.next).collect();
            let mut installs: BTreeSet<String> = self.planners.keys().cloned().collect();
            installs.extend(wanted.iter().map(|served| served.planner.clone()));
            load.keep(&wanted, &installs);
            self.dirty = false;
        }
    }

    /// Wait for each fetch, after [`crate::fetch::http::stop`] ended it: a stopped serve writes
    /// nothing more.
    pub fn finish(self) {
        let fetching = self.planners.into_values().filter_map(|state| match state {
            State::Fetching { worker, .. } => Some(worker),
            _ => None,
        });
        for worker in fetching.chain(self.abandoned) {
            let _ = worker.join();
        }
    }

    /// The release that clients get.
    pub fn current(&self) -> Option<&Served> {
        self.current.as_ref()
    }

    /// The release `id`, when the follower read it before.
    pub fn known(&self, id: &str) -> Option<Served> {
        let mut served = self.wanted.serving.iter().chain(&self.wanted.next).chain(&self.current);
        served.find(|served| served.id == id).cloned()
    }

    /// What the HTTP side gives clients: the current release, and the state of each planner release
    /// with the port of each service that answers.
    pub fn view(&self) -> http::View {
        let planners = self.planners.iter().map(|(id, state)| {
            let mut ports = [None; services::SERVICES.len()];
            let status = match state {
                State::Fetching { .. } => http::Status::Starting,
                State::Failed { reason, .. } => http::Status::Failed(reason.clone()),
                State::Installed { services } => {
                    for (port, service) in ports.iter_mut().zip(services) {
                        if let Run::Ready { process, .. } = &service.run {
                            *port = Some(process.port());
                        }
                    }
                    let reasons: Vec<&str> = (services.iter())
                        .filter_map(|service| match &service.run {
                            Run::Failed { reason, .. } => Some(reason.as_str()),
                            _ => None,
                        })
                        .collect();
                    match () {
                        _ if !reasons.is_empty() => http::Status::Failed(reasons.join("; ")),
                        _ if ports.iter().all(Option::is_some) => http::Status::Ready,
                        _ if services.iter().all(|service| matches!(service.run, Run::Stopped)) => {
                            http::Status::Installed
                        }
                        _ => http::Status::Starting,
                    }
                }
            };
            (id.clone(), http::Planner { status, ports })
        });
        http::View { current: self.current.clone(), planners: planners.collect() }
    }
}

/// The state of a planner release whose fetch failed at `now`, and that waits `backoff`.
fn failed<P>(planner: &str, reason: String, now: Instant, backoff: Duration) -> State<P> {
    say(format!("planner release {} failed: {reason}; it fetches again in {} s", short(planner), backoff.as_secs()));
    State::Failed { reason, retry: now + backoff, backoff: (backoff * 2).min(BACKOFF_MAX) }
}

/// Write `line` to the log of serve.
fn say(line: String) {
    #[cfg(test)]
    tests::SAID.with(|said| said.borrow_mut().push(line.clone()));
    eprintln!("obc data serve: {line}");
}

/// `bytes` in GB, with one decimal.
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::SystemTime;

    use serde_json::json;

    use super::*;
    use crate::engine::LayerFile;
    use crate::release::{in_store, serving, switch};
    use crate::serve::services::Disk;
    use crate::store::tests::Scratch;
    use crate::store::Store;
    use crate::target::Condition;

    thread_local! {
        /// What [`say`] wrote on this thread.
        pub(super) static SAID: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// What the fake services of each planner release do, and what happened to them.
    #[derive(Default)]
    struct Log {
        ready: BTreeSet<String>,
        broken: BTreeSet<String>,
        /// The services that exited, by planner release and place in [`services::SERVICES`].
        exited: BTreeSet<(String, usize)>,
        ports: BTreeMap<String, u16>,
        /// The planner releases whose download waits until the test sends on the channel.
        held: BTreeMap<String, mpsc::Receiver<()>>,
        holding: BTreeSet<String>,
        /// Each start and each stop of a service, by planner release and place.
        started: Vec<(String, usize)>,
        stopped: Vec<(String, usize)>,
        /// How often the follower pruned the store.
        pruned: usize,
    }

    impl Log {
        /// The planner release of each start of a routing service: one for each start of a release.
        fn starts(&self) -> Vec<String> {
            self.started.iter().filter(|(_, at)| *at == 0).map(|(planner, _)| planner.clone()).collect()
        }

        fn stops(&self) -> Vec<String> {
            self.stopped.iter().filter(|(_, at)| *at == 0).map(|(planner, _)| planner.clone()).collect()
        }
    }

    /// A fake service of a planner release.
    struct Fake {
        planner: String,
        service: usize,
        log: Rc<RefCell<Log>>,
    }

    impl Process for Fake {
        fn ready(&mut self) -> Result<bool, String> {
            if let Some(stopped) = self.stopped() {
                return Err(stopped);
            }
            let log = self.log.borrow();
            match log.broken.contains(&self.planner) {
                true => Err("the routing service stopped".into()),
                false => Ok(log.ready.contains(&self.planner)),
            }
        }

        fn stopped(&mut self) -> Option<String> {
            let exited = self.log.borrow().exited.contains(&(self.planner.clone(), self.service));
            exited.then(|| format!("the {} service stopped: exit status 1", services::SERVICES[self.service].name))
        }

        fn port(&self) -> u16 {
            self.log.borrow().ports.get(&self.planner).copied().unwrap_or(1)
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.log.borrow_mut().stopped.push((self.planner.clone(), self.service));
        }
    }

    /// Upload a release as [`release_at`] does, whose API is at `https://api.test`.
    fn release(target: &Target, device: &str, routing: &[u8], format: u32) -> (String, String) {
        release_at(target, device, routing, format, "https://api.test")
    }

    /// Upload a release of the layer `test` in `format`: its device pointer is `device`, its
    /// planner release has the routing file `routing`, and its planner pointer names the API origin
    /// `api`. Give its id and the id of the planner release.
    pub(crate) fn release_at(
        target: &Target,
        device: &str,
        routing: &[u8],
        format: u32,
        api: &str,
    ) -> (String, String) {
        let sha256 = sha256_hex(routing);
        let transport = json!({"bytes": routing.len(), "sha256": sha256, "encoding": "identity"});
        let entry = json!({"bytes": routing.len(), "sha256": sha256, "transport": transport});
        let document =
            serde_json::to_vec(&json!({"format": 1, "region": "test", "files": {"routing/pages.bin": entry}})).unwrap();
        let planner = crate::engine::digest([("release.json", sha256_hex(&document).as_str())]);
        let origins = format!(r#"{{"api_origin":"{api}","objects_origin":"https://objects.test"}}"#);
        let catalog = format!(r#"{{"format":1,"origins":{origins},"active":{{"id":"{planner}"}}}}"#);
        let files = [device.as_bytes().to_vec(), catalog.into_bytes(), document, routing.to_vec()];
        let mut release = crate::release::tests::reading(b"", "osm", "1");
        let output = &mut release.outputs[0];
        output.format = format;
        output.files = files
            .iter()
            .zip(["device.json", "catalog.json", "release.json", "pages.bin"])
            .map(|(bytes, path)| LayerFile { path: path.into(), size: bytes.len() as u64, sha256: sha256_hex(bytes) })
            .collect();
        let file = |key: String, file: &LayerFile| File { key, size: file.size, sha256: file.sha256.clone() };
        let pointers = POINTERS.iter().zip(&output.files).map(|(key, layer)| file(key.to_string(), layer)).collect();
        release.named = vec![file(format!("releases/{planner}/release.json"), &output.files[2])];
        for bytes in &files {
            target.put(&format!("objects/{}", sha256_hex(bytes)), bytes, &Condition::Any).unwrap();
        }
        target.put(&release.named[0].key, &files[2], &Condition::Any).unwrap();
        let time = "2026-10-12T00:00:00Z".to_string();
        let release = Release::new(Default::default(), pointers, &release, "test".into(), time);
        (crate::release::tests::put(target, &release), planner)
    }

    /// Upload a release as [`release`] does, and switch `target` to it.
    fn publish(target: &Target, device: &str, routing: &[u8]) -> (String, String) {
        let (id, planner) = release(target, device, routing, 1);
        assert!(switch(target, serving(target).unwrap().as_ref(), &id).unwrap());
        (id, planner)
    }

    struct Case {
        scratch: Scratch,
        /// The target that a publish writes.
        target: Target,
        /// The target that the follower reads: the same folder, also over HTTP.
        reader: Target,
        log: Rc<RefCell<Log>>,
        follower: Follower<Fake>,
        seen: Seen,
        now: Instant,
        /// The store and the programs of a load that downloads and installs the files of a release.
        install: Option<(PathBuf, Arc<services::Programs>)>,
        /// The free bytes of the disk of an install: the first while the folder exists, else the
        /// second. Without it the disk has room.
        free: Room,
    }

    type Room = Arc<Mutex<Option<(PathBuf, u64, u64)>>>;

    /// The size of the disk of [`Case::free`]: its reserve is 2 GB.
    const DISK: u64 = 40_000_000_000;

    /// The load of the fake services: a download that the test can hold, and with `install` the
    /// real download and install in `work`.
    struct Fakes<'a> {
        log: &'a Rc<RefCell<Log>>,
        install: &'a Option<(PathBuf, Arc<services::Programs>)>,
        free: &'a Room,
        reader: &'a Target,
        work: PathBuf,
    }

    impl Load for Fakes<'_> {
        type Process = Fake;

        fn fetch(&mut self, served: &Served, wanted: Arc<AtomicBool>) -> Result<JoinHandle<Fetched>, String> {
            let held = self.log.borrow_mut().held.remove(&served.planner);
            let real =
                self.install.clone().map(|(store, programs)| (store, programs, self.reader.clone(), served.clone()));
            let (dir, free) = (self.work.join(&served.planner), self.free.clone());
            Ok(std::thread::spawn(move || {
                if let Some(held) = held {
                    let _ = held.recv();
                }
                let disk = |_: &Path| {
                    Ok(match &*free.lock().unwrap() {
                        None => Disk { free: u64::MAX, size: 0 },
                        Some((folder, with, without)) => {
                            Disk { free: if folder.exists() { *with } else { *without }, size: DISK }
                        }
                    })
                };
                match real {
                    Some((store, programs, target, served)) => {
                        services::fetch(&programs, &Store::at(store), &target, &served, &dir, &wanted, &disk)
                    }
                    None => Fetched::Installed,
                }
            }))
        }

        fn keep(&mut self, wanted: &[&Served], installs: &BTreeSet<String>) {
            self.log.borrow_mut().pruned += 1;
            if let Some((store, _)) = self.install {
                services::prune(&Store::at(store.clone()), self.reader, &self.work, wanted).unwrap();
                services::uninstall(&self.work, |planner| installs.contains(planner)).unwrap();
            }
        }

        fn delete(&mut self, planner: &str) -> bool {
            services::remove(&self.work.join(planner)).unwrap()
        }

        fn start(&mut self, served: &Served, service: usize) -> Result<Fake, String> {
            self.log.borrow_mut().started.push((served.planner.clone(), service));
            Ok(Fake { planner: served.planner.clone(), service, log: self.log.clone() })
        }
    }

    impl Case {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let target = Target::Dir(scratch.0.join("target"));
            let (log, follower, seen, now) = (Rc::default(), Follower::default(), Seen::default(), Instant::now());
            Self {
                reader: target.clone(),
                scratch,
                target,
                log,
                follower,
                seen,
                now,
                install: None,
                free: Room::default(),
            }
        }

        /// Look at what the target names, or only poll; then poll until each download that the
        /// test does not hold ended and the follower took it.
        fn step(&mut self, look_at_target: bool) {
            let Self { scratch, reader, log, follower, seen, now, install, free, .. } = self;
            let now = *now;
            let mut load = Fakes { log, install, free, reader, work: scratch.0.join("work") };
            match look_at_target {
                true => look(follower, reader, seen, &mut load, &|| now),
                false => follower.poll(&mut load, &|| now),
            }
            loop {
                let downloads: Vec<bool> = (follower.planners.iter())
                    .filter_map(|(id, state)| match state {
                        State::Fetching { worker, .. } if !log.borrow().holding.contains(id) => {
                            Some(worker.is_finished())
                        }
                        _ => None,
                    })
                    .collect();
                match downloads.iter().all(|finished| *finished) {
                    _ if downloads.is_empty() => break,
                    true => follower.poll(&mut load, &|| now),
                    false => std::thread::sleep(Duration::from_millis(1)),
                }
            }
        }

        /// Hold the download of `planner` until [`Self::release`].
        fn hold(&mut self, planner: &str) -> mpsc::Sender<()> {
            let (send, receive) = mpsc::channel();
            let mut log = self.log.borrow_mut();
            log.held.insert(planner.into(), receive);
            log.holding.insert(planner.into());
            send
        }

        fn release(&mut self, planner: &str, held: mpsc::Sender<()>) {
            self.log.borrow_mut().holding.remove(planner);
            held.send(()).unwrap();
            self.step(false);
        }

        fn follow(&mut self) {
            self.step(true);
        }

        fn switch(&mut self, id: &str) {
            assert!(switch(&self.target, serving(&self.target).unwrap().as_ref(), id).unwrap());
            self.follow();
        }

        fn answers(&mut self, planner: &str) {
            self.log.borrow_mut().ready.insert(planner.into());
            self.step(false);
        }

        fn current(&self) -> &str {
            &self.follower.current().unwrap().id
        }

        fn planner(&self, planner: &str) -> Option<http::Planner> {
            self.follower.view().planners.get(planner).cloned()
        }

        /// A follower that starts again on the same store.
        fn restart(&mut self) {
            (self.follower, self.seen) = (Follower::default(), Seen::default());
        }

        /// The install of `planner`.
        fn install(&self, planner: &str) -> PathBuf {
            self.scratch.0.join("work").join(planner)
        }
    }

    #[test]
    fn a_new_release_loads_beside_the_current_one_and_clients_get_it_once_it_answers() {
        let mut case = Case::new("serve-follow");
        let (a, p1) = publish(&case.target, "device a", b"routing 1");
        case.follow();
        assert!(case.follower.current().is_none(), "no client gets a release before its services answer");
        case.answers(&p1);
        assert_eq!(case.current(), a);

        let (b, p2) = publish(&case.target, "device b", b"routing 2");
        case.follow();
        assert_eq!(case.current(), a, "the current release serves while the new one starts");
        case.answers(&p2);
        assert_eq!(case.current(), b);
        let ready = |case: &Case| {
            let view = case.follower.view();
            let ready = view.planners.into_iter().filter(|(_, planner)| planner.status == http::Status::Ready);
            ready.map(|(id, _)| id).collect::<BTreeSet<_>>()
        };
        assert_eq!(ready(&case), [p1.clone(), p2.clone()].into(), "the rollback runs");

        // A release of the same planner release, such as one of `--only device/map`, switches at once.
        let (c, _) = publish(&case.target, "device c", b"routing 2");
        case.follow();
        assert_eq!(case.current(), c);
        assert_eq!(case.log.borrow().starts(), [p1.clone(), p2.clone()]);
        assert_eq!(
            case.log.borrow().stops(),
            std::slice::from_ref(&p1),
            "no release of the target needs the oldest services"
        );

        case.switch(&b);
        assert_eq!(case.current(), b, "a rollback starts nothing");

        let (d, p3) = publish(&case.target, "device d", b"routing 3");
        case.follow();
        case.answers(&p3);
        assert_eq!(case.current(), d);
        assert_eq!(case.follower.current().unwrap().pointers[POINTERS[0]], b"device d");
        assert_eq!(case.follower.current().unwrap().formats, [("test".to_string(), 1)].into(), "the layer formats");
        case.switch(&c);
        assert_eq!(case.current(), c, "a rollback to another planner release that runs");
        assert_eq!(case.log.borrow().starts(), [p1, p2, p3]);
    }

    #[test]
    fn a_release_whose_services_fail_leaves_clients_on_the_current_one_and_starts_again_later() {
        let mut case = Case::new("serve-fail");
        let (_, p1) = publish(&case.target, "device a", b"routing 1");
        case.follow();
        case.answers(&p1);

        let (_, p2) = release(&case.target, "device b", b"routing 2", 1);
        case.log.borrow_mut().broken.insert(p2.clone());
        let (b, _) = publish(&case.target, "device b", b"routing 2");
        case.follow();
        assert_eq!(case.follower.current().unwrap().id, release(&case.target, "device a", b"routing 1", 1).0);
        assert_eq!(case.log.borrow().stops(), std::slice::from_ref(&p2), "the services of a failed release stop");
        case.now += BACKOFF - Duration::from_secs(1);
        case.step(false);
        assert_eq!(case.log.borrow().starts(), [p1.clone(), p2.clone()], "a failed release waits");
        case.log.borrow_mut().broken.clear();
        case.now += Duration::from_secs(1);
        case.answers(&p2);
        assert_eq!(case.log.borrow().starts(), [p1.clone(), p2.clone(), p2.clone()], "then it starts again");
        assert_eq!(case.current(), b);

        let (_, p3) = publish(&case.target, "device c", b"routing 3");
        case.follow();
        case.now += READY;
        case.step(false);
        assert_eq!(case.current(), b, "services that do not answer in time fail");
        let failed = case.planner(&p3).unwrap().status;
        assert!(matches!(failed, http::Status::Failed(reason) if reason.contains("does not answer")));

        case.log.borrow_mut().exited.insert((p2.clone(), 1));
        case.step(false);
        let ports = case.planner(&p2).unwrap().ports;
        assert!(ports[0].is_some() && ports[1].is_none(), "only the search service of its planner release stops");
        assert_eq!(case.current(), b);
    }

    /// Serve the folder `dir` over HTTP on this machine, as the public origin of an R2 bucket does,
    /// and log the path of each request.
    fn bucket(runtime: &tokio::runtime::Runtime, dir: &Path, log: Arc<Mutex<Vec<String>>>) -> String {
        let logged = axum::middleware::from_fn(move |request: axum::extract::Request, next: axum::middleware::Next| {
            log.lock().unwrap().push(request.uri().path().to_string());
            next.run(request)
        });
        let router = axum::Router::new().fallback_service(tower_http::services::ServeDir::new(dir)).layer(logged);
        format!("http://127.0.0.1:{}", listen(runtime, router))
    }

    fn listen(runtime: &tokio::runtime::Runtime, router: axum::Router) -> u16 {
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let port = listener.local_addr().unwrap().port();
        runtime.spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    /// A case whose follower reads its target over HTTP, installs the files of each release that it
    /// starts, and whose services read the layer `test` in format 1. `api` answers HTTP requests
    /// as `obc data serve` does, from `view`.
    struct Published {
        case: Case,
        gets: Arc<Mutex<Vec<String>>>,
        view: http::Shared,
        api: String,
        runtime: tokio::runtime::Runtime,
    }

    impl Published {
        fn new(name: &str) -> Self {
            let mut case = Case::new(name);
            let (runtime, gets) = (tokio::runtime::Runtime::new().unwrap(), Arc::<Mutex<Vec<String>>>::default());
            let Target::Dir(dir) = &case.target else { unreachable!("a dir target") };
            std::fs::create_dir_all(dir).unwrap();
            case.reader = Target::R2(crate::r2::Remote {
                public: bucket(&runtime, dir, gets.clone()),
                bucket: "test".into(),
                prefix: String::new(),
            });
            let programs = services::Programs::reading(vec![("routing", "test".into(), 1)]);
            case.install = Some((case.scratch.0.join("store"), Arc::new(programs)));
            let view = http::Shared::default();
            let api = format!(
                "http://127.0.0.1:{}",
                listen(&runtime, http::router(view.clone(), None, http::Version::default()))
            );
            Self { case, gets, view, api, runtime }
        }

        fn follow(&mut self) {
            self.case.follow();
            *self.view.write().unwrap() = self.case.follower.view();
        }

        fn answers(&mut self, planner: &str) {
            self.case.answers(planner);
            *self.view.write().unwrap() = self.case.follower.view();
        }

        /// Name `id` in `planner/next.json`.
        fn next(&mut self, id: &str) {
            let next = serde_json::to_vec(&Role { id: id.into(), since: "2026-10-12T00:00:00Z".into() }).unwrap();
            self.case.target.put(NEXT, &next, &Condition::Any).unwrap();
            self.follow();
        }

        /// The status and the body of a GET of `path` of the API.
        fn get(&self, path: &str) -> (u16, String) {
            *self.view.write().unwrap() = self.case.follower.view();
            let agent: ureq::Agent =
                ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into();
            let mut response = agent.get(format!("{}{path}", self.api)).call().unwrap();
            (response.status().as_u16(), response.body_mut().read_to_string().unwrap())
        }
    }

    #[test]
    fn an_r2_target_loads_the_next_release_before_the_switch_and_a_rollback_downloads_nothing() {
        let mut published = Published::new("serve-next");
        let echo = listen(
            &published.runtime,
            axum::Router::new().fallback(|uri: axum::http::Uri| async move { format!("downloads {uri}") }),
        );
        let (a, pa) = publish(&published.case.target, "device a", b"routing a");
        published.case.log.borrow_mut().ports.insert(pa.clone(), echo);
        published.follow();
        published.answers(&pa);
        assert_eq!(published.case.current(), a);
        let work = published.case.scratch.0.join("work");
        assert_eq!(std::fs::read(work.join(&pa).join("routing/pages.bin")).unwrap(), b"routing a", "from the store");

        let (b, pb) = release(&published.case.target, "device b", b"routing b", 1);
        published.next(&b);
        assert_eq!(
            published.get(&format!("/planner-api/releases/{pb}/ready")),
            (503, r#"{"state":"starting"}"#.into())
        );
        published.answers(&pb);
        assert_eq!(published.get(&format!("/planner-api/releases/{pb}/ready")), (200, r#"{"state":"ready"}"#.into()));
        assert_eq!(published.case.current(), a, "no client gets the next release");
        assert_eq!(
            published.get("/planner-offline/catalog?x=1"),
            (200, "downloads /catalog?x=1".into()),
            "the serving one"
        );
        assert_eq!(published.get(&format!("/planner-api/releases/{}/ready", "c".repeat(64))).0, 404);

        published.case.switch(&b);
        assert_eq!(published.case.current(), b, "the switch to the next release is instant");
        published.gets.lock().unwrap().clear();
        published.case.switch(&a);
        assert_eq!(published.case.current(), a);
        assert_eq!(
            *published.gets.lock().unwrap(),
            ["/release.json", "/planner/next.json"],
            "a rollback reads no object"
        );

        published.case.target.remove(NEXT).unwrap();
        let (c, pc) = publish(&published.case.target, "device c", b"routing c");
        published.follow();
        published.answers(&pc);
        assert_eq!(published.case.current(), c);
        assert_eq!(published.case.log.borrow().stops(), std::slice::from_ref(&pb), "a third release stops the oldest");
        assert!(!work.join(&pb).exists(), "and removes its files");
    }

    #[test]
    fn an_r2_target_release_that_cannot_start_says_why() {
        let mut published = Published::new("serve-refuse");
        let (_, pa) = publish(&published.case.target, "device a", b"routing a");
        published.follow();
        published.answers(&pa);

        let (b, pb) = release(&published.case.target, "device b", b"routing b", 1);
        let Target::Dir(dir) = &published.case.target else { unreachable!("a dir target") };
        let object = dir.join(format!("objects/{}", sha256_hex(b"routing b")));
        std::fs::remove_file(&object).unwrap();
        std::fs::write(&object, b"other bytes").unwrap();
        published.next(&b);
        let (status, body) = published.get(&format!("/planner-api/releases/{pb}/ready"));
        assert_eq!(status, 503);
        assert!(body.contains(r#""reason":"not ready: "#) && body.contains("SHA-256"), "{body}");

        let (c, pc) = release(&published.case.target, "device c", b"routing c", 2);
        published.next(&c);
        let body: Value = serde_json::from_str(&published.get(&format!("/planner-api/releases/{pc}/ready")).1).unwrap();
        let reason = "not ready: needs a routing service that reads test format 2; it reads 1";
        assert_eq!(body, json!({"state": "failed", "reason": reason}));
        assert_eq!(published.case.log.borrow().starts(), [pa], "neither release starts");
    }

    #[test]
    fn a_slow_download_holds_back_no_other_release() {
        let mut published = Published::new("serve-slow");
        let (r, pr) = publish(&published.case.target, "device r", b"routing r");
        published.follow();
        published.answers(&pr);
        let (_, ps) = publish(&published.case.target, "device s", b"routing s");
        published.follow();
        published.answers(&ps);

        let (n, pn) = release(&published.case.target, "device n", b"routing n", 1);
        let held = published.case.hold(&pn);
        published.next(&n);
        let ready =
            |published: &Published, planner: &str| published.get(&format!("/planner-api/releases/{planner}/ready"));
        assert_eq!(ready(&published, &ps), (200, r#"{"state":"ready"}"#.into()), "the serving release answers");
        assert_eq!(ready(&published, &pn).0, 503, "the next release downloads");
        published.case.switch(&r);
        assert_eq!(published.case.current(), r, "a change of release.json is seen during the download");
        published.case.log.borrow_mut().exited.insert((ps.clone(), 1));
        published.case.step(false);
        let failed = published.case.planner(&ps).unwrap().status;
        assert!(matches!(failed, http::Status::Failed(_)), "and a service that stops");

        // The publish gives up during the download: its release never starts.
        published.case.target.remove(NEXT).unwrap();
        published.follow();
        published.case.release(&pn, held);
        while !published.case.follower.abandoned.iter().all(JoinHandle::is_finished) {
            std::thread::sleep(Duration::from_millis(1));
        }
        published.case.step(false);
        assert!(!published.case.log.borrow().starts().contains(&pn), "a download that no release needs starts nothing");
        assert_eq!(ready(&published, &pn).0, 404);
    }

    #[test]
    fn a_next_release_that_cannot_be_read_holds_back_nothing() {
        let mut case = Case::new("serve-bad-next");
        let (s, ps) = publish(&case.target, "device s", b"routing s");
        for next in
            [b"not json".to_vec(), serde_json::to_vec(&Role { id: "f".repeat(64), since: String::new() }).unwrap()]
        {
            case.target.put(NEXT, &next, &Condition::Any).unwrap();
            case.follow();
            case.answers(&ps);
            assert_eq!(case.current(), s, "the serving release loads");
            assert_eq!(case.log.borrow().starts(), std::slice::from_ref(&ps), "and no next release");
        }
        let (n, pn) = release(&case.target, "device n", b"routing n", 1);
        case.target
            .put(NEXT, &serde_json::to_vec(&Role { id: n, since: String::new() }).unwrap(), &Condition::Any)
            .unwrap();
        case.follow();
        assert_eq!(case.log.borrow().starts(), [ps, pn], "a next release that can be read loads");
    }

    #[test]
    fn the_store_of_a_follower_keeps_only_what_the_releases_of_the_target_read() {
        let mut published = Published::new("serve-store");
        let store = published.case.scratch.0.join("store");
        let held = || -> BTreeSet<String> {
            let prefixes = std::fs::read_dir(store.join("objects")).unwrap();
            let objects = prefixes.flat_map(|prefix| std::fs::read_dir(prefix.unwrap().path()).unwrap());
            objects.map(|object| object.unwrap().file_name().into_string().unwrap()).collect()
        };
        let target = published.case.target.clone();
        let reader = published.case.scratch.0.join("reader");
        let reads = |releases: &[(&str, &[u8])]| -> BTreeSet<String> {
            let read = |(id, routing): &(&str, &[u8])| {
                let reader = Store::at(&reader).reading(&target);
                let named = Release::read(&target, id).unwrap().named_files(in_store(&reader)).unwrap();
                [named[0].sha256.clone(), sha256_hex(routing)]
            };
            releases.iter().flat_map(read).collect()
        };
        let (a, pa) = publish(&target, "device a", b"routing a");
        published.follow();
        published.answers(&pa);
        let (b, pb) = publish(&target, "device b", b"routing b");
        published.follow();
        published.answers(&pb);
        let (c, _) = release(&target, "device c", b"routing c", 1);
        published.next(&c);
        assert_eq!(held(), reads(&[(&a, b"routing a"), (&b, b"routing b"), (&c, b"routing c")]));

        // An unfinished download of an object that a release reads stays; another goes.
        let Target::R2(remote) = &published.case.reader else { unreachable!("an R2 target") };
        let part =
            format!("{}.part", crate::fetch::http::key(&remote.url(&format!("objects/{}", sha256_hex(b"routing b")))));
        let partial = store.join("partial");
        std::fs::create_dir_all(&partial).unwrap();
        for name in [part.as_str(), "0123.part"] {
            std::fs::write(partial.join(name), b"x").unwrap();
        }
        let (d, pd) = publish(&target, "device d", b"routing d");
        published.follow();
        published.answers(&pd);
        assert_eq!(held(), reads(&[(&b, b"routing b"), (&c, b"routing c"), (&d, b"routing d")]), "not those of a");
        assert!(partial.join(&part).exists() && !partial.join("0123.part").exists());

        target.remove(NEXT).unwrap();
        published.follow();
        assert_eq!(held(), reads(&[(&b, b"routing b"), (&d, b"routing d")]));
    }

    #[test]
    fn an_abandoned_download_ends_before_the_store_is_pruned_and_the_next_download_starts() {
        let mut case = Case::new("serve-join");
        let (_, pa) = publish(&case.target, "device a", b"routing a");
        case.follow();
        case.answers(&pa);
        let (_, pb) = release(&case.target, "device b", b"routing b", 1);
        let held = case.hold(&pb);
        publish(&case.target, "device b", b"routing b");
        case.follow();
        let pruned = case.log.borrow().pruned;
        publish(&case.target, "device c", b"routing c");
        let (d, pd) = publish(&case.target, "device d", b"routing d");
        case.follow();
        assert_eq!(case.log.borrow().pruned, pruned, "the download of b defers the prune");
        assert!(case.planner(&pd).is_none(), "and the next download");

        case.release(&pb, held);
        while !case.follower.abandoned.iter().all(JoinHandle::is_finished) {
            std::thread::sleep(Duration::from_millis(1));
        }
        case.answers(&pd);
        assert!(case.log.borrow().pruned > pruned, "the prune waited for the download of b");
        assert_eq!(case.current(), d);
    }

    /// Each file and folder under `dir` with its inode and its time of change: an install that
    /// writes again gives other values.
    fn writes(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
        use std::os::unix::fs::MetadataExt;
        let mut all = BTreeMap::new();
        let mut folders = vec![dir.to_path_buf()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let path = entry.unwrap().path();
                let metadata = std::fs::symlink_metadata(&path).unwrap();
                if metadata.is_dir() {
                    folders.push(path.clone());
                }
                all.insert(path, (metadata.ino(), metadata.modified().unwrap()));
            }
        }
        all
    }

    /// Whether the follower asked the target for the document or a file object of a planner release.
    fn fetched(gets: &[String], planner: &str, routing: &[u8]) -> bool {
        let objects = [format!("/releases/{planner}/release.json"), format!("/objects/{}", sha256_hex(routing))];
        gets.iter().any(|get| objects.contains(get))
    }

    #[test]
    fn an_install_stays_across_a_restart_and_only_an_unmarked_one_installs_again() {
        let mut published = Published::new("serve-restart");
        let (a, pa) = publish(&published.case.target, "device a", b"routing a");
        published.follow();
        published.answers(&pa);
        let dir = published.case.install(&pa);
        let installed = writes(&dir);
        assert!(services::installed(&dir) && dir.join("release.json").is_file());

        published.case.restart();
        published.gets.lock().unwrap().clear();
        published.follow();
        published.answers(&pa);
        assert_eq!(published.case.current(), a);
        assert!(!fetched(&published.gets.lock().unwrap(), &pa, b"routing a"), "no object request");
        assert_eq!(writes(&dir), installed, "and no write");

        std::fs::remove_file(dir.join("installed")).unwrap();
        published.case.restart();
        published.follow();
        published.answers(&pa);
        assert_eq!(published.case.current(), a);
        assert!(services::installed(&dir), "a folder without a marker installs again");
        assert_ne!(writes(&dir), installed);
        assert_eq!(std::fs::read(dir.join("routing/pages.bin")).unwrap(), b"routing a");
    }

    #[test]
    fn a_stopped_service_restarts_alone_and_keeps_its_files() {
        let mut published = Published::new("serve-restart-service");
        let echo = listen(
            &published.runtime,
            axum::Router::new().fallback(|uri: axum::http::Uri| async move { format!("{uri}") }),
        );
        let (_, pa) = publish(&published.case.target, "device a", b"routing a");
        published.case.log.borrow_mut().ports.insert(pa.clone(), echo);
        published.follow();
        published.answers(&pa);
        let get =
            |published: &Published, service: &str| published.get(&format!("/planner-api/releases/{pa}/{service}/x"));
        let searches =
            |published: &Published| published.case.log.borrow().started.iter().filter(|(_, at)| *at == 1).count();
        let busy = (503, r#"{"code":"busy","message":"The search service restarts. Try again shortly."}"#.to_string());

        published.case.now += BACKOFF;
        published.case.log.borrow_mut().exited.insert((pa.clone(), 1));
        published.case.step(false);
        assert_eq!(get(&published, "routing"), (200, "/x".into()), "routing keeps answering");
        assert_eq!(get(&published, "search"), busy);
        published.case.log.borrow_mut().exited.clear();
        published.case.step(false);
        assert_eq!(
            get(&published, "search"),
            (200, "/api/planner-search/x".into()),
            "a service that ran 60 s starts again at once"
        );
        assert_eq!((searches(&published), published.case.log.borrow().starts()), (2, vec![pa.clone()]));
        assert!(services::installed(&published.case.install(&pa)), "its files stay");

        published.case.log.borrow_mut().exited.insert((pa.clone(), 1));
        published.case.step(false);
        published.case.log.borrow_mut().exited.clear();
        published.case.step(false);
        assert_eq!(get(&published, "search"), busy, "a second exit within 60 s waits the back-off");
        assert_eq!(searches(&published), 2);
        published.case.now += BACKOFF;
        published.case.step(false);
        assert_eq!(searches(&published), 3);
        assert_eq!(get(&published, "search").0, 200);
    }

    #[test]
    fn during_an_update_the_rollback_stops_and_keeps_its_files() {
        let mut published = Published::new("serve-update");
        let (_, pa) = publish(&published.case.target, "device a", b"routing a");
        published.follow();
        published.answers(&pa);
        let (b, pb) = publish(&published.case.target, "device b", b"routing b");
        published.follow();
        published.answers(&pb);
        let (c, pc) = release(&published.case.target, "device c", b"routing c", 1);
        published.next(&c);
        published.answers(&pc);
        let ready =
            |published: &Published, planner: &str| published.get(&format!("/planner-api/releases/{planner}/ready"));
        assert_eq!(ready(&published, &pb).0, 200, "the serving release runs");
        assert_eq!(ready(&published, &pc).0, 200, "and the next one");
        assert_eq!(ready(&published, &pa), (503, r#"{"state":"installed"}"#.into()), "the rollback stops");
        assert!(published.case.log.borrow().stops().contains(&pa));
        assert!(services::installed(&published.case.install(&pa)), "and keeps its files");

        published.case.restart();
        published.gets.lock().unwrap().clear();
        published.follow();
        assert_eq!(published.case.current(), b);
        assert_eq!(ready(&published, &pc).0, 200);
        assert_eq!(ready(&published, &pa).0, 404, "the rollback does not fetch during the update");

        published.case.target.remove(NEXT).unwrap();
        published.follow();
        published.answers(&pa);
        assert_eq!(ready(&published, &pa).0, 200, "after it the rollback starts");
        assert!(!fetched(&published.gets.lock().unwrap(), &pa, b"routing a"), "with no object request");
    }

    #[test]
    fn an_install_that_does_not_fit_deletes_the_rollback_once() {
        let mut published = Published::new("serve-room");
        let (_, pa) = publish(&published.case.target, "device a", b"routing a");
        published.follow();
        published.answers(&pa);
        let (_, pb) = publish(&published.case.target, "device b", b"routing b");
        published.follow();
        published.answers(&pb);
        let rollback = published.case.install(&pa);
        *published.case.free.lock().unwrap() = Some((rollback.clone(), 1_500_000_000, 2_500_000_000));
        SAID.with(|said| said.borrow_mut().clear());

        let (c, pc) = release(&published.case.target, "device c", b"routing c", 1);
        published.next(&c);
        assert!(!rollback.exists(), "the files of the rollback go");
        let warning = format!(
            "WARNING: no rollback during the install of {}: deleted the files of planner release {}; the disk has 1.5 GB \
             free and the install needs 2.0 GB",
            short(&pc),
            short(&pa)
        );
        assert!(SAID.with(|said| said.borrow().contains(&warning)), "{:?}", SAID.with(|said| said.borrow().clone()));
        published.case.step(false);
        published.answers(&pc);
        let ready =
            |published: &Published, planner: &str| published.get(&format!("/planner-api/releases/{planner}/ready"));
        assert_eq!(ready(&published, &pc).0, 200, "the next release installs");
        published.case.step(false);
        assert_eq!(ready(&published, &pa).0, 404, "the rollback does not fetch while the next release is named");

        *published.case.free.lock().unwrap() = Some((rollback, 500_000_000, 500_000_000));
        let (d, pd) = release(&published.case.target, "device d", b"routing d", 1);
        published.next(&d);
        let (status, body) = ready(&published, &pd);
        let reason = "not ready: the disk has 0.5 GB free; the install needs 2.0 GB";
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!((status, body), (503, json!({"state": "failed", "reason": reason})), "with no rollback it fails");
    }
}
