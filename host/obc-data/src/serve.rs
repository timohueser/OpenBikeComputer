//! `obc data serve`: run the planner services of the releases that a target names, and give the
//! apps a dir target. The services of a new release start beside those of the current one, and
//! clients get the new release once its services answer. The services of the serving release, of
//! its rollback and of the release of `planner/next.json` stay loaded, so a rollback, and a switch
//! that waited for the next release, are instant.

pub mod http;
pub mod services;

use std::collections::{BTreeMap, BTreeSet};
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

/// How long a failed planner release waits before it starts again. Each failure in a row doubles
/// the wait, up to [`BACKOFF_MAX`]: a download that broke soon tries again, and a release that
/// cannot start, such as one in a format that the services do not read, logs at most twice an hour.
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

fn served(target: &Target, id: &str) -> Result<Served, String> {
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
    let document = release.named.iter().find(|file| file.key == key).cloned();
    let document = document.ok_or_else(|| format!("release {id} has no {key}"))?;
    let formats = release.outputs.iter().map(|output| (output.layer.clone(), output.format)).collect();
    Ok(Served { id: id.into(), planner: planner.into(), document, pointers, formats })
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
pub fn look<S: Services>(
    follower: &mut Follower<S>,
    target: &Target,
    seen: &mut Seen,
    load: &mut dyn Load<Services = S>,
    now: &dyn Fn() -> Instant,
) {
    let read = names(target).and_then(|names| match seen.whole && seen.names.as_ref() == Some(&names) {
        true => Ok(None),
        false => wanted(target, &names, |id| follower.known(id)).map(|wanted| Some((names, wanted))),
    });
    let problem = match read {
        Ok(Some((names, (wanted, problem)))) => {
            if names[0].is_none() && seen.names.as_ref().is_none_or(|seen| seen[0].is_some()) {
                eprintln!("obc data serve: the target names no release yet");
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
            eprintln!("obc data serve: {problem}");
        }
        seen.error = problem;
    }
}

/// The services of a planner release. They stop when they drop.
pub trait Services {
    /// Whether each service answers; an error when one stopped.
    fn ready(&mut self) -> Result<bool, String>;
    /// Why a service stopped; `None` while each runs.
    fn stopped(&mut self) -> Option<String>;
    /// The port of each of the [`services::SERVICES`] on this machine.
    fn ports(&self) -> [u16; services::SERVICES.len()];
}

/// How the follower loads the planner release of a release.
pub trait Load {
    type Services: Services;
    /// Give the store what the services of `served` read, on a thread of its own: a download can
    /// take long, and the follower keeps polling. An error when the system gives no thread.
    fn fetch(&mut self, served: &Served) -> Result<JoinHandle<Result<(), String>>, String>;
    /// Install the files of `served` from the store and start its services. The follower waits for
    /// it.
    fn start(&mut self, served: &Served) -> Result<Self::Services, String>;
}

enum State<S> {
    /// Its objects download. `backoff` is the wait after a failure.
    Fetching {
        worker: JoinHandle<Result<(), String>>,
        backoff: Duration,
    },
    Starting {
        services: S,
        since: Instant,
        backoff: Duration,
    },
    Ready(S),
    /// Its services stopped, for this reason. It starts again at `retry`, or at once when the
    /// target names other releases.
    Failed {
        reason: String,
        retry: Instant,
        backoff: Duration,
    },
}

/// The planner releases whose services run, and the release that clients get.
pub struct Follower<S> {
    /// By planner release id: those of the wanted releases and of the current one.
    planners: BTreeMap<String, State<S>>,
    wanted: Wanted,
    current: Option<Served>,
    /// The downloads of planner releases that no release names any more.
    abandoned: Vec<JoinHandle<Result<(), String>>>,
}

impl<S: Services> Default for Follower<S> {
    fn default() -> Self {
        Self { planners: BTreeMap::new(), wanted: Wanted::default(), current: None, abandoned: Vec::new() }
    }
}

impl<S: Services> Follower<S> {
    /// Follow `wanted`, then [`Self::poll`]. The planner releases that failed start again at once.
    pub fn want(&mut self, wanted: Wanted, load: &mut dyn Load<Services = S>, now: &dyn Fn() -> Instant) {
        self.planners.retain(|_, state| !matches!(state, State::Failed { .. }));
        self.wanted = wanted;
        self.poll(load, now);
        if let Some(current) = self.current.as_ref().filter(|current| !self.wanted.serving.contains(current)) {
            eprintln!(
                "obc data serve: WARNING: clients get release {}, which release.json no longer names. An update \
                 can delete its files. Fix: run `obc data update` until a release loads.",
                short(&current.id)
            );
        }
    }

    /// Fetch the objects of each wanted planner release that does not run, or that failed and
    /// waited, and start its services once they are in the store; the time to answer counts from
    /// the return of the start. Check the services that start, and fail a planner release whose
    /// service stopped. Clients get the serving release of the target once its services answer;
    /// until then they keep the current release. Stop the services that neither the wanted
    /// releases nor the current one need.
    pub fn poll(&mut self, load: &mut dyn Load<Services = S>, now: &dyn Fn() -> Instant) {
        for served in self.wanted.serving.iter().chain(&self.wanted.next) {
            let mut fetch = |backoff| match load.fetch(served) {
                Ok(worker) => State::Fetching { worker, backoff },
                Err(e) => failed(&served.planner, e, now(), backoff),
            };
            let state = match self.planners.remove(&served.planner) {
                None => fetch(BACKOFF),
                Some(State::Failed { retry, backoff, .. }) if retry <= now() => fetch(backoff),
                Some(State::Fetching { worker, backoff }) if worker.is_finished() => {
                    let fetched = worker.join().unwrap_or_else(|_| Err("the download stopped".into()));
                    match fetched.and_then(|()| load.start(served)) {
                        Ok(services) => State::Starting { services, since: now(), backoff },
                        Err(e) => failed(&served.planner, e, now(), backoff),
                    }
                }
                Some(state) => state,
            };
            self.planners.insert(served.planner.clone(), state);
        }
        let now = now();
        for (id, state) in &mut self.planners {
            match state {
                State::Starting { services, since, backoff } => match services.ready() {
                    Ok(false) if now.duration_since(*since) < READY => {}
                    Ok(false) => {
                        let reason = format!("its services do not answer after {} s", READY.as_secs());
                        *state = failed(id, reason, now, *backoff);
                    }
                    Err(e) => *state = failed(id, e, now, *backoff),
                    Ok(true) => {
                        let placeholder = State::Failed { reason: String::new(), retry: now, backoff: BACKOFF };
                        let State::Starting { services, .. } = std::mem::replace(state, placeholder) else {
                            unreachable!("the state starts")
                        };
                        *state = State::Ready(services);
                    }
                },
                State::Ready(services) => {
                    if let Some(e) = services.stopped() {
                        *state = failed(id, e, now, BACKOFF);
                    }
                }
                State::Fetching { .. } | State::Failed { .. } => {}
            }
        }
        if let Some(next) = self.wanted.serving.first().filter(|next| self.current.as_ref() != Some(*next)) {
            if let Some(State::Ready(_)) = self.planners.get(&next.planner) {
                eprintln!("obc data serve: release {} with planner release {}", short(&next.id), short(&next.planner));
                self.current = Some(next.clone());
            }
        }
        let wanted = self.wanted.serving.iter().chain(&self.wanted.next).chain(&self.current);
        let needed: BTreeSet<String> = wanted.map(|served| served.planner.clone()).collect();
        let gone: Vec<String> = self.planners.keys().filter(|id| !needed.contains(*id)).cloned().collect();
        for id in gone {
            if let Some(State::Fetching { worker, .. }) = self.planners.remove(&id) {
                self.abandoned.push(worker);
            }
        }
        self.abandoned.retain(|worker| !worker.is_finished());
    }

    /// Wait for each download, after [`crate::fetch::http::stop`] ended it: a stopped serve
    /// writes nothing more.
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

    /// What the HTTP side gives clients: the current release, and what the services of each
    /// planner release do.
    pub fn view(&self) -> http::View {
        let planners = self.planners.iter().map(|(id, state)| {
            let planner = match state {
                State::Fetching { .. } | State::Starting { .. } => http::Planner::Starting,
                State::Ready(services) => http::Planner::Ready(services.ports()),
                State::Failed { reason, .. } => http::Planner::Failed(reason.clone()),
            };
            (id.clone(), planner)
        });
        http::View { current: self.current.clone(), planners: planners.collect() }
    }
}

/// The state of a planner release that failed at `now`, and waits `backoff`.
fn failed<S>(planner: &str, reason: String, now: Instant, backoff: Duration) -> State<S> {
    let (planner, wait) = (short(planner), backoff.as_secs());
    eprintln!("obc data serve: planner release {planner} failed: {reason}; it starts again in {wait} s");
    State::Failed { reason, retry: now + backoff, backoff: (backoff * 2).min(BACKOFF_MAX) }
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::{mpsc, Arc, Mutex};

    use serde_json::json;

    use super::*;
    use crate::engine::LayerFile;
    use crate::release::{manifest_key, serving, switch};
    use crate::store::tests::Scratch;
    use crate::store::Store;
    use crate::target::Condition;

    /// What the fake services of each planner release do, and what happened to them.
    #[derive(Default)]
    struct Log {
        ready: BTreeSet<String>,
        broken: BTreeSet<String>,
        exited: BTreeSet<String>,
        ports: BTreeMap<String, u16>,
        /// The planner releases whose download waits until the test sends on the channel.
        held: BTreeMap<String, mpsc::Receiver<()>>,
        holding: BTreeSet<String>,
        started: Vec<String>,
        stopped: Vec<String>,
    }

    /// The fake services of a planner release, and the files that its start installed.
    struct Fake {
        planner: String,
        log: Rc<RefCell<Log>>,
        _installed: Option<services::Processes>,
    }

    impl Services for Fake {
        fn ready(&mut self) -> Result<bool, String> {
            let log = self.log.borrow();
            match log.broken.contains(&self.planner) {
                true => Err("the routing service stopped".into()),
                false => Ok(log.ready.contains(&self.planner)),
            }
        }

        fn stopped(&mut self) -> Option<String> {
            let exited = self.log.borrow().exited.contains(&self.planner);
            exited.then(|| "the search service stopped: exit status 1".into())
        }

        fn ports(&self) -> [u16; services::SERVICES.len()] {
            [self.log.borrow().ports.get(&self.planner).copied().unwrap_or(1); 3]
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.log.borrow_mut().stopped.push(self.planner.clone());
        }
    }

    /// Upload a release of the layer `test` in `format`: its device pointer is `device`, and its
    /// planner release has the routing file `routing`. Give its id and the id of the planner release.
    fn release(target: &Target, device: &str, routing: &[u8], format: u32) -> (String, String) {
        let sha256 = sha256_hex(routing);
        let entry =
            json!({"bytes": routing.len(), "sha256": sha256, "transport": {"sha256": sha256, "encoding": "identity"}});
        let document = serde_json::to_vec(&json!({"region": "test", "files": {"routing/pages.bin": entry}})).unwrap();
        let planner = crate::engine::digest([("release.json", sha256_hex(&document).as_str())]);
        let origins = r#"{"api_origin":"https://api.test","objects_origin":"https://objects.test"}"#;
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
        release.pointers =
            POINTERS.iter().zip(&output.files).map(|(key, layer)| file(key.to_string(), layer)).collect();
        release.named = vec![file(format!("releases/{planner}/release.json"), &output.files[2])];
        for bytes in &files {
            target.put(&format!("objects/{}", sha256_hex(bytes)), bytes, &Condition::Any).unwrap();
        }
        target.put(&release.named[0].key, &files[2], &Condition::Any).unwrap();
        let id = release.id();
        target.put(&manifest_key(&id), &release.canonical(), &Condition::Any).unwrap();
        (id, planner)
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
    }

    /// The load of the fake services: a download that the test can hold, and with `install` the
    /// real download and install.
    struct Fakes<'a> {
        log: &'a Rc<RefCell<Log>>,
        install: &'a Option<(PathBuf, Arc<services::Programs>)>,
        reader: &'a Target,
        work: PathBuf,
    }

    impl Load for Fakes<'_> {
        type Services = Fake;

        fn fetch(&mut self, served: &Served) -> Result<JoinHandle<Result<(), String>>, String> {
            let held = self.log.borrow_mut().held.remove(&served.planner);
            let real =
                self.install.clone().map(|(store, programs)| (store, programs, self.reader.clone(), served.clone()));
            Ok(std::thread::spawn(move || {
                if let Some(held) = held {
                    let _ = held.recv();
                }
                match real {
                    Some((store, programs, target, served)) => {
                        services::fetch(&programs, &Store::at(store), &target, &served)
                    }
                    None => Ok(()),
                }
            }))
        }

        fn start(&mut self, served: &Served) -> Result<Fake, String> {
            let installed = match self.install {
                Some((store, _)) => Some(services::install(&Store::at(store.clone()), &self.work, served)?.0),
                None => None,
            };
            self.log.borrow_mut().started.push(served.planner.clone());
            Ok(Fake { planner: served.planner.clone(), log: self.log.clone(), _installed: installed })
        }
    }

    impl Case {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let target = Target::Dir(scratch.0.join("target"));
            let (log, follower, seen, now) = (Rc::default(), Follower::default(), Seen::default(), Instant::now());
            Self { reader: target.clone(), scratch, target, log, follower, seen, now, install: None }
        }

        /// Look at what the target names, or only poll; then poll until each download that the
        /// test does not hold ended and the follower took it.
        fn step(&mut self, look_at_target: bool) {
            let Self { scratch, reader, log, follower, seen, now, install, .. } = self;
            let now = *now;
            let mut load = Fakes { log, install, reader, work: scratch.0.join("work") };
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
            let ready = view.planners.into_iter().filter(|(_, planner)| matches!(planner, http::Planner::Ready(_)));
            ready.map(|(id, _)| id).collect::<BTreeSet<_>>()
        };
        assert_eq!(ready(&case), [p1.clone(), p2.clone()].into(), "the rollback runs");

        // A release of the same planner release, such as one of `--only device/map`, switches at once.
        let (c, _) = publish(&case.target, "device c", b"routing 2");
        case.follow();
        assert_eq!(case.current(), c);
        assert_eq!(case.log.borrow().started, [p1.clone(), p2.clone()]);
        assert_eq!(
            case.log.borrow().stopped,
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
        assert_eq!(case.log.borrow().started, [p1, p2, p3]);
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
        assert_eq!(case.log.borrow().stopped, std::slice::from_ref(&p2), "the services of a failed release stop");
        case.now += BACKOFF - Duration::from_secs(1);
        case.step(false);
        assert_eq!(case.log.borrow().started, [p1.clone(), p2.clone()], "a failed release waits");
        case.log.borrow_mut().broken.clear();
        case.now += Duration::from_secs(1);
        case.answers(&p2);
        assert_eq!(case.log.borrow().started, [p1.clone(), p2.clone(), p2.clone()], "then it starts again");
        assert_eq!(case.current(), b);

        let (_, p3) = publish(&case.target, "device c", b"routing 3");
        case.follow();
        case.now += READY;
        case.step(false);
        assert_eq!(case.current(), b, "services that do not answer in time fail");
        assert!(matches!(case.planner(&p3), Some(http::Planner::Failed(reason)) if reason.contains("do not answer")));

        case.log.borrow_mut().exited.insert(p2.clone());
        case.step(false);
        let stopped = "the search service stopped: exit status 1".to_string();
        assert_eq!(case.planner(&p2), Some(http::Planner::Failed(stopped)), "the API of its planner release ends");
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
            let api = format!("http://127.0.0.1:{}", listen(&runtime, http::router(view.clone(), None)));
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
        assert_eq!(published.case.log.borrow().stopped, std::slice::from_ref(&pb), "a third release stops the oldest");
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
        assert_eq!(published.case.log.borrow().started, [pa], "neither release starts");
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
        published.case.log.borrow_mut().exited.insert(ps.clone());
        published.case.step(false);
        assert!(matches!(published.case.planner(&ps), Some(http::Planner::Failed(_))), "and a service that stops");

        // The publish gives up during the download: its release never starts.
        published.case.target.remove(NEXT).unwrap();
        published.follow();
        published.case.release(&pn, held);
        while !published.case.follower.abandoned.iter().all(JoinHandle::is_finished) {
            std::thread::sleep(Duration::from_millis(1));
        }
        published.case.step(false);
        assert!(!published.case.log.borrow().started.contains(&pn), "a download that no release needs starts nothing");
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
            assert_eq!(case.log.borrow().started, std::slice::from_ref(&ps), "and no next release");
        }
        let (n, pn) = release(&case.target, "device n", b"routing n", 1);
        case.target
            .put(NEXT, &serde_json::to_vec(&Role { id: n, since: String::new() }).unwrap(), &Condition::Any)
            .unwrap();
        case.follow();
        assert_eq!(case.log.borrow().started, [ps, pn], "a next release that can be read loads");
    }
}
