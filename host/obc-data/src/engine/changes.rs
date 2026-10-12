//! The plan against live: one group per layer that differs from its live layer, with its cause as
//! [`state`](super::state) judges it.

use std::collections::{BTreeMap, BTreeSet};

use super::plan::{build, build_of, needs, sized, walk, Cause, Group, Plan};
use super::release::Layer;
use super::state::judge;
use super::Step;
use crate::store::Store;

/// What the steps are compared with.
#[derive(Default)]
pub struct Against<'a> {
    /// Each layer of the live releases of the products that the steps make, by step.
    pub layers: BTreeMap<&'a str, &'a Layer>,
    /// The keys of live that R2 lacks or holds with another size, and the layers of their files.
    pub drift: Option<(Vec<String>, BTreeSet<String>)>,
}

/// One group per layer that is stale or that only live has, then a group that repairs the drift.
pub fn changes(store: &Store, steps: &[Step], against: &Against) -> Result<Plan, String> {
    let walked = walk(store, steps)?;
    let receipts = store.layers()?;
    let (mut groups, mut unchanged) = (Vec::new(), BTreeSet::new());
    for layer in judge(store, steps, &against.layers, &BTreeMap::new())? {
        let changed: BTreeSet<&str> =
            layer.steps.iter().filter(|(_, cause)| cause.is_some()).map(|(step, _)| step.name.as_str()).collect();
        unchanged.extend(layer.steps.iter().filter(|(_, cause)| cause.is_none()).map(|(step, _)| step.name.as_str()));
        let cause = match (layer.cause, layer.steps.is_empty()) {
            (_, true) => Cause::Off,
            (Some(cause), false) => Cause::Stale { cause },
            (None, false) => continue,
        };
        let group = Group::new(layer.name, Some(cause));
        let mut group = needs(&walked, group, &changed, |walked| build(walked, &receipts));
        let layers = walked.iter().filter(|walked| changed.contains(walked.step.name.as_str()));
        group.layers = layers.map(|walked| build_of(walked, &receipts)).collect();
        group.drops = layer.drops;
        groups.push(group);
    }
    if let Some((keys, owners)) = against.drift.as_ref().filter(|(keys, _)| !keys.is_empty()) {
        // A group makes a changed layer again; the store gives the files of the others.
        let unchanged = owners.iter().map(String::as_str).filter(|name| unchanged.contains(name)).collect();
        let repair = Group::new("repair".into(), Some(Cause::Repair { keys: keys.clone() }));
        groups.push(needs(&walked, repair, &unchanged, |walked| build(walked, &receipts)));
    }
    sized(store, groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::state::{Cause as Why, Field, OPTIONS};
    use crate::engine::tests::{fixture, layered, TEST};

    /// The id, the cause, the layers and the drops of a group.
    type Outline = (String, Option<Cause>, Vec<String>, Vec<String>);

    fn outline(plan: Plan) -> Vec<Outline> {
        let steps = |layers: Vec<crate::engine::plan::Build>| layers.into_iter().map(|layer| layer.step).collect();
        plan.groups.into_iter().map(|group| (group.id, group.cause, steps(group.layers), group.drops)).collect()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_plan_of_live_has_a_group_per_stale_layer_and_per_layer_that_only_live_has() {
        let fixture = fixture("changes-groups");
        let steps = layered();
        let built = fixture.build(&steps).unwrap();
        let step = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let mut live: Vec<Layer> = built.iter().map(|b| Layer::new(&b.receipt, step(&b.receipt.step))).collect();
        live.push(Layer { layer: "sun".into(), step: "test/sun".into(), ..live[0].clone() });
        let against = Against { layers: live.iter().map(|layer| (layer.step.as_str(), layer)).collect(), drift: None };
        let changes = |steps: &[Step]| outline(changes(&fixture.store, steps, &against).unwrap());
        let off = ("sun".to_string(), Some(Cause::Off), Vec::new(), names(&["test/sun"]));
        assert_eq!(changes(&steps), std::slice::from_ref(&off));

        static REVISION: crate::engine::Layer = crate::engine::Layer { revision: 2, ..TEST };
        let mut revised = layered();
        revised[0].layer = &REVISION;
        revised[1].layer = &REVISION;
        let revision = Cause::Stale { cause: Why::Version { field: Field::Revision, from: Some(1), to: 2 } };
        let test = ("test".to_string(), Some(revision), names(&["test/join", "test/count"]), Vec::new());
        assert_eq!(changes(&revised), [test, off.clone()]);

        let mut wider = layered();
        wider[2].options = serde_json::json!({"bounds": [7, 43, 8, 44]});
        let options = Cause::Stale { cause: Why::Input { name: OPTIONS.into() } };
        let upper = ("upper".into(), Some(options), names(&["test/upper"]), Vec::new());
        let follows = Cause::Stale { cause: Why::Input { name: "upper".into() } };
        let test = ("test".into(), Some(follows), names(&["test/join", "test/count"]), Vec::new());
        assert_eq!(changes(&wider), [upper, test, off]);
    }
}
