//! Every host mutation shares the publication owner's durable intent boundary.

use super::{Candidate, Installed, Ready, Stage, State, Vps};
use crate::commit::{Intent, Owner};
use crate::engine::runs::{Publication, Run};

pub(crate) struct Guarded<'a, V> {
    pub backend: &'a mut V,
    pub owner: &'a mut Owner,
    pub run: &'a mut Run,
}

impl<V: Vps> Vps for Guarded<'_, V> {
    fn inspect(&mut self) -> Result<State, String> {
        self.backend.inspect()
    }
    fn prepare(&mut self, staged: &Stage) -> Result<Stage, String> {
        self.backend.prepare(staged)
    }
    fn probe(&mut self, staged: &Stage) -> Result<Ready, String> {
        self.backend.probe(staged)
    }
    fn stage(&mut self, staged: &Stage) -> Result<(), String> {
        self.owner.mutate(
            self.run,
            Intent {
                mutation: Publication::ServiceStaged {
                    service: staged.installed.service.name().into(),
                    slot: staged.installed.slot,
                    binding: staged.installed.binding.clone(),
                },
                expected: None,
                desired: Some(staged.installed.binding.clone()),
            },
            || self.backend.stage(staged),
        )
    }
    fn activate(&mut self, stages: &[Stage]) -> Result<Vec<Ready>, String> {
        let bindings: Vec<_> = stages.iter().map(|stage| stage.installed.binding.clone()).collect();
        self.owner.mutate(
            self.run,
            Intent {
                mutation: Publication::ServicesActivated { bindings: bindings.clone() },
                expected: None,
                desired: Some(crate::store::sha256_hex(&serde_json::to_vec(&bindings).map_err(|e| e.to_string())?)),
            },
            || self.backend.activate(stages),
        )
    }
    fn retire(&mut self, previous: &[Installed], current: &[Installed]) -> Result<(), String> {
        let bindings: Vec<_> =
            previous.iter().filter(|old| !current.contains(old)).map(|old| old.binding.clone()).collect();
        if bindings.is_empty() {
            return Ok(());
        }
        self.owner.mutate(
            self.run,
            Intent {
                mutation: Publication::ServicesRetired { bindings: bindings.clone() },
                expected: Some(crate::store::sha256_hex(&serde_json::to_vec(&bindings).map_err(|e| e.to_string())?)),
                desired: None,
            },
            || self.backend.retire(previous, current),
        )
    }
}

pub(crate) fn stages(candidates: &[Candidate], installed: &[Installed]) -> Result<Vec<Stage>, String> {
    installed
        .iter()
        .map(|unit| {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.service == unit.service)
                .ok_or("service slot has no verified candidate metadata")?;
            if candidate.id != unit.id || candidate.binding() != unit.binding {
                return Err("service metadata differs from the published slot binding".into());
            }
            Ok(Stage { installed: unit.clone(), candidate: candidate.clone() })
        })
        .collect()
}

