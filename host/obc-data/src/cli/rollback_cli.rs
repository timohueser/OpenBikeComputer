//! `obc data rollback TARGET`: make a target serve its rollback release again. The release that
//! served becomes the rollback, so a second rollback undoes the first. It bakes, uploads and
//! deletes nothing: the next update collects.

use std::path::Path;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::{print_json, update_cli, Code, Error};
use crate::engine::runs::{Event, Phase, Run};
use crate::lock::Held;
use crate::release;
use crate::store::Store;
use crate::target::Target;

#[derive(Args)]
pub struct RollbackArgs {
    /// The target, as `data/targets.toml` names it.
    target: String,
    /// Roll back only when the rollback release has this id, or an id that starts with these 12 or
    /// more hex digits, so a release published since it was read is not the one that goes.
    #[arg(long, value_name = "ID")]
    expect: Option<String>,
    /// Do not ask before the rollback of an R2 target. Required without a terminal.
    #[arg(long)]
    yes: bool,
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
    super::api::writable(&args.target, &target)?;
    if let Target::R2(remote) = &target {
        let question = format!("Serve the rollback release of `{}` ({}) to riders again?", args.target, remote.url(""));
        super::confirm(&question, args.yes)?;
    }
    let mut run = super::api::start_run(&store, &["rollback", &args.target])?;
    let result = rollback((&args.target, &target), args.expect.as_deref(), &mut run);
    let rolled = super::api::finish_run(run, result)?;
    if json {
        return print_json(&rolled);
    }
    println!("{} serves release {}; rollback {}", rolled.target, &rolled.serving[..12], &rolled.rollback[..12]);
    Ok(())
}

/// Switch `target` to its rollback release, after a check that the target has each of its keys,
/// while the run holds the lock of the target.
pub(super) fn rollback(
    (name, target): (&str, &Target),
    expect: Option<&str>,
    run: &mut Run,
) -> Result<RolledBack, Error> {
    update_cli::locked((name, target), run, |run, held| {
        let rolled = switch((name, target), expect, (run, held))?;
        Ok(RolledBack { run: run.id().into(), target: name.into(), serving: rolled.0, rollback: rolled.1 })
    })
}

/// The switch of a rollback: the release that the target serves now, and its rollback.
fn switch(
    (name, target): (&str, &Target),
    expect: Option<&str>,
    (run, held): (&mut Run, &Held),
) -> Result<(String, String), Error> {
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
    if let Some(expect) = expect.filter(|expect| expect.len() < 12 || !rollback.id.starts_with(expect)) {
        return Err(Code::Blocked
            .error(format!("the rollback release of `{name}` is {}, not {expect}; nothing changed", &rollback.id[..12]))
            .fix(format!("Run `obc data` to see what `{name}` serves now.")));
    }
    update_cli::enter((run, held), name, Phase::Verify)?;
    let release = update_cli::read(name, target, &rollback.id)?;
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
    update_cli::enter((run, held), name, Phase::Switch)?;
    if !release::switch(target, Some(&base), &rollback.id)? {
        return Err(Code::Failed
            .error(format!("release.json of `{name}` changed during the rollback; nothing switched"))
            .fix(format!("Run `obc data` to see what `{name}` serves now.")));
    }
    run.record(&Event::Switched { release: rollback.id.clone() })?;
    run.sync()?;
    Ok((rollback.id, base.0.serving.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::update_cli::tests::{region, update};
    use crate::engine::tests::fixture;
    use crate::release::Release;

    fn rollback(
        fixture: &crate::engine::tests::Fixture,
        target: &Target,
        expect: Option<&str>,
    ) -> Result<RolledBack, Error> {
        let mut run = Run::create(&fixture.store, ["rollback", "local"]).unwrap();
        let rolled = super::rollback(("local", target), expect, &mut run);
        crate::cli::api::finish_run(run, rolled)
    }

    #[test]
    fn a_rollback_swaps_the_serving_and_the_rollback_release_and_deletes_nothing() {
        let fixture = fixture("rollback");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let a = update(&fixture, &target, region).unwrap().serving;
        let refused = rollback(&fixture, &target, None).unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (Code::Usage, "target `local` has no rollback release"));
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let b = update(&fixture, &target, |_| Ok(())).unwrap().serving;
        let keys = || target.list().unwrap().into_iter().map(|listed| listed.key).collect::<Vec<_>>();
        let before = keys();

        // A rollback that expects another release than the rollback changes nothing.
        let json = || target.get(release::SERVING).unwrap().unwrap();
        let read = json();
        let refused = rollback(&fixture, &target, Some(&b[..12])).unwrap_err();
        assert_eq!(refused.code, Code::Blocked, "{}", refused.message);
        assert_eq!(json(), read);

        for (serving, rollback_id, expect) in [(&a, &b, &a[..]), (&b, &a, &b[..12])] {
            let rolled = rollback(&fixture, &target, Some(expect)).unwrap();
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
        let refused = rollback(&fixture, &target, None).unwrap_err();
        assert_eq!(refused.code, Code::VerifyFailed, "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, b);
    }
}
