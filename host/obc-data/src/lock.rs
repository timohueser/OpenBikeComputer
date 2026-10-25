//! `lock.json` of a target: one run at a time changes a target. A run takes the lock with a
//! conditional write, writes a heartbeat each minute, checks that it still holds the lock just
//! before its switch, and frees the lock at its end, also when it fails, stops or panics. A write
//! whose condition fails, while `lock.json` names another run, means that another run took the
//! lock: this run stops and changes nothing more.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::runs::Phase;
use crate::target::{Condition, Tag, Target};

/// The key of the lock of a target.
pub const LOCK: &str = "lock.json";

/// How often a holder writes its heartbeat, in seconds.
pub const BEAT: u64 = 60;

/// How old a heartbeat is before `obc data unlock --from` frees its lock, in seconds: 10 minutes,
/// from the P2 plan that the owner approved. That is ten heartbeats that did not come.
pub const STALE: u64 = 600;

/// `lock.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LockFile {
    Free,
    Held(Holder),
}

/// The run that holds the lock of a target. A reader ignores keys that it does not know, so builds
/// of `obc data` read each other's lock.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Holder {
    /// `OBC_DATA_MACHINE`, or else the short host name.
    pub machine: String,
    pub run: String,
    /// The id of the review that the run does; `null` until the run has it.
    pub review: Option<String>,
    pub phase: Phase,
    /// The time of the last heartbeat, `YYYY-MM-DDTHH:MM:SSZ`.
    pub heartbeat: String,
    /// Each layer that the run bakes, at the last heartbeat. The status of the run on its machine
    /// has the detail.
    #[serde(default)]
    pub progress: Vec<Progress>,
    /// Why the run waits, in the phase `router-ready`: the last answer of the follower of the
    /// target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<String>,
}

/// The steps of a layer that a run bakes, and those that finished.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Progress {
    pub layer: String,
    pub done: usize,
    pub total: usize,
}

/// The lock of a target that this run holds. Dropped, it is free again.
pub struct Held {
    target: Target,
    state: Mutex<(Holder, Tag)>,
    lost: AtomicBool,
    released: bool,
}

/// Why a run did not take the lock.
#[derive(Debug)]
pub enum Refused {
    /// Another run holds it.
    Held(Box<Holder>),
    Failed(String),
}

/// The lock of `target` and its tag; `None` when the target has no lock.
pub fn read(target: &Target) -> Result<Option<(LockFile, Tag)>, String> {
    let Some((bytes, tag)) = target.get_tagged(LOCK)? else { return Ok(None) };
    let lock = serde_json::from_slice(&bytes).map_err(|e| format!("{LOCK}: {e}"))?;
    Ok(Some((lock, tag)))
}

/// The run that holds the lock of `target`; `None` when the lock is free. It reads an R2 target
/// too.
pub fn holder(target: &Target) -> Result<Option<Holder>, String> {
    let Some(bytes) = target.get(LOCK)? else { return Ok(None) };
    match serde_json::from_slice(&bytes).map_err(|e| format!("{LOCK}: {e}"))? {
        LockFile::Held(holder) => Ok(Some(holder)),
        LockFile::Free => Ok(None),
    }
}

fn bytes(lock: &LockFile) -> Vec<u8> {
    serde_json::to_vec_pretty(lock).expect("a lock serializes")
}

impl Held {
    /// Take the lock of `target` for the run `run` at the time `now`.
    pub fn take(target: &Target, run: &str, now: u64) -> Result<Held, Refused> {
        Held::take_over(target, run, now, None)
    }

    /// [`Held::take`], also while `ended` holds the lock: a run whose process ended, which cannot
    /// free it any more.
    pub fn take_over(target: &Target, run: &str, now: u64, ended: Option<&Holder>) -> Result<Held, Refused> {
        let same =
            |holder: &Holder| ended.is_some_and(|ended| (&ended.machine, &ended.run) == (&holder.machine, &holder.run));
        let condition = match read(target).map_err(Refused::Failed)? {
            None => Condition::Absent,
            Some((LockFile::Free, tag)) => Condition::Match(tag),
            Some((LockFile::Held(holder), tag)) if same(&holder) => Condition::Match(tag),
            Some((LockFile::Held(holder), _)) => return Err(Refused::Held(Box::new(holder))),
        };
        let holder = Holder {
            machine: crate::machine(),
            run: run.into(),
            review: None,
            phase: Phase::Prepare,
            heartbeat: crate::date::timestamp(now),
            progress: Vec::new(),
            waiting: None,
        };
        match write(target, &LockFile::Held(holder.clone()), condition, run).map_err(Refused::Failed)? {
            Some(tag) => Ok(Held {
                target: target.clone(),
                state: Mutex::new((holder, tag)),
                lost: AtomicBool::new(false),
                released: false,
            }),
            None => match read(target).map_err(Refused::Failed)? {
                Some((LockFile::Held(holder), _)) => Err(Refused::Held(Box::new(holder))),
                _ => Err(Refused::Failed(format!("{LOCK} changed while this run took it"))),
            },
        }
    }

    /// Write the heartbeat at the time `now`, with `review` and `phase` when they are given. `false`
    /// when another run took the lock: then this run must stop.
    pub fn beat(&self, review: Option<&str>, phase: Option<Phase>, now: u64) -> Result<bool, String> {
        let mut state = self.state.lock().expect("a heartbeat does not panic");
        if self.lost() {
            return Ok(false);
        }
        let mut holder = state.0.clone();
        holder.review = review.map(str::to_string).or(holder.review);
        holder.phase = phase.unwrap_or(holder.phase);
        holder.heartbeat = crate::date::timestamp(now);
        match write(&self.target, &LockFile::Held(holder.clone()), Condition::Match(state.1.clone()), &holder.run)? {
            Some(tag) => *state = (holder, tag),
            None => self.lost.store(true, Ordering::Relaxed),
        }
        Ok(!self.lost())
    }

