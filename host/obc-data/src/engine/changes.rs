//! What an update of a release bakes: one group per layer that is stale against the release, or
//! with `only` one group of the named layers. Every other step keeps its output of the release by
//! reference.

use std::collections::{BTreeMap, BTreeSet};

use super::order;
use super::plan::{build, build_of, needs, sized, walk, Build, Cause, Costs, Group, Kept, Plan};
use super::state::{judge, LayerState, Newer};
use super::{select_layer, Input, Step};
use crate::release::Output;
use crate::store::Store;

/// The work of an update, the outputs of the release that it keeps, and the state of each operator
/// layer as [`super::state::state`] gives it.
pub struct Update {
    pub plan: Plan,
    pub kept: Kept,
    pub layers: Vec<LayerState>,
}

/// The update of `release` to `steps`. Without `only`, it bakes each stale step. With `only`, it
/// bakes the steps of the named layers again, the stale steps that they read at any depth, and the
/// steps that the release lacks; a step that reads one of them bakes when its key changes. Every
/// other step keeps its output, also a stale one.
pub fn update(
    store: &Store,
    steps: &[Step],
    release: &BTreeMap<&str, &Output>,
    newer: &Newer,
    only: &[String],
) -> Result<Update, String> {
    if let Some(name) = only.iter().find(|name| !steps.iter().any(|step| step.layer.name == name.as_str())) {
        return Err(format!("no step makes the layer `{name}`"));
    }
    let judged = judge(store, steps, release, newer)?;
    let layers = judged.iter().filter(|layer| !layer.internal).map(|layer| layer.state()).collect();
    let stale: BTreeSet<&str> = judged
        .iter()
        .flat_map(|layer| &layer.steps)
        .filter(|(_, cause)| cause.is_some())
        .map(|(step, _)| step.name.as_str())
        .collect();
    let named: BTreeSet<&str> =
        steps.iter().filter(|step| only.iter().any(|name| name == step.layer.name)).map(|s| s.name.as_str()).collect();
    let ordered = order(steps)?;
    let mut changed = if only.is_empty() {
        stale
    } else {
        let mut changed = named.clone();
        // Readers come after what they read, so a stale input at any depth joins before its turn.
        for step in ordered.iter().rev() {
            if changed.contains(step.name.as_str()) {
                changed.extend(step.reads().filter(|input| stale.contains(input)));
            }
        }
        let new = |step: &&Step| release.get(step.name.as_str()).is_none_or(|output| output.layer != step.layer.name);
        changed.extend(steps.iter().filter(new).map(|step| step.name.as_str()));
        for step in &ordered {
            if step.reads().any(|input| changed.contains(input)) {
                changed.insert(&step.name);
            }
        }
        changed
    };
    // A target holds only client files. A step that bakes and reads a file of a kept step that no
    // client reads, which the store lacks, needs that step baked again on this machine.
    for step in ordered.iter().rev() {
        if !changed.contains(step.name.as_str()) {
            continue;
        }
        for input in &step.inputs {
            let Input::Step { name, files } = input else { continue };
            let Some(output) = release.get(name.as_str()).filter(|_| !changed.contains(name.as_str())) else {
                continue;
            };
            let (read, _) = select_layer(&output.files, files);
            if read.iter().any(|file| !output.client.includes(&file.path) && !store.object(&file.sha256).is_file()) {
                changed.insert(name);
            }
        }
    }
    let kept: Kept = steps
        .iter()
        .filter(|step| !changed.contains(step.name.as_str()))
        .map(|step| (step.name.clone(), release[step.name.as_str()].clone()))
        .collect();
    let walked = walk(store, steps, &kept)?;
    let costs = Costs { receipts: store.layers()?, release };
    let mut groups = Vec::new();
    if only.is_empty() {
        for layer in judged {
            let steps: BTreeSet<&str> =
                layer.steps.iter().filter(|(_, cause)| cause.is_some()).map(|(step, _)| step.name.as_str()).collect();
            let cause = match (layer.cause, layer.steps.is_empty()) {
                (_, true) => Cause::Off,
                (Some(cause), false) => Cause::Stale { cause },
                (None, false) => continue,
            };
            let mut group = needs(&walked, Group::new(layer.name, Some(cause)), &steps, |walked| build(walked, &costs));
            let layers = walked.iter().filter(|walked| steps.contains(walked.step.name.as_str()));
            group.layers = layers.map(|walked| build_of(walked, &costs)).collect();
            group.drops = layer.drops;
            groups.push(group);
        }
    } else {
        let group = Group::new(only.join(","), Some(Cause::Named));
        let mut group = needs(&walked, group, &changed, |walked| {
            let name = walked.step.name.as_str();
            // The key of a reader waits for the files of the steps that bake before it.
            let waits = walked.step.reads().any(|input| changed.contains(input));
            match changed.contains(name) {
                true => Some(Build {
                    again: named.contains(name),
                    key: if waits { None } else { walked.key.clone() },
                    ..build_of(walked, &costs)
                }),
                false => build(walked, &costs),
            }
        });
        group.layers = group.builds.clone();
        groups.push(group);
    }
    Ok(Update { plan: sized(store, groups)?, kept, layers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::outputs;
    use crate::engine::runs::Limits;
    use crate::engine::state::{Cause as Why, Field, OPTIONS};
    use crate::engine::tests::{fixture, layered, summary, Fixture, REVISED, TEST};
    use crate::engine::{Request, Run};

    /// The id, the cause, the layers and the drops of a group.
    type Outline = (String, Option<Cause>, Vec<String>, Vec<String>);

    fn outline(plan: &Plan) -> Vec<Outline> {
        let steps = |layers: &[Build]| layers.iter().map(|layer| layer.step.clone()).collect();
        plan.groups
            .iter()
            .map(|group| (group.id.clone(), group.cause.clone(), steps(&group.layers), group.drops.clone()))
            .collect()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Build `steps`, and give their outputs as a release.
    fn released(fixture: &Fixture, steps: &[Step]) -> Vec<Output> {
        fixture.build(steps).unwrap();
        outputs(&fixture.store, steps, &Kept::new()).unwrap().unwrap()
    }

    fn by_step(release: &[Output]) -> BTreeMap<&str, &Output> {
        release.iter().map(|output| (output.step.as_str(), output)).collect()
    }

    #[test]
    fn an_update_has_a_group_per_stale_layer_and_per_layer_that_only_the_release_has() {
        let fixture = fixture("changes-groups");
        let mut release = released(&fixture, &layered());
        release.push(Output { layer: "sun".into(), step: "test/sun".into(), ..release[0].clone() });
        let release = by_step(&release);
        let update =
            |steps: &[Step]| outline(&super::update(&fixture.store, steps, &release, &Newer::new(), &[]).unwrap().plan);
        let off = ("sun".to_string(), Some(Cause::Off), Vec::new(), names(&["test/sun"]));
        assert_eq!(update(&layered()), std::slice::from_ref(&off));

        static REVISION: crate::engine::Layer = crate::engine::Layer { revision: 2, ..TEST };
        let mut revised = layered();
        revised[0].layer = &REVISION;
        revised[1].layer = &REVISION;
        let revision = Cause::Stale { cause: Why::Version { field: Field::Revision, from: Some(1), to: 2 } };
        let test = ("test".to_string(), Some(revision), names(&["test/join", "test/count"]), Vec::new());
        assert_eq!(update(&revised), [test, off.clone()]);

        let mut wider = layered();
        wider[2].options = serde_json::json!({"bounds": [7, 43, 8, 44]});
        let options = Cause::Stale { cause: Why::Input { name: OPTIONS.into() } };
        let upper = ("upper".into(), Some(options), names(&["test/upper"]), Vec::new());
        let follows = Cause::Stale { cause: Why::Input { name: "upper".into() } };
        let test = ("test".into(), Some(follows), names(&["test/join", "test/count"]), Vec::new());
        assert_eq!(update(&wider), [upper, test, off]);
    }

    /// `test/upper` with an exclamation mark: other files of the same key.
    fn louder(request: &Request) -> Result<(), String> {
        let text = std::fs::read_to_string(&request.snapshots["head"]["head.txt"]).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("upper.txt"), text.to_uppercase() + "!").map_err(|e| e.to_string())
    }

    #[test]
    fn a_step_reads_the_release_files_of_a_kept_step_and_not_the_store() {
        let fixture = fixture("changes-kept");
        let release = released(&fixture, &layered());
        // The store gets other files for the key of `test/upper`.
        let mut louder_steps = layered();
        louder_steps[2].run = Run::Rust(louder);
        let named = super::update(&fixture.store, &louder_steps, &by_step(&release), &Newer::new(), &names(&["upper"]))
            .unwrap();
        fixture.run(&louder_steps, &named.plan, Limits::machine()).unwrap();

        static REVISION: crate::engine::Layer = crate::engine::Layer { revision: 2, ..TEST };
        let mut revised = layered();
        revised[0].layer = &REVISION;
        revised[1].layer = &REVISION;
        let update = super::update(&fixture.store, &revised, &by_step(&release), &Newer::new(), &[]).unwrap();
        assert_eq!(update.kept.keys().collect::<Vec<_>>(), ["test/upper"]);
        let built = fixture.run_kept(&revised, &update.plan, &update.kept).unwrap();
        assert_eq!(summary(&built), [("test/join", false), ("test/count", false)]);
        let joined = std::fs::read_to_string(fixture.store.object(&built[0].receipt.files[0].sha256)).unwrap();
        assert_eq!(joined, "HEAD\ntail\n", "the file of the release, not the store's");
    }

    #[test]
    fn naming_a_layer_bakes_it_again_with_its_stale_inputs_and_its_readers_when_their_keys_change() {
        let fixture = fixture("changes-named");
        let release = released(&fixture, &layered());
        let release = by_step(&release);
        let named = |steps: &[Step], only: &[&str]| {
            super::update(&fixture.store, steps, &release, &Newer::new(), &names(only)).unwrap()
        };
        let update = named(&layered(), &["upper"]);
        assert!(update.kept.is_empty(), "the readers of `upper` bake when its files change");
        let built = fixture.run_kept(&layered(), &update.plan, &update.kept).unwrap();
        assert_eq!(summary(&built), [("test/upper", false), ("test/join", true), ("test/count", true)]);

        // `test` is named, and `test/upper`, which it reads, is stale: it bakes too.
        let mut wider = layered();
        wider[2].options = serde_json::json!({"bounds": [7, 43, 8, 44]});
        let update = named(&wider, &["test"]);
        let builds: Vec<(&str, bool)> = update.plan.builds().map(|b| (b.step.as_str(), b.again)).collect();
        assert_eq!(builds, [("test/upper", false), ("test/join", true), ("test/count", true)]);
        let unknown = super::update(&fixture.store, &layered(), &release, &Newer::new(), &names(&["nothing"]));
        assert_eq!(unknown.err(), Some("no step makes the layer `nothing`".into()));

        // `test/count` alone is named: it reads `test/join`, which reads the stale `test/upper`.
        let counted = || {
            let mut steps = layered();
            steps[0].layer = &REVISED;
            steps
        };
        let release = released(&fixture, &counted());
        let mut deep = counted();
        deep[2].options = serde_json::json!({"bounds": [7, 43, 8, 44]});
        let update = super::update(&fixture.store, &deep, &by_step(&release), &Newer::new(), &names(&["revised"]));
        let builds: Vec<(String, bool)> =
            update.unwrap().plan.builds().map(|build| (build.step.clone(), build.again)).collect();
        let expected = [("test/upper", false), ("test/join", false), ("test/count", true)];
        assert_eq!(builds, expected.map(|(step, again)| (step.to_string(), again)));
    }
}
