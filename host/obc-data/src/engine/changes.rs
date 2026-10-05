//! The plan against live: each layer of the steps that differs from its live layer, in one group
//! per cause. A layer can have more causes, so two groups can name it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use super::plan::{add_fetch, build, sized, walk, Build, Cause, Group, Plan, Walked};
use super::release::Layer;
use super::state::{code_differs, other_read};
use super::{recipe, Input, Receipt, Step};
use crate::store::Store;

/// What the steps are compared with.
#[derive(Default)]
pub struct Against<'a> {
    /// Each layer of the live releases of the products that the steps make, by step.
    pub layers: BTreeMap<&'a str, &'a Layer>,
    /// The products whose live release has another region, and those with nothing live.
    pub region: BTreeSet<String>,
    /// The products whose live release has other optional layers.
    pub optional: BTreeSet<String>,
    /// Each source that the plan moves: the versions that live reads, and the version of the plan.
    pub moves: BTreeMap<String, (Vec<String>, String)>,
    /// The keys of live that R2 lacks or holds with another size, and the layers of their files.
    pub drift: Option<(Vec<String>, BTreeSet<String>)>,
}

/// One group per cause: the layers that differ from live because of it, the changed layers that
/// they read, the live layers that go, and what the store lacks to make them.
pub fn changes(store: &Store, root: &Path, steps: &[Step], against: &Against) -> Result<Plan, String> {
    let walked = walk(store, root, steps)?;
    let mut causes: HashMap<&str, BTreeSet<Cause>> = HashMap::new();
    for Walked { step, code, .. } in &walked {
        let product = product(&step.name);
        let region = against.region.contains(product);
        let own = Cause::Code { paths: step.code.paths.clone(), crates: step.code.crates.clone() };
        let mut found = BTreeSet::new();
        match against.layers.get(step.name.as_str()) {
            None => {
                found.extend(region.then_some(Cause::Region));
                found.extend(against.optional.contains(product).then_some(Cause::Layers));
                if found.is_empty() {
                    found.insert(own);
                }
            }
            Some(live) => {
                let mut other = live.options != step.options;
                for input in &step.inputs {
                    let (Input::Snapshot { source, version, .. }, Some((true, _))) =
                        (input, other_read(store, live, input)?)
                    else {
                        continue;
                    };
                    match against.moves.get(source) {
                        Some((from, to)) if &live.snapshots[source].version != version => {
                            found.insert(Cause::Move { source: source.clone(), from: from.clone(), to: to.clone() });
                        }
                        _ => other = true,
                    }
                }
                if other {
                    found.insert(if region { Cause::Region } else { own.clone() });
                }
                if code_differs(step, code, live).is_some() {
                    found.insert(own);
                }
            }
        }
        for name in step.layers() {
            found.extend(causes[name].iter().cloned());
        }
        causes.insert(&step.name, found);
    }

    let mut layers: BTreeMap<Cause, Vec<&str>> = BTreeMap::new();
    for Walked { step, .. } in &walked {
        for cause in &causes[step.name.as_str()] {
            layers.entry(cause.clone()).or_default().push(&step.name);
        }
    }
    let mut drops: BTreeMap<Cause, Vec<&str>> = BTreeMap::new();
    for &name in against.layers.keys().filter(|name| !causes.contains_key(*name)) {
        let product = product(name);
        let mut why = Vec::new();
        why.extend(against.region.contains(product).then_some(Cause::Region));
        why.extend(against.optional.contains(product).then_some(Cause::Layers));
        if why.is_empty() {
            why.push(Cause::Code { paths: Vec::new(), crates: Vec::new() });
        }
        why.into_iter().for_each(|cause| drops.entry(cause).or_default().push(name));
    }

    let receipts = store.layers()?;
    let mut groups = Vec::new();
    for cause in layers.keys().chain(drops.keys()).collect::<BTreeSet<_>>() {
        let named = layers.get(cause).map_or(&[][..], Vec::as_slice);
        let dropped = drops.get(cause).map_or(&[][..], Vec::as_slice);
        let id = match cause {
            Cause::Region => "region".into(),
            Cause::Layers => "layers".into(),
            Cause::Move { source, .. } => format!("move:{source}"),
            // The first layer in dependency order has the cause itself, so no other code names it.
            Cause::Code { .. } => format!("code:{}", named.first().or(dropped.first()).expect("a cause has a layer")),
            Cause::Repair { .. } => unreachable!("drift is no cause of a layer"),
        };
        let mut group =
            needs(&walked, &receipts, Group::new(id, Some(cause.clone())), &named.iter().copied().collect());
        let changed = walked.iter().filter(|w| named.contains(&w.step.name.as_str()));
        group.layers = changed
            .map(|w| Build {
                step: w.step.name.clone(),
                recipe: recipe(w.step, &w.code),
                key: w.key.clone(),
                estimate: None,
            })
            .collect();
        group.drops = dropped.iter().map(|name| name.to_string()).collect();
        groups.push(group);
    }
    if let Some((keys, owners)) = against.drift.as_ref().filter(|(keys, _)| !keys.is_empty()) {
        // A group makes a changed layer again; the store gives the files of the others.
        let unchanged =
            owners.iter().map(String::as_str).filter(|name| causes.get(name).is_some_and(BTreeSet::is_empty));
        let repair = Group::new("repair".into(), Some(Cause::Repair { keys: keys.clone() }));
        groups.push(needs(&walked, &receipts, repair, &unchanged.collect()));
    }
    sized(store, groups)
}

