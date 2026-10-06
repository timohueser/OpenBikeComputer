//! The state of each layer, computed when asked and never stored.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::release::Layer;
use super::{digest, layer_digest, order, selection, Code, Codes, Input, InputKind, Selection, Step};
use crate::sources::{State, Status};
use crate::store::{sorted, Store};

/// What the state of a layer depends on besides the steps and the store.
pub struct Environment {
    /// The status of each active acquisition request, from `sources::status`.
    pub sources: BTreeMap<crate::env::RequestKey, Status>,
    /// Each layer that live has, from the live release manifests.
    pub live: BTreeMap<String, Layer>,
}

/// A layer with its state, what it reads, its code and the layers that read it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LayerState {
    pub layer: String,
    pub state: State,
    pub reason: Option<String>,
    /// The key of the layer that live has.
    pub live: Option<String>,
    pub reads: Vec<Read>,
    pub code: Code,
    pub code_hash: String,
    pub users: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub kind: InputKind,
    /// The source id or the layer name.
    pub name: String,
    /// The version of a snapshot.
    pub version: Option<String>,
}

/// The state of each layer of `steps`, in dependency order. When more than one state applies, the
/// first in the order of `State` is the state.
pub fn state(store: &Store, root: &Path, steps: &[Step], environment: &Environment) -> Result<Vec<LayerState>, String> {
    let mut users: HashMap<&str, Vec<String>> = HashMap::new();
    for step in steps {
        for name in step.layers() {
            users.entry(name).or_default().push(step.name.clone());
        }
    }
    let mut codes = Codes::default();
    let mut states: HashMap<&str, State> = HashMap::new();
    let mut layers = Vec::new();
    for step in order(steps)? {
        let (code_hash, files) = codes.get(root, &step.code).map_err(|e| format!("step `{}`: {e}", step.name))?;
        let (state, reason) = judge(store, step, code_hash, files, environment, &states)?;
        states.insert(&step.name, state);
        let reads = step.inputs.iter().map(|input| match input {
            Input::Snapshot { source, version, .. } => {
                Read { kind: InputKind::Snapshot, name: source.clone(), version: Some(version.clone()) }
            }
            Input::Layer { name, .. } => Read { kind: InputKind::Layer, name: name.clone(), version: None },
        });
        layers.push(LayerState {
            layer: step.name.clone(),
            state,
            reason,
            live: environment.live.get(&step.name).map(|live| live.key.clone()),
            reads: reads.collect(),
            code: step.code.clone(),
            code_hash: code_hash.clone(),
            users: users.remove(step.name.as_str()).unwrap_or_default(),
        });
    }
    Ok(layers)
}

fn judge(
    store: &Store,
    step: &Step,
    code_hash: &str,
    files: &BTreeMap<String, String>,
    environment: &Environment,
    states: &HashMap<&str, State>,
) -> Result<(State, Option<String>), String> {
    let found = |state, reason: String| Ok((state, Some(reason)));
    let Some(live) = environment.live.get(&step.name) else {
        return found(State::NotApplied, "missing in live".into());
    };
    let read = |kind, name: &str| live.inputs.iter().find(|input| input.kind == kind && input.name == name);

    if live.options != step.options {
        return found(State::NotApplied, "options".into());
    }
    for input in &step.inputs {
        let Input::Snapshot { source, version, .. } = input else { continue };
        // A source that live did not read is a new input: the inputs below differ.
        if let Some((true, fetched)) = other_read(store, live, input)? {
            let fetched = if fetched { "" } else { " (not fetched)" };
            return found(State::NotApplied, format!("{source}@{version} not in live{fetched}"));
        }
    }
    match code_differs(step, code_hash, live) {
        Some("code") => return found(State::CodeChanged, changed_code(store, &live.code, files, &step.code)?),
        Some(what) => return found(State::CodeChanged, what.into()),
        None => {}
    }

    for input in &step.inputs {
        let Input::Layer { name, files } = input else { continue };
        let rebuilds = matches!(states[name.as_str()], State::NotApplied | State::CodeChanged | State::InputChanged);
        let rebuilt = environment.live.get(name).map(|layer| layer_digest(&layer.files, files))
            != read(InputKind::Layer, name).map(|input| input.digest.clone());
        if rebuilds || rebuilt {
            return found(State::InputChanged, name.into());
        }
    }

    for wanted in [State::Stale, State::Blocked] {
        for input in &step.inputs {
            let Input::Snapshot { source, params, .. } = input else { continue };
            if let Some(status) =
                environment.sources.get(&(source.clone(), sorted(params))).filter(|status| status.state == wanted)
            {
                let reason = status.reason.as_deref().map_or(source.clone(), |reason| format!("{source}: {reason}"));
                return found(wanted, reason);
            }
        }
    }
    Ok((State::Ok, None))
}

