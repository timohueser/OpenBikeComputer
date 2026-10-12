//! `obc data serve`: give the apps a target. It follows the release that the target serves: the
//! services of a new release start beside those of the current one, and clients get the new
//! release once its services answer. The serving release and the rollback of the target stay
//! loaded, so a rollback is instant.

pub mod http;
pub mod services;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::release::{Release, Serving, POINTERS};
use crate::store::sha256_hex;
use crate::target::Target;

/// The pointer that names the planner release of a release.
pub const PLANNER: &str = POINTERS[1];

/// How long the services of a planner release may take to answer after they start.
pub const READY: Duration = Duration::from_secs(120);

/// A release of the target as its clients read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// The id of its manifest.
    pub id: String,
    /// The planner release that its planner pointer names. Its services answer the API.
    pub planner: String,
    /// The bytes of each pointer key.
    pub pointers: BTreeMap<String, Vec<u8>>,
}

/// The release that `roles` of `target` serves, then its rollback.
pub fn wanted(target: &Target, roles: &Serving) -> Result<Vec<Served>, String> {
    std::iter::once(&roles.serving).chain(&roles.rollback).map(|role| served(target, &role.id)).collect()
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
    Ok(Served { id: id.into(), planner: planner.into(), pointers })
}

/// The services of a planner release. They stop when they drop.
pub trait Services {
    /// Whether each service answers; an error when one stopped.
    fn ready(&mut self) -> Result<bool, String>;
    /// Why a service stopped; `None` while each runs.
    fn stopped(&mut self) -> Option<String>;
}

enum State<S> {
    Starting {
        services: S,
        since: Instant,
    },
    Ready(S),
    /// Its services stopped, for this reason. It starts again at the next [`Follower::want`].
    Failed(String),
}

/// The planner releases whose services run, and the release that clients get.
pub struct Follower<S> {
    /// By planner release id: those of the wanted releases and of the current one.
    planners: BTreeMap<String, State<S>>,
    /// The serving release of the target, then its rollback.
    wanted: Vec<Served>,
    current: Option<Served>,
}

impl<S: Services> Default for Follower<S> {
    fn default() -> Self {
        Self { planners: BTreeMap::new(), wanted: Vec::new(), current: None }
    }
}

impl<S: Services> Follower<S> {
    /// Follow `wanted`, the serving release of the target and its rollback: `start` the services
    /// of each planner release that does not run, then [`Self::poll`]. `start` installs the files
    /// of the release, and the follower waits for it: the time to answer counts from its return.
    pub fn want(
        &mut self,
        wanted: Vec<Served>,
        start: &mut dyn FnMut(&Served) -> Result<S, String>,
        now: &dyn Fn() -> Instant,
    ) {
        self.planners.retain(|_, state| !matches!(state, State::Failed(_)));
        for served in &wanted {
            if !self.planners.contains_key(&served.planner) {
                let state = match start(served) {
                    Ok(services) => State::Starting { services, since: now() },
                    Err(e) => failed(&served.planner, e),
                };
                self.planners.insert(served.planner.clone(), state);
            }
        }
        self.wanted = wanted;
        self.poll(now());
        if let Some(current) = self.current.as_ref().filter(|current| !self.wanted.contains(current)) {
            eprintln!(
                "obc data serve: WARNING: clients get release {}, which release.json no longer names. An update \
                 can delete its files. Fix: run `obc data update` until a release loads.",
                short(&current.id)
            );
        }
    }

