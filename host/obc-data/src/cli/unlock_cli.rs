//! `obc data unlock (TARGET | --source ID) --from MACHINE`: free the lock of a target, or of a
//! prepared source in the archive, whose run stopped without freeing it. The heartbeat of the holder
//! must be [`lock::STALE`] old.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use super::{confirm, print_json, Code, Error};
use crate::lock::{self, Holder, LockFile};
use crate::store::Store;
use crate::target::Target;

/// What `unlock` did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Unlocked {
    /// The target whose lock it freed; `null` for the lock of a prepared source.
    pub target: Option<String>,
    /// The prepared source whose lock it freed; `null` for the lock of a target.
    pub source: Option<String>,
    /// The run that held the lock.
    pub holder: Holder,
}

pub fn command(
    root: &Path,
    (name, source): (Option<&str>, Option<&str>),
    from: &str,
    yes: bool,
    json: bool,
) -> Result<(), Error> {
    let store = Store::open()?;
    let (label, unlock, target) = match (name, source) {
        (_, Some(id)) => source_lock(&crate::fetch::archive::roots(), id)?,
        (Some(name), None) => {
            let target = crate::target::find(root, name, &store).map_err(|e| Code::Usage.error(e))?;
            super::api::writable(name, &target, false)?;
            (format!("target `{name}`"), format!("obc data unlock {name}"), target)
        }
        (None, None) => return Err(Code::Usage.error("name a target or `--source ID`")),
    };
    let holder = free((&label, &unlock), &target, from, yes)?;
    let unlocked = Unlocked {
        target: name.filter(|_| source.is_none()).map(str::to_string),
        source: source.map(str::to_string),
        holder,
    };
    if json {
        return print_json(&unlocked);
    }
    println!("the lock of {label} is free; run {} of `{}` held it", unlocked.holder.run, unlocked.holder.machine);
    Ok(())
}

/// The label, the command and the home of the lock of the prepared source `id` in `roots`.
fn source_lock(roots: &[PathBuf], id: &str) -> Result<(String, String, Target), Error> {
    crate::prepared::warn_shadowed(roots, id);
    let home = crate::prepared::home(roots, id).ok_or_else(|| {
        Code::Usage
            .error(format!("no root of OBC_DATA_ARCHIVE has prepared/{id}/; nothing changed"))
            .fix("Set OBC_DATA_ARCHIVE to the roots of the archive and mount their drives.")
    })?;
    let (label, unlock) = super::prepare_cli::lock_names(id);
    Ok((label, unlock, crate::prepared::folder(home, id)))
}

/// Free the lock of `target` that a run of `from` holds with a stale heartbeat; give that run.
fn free((label, unlock): (&str, &str), target: &Target, from: &str, yes: bool) -> Result<Holder, Error> {
    let unreadable = |e: String| {
        Code::Failed
            .error(format!("{label}: {e}"))
            .fix(format!("Read {} of the target: another build of `obc data` can have written it, and a run can hold it. Wait for that run, or use that build.", lock::LOCK))
    };
    let (holder, tag) = match lock::read(target).map_err(unreadable)? {
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
    let (stale_at, now) = lock::stale_since(target, &holder).map_err(unreadable)?;
    if now < stale_at {
        return Err(Code::Blocked.error(format!("{label}: {}; nothing changed", lock::held_by(&holder))).fix(format!(
            "That run can still run. When `{}` stopped it, run this again after {}.",
            holder.machine,
            crate::date::timestamp(stale_at)
        )));
    }
    let question = format!("Free the lock of {label} that run {} of machine `{}` holds?", holder.run, holder.machine);
    confirm(&question, yes)?;
    lock::free(target, tag)
        .map_err(|e| Code::Failed.error(format!("{label}: {e}")).fix(format!("Run `{unlock} --from {from}` again.")))?;
    Ok(holder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlock_of_a_source_frees_the_stale_lock_in_its_home() {
        let scratch = crate::store::tests::Scratch::new("unlock-source");
        let roots = [scratch.0.join("empty"), scratch.0.join("archive")];
        assert_eq!(source_lock(&roots, "terrain").err().map(|e| e.code), Some(Code::Usage), "no home");
        let home = crate::prepared::path(&roots[1], "terrain");
        std::fs::create_dir_all(&home).unwrap();
        let held = r#"{"state": "held", "machine": "bake", "run": "r1", "review": null, "phase": "build", "heartbeat": "2026-09-03T00:00:00Z"}"#;
        std::fs::write(home.join(lock::LOCK), held).unwrap();
        let (label, unlock, target) = source_lock(&roots, "terrain").unwrap();
        assert_eq!(
            (label.as_str(), unlock.as_str()),
            ("source `terrain` in the archive", "obc data unlock --source terrain")
        );
        assert_eq!(free((&label, &unlock), &target, "laptop", true).unwrap_err().code, Code::Usage, "another machine");
        assert_eq!(free((&label, &unlock), &target, "bake", true).unwrap().run, "r1");
        assert_eq!(lock::read(&target).unwrap().unwrap().0, LockFile::Free);
    }
}
