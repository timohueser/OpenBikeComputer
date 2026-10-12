//! `obc data rollback TARGET`: make a target serve its rollback release again. The release that
//! served becomes the rollback, so a second rollback undoes the first. It bakes, uploads and
//! deletes nothing: the next update collects.

use std::path::Path;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::{print_json, Code, Error};
use crate::engine::runs::{Event, Phase, Publication, Run};
use crate::release;
use crate::store::Store;
use crate::target::Target;

#[derive(Args)]
pub struct RollbackArgs {
    /// The target, as `data/targets.toml` names it.
    target: String,
}

/// What a rollback did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RolledBack {
    pub run: String,
    pub target: String,
    /// The release that the target serves now: its rollback before.
    pub serving: String,
    /// The release that it served before.
    pub rollback: String,
}

pub fn command(root: &Path, args: RollbackArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, &args.target, &store).map_err(|e| Code::Usage.error(e))?;
    if let Target::R2 { .. } = target {
        return Err(Code::Blocked
            .error(format!("`obc data rollback` writes only a dir target, and `{}` is an R2 bucket", args.target)));
    }
    let mut run = super::api::start_run(&store, &format!("rollback {}", args.target))?;
    let result = rollback((&args.target, &target), &mut run);
    let rolled = super::api::finish_run(run, result, None)?;
    if json {
        return print_json(&rolled);
    }
    println!("{} serves release {}; rollback {}", rolled.target, &rolled.serving[..12], &rolled.rollback[..12]);
    Ok(())
}

/// Switch `target` to its rollback release, after a check that the target has each of its keys.
pub(super) fn rollback((name, target): (&str, &Target), run: &mut Run) -> Result<RolledBack, Error> {
    let base = release::serving(target)?.ok_or_else(|| {
        Code::Usage
            .error(format!("target `{name}` serves no release"))
            .fix(format!("Run `obc data update {name} --region ID` first."))
    })?;
    let Some(rollback) = base.0.rollback.clone() else {
        return Err(Code::Usage.error(format!("target `{name}` has no rollback release")).fix(format!(
            "An update of `{name}` that publishes a new release keeps the release before as the rollback."
        )));
    };
    run.record(&Event::Phase { phase: Phase::Verify })?;
    let release = super::update_cli::read(name, target, &rollback.id)?;
    let files: Vec<(String, u64)> = release.files().into_iter().map(|file| (file.key, file.size)).collect();
    let missing = target.missing(&files)?;
    if let Some(key) = missing.first() {
        return Err(Code::VerifyFailed
            .error(format!(
                "the target lacks {} keys of release {}, such as {key}; nothing changed",
                missing.len(),
                &rollback.id[..12]
            ))
            .fix(format!("Run `obc data update {name}` to publish again.")));
    }
    run.record(&Event::Phase { phase: Phase::Switch })?;
    if !release::switch(target, Some(&base), &rollback.id)? {
        return Err(Code::Failed
            .error(format!("release.json of `{name}` changed during the rollback; nothing switched"))
            .fix(format!("Run `obc data` to see what `{name}` serves now.")));
    }
    run.record(&Event::Published { mutation: Publication::Switched { release: rollback.id.clone() } })?;
    run.sync()?;
    Ok(RolledBack { run: run.id().into(), target: name.into(), serving: rollback.id, rollback: base.0.serving.id })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::update_cli::tests::{region, update};
    use crate::engine::tests::fixture;
    use crate::release::Release;

    fn rollback(fixture: &crate::engine::tests::Fixture, target: &Target) -> Result<RolledBack, Error> {
        let mut run = Run::create(&fixture.store, "rollback local").unwrap();
        let rolled = super::rollback(("local", target), &mut run);
        run.finish(rolled.as_ref().err().map(|error| error.message.as_str())).unwrap();
        rolled
    }

    #[test]
    fn a_rollback_swaps_the_serving_and_the_rollback_release_and_deletes_nothing() {
        let fixture = fixture("rollback");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let a = update(&fixture, &target, region).unwrap().serving;
        let refused = rollback(&fixture, &target).unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (Code::Usage, "target `local` has no rollback release"));
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let b = update(&fixture, &target, |_| Ok(())).unwrap().serving;
        let keys = || target.list().unwrap().into_iter().map(|listed| listed.key).collect::<Vec<_>>();
        let before = keys();

        for (serving, rollback_id) in [(&a, &b), (&b, &a)] {
            let rolled = rollback(&fixture, &target).unwrap();
            assert_eq!((&rolled.serving, &rolled.rollback), (serving, rollback_id));
            let now = release::serving(&target).unwrap().unwrap().0;
            assert_eq!((&now.serving.id, now.rollback.map(|role| role.id).as_ref()), (serving, Some(rollback_id)));
            assert!(release::drift(&target, &Release::read(&target, serving).unwrap()).unwrap().is_empty());
            assert_eq!(keys(), before, "a rollback deletes nothing");
        }

        // The rollback release lacks a key: nothing switches.
        let files = |id: &str| Release::read(&target, id).unwrap().files().into_iter().map(|file| file.key);
        let only_a = files(&a).find(|key| !files(&b).any(|other| other == *key)).unwrap();
        target.remove(&only_a).unwrap();
        let refused = rollback(&fixture, &target).unwrap_err();
        assert_eq!(refused.code, Code::VerifyFailed, "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, b);
    }
}