/// Restore the current entry before draining a slot from an acknowledged partial publication.
pub(crate) fn activate<V: Vps>(
    guarded: &mut Guarded<'_, V>,
    desired: &[Candidate],
    previous: &[Candidate],
    current: &[Installed],
    approved: &[Installed],
    wait: impl FnOnce(),
) -> Result<Vec<Stage>, String> {
    let inventory = guarded.inspect()?;
    for unit in current {
        if !inventory.installed.contains(unit) {
            return Err("current publication has no matching installed service slot".into());
        }
    }
    let conflicts: Vec<_> = inventory
        .installed
        .iter()
        .filter(|unit| {
            approved.iter().any(|wanted| wanted.service == unit.service && wanted.slot == unit.slot && wanted != *unit)
        })
        .cloned()
        .collect();
    if conflicts.iter().any(|unit| current.contains(unit)) {
        return Err("approved staging would replace a current published slot".into());
    }
    if !conflicts.is_empty() {
        let restored = stages(previous, current)?;
        for stage in &restored {
            if guarded.probe(stage)? != stage.candidate.expected {
                return Err("current service data readiness differs".into());
            }
        }
        let actual = guarded.activate(&restored)?;
        if actual != restored.iter().map(|stage| stage.candidate.expected.clone()).collect::<Vec<_>>() {
            return Err("restored public bindings report different data".into());
        }
        wait();
        guarded.retire(&conflicts, current)?;
    }
    let selected = super::stage(desired, current, guarded)?;
    if selected != approved {
        return Err("installed slots differ from the approved full pointer".into());
    }
    let staged = stages(desired, &selected)?;
    let actual = guarded.activate(&staged)?;
    if actual != staged.iter().map(|stage| stage.candidate.expected.clone()).collect::<Vec<_>>() {
        return Err("public service bindings report different data".into());
    }
    Ok(staged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::LayerFile,
        store::Store,
        vps::{Host, Service},
    };
    use std::{cell::RefCell, rc::Rc};

    struct Fake {
        state: State,
        events: Rc<RefCell<Vec<String>>>,
        fail_activate: bool,
    }

    impl Vps for Fake {
        fn inspect(&mut self) -> Result<State, String> {
            Ok(self.state.clone())
        }
        fn stage(&mut self, stage: &Stage) -> Result<(), String> {
            self.events.borrow_mut().push(format!("stage {}", stage.installed.service.name()));
            self.state.installed.push(stage.installed.clone());
            Ok(())
        }
        fn probe(&mut self, stage: &Stage) -> Result<Ready, String> {
            Ok(stage.candidate.expected.clone())
        }
        fn activate(&mut self, stages: &[Stage]) -> Result<Vec<Ready>, String> {
            self.events.borrow_mut().push("activate".into());
            if self.fail_activate {
                return Err("reload outcome is unknown".into());
            }
            Ok(stages.iter().map(|stage| stage.candidate.expected.clone()).collect())
        }
        fn retire(&mut self, previous: &[Installed], current: &[Installed]) -> Result<(), String> {
            self.events.borrow_mut().push("retire".into());
            self.state.installed.retain(|unit| !previous.contains(unit) || current.contains(unit));
            Ok(())
        }
    }

    fn candidates() -> Vec<Candidate> {
        [Service::Routing, Service::Search, Service::Downloads]
            .into_iter()
            .map(|service| Candidate {
                service,
                id: "a".repeat(64),
                target: Host {
                    triple: "x86_64-unknown-linux-gnu".into(),
                    glibc: "2.36".into(),
                    python: Some("3.12.0".into()),
                    node: Some("24.0.0".into()),
                },
                expected: match service {
                    Service::Routing => Ready::Routing { package: "a".repeat(64) },
                    Service::Search => Ready::Search { grid: "b".repeat(64), model: Default::default() },
                    Service::Downloads => Ready::Downloads { catalog: "c".repeat(64) },
                },
                source: "checked-view".into(),
                release: LayerFile { path: "release.json".into(), size: 1, sha256: "d".repeat(64) },
                runtime: LayerFile {
                    path: format!("runtime/{}.json", service.name()),
                    size: 1,
                    sha256: "e".repeat(64),
                },
                site_origin: "https://site.example".into(),
                api_origin: "https://api.example".into(),
                objects_url: "https://objects.example/planner/objects".into(),
            })
            .collect()
    }

    #[test]
    fn conflicting_inactive_slot_drains_before_staging_and_optional_data_preserves_other_services() {
        let temporary = crate::store::tests::Scratch::new("commit-services");
        let store = Store::at(temporary.0.join("store"));
        let mut run = Run::create(&store, "commit services").unwrap();
        let mut owner = Owner::open(&store.root().join("commits"), run.id(), b"services").unwrap();
        let previous = candidates();
        let current: Vec<_> = previous
            .iter()
            .map(|candidate| Installed {
                service: candidate.service,
                id: candidate.id.clone(),
                binding: candidate.binding(),
                slot: 0,
            })
            .collect();
        let mut desired = previous.clone();
        desired[2].id = "f".repeat(64);
        let mut approved = current.clone();
        approved[2] = Installed {
            service: Service::Downloads,
            id: desired[2].id.clone(),
            binding: desired[2].binding(),
            slot: 1,
        };
        let conflict = Installed { service: Service::Downloads, id: "b".repeat(64), binding: "c".repeat(64), slot: 1 };
        let events = Rc::new(RefCell::new(Vec::new()));
        let mut fake = Fake {
            state: State {
                host: desired[0].target.clone(),
                installed: current.iter().cloned().chain([conflict]).collect(),
            },
            events: events.clone(),
            fail_activate: false,
        };
        let mut guarded = Guarded { backend: &mut fake, owner: &mut owner, run: &mut run };
        let selected = activate(&mut guarded, &desired, &previous, &current, &approved, || {
            events.borrow_mut().push("reader window".into())
        })
        .unwrap();
        assert_eq!(
            selected.iter().map(|stage| &stage.installed).collect::<Vec<_>>(),
            approved.iter().collect::<Vec<_>>()
        );
        assert_eq!(*events.borrow(), ["activate", "reader window", "retire", "stage downloads", "activate"]);
        assert!(!owner.unknown());

        fake.fail_activate = true;
        let mut guarded = Guarded { backend: &mut fake, owner: &mut owner, run: &mut run };
        assert!(activate(&mut guarded, &desired, &desired, &approved, &approved, || panic!("no conflicting slot"))
            .unwrap_err()
            .contains("unknown"));
        assert!(owner.unknown(), "an ambiguous host mutation bars pointer switching and future commits");
    }
}
