//! `obc data unlock TARGET --from MACHINE [--source ID]`: free the lock of a target, or of a prepared
//! source on it, whose run stopped without freeing it. The heartbeat of the holder must be
//! [`lock::STALE`] old.

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{confirm, print_json, Code, Error};
use crate::lock::{self, Holder, LockFile};
use crate::store::Store;

/// What `unlock` did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Unlocked {
    pub target: String,
    /// The prepared source whose lock it freed; `null` for the lock of the target.
    pub source: Option<String>,
    /// The run that held the lock.
    pub holder: Holder,
}

pub fn command(
    root: &Path,
    (name, source): (&str, Option<&str>),
    from: &str,
    yes: bool,
    json: bool,
) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, name, &store).map_err(|e| Code::Usage.error(e))?;
    super::api::writable(name, &target)?;
    let (label, unlock, target) = match source {
        None => (format!("target `{name}`"), format!("obc data unlock {name}"), target),
        Some(id) => (
            format!("source `{id}` on `{name}`"),
            format!("obc data unlock {name} --source {id}"),
            crate::prepared::folder(&target, id),
        ),
    };
    let unreadable = |e: String| {
        Code::Failed
            .error(format!("{label}: {e}"))
            .fix(format!("Read {} of the target: another build of `obc data` can have written it, and a run can hold it. Wait for that run, or use that build.", lock::LOCK))
    };
    let (holder, tag) = match lock::read(&target).map_err(unreadable)? {
        Some((LockFile::Held(holder), tag)) => (holder, tag),
        _ => {
            return Err(Code::Usage
                .error(format!("the lock of {label} is free; nothing changed"))
                .fix("No run holds it: start the command that you want."))
        }
    };
    if holder.machine != from {
        return Err(Code::Usage
            .error(format!("{label}: {}, not a run of `{from}`; nothing changed", lock::held_by(&holder)))
            .fix(format!(
                "Name its machine when that machine stopped the run: `{unlock} --from {}`.",
                holder.machine
            )));
    }
    let (stale_at, now) = lock::stale_since(&target, &holder).map_err(unreadable)?;
    if now < stale_at {
        return Err(Code::Blocked.error(format!("{label}: {}; nothing changed", lock::held_by(&holder))).fix(format!(
            "That run can still run. When `{}` stopped it, run this again after {}.",
            holder.machine,
            crate::date::timestamp(stale_at)
        )));
    }
    let question = format!("Free the lock of {label} that run {} of machine `{}` holds?", holder.run, holder.machine);
    confirm(&question, yes)?;
    lock::free(&target, tag)
        .map_err(|e| Code::Failed.error(format!("{label}: {e}")).fix(format!("Run `{unlock} --from {from}` again.")))?;
    let unlocked = Unlocked { target: name.into(), source: source.map(str::to_string), holder };
    if json {
        return print_json(&unlocked);
    }
    println!("the lock of {label} is free; run {} of `{}` held it", unlocked.holder.run, unlocked.holder.machine);
    Ok(())
}
