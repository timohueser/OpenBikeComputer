//! Automatic work consumes the current approval; it cannot establish another one.

use std::collections::BTreeMap;
use std::path::Path;

use super::{Execution, Observation, Review};
use crate::engine::code::Context;
use crate::engine::{OwnerCode, Rust};
use crate::env::Env;
use crate::product::Product;
use crate::regions::Regions;
use crate::sources::Source;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Admission {
    pub observation: Observation,
    execution: Execution,
}

impl Admission {
    pub fn check_owner(&self, root: &Path, context: &mut Context, role: &str, owner: &OwnerCode) -> Result<(), String> {
        let expected = self.execution.roles.get(role).ok_or_else(manual)?;
        super::resolve::checked_owner(root, context, owner, expected)
    }

    pub fn check(&self, review: &Review) -> Result<(), String> {
        let current = self.observation.current.as_ref().ok_or_else(manual)?;
        match review {
            Review::Ready { owner, expected, config, executions, unavailable: None }
                if owner == &current.owner
                    && expected == &self.observation.sha256
                    && config == &current.config
                    && executions.iter().any(|entry| entry == &self.execution) =>
            {
                Ok(())
            }
            _ => Err(manual()),
        }
    }

    pub fn digest(&self) -> &str {
        self.observation.sha256.as_deref().expect("admission requires a present approval")
    }

    pub fn recheck(&self, store: &crate::store::Store, review: &Review) -> Result<(), String> {
        self.observation.check()?;
        self.check(review)?;
        if super::read(store)? != self.observation {
            return Err("automatic approval changed before the owner lock; retry".into());
        }
        Ok(())
    }
}

pub(crate) fn admit(
    root: &Path,
    env: &Env,
    regions: &Regions,
    products: &[&dyn Product],
    sources: &[Source],
    bucket: &str,
    observation: Observation,
) -> Result<Admission, String> {
    observation.check()?;
    let current = observation.current.as_ref().ok_or_else(manual)?;
    if observation.sha256.is_none() || current.bucket != bucket {
        return Err(manual());
    }
    let mut context = Context::default();
    let mut owners = Vec::new();
    for product in products {
        if let Some(owner) = product.planning_code(env)? {
            owners.push((format!("planning/{}", product.name()), owner));
        }
    }
    let (_, first) = owners.first().ok_or_else(manual)?;
    let rust = context.owner_identity(root, first)?.rust.ok_or_else(manual)?;
    let Rust::Native { profile } = rust.build else { return Err(manual()) };
    let execution = current
        .executions
        .iter()
        .find(|entry| entry.target == rust.target && entry.profile == profile)
        .ok_or_else(manual)?
        .clone();
    let mut settings = BTreeMap::new();
    for role in execution.roles.keys().filter_map(|role| role.strip_prefix("acquisition/")) {
        let source = sources.iter().find(|source| source.id == role).ok_or_else(manual)?;
        settings.insert(role.to_string(), crate::engine::code::source_hash(source, &["refresh"])?);
        owners.push((format!("acquisition/{role}"), crate::fetch::owner_code(source)));
    }
    if super::resolve::configuration(root, env, regions, products, &settings)? != current.config {
        return Err(manual());
    }
    let admission = Admission { observation, execution };
    for (role, owner) in owners {
        admission.check_owner(root, &mut context, &role, &owner)?;
    }
    Ok(admission)
}

