//! The state of each layer against a release, computed when asked and never stored. A layer is
//! `ok`, `off` or `stale`, and a stale layer has one cause.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::release::Layer;
use super::{digest, layer_digest, order, selection, Input, InputKind, Selection, Step};
use crate::env::RequestKey;
use crate::store::{sorted, Store};

/// The source requests that are due, each with the newer version upstream when it is known.
pub type Newer = BTreeMap<RequestKey, Option<String>>;

/// What the steps are judged against.
pub struct Environment {
    /// The layers of the releases, by step.
    pub release: BTreeMap<String, Layer>,
    pub newer: Newer,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerState {
    pub layer: String,
    pub state: Verdict,
    /// Why a `stale` layer is stale; `null` for another state.
    pub cause: Option<Cause>,
    /// The cause in a few words; `null` without a cause.
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The release has the layer as its steps make it now.
    Ok,
    /// No step makes the layer: an optional layer that is off, or a layer that only the release has.
    Off,
    /// A build changes the layer.
    Stale,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Verdict::Ok => "ok",
            Verdict::Off => "off",
            Verdict::Stale => "stale",
        })
    }
}

/// Why a layer is stale: exactly one of three causes. When more than one applies, the first in
/// this order is the cause.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cause {
    /// The version of the layer is not that of the release. `from` is `null` when the release has
    /// no step of the layer.
    Version { field: Field, from: Option<u32>, to: u32 },
    /// A source has a newer version to use. `to` is the version that the steps read, or the newer
    /// version upstream; `null` when upstream names none.
    Source { source: String, from: String, to: Option<String> },
    /// A layer or a source that the layer reads has other files, or `options`: the release has
    /// other steps of the layer, or a step with other options, inputs, `params` or `client`.
    Input { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// The number that the readers check.
    Format,
    /// The number of the output.
    Revision,
}

/// The [`Cause::Input`] of a step whose declaration is not that of its release layer.
pub const OPTIONS: &str = "options";

impl Cause {
    pub fn text(&self) -> String {
        match self {
            Cause::Version { from: None, .. } => "new".into(),
            Cause::Version { field: Field::Format, from: Some(from), to } => {
                format!("format {from} → {to}: readers need new data")
            }
            Cause::Version { field: Field::Revision, from: Some(from), to } => {
                format!("revision {from} → {to}: improved output")
            }
            Cause::Source { source, from, to } => format!("{source} {from} → {}", to.as_deref().unwrap_or("newer")),
            Cause::Input { name } if name == OPTIONS => "options changed".into(),
            Cause::Input { name } => format!("follows {name}"),
        }
    }

    fn options() -> Cause {
        Cause::Input { name: OPTIONS.into() }
    }
}

/// A layer of the steps or of the release.
pub(super) struct Judged<'a> {
    pub name: String,
    /// Whether no program reads the layer: only the steps of other layers read it.
    pub internal: bool,
    /// `None` when the layer is ok or no step makes it.
    pub cause: Option<Cause>,
    /// Its steps in dependency order, each with the cause that changes it.
    pub steps: Vec<(&'a Step, Option<Cause>)>,
    /// The steps of the layer in the release that no step makes now.
    pub drops: Vec<String>,
}

impl Judged<'_> {
    fn state(self) -> LayerState {
        let state = match (&self.cause, self.steps.is_empty()) {
            (_, true) => Verdict::Off,
            (None, false) => Verdict::Ok,
            (Some(_), false) => Verdict::Stale,
        };
        let text = self.cause.as_ref().map(Cause::text);
        LayerState { layer: self.name, state, cause: self.cause, text }
    }
}

/// The state of each operator layer of `steps` and of the release, in dependency order, then the
/// layers that only the release has. An internal layer is not listed: the layers that read it get
/// its cause.
pub fn state(store: &Store, steps: &[Step], environment: &Environment) -> Result<Vec<LayerState>, String> {
    let release = environment.release.iter().map(|(step, layer)| (step.as_str(), layer)).collect();
    let judged = judge(store, steps, &release, &environment.newer)?.into_iter().filter(|layer| !layer.internal);
    Ok(judged.map(Judged::state).collect())
}

