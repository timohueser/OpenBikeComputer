//! `obc data unlock TARGET --from MACHINE`: free the lock of a target whose run stopped without
//! freeing it. The heartbeat of the holder must be older than [`lock::STALE`].

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{confirm, print_json, Code, Error};
use crate::lock::{self, Holder};
use crate::store::Store;

/// What `unlock` did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Unlocked {
    pub target: String,
    /// The run that held the lock.
    pub holder: Holder,
}

pub fn command(root: &Path, name: &str, from: &str, yes: bool, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, name, &store).map_err(|e| Code::Usage.error(e))?;
    let refused = |e: String| {
        Code::Blocked
            .error(format!("target `{name}`: {e}"))
            .fix(format!("A run whose heartbeat is younger than {} minutes still runs: wait for it.", lock::STALE / 60))
    };
    let (holder, tag) = lock::stale(&target, from, crate::date::now()).map_err(refused)?;
    let question = format!("Free the lock of `{name}` that run {} of machine `{}` holds?", holder.run, holder.machine);
    confirm(&question, yes)?;
    lock::free(&target, tag).map_err(refused)?;
    let unlocked = Unlocked { target: name.into(), holder };
    if json {
        return print_json(&unlocked);
    }
    println!("{} is free; run {} of `{}` held it", unlocked.target, unlocked.holder.run, unlocked.holder.machine);
    Ok(())
}
