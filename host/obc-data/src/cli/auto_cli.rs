//! Automatic refresh uses the existing detached operation and verified build.

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use super::{api, apply_cli, build_cli, commit_cli, operation_cli, Code, Error};
use crate::fetch::http::Http;
use crate::live::Remote;
use crate::product::Product;
use crate::store::Store;

#[derive(Debug, Serialize, JsonSchema)]
pub struct Result {
    pub built: build_cli::Built,
    pub approval: Option<String>,
    /// Publication requires checked enabled-timer admission.
    pub publication: String,
    pub applied: Option<apply_cli::Applied>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Started {
    Admitted(operation_cli::Handle),
    Skipped { skipped: bool, env: String, reason: String, run: Option<String> },
}

fn started(
    result: std::result::Result<operation_cli::Handle, Error>,
    env: String,
) -> std::result::Result<Started, Error> {
    match result {
        Ok(handle) => Ok(Started::Admitted(handle)),
        Err(error) if error.code == Code::Busy => {
            Ok(Started::Skipped { skipped: true, env, reason: error.message, run: error.run })
        }
        Err(error) => Err(error),
    }
}

pub(super) fn check_env(name: &str) -> std::result::Result<(), Error> {
    if name == "fixture" || name == "fixtures" || name.starts_with("fixture-") {
        return Err(Code::Usage.error("fixture environments do not support automation"));
    }
    Ok(())
}

pub(super) fn start(root: &Path, env: String, json: bool) -> std::result::Result<(), Error> {
    check_env(&env)?;
    let store = Store::open()?;
    let request = crate::operation::Request {
        kind: crate::operation::Kind::Auto,
        env: env.clone(),
        only: Vec::new(),
        moves: Vec::new(),
        plan: None,
        dev: None,
    };
    match started(operation_cli::start(root, &store, request, None), env)? {
        Started::Admitted(handle) => operation_cli::print_handle(&handle, json),
        skipped @ Started::Skipped { .. } => {
            if json {
                api::print_json(&skipped)
            } else {
                println!("automatic run skipped: the environment is busy; retry at the next scheduled time");
                Ok(())
            }
        }
    }
}

pub(super) fn perform(root: &Path, products: &[&dyn Product], env: &str) -> std::result::Result<(), Error> {
    let store = Store::open()?;
    let http = Http::new();
    let remote = (env == "live").then(super::remote).transpose()?;
    let mut run = api::start_run(&store, &format!("auto {env}"))?;
    let (mut result, applying) = match execute(root, &store, &http, remote.as_ref(), products, env, &mut run) {
        Ok(executed) => executed,
        Err(error) => return api::finish_run(run, Err(error), None),
    };
    let pending = (|| -> std::result::Result<Option<apply_cli::Pending>, Error> {
        if let Some(applying) = applying {
            if cfg!(target_os = "linux") && crate::schedule::state(root, &store, env)?.runnable {
                return apply_cli::publication(
                    root,
                    &store,
                    &http,
                    remote.as_ref().expect("Live remote"),
                    products,
                    &applying.plan,
                    applying.next,
                    &mut run,
                )
                .map(Some);
            }
        }
        Ok(None)
    })();
    let pending = match pending {
        Ok(pending) => pending,
        Err(error) => return api::finish_run(run, Err(error), None),
    };
    if let Some(pending) = pending {
        let id = run.id().to_string();
        drop(run);
        #[cfg(not(test))]
        let committed = commit_cli::submit_checked(&pending.directory, &pending.digest, &id, &store, Some(root));
        #[cfg(test)]
        let committed = match crate::schedule::handoff(root, &store)? {
            Some(lock) => {
                drop(lock);
                commit_cli::execute_for_test(
                    &pending.directory,
                    &pending.digest,
                    &store,
                    remote.as_ref().unwrap(),
                    apply_cli::WAIT,
                )
                .map(Some)
            }
            None => Ok(None),
        };
        match committed {
            Ok(Some(committed)) => {
                let mut applied = apply_cli::Applied { run: id, ..Default::default() };
                committed.apply(&mut applied);
                result.publication = "applied under the original manual approval".into();
                result.applied = Some(applied);
                return api::print_json(&result);
            }
            Ok(None) => {
                run = crate::engine::runs::Run::attach(&store, &id, &crate::engine::runs::events(&store, &id)?)?
            }
            Err(error) => return Err(error.with_run(&id)),
        }
    }
    let result = api::finish_run(run, Ok(result), None)?;
    api::print_json(&result)
}

#[allow(clippy::too_many_arguments)]
fn execute(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: Option<&Remote>,
    products: &[&dyn Product],
    env: &str,
    run: &mut crate::engine::runs::Run,
) -> std::result::Result<(Result, Option<build_cli::Applying>), Error> {
    check_env(env)?;
    if let Some(remote) = remote {
        apply_cli::committed(root)?;
        let loaded = build_cli::load(root, env)?;
        let observed = commit_cli::approval_observation(store).map_err(|reason| Code::Blocked.error(reason))?;
        run.automatic = Some(
            crate::approval::admit(
                root,
                &loaded.env,
                &loaded.regions,
                products,
                &loaded.sources,
                remote.describe(),
                observed,
            )
            .map_err(|reason| Code::Blocked.error(reason))?,
        );
        run.require_committed_code();
    }
    let args = build_cli::BuildArgs { env: env.into(), only: Vec::new(), moves: Vec::new(), plan: None };
    let (built, applying) = build_cli::build_env(root, store, http, remote, products, &args, None, run)?;
    if !built.blocked.is_empty() {
        return Err(Code::Blocked.error("automatic build is incomplete; no publication starts"));
    }
    if let Some(applying) = &applying {
        apply_cli::verify_products(root, products, store, &applying.next)?;
        build_cli::recheck_approval(
            root,
            store,
            http,
            remote.expect("Live build has a remote"),
            products,
            &applying.plan,
            run,
        )?;
        let observed = commit_cli::approval_observation(store).map_err(|reason| Code::Blocked.error(reason))?;
        if run.automatic.as_ref().is_none_or(|approved| approved.observation != observed) {
            return Err(Code::PlanOutdated.error("automatic approval changed during the build; retry"));
        }
    } else {
        for published in &built.releases {
            let product = products
                .iter()
                .find(|product| product.name() == published.product)
                .ok_or_else(|| Code::VerifyFailed.error("built product has no verifier"))?;
            let release = crate::engine::release::Release::read(store, &published.product, &published.id)?;
            product.verify(root, None, &release, store).map_err(|reason| Code::VerifyFailed.error(reason))?;
        }
    }
    run.check_stop(store)?;
    crate::worker::check(root)?;
    Ok((
        Result {
            built,
            approval: run.automatic.as_ref().map(|approved| approved.digest().into()),
            publication: "verified, not applied: no checked enabled live timer at publication handoff".into(),
            applied: None,
        },
        applying,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, write};
    use crate::engine::{Client, Code as RecipeCode, Input, Run as StepRun};
    use crate::product::{Steps, Unplanned};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn busy_admission_is_a_successful_skip_but_real_failures_keep_their_error() {
        let skipped =
            started(Err(Code::Busy.error("known reservation busy").with_run("2026-10-07-120000")), "live".into())
                .unwrap();
        let value = serde_json::to_value(skipped).unwrap();
        assert_eq!(value["skipped"], true);
        assert_eq!(value["env"], "live");
        assert_eq!(value["run"], "2026-10-07-120000");
        for code in [Code::Blocked, Code::Failed, Code::PlanOutdated] {
            let error = started(Err(code.error("operation X already owns live; real setup failure")), "live".into())
                .unwrap_err();
            assert_eq!(error.code, code);
            assert_ne!(error.code.exit(), 0);
        }
        let handle = operation_cli::Handle { run: "run".into(), request: "request".into() };
        assert_eq!(
            serde_json::to_value(started(Ok(handle), "live".into()).unwrap()).unwrap(),
            serde_json::json!({"run":"run", "request":"request"})
        );
    }

    struct Data {
        calls: AtomicUsize,
        verifies: AtomicUsize,
        fail: AtomicBool,
    }
    impl Product for Data {
        fn name(&self) -> &'static str {
            "test"
        }
        fn steps(
            &self,
            _: &Path,
            _: &crate::env::Env,
            _: &crate::regions::Regions,
            _: &Store,
        ) -> std::result::Result<Steps, Unplanned> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut step = crate::engine::tests::step(
                "test/data",
                vec![Input::Snapshot {
                    source: "head".into(),
                    version: "1".into(),
                    params: Vec::new(),
                    files: vec!["head.txt".into()],
                }],
                RecipeCode { crates: vec!["steps".into()], ..Default::default() },
                "data.bin",
                StepRun::Rust(crate::engine::pass),
            );
            step.options = serde_json::json!({"path":"data.bin"});
            step.client = Client::All;
            Ok(vec![step].into())
        }
        fn verify(
            &self,
            _: &Path,
            _: Option<&crate::engine::release::Release>,
            release: &crate::engine::release::Release,
            store: &Store,
        ) -> std::result::Result<(), String> {
            self.verifies.fetch_add(1, Ordering::SeqCst);
            assert_eq!(std::fs::read(store.object(&release.layers[0].files[0].sha256)).unwrap(), b"head\n");
            if self.fail.load(Ordering::SeqCst) {
                Err("authored verifier refusal".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn local_auto_keeps_dirty_code_and_real_verification_while_live_requires_original_approval_before_work() {
        let fixture = fixture("auto-local-and-live");
        let root = fixture.root();
        write(&root.join("data/regions/test.toml"), "name=\"Test\"\nkind=\"box\"\nbox=[7.0,46.0,8.0,47.0]\n");
        write(&root.join("data/env/local.toml"), "region=\"test\"\n");
        write(&root.join("data/env/live.toml"), "region=\"test\"\n");
        write(&root.join("data/sources.toml"), "source=[]\n");
        write(&root.join("steps/src/lib.rs"), "pub fn dirty_local_code() {}\n");
        let data = Data { calls: AtomicUsize::new(0), verifies: AtomicUsize::new(0), fail: AtomicBool::new(false) };
        let http = Http::new();
        let mut run = crate::engine::runs::Run::create(&fixture.store, "auto local").unwrap();
        let (local, _) = execute(&root, &fixture.store, &http, None, &[&data], "local", &mut run).unwrap();
        assert!(local.approval.is_none());
        assert_eq!(local.built.layers.len(), 1);
        assert!(local.publication.starts_with("verified, not applied"));
        assert_eq!(data.verifies.load(Ordering::SeqCst), 1);
        data.fail.store(true, Ordering::SeqCst);
        let error = execute(&root, &fixture.store, &http, None, &[&data], "local", &mut run).err().unwrap();
        assert_eq!(error.code, Code::VerifyFailed);
        assert_eq!(data.verifies.load(Ordering::SeqCst), 2, "an unchanged build still has real verification");
        run.finish(None).unwrap();
        for args in [
            vec!["add", "."],
            vec!["-c", "user.name=Fixture", "-c", "user.email=fixture@example.org", "commit", "-qm", "fixture"],
        ] {
            assert!(std::process::Command::new("git").args(args).current_dir(&root).status().unwrap().success());
        }
        let bucket = fixture.scratch.0.join("bucket");
        std::fs::create_dir(&bucket).unwrap();
        let remote = Remote::Bucket(crate::r2::Bucket::local(&bucket));
        let before = data.calls.load(Ordering::SeqCst);
        let mut run = crate::engine::runs::Run::create(&fixture.store, "auto live").unwrap();
        let error = execute(&root, &fixture.store, &http, Some(&remote), &[&data], "live", &mut run).err().unwrap();
        assert!(error.message.contains("reviewed manual apply"), "{error:?}");
        assert_eq!(data.calls.load(Ordering::SeqCst), before, "no product planning or acquisition precedes approval");
        assert!(!fixture.store.root().join("commits/current.approval").exists());
        assert!(check_env("fixture-cells").is_err());
    }
}