/// Each layer of `steps` and of `release`, in the order of [`state`].
pub(super) fn judge<'a>(
    store: &Store,
    steps: &'a [Step],
    release: &BTreeMap<&str, &Layer>,
    newer: &Newer,
) -> Result<Vec<Judged<'a>>, String> {
    let layer_of: HashMap<&str, &super::Layer> = steps.iter().map(|step| (step.name.as_str(), step.layer)).collect();
    let mut causes: HashMap<&str, Option<Cause>> = HashMap::new();
    let mut judged: Vec<Judged> = Vec::new();
    let at = |judged: &mut Vec<Judged>, name: &str, internal: bool| {
        judged.iter().position(|layer| layer.name == name).unwrap_or_else(|| {
            judged.push(Judged { name: name.into(), internal, cause: None, steps: Vec::new(), drops: Vec::new() });
            judged.len() - 1
        })
    };
    for step in order(steps)? {
        let cause = cause(store, step, release, &causes, &layer_of, newer)?;
        causes.insert(&step.name, cause.clone());
        let i = at(&mut judged, step.layer.name, step.layer.readers.is_empty());
        judged[i].steps.push((step, cause));
    }
    for (step, layer) in release.iter().filter(|(step, _)| !layer_of.contains_key(*step)) {
        let i = at(&mut judged, &layer.layer, false);
        judged[i].drops.push(step.to_string());
    }
    for layer in judged.iter_mut().filter(|layer| !layer.steps.is_empty()) {
        layer.cause = if release.values().any(|released| released.layer == layer.name) {
            let drops = (!layer.drops.is_empty()).then(Cause::options);
            layer.steps.iter().filter_map(|(_, cause)| cause.clone()).chain(drops).min()
        } else {
            let format = layer.steps[0].0.layer.format;
            Some(Cause::Version { field: Field::Format, from: None, to: format })
        };
    }
    Ok(judged)
}

/// What changes the layer of `step` against its release layer. A step that it reads gives its
/// cause to a step of the same layer and to the steps that read an internal layer, and makes a
/// step of another layer follow its layer.
fn cause(
    store: &Store,
    step: &Step,
    release: &BTreeMap<&str, &Layer>,
    causes: &HashMap<&str, Option<Cause>>,
    layer_of: &HashMap<&str, &super::Layer>,
    newer: &Newer,
) -> Result<Option<Cause>, String> {
    let Some(live) = release.get(step.name.as_str()).filter(|live| live.layer == step.layer.name) else {
        return Ok(Some(Cause::options()));
    };
    let (layer, mut found) = (step.layer, Vec::new());
    if live.format != layer.format {
        found.push(Cause::Version { field: Field::Format, from: Some(live.format), to: layer.format });
    }
    if live.revision != layer.revision {
        found.push(Cause::Version { field: Field::Revision, from: Some(live.revision), to: layer.revision });
    }
    let declared: BTreeSet<(InputKind, &str)> = step
        .inputs
        .iter()
        .map(|input| match input {
            Input::Snapshot { source, .. } => (InputKind::Snapshot, source.as_str()),
            Input::Step { name, .. } => (InputKind::Step, name.as_str()),
        })
        .collect();
    let built: BTreeSet<(InputKind, &str)> =
        live.inputs.iter().map(|input| (input.kind, input.name.as_str())).collect();
    if declared != built || live.options != step.options || live.client != step.client.sorted() {
        found.push(Cause::options());
    }
    for input in &step.inputs {
        match input {
            Input::Snapshot { source, version, params, .. } => {
                let Some(read) = live.snapshots.get(source) else { continue };
                let from = read.version.clone();
                if &read.version != version {
                    found.push(Cause::Source { source: source.clone(), from, to: Some(version.clone()) });
                } else if read.params != sorted(params) {
                    found.push(Cause::options());
                } else if other_files(store, live, input)? {
                    found.push(Cause::Input { name: source.clone() });
                } else if let Some(to) = newer.get(&(source.clone(), sorted(params))) {
                    found.push(Cause::Source { source: source.clone(), from, to: to.clone() });
                }
            }
            Input::Step { name, files } => {
                let owner = layer_of[name.as_str()];
                match &causes[name.as_str()] {
                    Some(cause) if owner == layer || owner.readers.is_empty() => found.push(cause.clone()),
                    Some(_) => found.push(Cause::Input { name: owner.name.into() }),
                    None => {
                        let now = release.get(name.as_str()).map(|input| layer_digest(&input.files, files));
                        let read = live.inputs.iter().find(|read| read.kind == InputKind::Step && &read.name == name);
                        if now.as_ref() != read.map(|read| &read.digest) {
                            found.push(Cause::Input { name: owner.name.into() });
                        }
                    }
                }
            }
        }
    }
    Ok(found.into_iter().min())
}