fn manual() -> String {
    "Live automation needs a matching checked approval; complete a reviewed manual apply".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::Role;
    use crate::engine::tests::{fixture, write};
    use crate::engine::Code;
    use crate::product::{Steps, Unplanned};
    use crate::store::{sha256_hex, Store};

    struct Planning(OwnerCode);
    impl Product for Planning {
        fn name(&self) -> &'static str {
            "test"
        }
        fn planning_code(&self, _: &Env) -> Result<Option<OwnerCode>, String> {
            Ok(Some(self.0.clone()))
        }
        fn steps(&self, _: &Path, _: &Env, _: &Regions, _: &Store) -> Result<Steps, Unplanned> {
            unreachable!()
        }
    }

    fn hash(value: &impl serde::Serialize) -> String {
        sha256_hex(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn original_tools_are_required_before_work_and_a_prospective_manual_review_cannot_approve_them() {
        let fixture = fixture("auto-original-approval");
        let root = fixture.root();
        write(&root.join("steps/src/plan.rs"), "pub fn select() {}\n");
        write(&root.join("steps/src/tui.rs"), "pub fn draw() {}\n");
        write(&root.join("data/regions/test.toml"), "name=\"Test\"\nkind=\"box\"\nbox=[7.0,46.0,8.0,47.0]\n");
        let commit = || {
            for args in [
                vec!["add", "."],
                vec!["-c", "user.name=Fixture", "-c", "user.email=fixture@example.org", "commit", "-qm", "fixture"],
            ] {
                assert!(std::process::Command::new("git").args(args).current_dir(&root).status().unwrap().success());
            }
        };
        commit();
        let regions = Regions::load(&root).unwrap();
        let env = Env::parse("live", "region=\"test\"\n", &regions).unwrap();
        let owner = OwnerCode {
            crate_name: "steps".into(),
            code: Code { paths: vec!["steps/src/plan.rs".into()], ..Default::default() },
        };
        let product = Planning(owner.clone());
        let identity = owner.identity(&root).unwrap();
        let rust = identity.rust.clone().unwrap();
        let Rust::Native { profile } = rust.build else { unreachable!() };
        let execution = Execution {
            target: rust.target,
            profile,
            roles: BTreeMap::from([(
                "planning/test".into(),
                Role {
                    rust: identity.rust,
                    source_config: hash(&(&identity.source_config, None::<&serde_json::Value>)),
                    execution: hash(&(&identity.files, None::<&super::super::RuntimeBinding>)),
                },
            )]),
        };
        let config =
            super::super::resolve::configuration(&root, &env, &regions, &[&product], &BTreeMap::new()).unwrap();
        let run = crate::engine::runs::Run::create(&fixture.store, "apply live").unwrap();
        let id = run.id().to_string();
        let bundle = b"original checked manual publication";
        let mut owner = crate::commit::Owner::open(&fixture.store.root().join("commits"), &id, bundle).unwrap();
        let initial = super::super::read(&fixture.store).unwrap();
        let reviewed = Review::Ready {
            owner: initial.owner,
            expected: None,
            config: config.clone(),
            executions: vec![execution.clone()],
            unavailable: None,
        };
        let publication = b"{}";
        crate::commit::durable(&fixture.store.root().join("commits").join(format!("{id}.result")), publication)
            .unwrap();
        run.finish(None).unwrap();
        owner.finish().unwrap();
        assert!(matches!(
            super::super::record(
                &fixture.store,
                &reviewed,
                "bucket",
                &id,
                &sha256_hex(bundle),
                &sha256_hex(publication)
            ),
            super::super::Outcome::Recorded { .. }
        ));
        drop(owner);
        let observation = super::super::read(&fixture.store).unwrap();
        let record = super::super::path(&fixture.store);
        let approved_bytes = std::fs::read(&record).unwrap();
        let admitted = admit(&root, &env, &regions, &[&product], &[], "bucket", observation.clone()).unwrap();
        let review = Review::Ready {
            owner: observation.owner.clone(),
            expected: observation.sha256.clone(),
            config,
            executions: vec![execution],
            unavailable: None,
        };
        admitted.check(&review).unwrap();
        admitted.recheck(&fixture.store, &review).unwrap();
        let mut replacement = observation.current.clone().unwrap();
        replacement.config = sha256_hex(b"newly approved scope");
        crate::commit::durable(&record, &serde_json::to_vec(&replacement).unwrap()).unwrap();
        assert!(admitted.recheck(&fixture.store, &review).unwrap_err().contains("changed before the owner lock"));
        crate::commit::durable(&record, &approved_bytes).unwrap();
        admitted.recheck(&fixture.store, &review).unwrap();
        assert_eq!(std::fs::read(&record).unwrap(), approved_bytes, "automatic checks cannot establish a new approval");
        let mut changed = review.clone();
        let Review::Ready { executions, .. } = &mut changed else { unreachable!() };
        executions[0].roles.get_mut("planning/test").unwrap().execution =
            sha256_hex(b"replacement same-version compiler");
        assert!(admitted.check(&changed).unwrap_err().contains("manual apply"));
        write(&root.join("steps/src/tui.rs"), "pub fn other_screen() {}\n");
        admit(&root, &env, &regions, &[&product], &[], "bucket", observation.clone()).unwrap();
        write(&root.join("steps/src/plan.rs"), "pub fn different_selection() {}\n");
        assert!(admit(&root, &env, &regions, &[&product], &[], "bucket", observation.clone()).is_err());
        commit();
        assert!(admit(&root, &env, &regions, &[&product], &[], "bucket", observation)
            .unwrap_err()
            .contains("manual apply"));
        assert_eq!(std::fs::read(&record).unwrap(), approved_bytes, "failed admission preserves the original approval");
    }
}