/// Whether the live layer read the snapshot input with another version, other `params` (in any
/// order) or, when the store has the files that it reads, other files; and whether the store has
/// them. `None` for a layer input, or a source that the live layer did not read.
pub(super) fn other_read(store: &Store, live: &Layer, input: &Input) -> Result<Option<(bool, bool)>, String> {
    let Input::Snapshot { source, version, params, files } = input else { return Ok(None) };
    let Some(read) = live.snapshots.get(source) else { return Ok(None) };
    let digest = match selection(store, source, version, params, files)? {
        Selection::Present(files) => Some(digest(files.iter().map(|file| (file.name.as_str(), file.sha256.as_str())))),
        Selection::Lacks(_) => None,
    };
    let other = &read.version != version || sorted(&read.params) != sorted(params);
    let recorded = live.inputs.iter().find(|input| input.kind == InputKind::Snapshot && &input.name == source);
    let other_files = digest.as_ref().is_some_and(|digest| recorded.is_some_and(|input| &input.digest != digest));
    Ok(Some((other || other_files, digest.is_some())))
}

/// What of the code of `step` is not that of its live layer: `inputs` (kind and name), `command`,
/// `outputs`, `client` (whether a client reads the layer) or `code` (the code hash).
pub(super) fn code_differs(step: &Step, code_hash: &str, live: &Layer) -> Option<&'static str> {
    let declared: BTreeSet<(InputKind, &str)> = step
        .inputs
        .iter()
        .map(|input| match input {
            Input::Snapshot { source, .. } => (InputKind::Snapshot, source.as_str()),
            Input::Layer { name, .. } => (InputKind::Layer, name.as_str()),
        })
        .collect();
    let built: BTreeSet<(InputKind, &str)> =
        live.inputs.iter().map(|input| (input.kind, input.name.as_str())).collect();
    if declared != built {
        Some("inputs")
    } else if live.command != step.command() {
        Some("command")
    } else if live.outputs != step.sorted_outputs() {
        Some("outputs")
    } else if live.client != step.client.sorted() {
        Some("client")
    } else if live.code != code_hash {
        Some("code")
    } else {
        None
    }
}

