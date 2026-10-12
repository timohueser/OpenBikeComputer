//! `obc data serve`: run the planner services of the releases that a target names, and give the
//! apps a dir target. The services of a new release start beside those of the current one, and
//! clients get the new release once its services answer. The services of the serving release, of
//! its rollback and of the release of `planner/next.json` stay loaded, so a rollback, and a switch
//! that waited for the next release, are instant.

pub mod http;
pub mod services;

use std::collections::{BTreeMap, BTreeSet};
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

/// The bytes of `release.json` and of `planner/next.json` of a target: the releases that it names.
type Names = [Option<Vec<u8>>; 2];

/// The releases that a target names.
#[derive(Default)]
pub struct Wanted {
    /// The release that it serves, then its rollback.
    pub serving: Vec<Served>,
    /// The release of `planner/next.json`, unless it is one of `serving`. No client gets it.
    pub next: Option<Served>,
}

fn names(target: &Target) -> Result<Names, String> {
    Ok([target.get(SERVING)?, target.get(NEXT)?])
}

/// The releases that `names` of `target` name. `known` gives a release that the follower read
/// before, so a rollback reads nothing but `release.json`.
fn wanted(target: &Target, names: &Names, known: impl Fn(&str) -> Option<Served>) -> Result<Wanted, String> {
    let read = |id: &str| known(id).map_or_else(|| served(target, id), Ok);
    let mut wanted = Wanted::default();
    if let Some(bytes) = &names[0] {
        let roles: Serving = serde_json::from_slice(bytes).map_err(|e| format!("{SERVING}: {e}"))?;
        let ids = std::iter::once(&roles.serving).chain(&roles.rollback);
        wanted.serving = ids.map(|role| read(&role.id)).collect::<Result<_, _>>()?;
    }
    if let Some(bytes) = &names[1] {
        let next: Role = serde_json::from_slice(bytes).map_err(|e| format!("{NEXT}: {e}"))?;
        if !wanted.serving.iter().any(|served| served.id == next.id) {
            wanted.next = Some(read(&next.id)?);
        }
    }
    Ok(wanted)
}

fn served(target: &Target, id: &str) -> Result<Served, String> {
    let release = Release::read(target, id)?;
    let mut pointers = BTreeMap::new();
    for pointer in &release.pointers {
        let key = format!("objects/{}", pointer.sha256);
        let bytes = target.get(&key)?.filter(|bytes| sha256_hex(bytes) == pointer.sha256);
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
    error: Option<String>,
}

/// Read what `target` names. When it changed since `seen`, the follower wants its releases; else
/// it polls. An error leaves the releases as they are, and the next look reads again.
pub fn look<S: Services>(
    follower: &mut Follower<S>,
    target: &Target,
    seen: &mut Seen,
    start: Start<'_, S>,
    now: &dyn Fn() -> Instant,
) {
    let read = names(target).and_then(|names| match seen.names.as_ref() == Some(&names) {
        true => Ok(None),
        false => wanted(target, &names, |id| follower.known(id)).map(|wanted| Some((names, wanted))),
    });
    match read {
        Ok(None) => follower.poll(start, now),
        Ok(Some((names, wanted))) => {
            if names[0].is_none() {
                eprintln!("obc data serve: the target serves no release yet; `obc data update` makes one");
            }
            follower.want(wanted, start, now);
            *seen = Seen { names: Some(names), error: None };
        }
        Err(e) => {
            if seen.error.as_ref() != Some(&e) {
                eprintln!("obc data serve: {e}; the current release stays");
                seen.error = Some(e);
            }
            follower.poll(start, now);
        }
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

/// Install the files of a release and start its services. The follower waits for it.
pub type Start<'a, S> = &'a mut dyn FnMut(&Served) -> Result<S, String>;

enum State<S> {
    /// `backoff` is the wait after a failure.
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
}

impl<S: Services> Default for Follower<S> {
    fn default() -> Self {
        Self { planners: BTreeMap::new(), wanted: Wanted::default(), current: None }
    }
}

impl<S: Services> Follower<S> {
    /// Follow `wanted`, then [`Self::poll`]. The planner releases that failed start again at once.
    pub fn want(&mut self, wanted: Wanted, start: Start<'_, S>, now: &dyn Fn() -> Instant) {
        self.planners.retain(|_, state| !matches!(state, State::Failed { .. }));
        self.wanted = wanted;
        self.poll(start, now);
        if let Some(current) = self.current.as_ref().filter(|current| !self.wanted.serving.contains(current)) {
            eprintln!(
                "obc data serve: WARNING: clients get release {}, which release.json no longer names. An update \
                 can delete its files. Fix: run `obc data update` until a release loads.",
                short(&current.id)
            );
        }
    }

    /// `start` the services of each wanted planner release that does not run, or that failed and
    /// waited; the time to answer counts from the return of `start`. Check the services that
    /// start, and fail a planner release whose service stopped. Clients get the serving release of
    /// the target once its services answer; until then they keep the current release. Stop the
    /// services that neither the wanted releases nor the current one need.
    pub fn poll(&mut self, start: Start<'_, S>, now: &dyn Fn() -> Instant) {
        for served in self.wanted.serving.iter().chain(&self.wanted.next) {
            let backoff = match self.planners.get(&served.planner) {
                None => BACKOFF,
                Some(State::Failed { retry, backoff, .. }) if *retry <= now() => *backoff,
                Some(_) => continue,
            };
            let state = match start(served) {
                Ok(services) => State::Starting { services, since: now(), backoff },
                Err(e) => failed(&served.planner, e, now(), backoff),
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
                        let started = std::mem::replace(state, placeholder);
                        let State::Starting { services, .. } = started else { unreachable!("the state starts") };
                        *state = State::Ready(services);
                    }
                },
                State::Ready(services) => {
                    if let Some(e) = services.stopped() {
                        *state = failed(id, e, now, BACKOFF);
                    }
                }
                State::Failed { .. } => {}
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
        self.planners.retain(|id, _| needed.contains(id));
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
                State::Starting { .. } => http::Planner::Starting,
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
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

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
        /// The store and the programs of a start that installs the files of a release.
        install: Option<(Store, services::Programs)>,
    }

    impl Case {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let target = Target::Dir(scratch.0.join("target"));
            let (log, follower, seen, now) = (Rc::default(), Follower::default(), Seen::default(), Instant::now());
            Self { reader: target.clone(), scratch, target, log, follower, seen, now, install: None }
        }

        /// Look at what the target names, or only poll.
        fn step(&mut self, look_at_target: bool) {
            let Self { scratch, reader, log, follower, seen, now, install, .. } = self;
            let (now, work) = (*now, scratch.0.join("work"));
            let mut start = |served: &Served| {
                let installed = match install {
                    Some((store, programs)) => Some(services::install(programs, store, reader, &work, served)?.0),
                    None => None,
                };
                log.borrow_mut().started.push(served.planner.clone());
                Ok(Fake { planner: served.planner.clone(), log: log.clone(), _installed: installed })
            };
            match look_at_target {
                true => look(follower, reader, seen, &mut start, &|| now),
                false => follower.poll(&mut start, &|| now),
            }
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
            case.reader = Target::R2 { public: bucket(&runtime, dir, gets.clone()) };
            let programs = services::Programs::reading(vec![("routing", "test".into(), 1)]);
            case.install = Some((Store::at(case.scratch.0.join("store")), programs));
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
}