/// Whether the store has the files that the snapshot `input` reads, with another digest than the
/// one that `live` read.
fn other_files(store: &Store, live: &Layer, input: &Input) -> Result<bool, String> {
    let Input::Snapshot { source, version, params, files } = input else { return Ok(false) };
    let Selection::Present(files) = selection(store, source, version, params, files)? else { return Ok(false) };
    let digest = digest(files.iter().map(|file| (file.name.as_str(), file.sha256.as_str())));
    let read = live.inputs.iter().find(|read| read.kind == InputKind::Snapshot && &read.name == source);
    Ok(read.is_some_and(|read| read.digest != digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, layered, snapshot, write, Fixture, JOIN, TEST, UPPER};

    /// Build `steps`, and give the release their layers.
    fn released(fixture: &Fixture, steps: &[Step]) -> BTreeMap<String, Layer> {
        let built = fixture.build(steps).unwrap();
        let step = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        built.iter().map(|b| (b.receipt.step.clone(), Layer::new(&b.receipt, step(&b.receipt.step)))).collect()
    }

    /// Each layer with its state and the text of its cause.
    fn states(
        fixture: &Fixture,
        steps: &[Step],
        release: &BTreeMap<String, Layer>,
        newer: &Newer,
    ) -> Vec<(String, Verdict, Option<String>)> {
        let layers = judged(fixture, steps, release, newer);
        layers.into_iter().map(|layer| (layer.layer, layer.state, layer.text)).collect()
    }

    fn judged(fixture: &Fixture, steps: &[Step], release: &BTreeMap<String, Layer>, newer: &Newer) -> Vec<LayerState> {
        let environment = Environment { release: release.clone(), newer: newer.clone() };
        state(&fixture.store, steps, &environment).unwrap()
    }

    fn expect(states: &[(&str, Verdict, Option<&str>)]) -> Vec<(String, Verdict, Option<String>)> {
        states.iter().map(|(layer, state, text)| (layer.to_string(), *state, text.map(str::to_string))).collect()
    }

    #[test]
    fn a_refactor_with_no_version_change_leaves_every_layer_ok() {
        let fixture = fixture("state-refactor");
        let release = released(&fixture, &layered());
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "(upper + tail)"));
        let ok = [("upper", Verdict::Ok, None), ("test", Verdict::Ok, None)];
        assert_eq!(states(&fixture, &layered(), &release, &Newer::new()), expect(&ok));
    }

    #[test]
    fn a_version_change_makes_its_layer_stale_and_the_layers_that_read_it_follow() {
        static FORMAT: crate::engine::Layer = crate::engine::Layer { format: 2, ..UPPER };
        static REVISION: crate::engine::Layer = crate::engine::Layer { revision: 2, ..TEST };
        let fixture = fixture("state-version");
        let release = released(&fixture, &layered());
        let mut steps = layered();
        steps[2].layer = &FORMAT;
        assert_eq!(
            states(&fixture, &steps, &release, &Newer::new()),
            expect(&[
                ("upper", Verdict::Stale, Some("format 1 → 2: readers need new data")),
                ("test", Verdict::Stale, Some("follows upper")),
            ])
        );
        let mut steps = layered();
        steps[0].layer = &REVISION;
        steps[1].layer = &REVISION;
        let revision = Some(Cause::Version { field: Field::Revision, from: Some(1), to: 2 });
        let layers = judged(&fixture, &steps, &release, &Newer::new());
        assert_eq!((layers[0].state, &layers[1].cause), (Verdict::Ok, &revision));
        assert_eq!(layers[1].text.as_deref(), Some("revision 1 → 2: improved output"));
    }

    #[test]
    fn a_newer_source_version_makes_the_layers_that_read_it_stale() {
        let fixture = fixture("state-source");
        let release = released(&fixture, &layered());
        let mut steps = layered();
        steps[2].inputs = vec![snapshot("head", "2", &["head.txt"])];
        assert_eq!(
            states(&fixture, &steps, &release, &Newer::new()),
            expect(&[("upper", Verdict::Stale, Some("head 1 → 2")), ("test", Verdict::Stale, Some("follows upper"))])
        );
        // A due request reads the version of the release, and upstream has a newer one.
        let mut newer = Newer::from([(("head".into(), Vec::new()), Some("3".into()))]);
        let text = |newer: &Newer| states(&fixture, &layered(), &release, newer)[0].2.clone();
        assert_eq!(text(&newer).as_deref(), Some("head 1 → 3"));
        newer.insert(("head".into(), Vec::new()), None);
        assert_eq!(text(&newer).as_deref(), Some("head 1 → newer"));
        let other = Newer::from([(("head".into(), vec![("area".into(), "another".into())]), None)]);
        assert_eq!(text(&other), None, "another request of the source is due");
    }

    #[test]
    fn an_internal_layer_is_not_listed_and_the_layers_that_read_it_get_its_cause() {
        static INTERNAL: crate::engine::Layer = crate::engine::Layer { name: "source", readers: &[], ..UPPER };
        let fixture = fixture("state-internal");
        let mut steps = layered();
        steps[2].layer = &INTERNAL;
        let release = released(&fixture, &steps);
        steps[2].inputs = vec![snapshot("head", "2", &["head.txt"])];
        assert_eq!(
            states(&fixture, &steps, &release, &Newer::new()),
            expect(&[("test", Verdict::Stale, Some("head 1 → 2"))])
        );
    }

    #[test]
    fn an_input_with_other_files_makes_its_readers_stale() {
        let fixture = fixture("state-input");
        let built = released(&fixture, &layered());
        let mut release = built.clone();
        release.get_mut("test/upper").unwrap().files[0].sha256 = "0".repeat(64);
        assert_eq!(
            states(&fixture, &layered(), &release, &Newer::new()),
            expect(&[("upper", Verdict::Ok, None), ("test", Verdict::Stale, Some("follows upper"))])
        );
        let mut release = built;
        let join = release.get_mut("test/join").unwrap();
        join.inputs.iter_mut().find(|input| input.name == "tail").unwrap().digest = "0".repeat(64);
        let layers = judged(&fixture, &layered(), &release, &Newer::new());
        assert_eq!(layers[1].text.as_deref(), Some("follows tail"), "the store has other files of tail@1");
    }

    #[test]
    fn the_release_makes_a_layer_new_or_off_and_other_options_are_an_input() {
        let fixture = fixture("state-release");
        let mut release = released(&fixture, &layered());
        let mut steps = layered();
        steps[2].options = serde_json::json!({"bounds": [7, 43, 8, 44]});
        let layers = judged(&fixture, &steps, &release, &Newer::new());
        assert_eq!((&layers[0].cause, layers[0].text.as_deref()), (&Some(Cause::options()), Some("options changed")));

        let sun = Layer { layer: "sun".into(), step: "test/sun".into(), ..release["test/upper"].clone() };
        release.insert("test/sun".into(), sun);
        release.retain(|step, _| step != "test/join" && step != "test/count");
        assert_eq!(
            states(&fixture, &layered(), &release, &Newer::new()),
            expect(
                &[("upper", Verdict::Ok, None), ("test", Verdict::Stale, Some("new")), ("sun", Verdict::Off, None),]
            )
        );
    }
}