/// The code files that differ from the code that built the live layer: the first, and how many
/// more. Without a record of that code, the declared code.
fn changed_code(store: &Store, before: &str, now: &BTreeMap<String, String>, code: &Code) -> Result<String, String> {
    let Some(before) = store.code(before)? else {
        return Ok(code.paths.iter().chain(&code.crates).cloned().collect::<Vec<_>>().join(", "));
    };
    let paths: BTreeSet<&String> = before.keys().chain(now.keys()).collect();
    let changed: Vec<&String> = paths.into_iter().filter(|path| before.get(*path) != now.get(*path)).collect();
    Ok(match changed.as_slice() {
        [] => "the code hash".into(),
        [one] => one.to_string(),
        [first, rest @ ..] => format!("{first} and {} more", rest.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline, snapshot, write, Fixture, JOIN};

    /// Build the test pipeline, and make its layers live.
    fn live(fixture: &Fixture) -> Environment {
        let steps = pipeline();
        let built = fixture.build(&steps).unwrap();
        let live = built.into_iter().map(|built| {
            let step = steps.iter().find(|step| step.name == built.receipt.step).unwrap();
            (step.name.clone(), Layer::new(&built.receipt, step))
        });
        Environment { sources: BTreeMap::new(), live: live.collect() }
    }

    fn states(fixture: &Fixture, steps: &[Step], environment: &Environment) -> Vec<(String, State, Option<String>)> {
        let layers = state(&fixture.store, &fixture.root(), steps, environment).unwrap();
        layers.into_iter().map(|layer| (layer.layer, layer.state, layer.reason)).collect()
    }

    fn expect(states: &[(&str, State, Option<&str>)]) -> Vec<(String, State, Option<String>)> {
        states.iter().map(|(layer, state, reason)| (layer.to_string(), *state, reason.map(str::to_string))).collect()
    }

    #[test]
    fn a_change_in_a_declared_code_file_marks_exactly_the_layers_that_declare_it_code_changed() {
        let fixture = fixture("state-code");
        let environment = live(&fixture);
        let ok = [("test/upper", State::Ok, None), ("test/join", State::Ok, None), ("test/count", State::Ok, None)];
        assert_eq!(states(&fixture, &pipeline(), &environment), expect(&ok));

        write(&fixture.root().join("join.py"), &format!("# A comment.\n{JOIN}"));
        assert_eq!(
            states(&fixture, &pipeline(), &environment),
            expect(&[
                ("test/upper", State::Ok, None),
                ("test/join", State::CodeChanged, Some("join.py")),
                ("test/count", State::InputChanged, Some("test/join")),
            ])
        );

        write(&fixture.root().join("join.py"), JOIN);
        write(&fixture.root().join("steps/src/lib.rs"), "// A comment.\n");
        assert_eq!(
            states(&fixture, &pipeline(), &environment),
            expect(&[
                ("test/upper", State::CodeChanged, Some("steps/src/lib.rs")),
                ("test/join", State::InputChanged, Some("test/upper")),
                ("test/count", State::CodeChanged, Some("steps/src/lib.rs")),
            ])
        );
    }

    #[test]
    fn each_state_has_its_reason() {
        let fixture = fixture("state-each");
        let mut environment = live(&fixture);
        let status = |state, reason: &str| Status { state, reason: Some(reason.into()), age_days: None };
        environment
            .sources
            .insert(("head".into(), Vec::new()), status(State::Stale, "14 d > 7 d, upstream 2026-10-04"));
        environment.sources.insert(("tail".into(), Vec::new()), status(State::Blocked, "no licence recorded"));
        environment.live.remove("test/count");
        assert_eq!(
            states(&fixture, &pipeline(), &environment),
            expect(&[
                ("test/upper", State::Stale, Some("head: 14 d > 7 d, upstream 2026-10-04")),
                ("test/join", State::Blocked, Some("tail: no licence recorded")),
                ("test/count", State::NotApplied, Some("missing in live")),
            ])
        );

        let mut unrelated = live(&fixture);
        unrelated.sources.insert(
            ("head".into(), vec![("area".into(), "another".into())]),
            status(State::Stale, "another area is due"),
        );
        assert!(
            states(&fixture, &pipeline(), &unrelated).iter().all(|(_, state, _)| *state == State::Ok),
            "held snapshots and unrelated requests do not inherit source-wide freshness"
        );

        fixture.fetched_version("head", "2", "head.txt", b"head 2\n");
        // The environment reads a version or params that live was not built from.
        let tile = Input::Snapshot {
            source: "head".into(),
            version: "1".into(),
            params: vec![("tile".into(), "c".into())],
            files: Vec::new(),
        };
        let cases = [
            (snapshot("head", "2", &["head.txt"]), "head@2 not in live"),
            (snapshot("head", "3", &["head.txt"]), "head@3 not in live (not fetched)"),
            (tile, "head@1 not in live (not fetched)"),
        ];
        for (input, reason) in cases {
            let mut steps = pipeline();
            steps[2].inputs = vec![input];
            let layers = state(&fixture.store, &fixture.root(), &steps, &environment).unwrap();
            assert_eq!((layers[0].state, layers[0].reason.as_deref()), (State::NotApplied, Some(reason)));
            assert_eq!((layers[1].state, layers[1].reason.as_deref()), (State::InputChanged, Some("test/upper")));
        }

        // A snapshot that live did not read is a new input.
        let mut steps = pipeline();
        steps[2].inputs.push(snapshot("tail", "1", &[]));
        let layers = state(&fixture.store, &fixture.root(), &steps, &environment).unwrap();
        assert_eq!((layers[0].state, layers[0].reason.as_deref()), (State::CodeChanged, Some("inputs")));

        let layers = state(&fixture.store, &fixture.root(), &pipeline(), &environment).unwrap();
        let (upper, join) = (&layers[0], &layers[1]);
        assert_eq!(upper.users, ["test/join"]);
        assert_eq!(upper.live, Some(environment.live["test/upper"].key.clone()));
        let reads: Vec<(InputKind, &str, Option<&str>)> =
            join.reads.iter().map(|read| (read.kind, read.name.as_str(), read.version.as_deref())).collect();
        assert_eq!(reads, [(InputKind::Layer, "test/upper", None), (InputKind::Snapshot, "tail", Some("1"))]);
        assert_eq!(join.code.paths, ["join.py"]);
    }
}
