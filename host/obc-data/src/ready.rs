//! The publisher side of [`crate::serve`]: before an R2 target switches to a new release,
//! `planner/next.json` names it, so the follower of the target starts its planner release, and the
//! run waits until the follower says that it serves it.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::fetch::http::Http;
use crate::release::{Release, Role, NEXT};
use crate::serve::PLANNER;
use crate::target::{Condition, Target};

/// How often a run asks the follower whether it serves the next release.
pub const EVERY: Duration = Duration::from_secs(10);

/// How long a run waits for the follower before the target stays as it was: 24 hours, which the
/// owner chose so that a deploy of services that read a new format fits in the wait.
pub const GIVE_UP: Duration = Duration::from_secs(24 * 60 * 60);

/// How a run waits for the follower: in real time, or on the clock of a test.
#[derive(Clone, Copy)]
pub struct Wait {
    pub every: Duration,
    pub give_up: Duration,
    /// The time since an origin.
    pub now: fn() -> Duration,
    pub sleep: fn(Duration),
}

impl Wait {
    pub const RUN: Wait = Wait { every: EVERY, give_up: GIVE_UP, now: monotonic, sleep: std::thread::sleep };
}

fn monotonic() -> Duration {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed()
}

/// The URL at which the follower of `target` says whether it serves the planner release of
/// `release`: `/planner-api/releases/<planner>/ready` at the API origin of its planner pointer.
pub fn url(target: &Target, release: &Release) -> Result<String, String> {
    let pointer = release.pointers.iter().find(|pointer| pointer.key == PLANNER);
    let key = format!("objects/{}", pointer.ok_or_else(|| format!("the release has no {PLANNER}"))?.sha256);
    let bytes = target.get(&key)?.ok_or_else(|| format!("the target has no {key}"))?;
    let catalog: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{PLANNER}: {e}"))?;
    let planner = catalog["active"]["id"].as_str().filter(|planner| crate::release::is_sha256(planner));
    match (catalog["origins"]["api_origin"].as_str(), planner) {
        (Some(api), Some(planner)) => Ok(format!("{}/planner-api/releases/{planner}/ready", api.trim_end_matches('/'))),
        _ => Err(format!("{PLANNER} names no API origin or no planner release")),
    }
}

/// Why the follower does not serve at `url` yet, as it says; `None` when it serves.
pub fn ask(http: &Http, url: &str) -> Option<String> {
    let (status, body) = match http.answer(url, "application/json") {
        Ok(answer) => answer,
        Err(e) => return Some(e),
    };
    let body: Value = serde_json::from_slice(&body).unwrap_or_default();
    match (status, body["reason"].as_str(), body["state"].as_str()) {
        (200, ..) => None,
        (_, Some(reason), _) => Some(reason.into()),
        (_, None, Some(state)) => Some(format!("the planner release is {state}")),
        _ => Some(format!("GET {url}: HTTP {status}")),
    }
}

/// `planner/next.json` of a target while a run waits for its release. Dropped, it is gone.
pub struct Next<'a> {
    target: &'a Target,
    id: String,
}

impl<'a> Next<'a> {
    /// Name the release `id` in `planner/next.json` of `target`, in place of what a run that
    /// stopped left there.
    pub fn name(target: &'a Target, id: &str) -> Result<Self, String> {
        let role = Role { id: id.into(), since: crate::date::timestamp(target.now()?) };
        let bytes = serde_json::to_vec_pretty(&role).map_err(|e| e.to_string())?;
        let condition = target.get_tagged(NEXT)?.map_or(Condition::Absent, |(_, tag)| Condition::Match(tag));
        match target.put(NEXT, &bytes, &condition)? {
            Some(_) => Ok(Self { target, id: id.into() }),
            None => Err(format!("{NEXT} changed while this run wrote it")),
        }
    }
}

impl Drop for Next<'_> {
    /// Delete `planner/next.json` while it names this release: a run that took the lock since can
    /// have named another.
    fn drop(&mut self) {
        let named = self.target.get(NEXT).map(|bytes| {
            bytes.and_then(|bytes| serde_json::from_slice::<Role>(&bytes).ok()).is_some_and(|role| role.id == self.id)
        });
        if let Err(e) = named.and_then(|named| if named { self.target.remove(NEXT) } else { Ok(()) }) {
            eprintln!("obc data: {NEXT} stays: {e}; the follower keeps its release until a run deletes it");
        }
    }
}