    /// Check the services that start, and fail a planner release whose service stopped. Clients
    /// get the serving release of the target once its services answer; until then they keep the
    /// current release. Stop the services that neither the wanted releases nor the current one
    /// need.
    pub fn poll(&mut self, now: Instant) {
        for (id, state) in &mut self.planners {
            match state {
                State::Starting { services, since } => match services.ready() {
                    Ok(false) if now.duration_since(*since) < READY => {}
                    Ok(false) => *state = failed(id, format!("its services do not answer after {} s", READY.as_secs())),
                    Err(e) => *state = failed(id, e),
                    Ok(true) => {
                        let State::Starting { services, .. } = std::mem::replace(state, State::Failed(String::new()))
                        else {
                            unreachable!("the state starts")
                        };
                        *state = State::Ready(services);
                    }
                },
                State::Ready(services) => {
                    if let Some(e) = services.stopped() {
                        *state = failed(id, e);
                    }
                }
                State::Failed(_) => {}
            }
        }
        if let Some(next) = self.wanted.first().filter(|next| self.current.as_ref() != Some(*next)) {
            if let Some(State::Ready(_)) = self.planners.get(&next.planner) {
                eprintln!("obc data serve: release {} with planner release {}", short(&next.id), short(&next.planner));
                self.current = Some(next.clone());
            }
        }
        let needed: BTreeSet<String> = self.wanted.iter().chain(&self.current).map(|s| s.planner.clone()).collect();
        self.planners.retain(|id, _| needed.contains(id));
    }

    /// The release that clients get.
    pub fn current(&self) -> Option<&Served> {
        self.current.as_ref()
    }

    /// The planner releases whose services answer, by id.
    pub fn ready(&self) -> impl Iterator<Item = (&str, &S)> {
        self.planners.iter().filter_map(|(id, state)| match state {
            State::Ready(services) => Some((id.as_str(), services)),
            _ => None,
        })
    }

    /// Why the services of the planner release `planner` stopped, while it fails.
    pub fn failure(&self, planner: &str) -> Option<&str> {
        match self.planners.get(planner) {
            Some(State::Failed(reason)) => Some(reason),
            _ => None,
        }
    }
}

