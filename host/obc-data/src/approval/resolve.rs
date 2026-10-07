//! Approval uses the same declared code traversal as receipts, including acquisition and planning.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::json;

use super::{Execution, Observation, Review, Role, RuntimeBinding};
use crate::engine::code::Context;
use crate::engine::{Code, Input, OwnerCode, Profile, ResolvedRust, Run, Rust, Step};
use crate::env::Env;
use crate::product::Product;
use crate::regions::Regions;
use crate::sources::Source;

struct Declaration {
    code: Code,
    owner: Option<String>,
    runtime: Option<RuntimeBinding>,
}

impl Declaration {
    fn owner(owner: OwnerCode) -> Self {
        Self { code: owner.code, owner: Some(owner.crate_name), runtime: None }
    }

    fn identity(&self, root: &Path, context: &mut Context) -> Result<crate::engine::CodeIdentity, String> {
        match &self.owner {
            Some(crate_name) => {
                context.owner_identity(root, &OwnerCode { crate_name: crate_name.clone(), code: self.code.clone() })
            }
            None => context.identity(root, &self.code),
        }
    }

    fn witness(&self, root: &Path, context: &mut Context, rust: Option<&ResolvedRust>) -> Result<String, String> {
        let identity = match &self.owner {
            Some(crate_name) => context.owner_source_config(
                root,
                &OwnerCode { crate_name: crate_name.clone(), code: self.code.clone() },
                rust.ok_or("planning/acquisition witness has no recorded native context")?,
            )?,
            None => context.source_config(root, &self.code, rust)?,
        };
        // Physical selected files stay committed even when an unused runtime cannot be selected.
        identity.committed(root)?;
        hash(&(&identity.files, self.runtime.as_ref().map(|binding| &binding.target)))
    }
}

/// Geographic step IDs and source versions are data; roles name existing code declarations.
fn producer(
    declarations: &mut BTreeMap<String, Declaration>,
    step: &Step,
    runtime: Option<RuntimeBinding>,
) -> Result<(), String> {
    let mut code = step.code.clone();
    for items in [&mut code.paths, &mut code.crates, &mut code.sources] {
        items.sort();
        items.dedup();
    }
    if code.rust.is_none() && !code.crates.is_empty() {
        code.rust = Some(Rust::Native { profile: Profile::Dev });
    }
    code.libraries.sort_by(|a, b| (&a.name, &a.path, &a.sha256).cmp(&(&b.name, &b.path, &b.sha256)));
    code.libraries.dedup();
    let libraries = std::mem::take(&mut code.libraries);
    let command = match &step.run {
        Run::Rust(_) => json!({"kind":"rust"}),
        Run::Command(argv) => json!({"kind":"command","argv":argv}),
    };
    let role = format!(
        "producer/{}",
        hash(&(
            &code,
            libraries.iter().map(|library| &library.name).collect::<BTreeSet<_>>(),
            command,
            runtime.as_ref().map(|binding| &binding.target),
        ))?
    );
    code.libraries = libraries;
    if let Some(previous) = declarations.get(&role) {
        if previous.code != code || previous.runtime != runtime {
            return Err("a shared producer role has conflicting execution bindings".into());
        }
    } else {
        declarations.insert(role, Declaration { code, owner: None, runtime });
    }
    Ok(())
}