    /// Give the next heartbeat `progress`.
    pub fn progress(&self, progress: Vec<Progress>) {
        self.state.lock().expect("a heartbeat does not panic").0.progress = progress;
    }

    /// Give the next heartbeat why the run waits.
    pub fn waiting(&self, reason: Option<String>) {
        self.state.lock().expect("a heartbeat does not panic").0.waiting = reason;
    }

    /// Whether another run took the lock.
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// Free the lock, when this run still holds it. `Refused::Held` when another run took it.
    pub fn release(mut self) -> Result<(), Refused> {
        self.released = true;
        self.free_if_held()
    }

    fn free_if_held(&mut self) -> Result<(), Refused> {
        if self.lost() {
            return Ok(());
        }
        let (holder, tag) = self.state.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
        match write(&self.target, &LockFile::Free, Condition::Match(tag.clone()), &holder.run) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(match read(&self.target) {
                Ok(Some((LockFile::Held(holder), _))) => Refused::Held(Box::new(holder)),
                _ => Refused::Failed(format!("{LOCK} changed before this run freed it")),
            }),
            Err(e) => Err(Refused::Failed(e)),
        }
    }
}

/// Write `lock` on `condition` for the run `run`, and give its tag; `None` when another run changed
/// `lock.json`. The server can apply a write whose answer does not come: then a later write on the
/// tag that the run knows fails. So after a write that failed or got no answer, `lock.json` decides.
/// When it is `lock`, the write is done. When it names this run, the write is done again on its tag.
fn write(target: &Target, lock: &LockFile, condition: Condition, run: &str) -> Result<Option<Tag>, String> {
    let written = target.put(LOCK, &bytes(lock), &condition);
    if let Ok(Some(tag)) = written {
        return Ok(Some(tag));
    }
    match read(target)? {
        Some((now, tag)) if now == *lock => Ok(Some(tag)),
        Some((LockFile::Held(holder), tag)) if holder.machine == crate::machine() && holder.run == run => {
            target.put(LOCK, &bytes(lock), &Condition::Match(tag))
        }
        _ => written,
    }
}

impl Drop for Held {
    /// A run that panics frees the lock too.
    fn drop(&mut self) {
        if !self.released {
            let _ = self.free_if_held();
        }
    }
}

/// The time at which the heartbeat of `holder` is [`STALE`].
pub fn stale_at(holder: &Holder) -> u64 {
    crate::date::seconds(&holder.heartbeat).unwrap_or(0).saturating_add(STALE)
}

/// The time at which the lock of `target` is [`STALE`], and the time of now, by the clock of the
/// target: the heartbeat of the holder on a dir target, whose holder is this machine; the time
/// when the server got the last heartbeat on an R2 target.
pub fn stale_since(target: &Target, holder: &Holder) -> Result<(u64, u64), String> {
    let Target::R2(remote) = target else { return Ok((stale_at(holder), crate::date::now())) };
    let (modified, now) = remote.writer()?.head(LOCK)?;
    let modified = modified.ok_or_else(|| format!("{LOCK} is gone"))?;
    Ok((modified.saturating_add(STALE), now))
}

/// Free the lock of `target` that has the tag `tag`.
pub fn free(target: &Target, tag: Tag) -> Result<(), String> {
    match target.put(LOCK, &bytes(&LockFile::Free), &Condition::Match(tag))? {
        Some(_) => Ok(()),
        None => Err("the lock changed after it was read; nothing changed".into()),
    }
}

/// `run R of machine M holds the lock (phase P, heartbeat T)`
pub fn held_by(holder: &Holder) -> String {
    format!(
        "run {} of machine `{}` holds the lock (phase {}, heartbeat {})",
        holder.run, holder.machine, holder.phase, holder.heartbeat
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Give `lock.json` the holder of `holder` with `edit`, as a write whose answer did not come.
    fn applied(target: &Target, edit: impl FnOnce(&mut Holder)) {
        let Some((LockFile::Held(mut holder), _)) = read(target).unwrap() else { panic!("free") };
        edit(&mut holder);
        target.put(LOCK, &bytes(&LockFile::Held(holder)), &Condition::Any).unwrap();
    }

    #[test]
    fn a_write_that_the_server_applied_without_an_answer_keeps_the_lock_and_another_run_does_not() {
        let fixture = crate::engine::tests::fixture("lock-applied");
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let held = Held::take(&target, "one", 0).unwrap();
        applied(&target, |holder| holder.heartbeat = crate::date::timestamp(60));
        assert!(held.beat(None, None, 120).unwrap(), "lock.json names this run");
        let Some((LockFile::Held(holder), _)) = read(&target).unwrap() else { panic!("free") };
        assert_eq!(holder.heartbeat, crate::date::timestamp(120));
        applied(&target, |holder| holder.heartbeat = crate::date::timestamp(180));
        held.release().unwrap();
        assert_eq!(read(&target).unwrap().unwrap().0, LockFile::Free);

        let held = Held::take(&target, "two", 0).unwrap();
        applied(&target, |holder| holder.run = "three".into());
        let Err(Refused::Held(holder)) = held.release() else { panic!("freed") };
        assert_eq!(holder.run, "three", "another run holds the lock");
    }

    #[test]
    fn a_lock_that_a_run_drops_without_release_is_free() {
        let fixture = crate::engine::tests::fixture("lock-drop");
        let target = Target::Dir(fixture.scratch.0.join("local"));
        drop(Held::take(&target, "panicked", 0).unwrap());
        assert_eq!(read(&target).unwrap().unwrap().0, LockFile::Free);
    }
}