/// The product of a layer: the first part of its name.
fn product(layer: &str) -> &str {
    layer.split('/').next().unwrap_or_default()
}

/// `group` with the fetches and builds of the layers `names`, and of the layers that they read at
/// any depth, that the store lacks.
fn needs<'a>(walked: &'a [Walked], receipts: &[Receipt], mut group: Group, names: &BTreeSet<&'a str>) -> Group {
    let mut needed = names.clone();
    for Walked { step, .. } in walked.iter().rev() {
        if needed.contains(step.name.as_str()) {
            needed.extend(step.layers());
        }
    }
    for walked in walked.iter().filter(|w| needed.contains(w.step.name.as_str())) {
        if let Some(build) = build(walked, receipts) {
            group.builds.push(build);
            walked.fetches.iter().for_each(|fetch| add_fetch(&mut group.fetches, fetch));
        }
    }
    group
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline};

    /// The id, the layers and the drops of each group.
    fn outline(plan: Plan) -> Vec<(String, Vec<String>, Vec<String>)> {
        let steps = |layers: Vec<Build>| layers.into_iter().map(|layer| layer.step).collect();
        plan.groups.into_iter().map(|group| (group.id, steps(group.layers), group.drops)).collect()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn an_edit_or_the_code_is_the_cause_of_the_layers_that_it_changes() {
        let fixture = fixture("changes-causes");
        let built = fixture.build(&pipeline()).unwrap();
        let steps = pipeline();
        let step = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let mut live: Vec<Layer> = built.iter().map(|b| Layer::new(&b.receipt, step(&b.receipt.step))).collect();
        live.push(Layer { step: "test/sun".into(), ..live[0].clone() });
        let layers = live.iter().map(|layer| (layer.step.as_str(), layer)).collect();
        let mut against = Against { layers, ..Against::default() };
        let changes = |steps: &[Step], against: &Against| {
            outline(changes(&fixture.store, &fixture.root(), steps, against).unwrap())
        };

        let all = names(&["test/upper", "test/join", "test/count"]);
        assert_eq!(changes(&steps, &against), [("code:test/sun".into(), Vec::new(), names(&["test/sun"]))]);
        against.optional.insert("test".into());
        assert_eq!(changes(&steps, &against), [("layers".into(), Vec::new(), names(&["test/sun"]))]);

        let mut wider = pipeline();
        wider[2].options = serde_json::json!({"bounds": [7, 43, 8, 44]});
        let code = ("code:test/upper".to_string(), all.clone(), Vec::new());
        assert_eq!(changes(&wider, &against)[1], code, "options without an edit of the region");
        against.region.insert("test".into());
        let region = ("region".to_string(), all, names(&["test/sun"]));
        assert_eq!(changes(&wider, &against), [region, ("layers".into(), Vec::new(), names(&["test/sun"]))]);
    }
}