fn hash(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_vec(value).map(|bytes| crate::store::sha256_hex(&bytes)).map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn review(
    root: &Path,
    env: &Env,
    regions: &Regions,
    steps: &[Step],
    products: &[&dyn Product],
    sources: &[Source],
    bucket: &str,
    prior: Result<Observation, String>,
) -> Review {
    let owner = prior.as_ref().ok().map(|prior| prior.owner.clone());
    let build = || -> Result<Review, String> {
        let prior = prior?;
        prior.check()?;
        let mut config = BTreeMap::new();
        for product in products {
            config.insert(product.name(), product.approval_config(root)?);
        }
        let mut selected_regions = Vec::new();
        selected_regions.push(regions.get(&env.region).ok_or("approval region is missing")?);
        for leaf in regions.leaves(&env.region)? {
            if leaf != env.region {
                selected_regions.push(regions.get(leaf).ok_or("approval region leaf is missing")?);
            }
        }
        let mut layers = env.layers.clone();
        layers.sort();
        let mut declarations = BTreeMap::new();
        for product in products {
            if let Some(owner) = product.planning_code(env)? {
                declarations.insert(format!("planning/{}", product.name()), Declaration::owner(owner));
            }
        }
        let mut selected_sources: BTreeSet<_> =
            env.requests.borrow().iter().map(|(source, _)| source.clone()).collect();
        let mut unavailable = Vec::new();
        for step in steps {
            selected_sources.extend(step.code.sources.iter().cloned());
            for input in &step.inputs {
                if let Input::Snapshot { source, .. } = input {
                    selected_sources.insert(source.clone());
                }
            }
            let mut runtime = None;
            for product in products {
                match product.runtime_binding(step) {
                    Ok(Some(binding)) => {
                        if runtime.as_ref().is_some_and(|previous| previous != &binding) {
                            return Err("a runtime has conflicting provider declarations".into());
                        }
                        runtime = Some(binding);
                    }
                    Ok(None) => {}
                    Err(reason) => unavailable.push(format!("{}: {reason}", step.name)),
                }
            }
            producer(&mut declarations, step, runtime)?;
        }
        let mut pending: Vec<_> = selected_sources.iter().cloned().collect();
        while let Some(id) = pending.pop() {
            let source = sources
                .iter()
                .find(|source| source.id == id)
                .ok_or_else(|| format!("approval source `{id}` is missing"))?;
            if let Some(base) = &source.fetch.from {
                if selected_sources.insert(base.clone()) {
                    pending.push(base.clone());
                }
            }
        }
        let mut source_settings = BTreeMap::new();
        for id in selected_sources {
            let source = sources
                .iter()
                .find(|source| source.id == id)
                .ok_or_else(|| format!("approval source `{id}` is missing"))?;
            source_settings.insert(id.clone(), crate::engine::code::source_hash(source, &["refresh"])?);
            let owner = crate::fetch::owner_code(source);
            declarations.insert(format!("acquisition/{id}"), Declaration::owner(owner));
        }
        let config =
            hash(&json!({"regions":selected_regions,"layers":layers,"products":config,"sources":source_settings}))?;
        if declarations.is_empty() {
            return Ok(Review::Unavailable {
                reason: "no complete product declares acquisition, planning or producer code".into(),
                owner: Some(prior.owner),
            });
        }
        let mut context = Context::default();
        let mut native = None;
        let mut roles = BTreeMap::new();
        for (name, declaration) in &declarations {
            let identity = declaration.identity(root, &mut context);
            match identity {
                Ok(identity) => {
                    identity.committed(root)?;
                    if let Some(ResolvedRust { target, build: Rust::Native { profile } }) = &identity.rust {
                        let selected = (target.clone(), *profile);
                        if native.as_ref().is_some_and(|current| current != &selected) {
                            unavailable
                                .push("the product graph declares more than one native execution context".into());
                        } else {
                            native = Some(selected);
                        }
                    }
                    roles.insert(
                        name.clone(),
                        Role {
                            source_config: hash(&(
                                &identity.source_config,
                                declaration.runtime.as_ref().map(|binding| &binding.target),
                            ))?,
                            execution: hash(&(&identity.files, &declaration.runtime))?,
                            rust: identity.rust,
                        },
                    );
                }
                Err(reason) => unavailable.push(format!("{name}: {reason}")),
            }
        }
        if native.is_none() {
            unavailable.push("no scoped owner declares a checked native execution context".into());
        }
        let checked = if unavailable.is_empty() {
            let (target, profile) = native.unwrap();
            Some(Execution { target, profile, roles })
        } else {
            None
        };
        let executions = super::retain(&prior, bucket, &config, checked, |entry| {
            entry.roles.len() == declarations.len()
                && entry.roles.iter().all(|(name, role)| {
                    declarations.get(name).is_some_and(|declaration| {
                        declaration
                            .witness(root, &mut context, role.rust.as_ref())
                            .is_ok_and(|witness| witness == role.source_config)
                    })
                })
        });
        Ok(Review::Ready {
            owner: prior.owner,
            expected: prior.sha256,
            config,
            executions,
            unavailable: (!unavailable.is_empty()).then(|| unavailable.join("; ")),
        })
    };
    build().unwrap_or_else(|reason| Review::Unavailable { reason, owner })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::Current;
    use crate::engine::tests::{fixture, repository, write};

    #[test]
    fn producer_roles_ignore_geographic_data_but_bind_code_commands_and_provider_execution() {
        let step = |name: &str, version: &str, code: Code| Step {
            name: name.into(),
            inputs: vec![Input::Snapshot {
                source: "osm".into(),
                version: version.into(),
                params: vec![],
                files: vec![],
            }],
            options: json!({"bounds":[version]}),
            code,
            outputs: vec!["cells".into()],
            run: Run::Command(vec!["python".into(), "-m".into(), "tools.cells".into()]),
            client: crate::engine::Client::All,
        };
        let code = Code {
            paths: vec!["z.rs".into(), "a.rs".into()],
            crates: vec!["draw".into()],
            libraries: vec![crate::engine::Library {
                name: "geos-c".into(),
                path: "/provider/libgeos_c".into(),
                sha256: "a".repeat(64),
            }],
            ..Default::default()
        };
        let first = step("maps/alps/geometry", "old", code.clone());
        let mut reordered = code.clone();
        reordered.paths.reverse();
        reordered.rust = Some(Rust::Native { profile: Profile::Dev });
        let second = step("maps/coast/geometry", "new", reordered);
        let mut declarations = BTreeMap::new();
        producer(&mut declarations, &first, None).unwrap();
        producer(&mut declarations, &second, None).unwrap();
        assert_eq!(declarations.len(), 1, "available leaf inventory and source versions are not approval roles");
        let mut changed = code.clone();
        changed.libraries[0].sha256 = "b".repeat(64);
        assert!(producer(&mut declarations, &step("maps/other/geometry", "new", changed), None)
            .unwrap_err()
            .contains("conflicting"));
        let mut other_command = step("maps/alps/geometry", "old", code.clone());
        other_command.run = Run::Command(vec!["python".into(), "-m".into(), "tools.other".into()]);
        producer(&mut declarations, &other_command, None).unwrap();
        assert_eq!(declarations.len(), 2, "execution entrypoint is part of the declaration");
        let runtime = RuntimeBinding { target: json!({"target":"linux"}), builder: json!({"image":"first"}) };
        let mut runtimes = BTreeMap::new();
        producer(&mut runtimes, &first, Some(runtime.clone())).unwrap();
        let changed = RuntimeBinding { builder: json!({"image":"second"}), ..runtime };
        assert!(producer(&mut runtimes, &second, Some(changed)).unwrap_err().contains("conflicting"));
    }

    #[test]
    fn recorded_target_witness_preserves_checked_tools_until_used_planning_code_changes() {
        let fixture = fixture("approval-recorded-witness");
        let root = fixture.root();
        repository(&root, &[
            ("steps", "[dependencies]\nobc-data = { path = \"../obc-data\" }\n[target.'cfg(target_os = \"linux\")'.dependencies]\nlinux = { path = \"../linux\" }\n"),
            ("obc-data", ""), ("linux", ""),
        ]);
        write(&root.join("steps/src/plan.rs"), "pub fn plan() {}\n");
        write(&root.join("obc-data/src/cli/tui.rs"), "pub fn draw() {}\n");
        let commit = || {
            for args in [
                vec!["add", "."],
                vec!["-c", "user.name=Fixture", "-c", "user.email=fixture@example.org", "commit", "-qm", "fixture"],
            ] {
                assert!(std::process::Command::new("git").args(args).current_dir(&root).status().unwrap().success());
            }
        };
        commit();
        let declaration = Declaration::owner(OwnerCode {
            crate_name: "steps".into(),
            code: Code { paths: vec!["steps/src/plan.rs".into()], ..Default::default() },
        });
        let recorded =
            ResolvedRust { target: "x86_64-unknown-linux-gnu".into(), build: Rust::Native { profile: Profile::Dev } };
        let mut context = Context::default();
        let source = declaration.witness(&root, &mut context, Some(&recorded)).unwrap();
        let entry = Execution {
            target: recorded.target.clone(),
            profile: Profile::Dev,
            roles: BTreeMap::from([(
                "planning/maps".into(),
                Role {
                    rust: Some(recorded.clone()),
                    source_config: source.clone(),
                    execution: crate::store::sha256_hex(b"previously checked VPS tools"),
                },
            )]),
        };
        let config = crate::store::sha256_hex(b"same publication settings");
        let prior = Observation {
            owner: crate::store::sha256_hex(b"fixture owner"),
            sha256: Some(crate::store::sha256_hex(b"record")),
            current: Some(Current {
                owner: crate::store::sha256_hex(b"fixture owner"),
                bucket: "bucket".into(),
                config: config.clone(),
                executions: vec![entry.clone()],
                unavailable: None,
                run: "2026-10-07-000000".into(),
                bundle: crate::store::sha256_hex(b"bundle"),
                publication: crate::store::sha256_hex(b"publication"),
            }),
        };
        let comparable = |context: &mut Context, entry: &Execution| {
            declaration
                .witness(&root, context, entry.roles["planning/maps"].rust.as_ref())
                .is_ok_and(|value| value == entry.roles["planning/maps"].source_config)
        };
        write(&root.join("obc-data/src/cli/tui.rs"), "pub fn other_screen() {}\n");
        assert_eq!(
            super::super::retain(&prior, "bucket", &config, None, |entry| comparable(&mut context, entry)),
            [entry]
        );
        write(&root.join("steps/src/plan.rs"), "pub fn different_selection() {}\n");
        assert!(declaration.witness(&root, &mut context, Some(&recorded)).unwrap_err().contains("not committed"));
        commit();
        assert_ne!(declaration.witness(&root, &mut context, Some(&recorded)).unwrap(), source);
        assert!(
            super::super::retain(&prior, "bucket", &config, None, |entry| comparable(&mut context, entry)).is_empty(),
            "old checked execution cannot approve newly committed planning code"
        );
    }
}
