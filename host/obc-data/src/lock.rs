//! `lock.json` of a target: one run at a time changes a target. A run takes the lock with a
//! conditional write, writes a heartbeat each minute, checks that it still holds the lock just
//! before its switch, and frees the lock at its end. A write whose condition fails means that
//! another run took the lock: this run stops and changes nothing more.

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
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum LockFile {
    Free,
    Held(Holder),
}

/// The run that holds the lock of a target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Holder {
    /// `OBC_DATA_MACHINE`, or else the short host name.
    pub machine: String,
    pub run: String,
    /// The id of the review that the run does; `null` until the run has it.
    pub review: Option<String>,
    pub phase: Phase,
    /// The time of the last heartbeat, `YYYY-MM-DDTHH:MM:SSZ`.
    pub heartbeat: String,
}

/// The lock of a target that this run holds.
pub struct Held {
    target: Target,
    state: Mutex<(Holder, Tag)>,
    lost: AtomicBool,
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
    /// Take the lock of `target` for the run `run` at the time `now`. An error names the holder.
    pub fn take(target: &Target, run: &str, now: u64) -> Result<Held, String> {
        let condition = match read(target)? {
            None => Condition::Absent,
            Some((LockFile::Free, tag)) => Condition::Match(tag),
            Some((LockFile::Held(holder), _)) => return Err(held_by(&holder)),
        };
        let holder = Holder {
            machine: crate::machine(),
            run: run.into(),
            review: None,
            phase: Phase::Prepare,
            heartbeat: crate::date::timestamp(now),
        };
        match target.put(LOCK, &bytes(&LockFile::Held(holder.clone())), &condition)? {
            Some(tag) => {
                Ok(Held { target: target.clone(), state: Mutex::new((holder, tag)), lost: AtomicBool::new(false) })
            }
            None => Err("another run took the lock at the same time".into()),
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
        match self.target.put(LOCK, &bytes(&LockFile::Held(holder.clone())), &Condition::Match(state.1.clone()))? {
            Some(tag) => *state = (holder, tag),
            None => self.lost.store(true, Ordering::Relaxed),
        }
        Ok(!self.lost())
    }

    /// Whether another run took the lock.
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// Free the lock, when this run still holds it.
    pub fn release(self) -> Result<(), String> {
        let (_, tag) = self.state.into_inner().expect("a heartbeat does not panic");
        if !self.lost.into_inner() {
            self.target.put(LOCK, &bytes(&LockFile::Free), &Condition::Match(tag))?;
        }
        Ok(())
    }
}

/// The lock of `target` that a run of `machine` holds, with its tag, when its heartbeat is
/// [`STALE`] at the time `now`.
pub fn stale(target: &Target, machine: &str, now: u64) -> Result<(Holder, Tag), String> {
    let Some((LockFile::Held(holder), tag)) = read(target)? else {
        return Err("the lock is free".into());
    };
    if holder.machine != machine {
        return Err(format!("{}, not a run of `{machine}`", held_by(&holder)));
    }
    let beat = crate::date::seconds(&holder.heartbeat).unwrap_or(0);
    if beat.saturating_add(STALE) > now {
        return Err(format!("{}: its heartbeat is younger than {} minutes", held_by(&holder), STALE / 60));
    }
    Ok((holder, tag))
}

/// Free the lock of `target` that has the tag `tag`.
pub fn free(target: &Target, tag: Tag) -> Result<(), String> {
    match target.put(LOCK, &bytes(&LockFile::Free), &Condition::Match(tag))? {
        Some(_) => Ok(()),
        None => Err("the lock changed after it was read; nothing changed".into()),
    }
}

fn held_by(holder: &Holder) -> String {
    format!(
        "run {} of machine `{}` holds the lock (phase {}, heartbeat {})",
        holder.run, holder.machine, holder.phase, holder.heartbeat
    )
}