fn failed<S>(planner: &str, reason: String) -> State<S> {
    eprintln!("obc data serve: planner release {} failed: {reason}", short(planner));
    State::Failed(reason)
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::engine::LayerFile;
    use crate::release::{manifest_key, serving, switch, File};
    use crate::store::tests::Scratch;
    use crate::target::Condition;

    /// What the fake services of each planner release do, and what happened to them.
    #[derive(Default)]
    struct Log {
        ready: BTreeSet<String>,
        broken: BTreeSet<String>,
        exited: BTreeSet<String>,
        started: Vec<String>,
        stopped: Vec<String>,
    }

    struct Fake(String, Rc<RefCell<Log>>);

    impl Services for Fake {
        fn ready(&mut self) -> Result<bool, String> {
            let log = self.1.borrow();
            match log.broken.contains(&self.0) {
                true => Err("the routing service stopped".into()),
                false => Ok(log.ready.contains(&self.0)),
            }
        }

        fn stopped(&mut self) -> Option<String> {
            self.1.borrow().exited.contains(&self.0).then(|| "the search service stopped: exit status 1".into())
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.1.borrow_mut().stopped.push(self.0.clone());
        }
    }

    /// Publish a release whose device pointer is `device` and whose planner pointer names
    /// `planner`, and switch `target` to it.
    fn publish(target: &Target, device: &str, planner: &str) -> String {
        let catalog = format!(r#"{{"format":1,"active":{{"id":"{planner}"}}}}"#);
        let files = [device.as_bytes().to_vec(), catalog.into_bytes()];
        let mut release = crate::release::tests::reading(b"", "osm", "1");
        let output = &mut release.outputs[0];
        output.files = files
            .iter()
            .zip(["device.json", "catalog.json"])
            .map(|(bytes, path)| LayerFile { path: path.into(), size: bytes.len() as u64, sha256: sha256_hex(bytes) })
            .collect();
        let pointer = |(key, file): (&&str, &LayerFile)| File {
            key: key.to_string(),
            size: file.size,
            sha256: file.sha256.clone(),
        };
        release.pointers = POINTERS.iter().zip(&output.files).map(pointer).collect();
        for bytes in &files {
            target.put(&format!("objects/{}", sha256_hex(bytes)), bytes, &Condition::Any).unwrap();
        }
        let id = release.id();
        target.put(&manifest_key(&id), &release.canonical(), &Condition::Any).unwrap();
        assert!(switch(target, serving(target).unwrap().as_ref(), &id).unwrap());
        id
    }

    struct Case {
        _scratch: Scratch,
        target: Target,
        log: Rc<RefCell<Log>>,
        follower: Follower<Fake>,
        now: Instant,
    }

    impl Case {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let target = Target::Dir(scratch.0.join("target"));
            let (log, follower, now) = (Rc::default(), Follower::default(), Instant::now());
            Self { _scratch: scratch, target, log, follower, now }
        }

        /// Follow the release that the target serves now.
        fn follow(&mut self) {
            let (log, now) = (self.log.clone(), self.now);
            let mut start = |served: &Served| {
                log.borrow_mut().started.push(served.planner.clone());
                Ok(Fake(served.planner.clone(), log.clone()))
            };
            let (roles, _) = serving(&self.target).unwrap().unwrap();
            self.follower.want(wanted(&self.target, &roles).unwrap(), &mut start, &|| now);
        }

        fn switch(&mut self, id: &str) {
            assert!(switch(&self.target, serving(&self.target).unwrap().as_ref(), id).unwrap());
            self.follow();
        }

        fn answers(&mut self, planner: &str) {
            self.log.borrow_mut().ready.insert(planner.into());
            self.follower.poll(self.now);
        }

        fn current(&self) -> &str {
            &self.follower.current().unwrap().id
        }
    }

    const P1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const P2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const P3: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    #[test]
    fn a_new_release_loads_beside_the_current_one_and_clients_get_it_once_it_answers() {
        let mut case = Case::new("serve-follow");
        let a = publish(&case.target, "device a", P1);
        case.follow();
        assert!(case.follower.current().is_none(), "no client gets a release before its services answer");
        case.answers(P1);
        assert_eq!(case.current(), a);

        let b = publish(&case.target, "device b", P2);
        case.follow();
        assert_eq!(case.current(), a, "the current release serves while the new one starts");
        case.answers(P2);
        assert_eq!(case.current(), b);
        assert_eq!(case.follower.ready().map(|(id, _)| id).collect::<Vec<_>>(), [P1, P2], "the rollback runs");

        // A release of the same planner release, such as one of `--only device/map`, switches at once.
        let c = publish(&case.target, "device c", P2);
        case.follow();
        assert_eq!(case.current(), c);
        assert_eq!(case.log.borrow().started, [P1, P2]);
        assert_eq!(case.log.borrow().stopped, [P1], "no release of the target needs the oldest services");

        case.switch(&b);
        assert_eq!(case.current(), b, "a rollback starts nothing");

        let d = publish(&case.target, "device d", P3);
        case.follow();
        case.answers(P3);
        assert_eq!(case.current(), d);
        assert_eq!(case.follower.current().unwrap().pointers[POINTERS[0]], b"device d");
        case.switch(&c);
        assert_eq!(case.current(), c, "a rollback to another planner release that runs");
        assert_eq!(case.log.borrow().started, [P1, P2, P3]);
    }

    #[test]
    fn a_release_whose_services_fail_leaves_clients_on_the_current_one() {
        let mut case = Case::new("serve-fail");
        let a = publish(&case.target, "device a", P1);
        case.follow();
        case.answers(P1);

        case.log.borrow_mut().broken.insert(P2.into());
        publish(&case.target, "device b", P2);
        case.follow();
        assert_eq!(case.current(), a);
        assert_eq!(case.log.borrow().stopped, [P2], "the services of a failed release stop");
        case.follower.poll(case.now);
        assert_eq!(case.log.borrow().started, [P1, P2], "a failed release does not start again by itself");

        // The same planner release starts again when the target names another release.
        case.log.borrow_mut().broken.clear();
        let c = publish(&case.target, "device c", P2);
        case.follow();
        case.answers(P2);
        assert_eq!(case.current(), c);

        publish(&case.target, "device d", P3);
        case.follow();
        case.now += READY;
        case.follower.poll(case.now);
        assert_eq!(case.current(), c, "services that do not answer in time fail");
        assert!(case.follower.failure(P3).unwrap().contains("do not answer"));

        case.log.borrow_mut().exited.insert(P2.into());
        case.follower.poll(case.now);
        assert_eq!(case.follower.failure(P2), Some("the search service stopped: exit status 1"));
        assert!(case.follower.ready().next().is_none(), "the API of a planner release whose service stopped ends");
        assert_eq!(case.current(), c);
    }
}
