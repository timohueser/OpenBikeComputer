//! The plan: what a run would fetch and build, in groups that do not depend on each other.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{order, prepare, recipe, reusable, select, selection, Input, Receipt, Selection, Step};
use crate::release::Output;
use crate::store::{sorted, Store};

/// The outputs of a release that an update keeps by reference, by step: a step that reads one of
/// them reads its files in the release, never the store's layer of the same key.
pub type Kept = BTreeMap<String, Output>;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub groups: Vec<Group>,
}

/// One change, and the fetches and builds that it needs. Without a release, a group is builds that
/// read each other's layers: it never needs a build of another group. Against a release, a group
/// is one layer, and two groups can need the same build. Two groups can need the same fetch.
/// `--only` gives one group of the named layers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanGroup")]
pub struct Group {
    /// It names the group only in the plan that it comes from.
    pub id: String,
    /// Why the group changes its layer; `None` without a release.
    pub cause: Option<Cause>,
    /// The layers that the group changes, in dependency order, each with its recipe and key as in
    /// `builds`.
    pub layers: Vec<Build>,
    /// The steps of the release that no step makes now.
    pub drops: Vec<String>,
    pub fetches: Vec<Fetch>,
    /// In dependency order.
    pub builds: Vec<Build>,
}

/// Why a group changes a release.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "PlanCause")]
pub enum Cause {
    /// The layer is stale.
    Stale { cause: super::state::Cause },
    /// `--only` names the layer: its steps build again.
    Named,
    /// The release has the layer, and no step makes it now.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanFetch")]
pub struct Fetch {
    pub source: String,
    pub version: String,
    pub params: Vec<(String, String)>,
    /// The names of the files that the store lacks, or none when the store cannot name them: then
    /// the fetch gets every file that it gives.
    pub files: Vec<String>,
    /// The size of these files in a snapshot record of the source, or `None`.
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanBuild")]
pub struct Build {
    pub step: String,
    /// The key of the step without the digests of its inputs.
    pub recipe: String,
    /// `None` until the layers and snapshots that it reads are in the store.
    pub key: Option<String>,
    pub estimate: Option<Estimate>,
    /// Whether the step builds also when the store has the layer of its key.
    pub again: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    pub wall_ms: u64,
    pub bytes_out: u64,
    pub peak_rss_bytes: Option<u64>,
}

/// A step in dependency order, with what the store has for it.
pub(super) struct Walked<'a> {
    pub step: &'a Step,
    /// The snapshot inputs whose files the store lacks.
    pub fetches: Vec<Fetch>,
    /// `None` until the store has every snapshot and layer that the step reads.
    pub key: Option<String>,
    /// The output of `key`: kept by reference, or else the store's layer with all of its objects.
    pub stored: Option<Output>,
}

/// Each step in dependency order, with its key and its output. A kept step has the output of the
/// release.
pub(super) fn walk<'a>(store: &Store, steps: &'a [Step], kept: &Kept) -> Result<Vec<Walked<'a>>, String> {
    let mut reused: HashMap<&str, Output> = HashMap::new();
    let mut walked = Vec::new();
    for step in order(steps)? {
        if let Some(output) = kept.get(&step.name) {
            reused.insert(&step.name, output.clone());
            let (key, stored) = (Some(output.key.clone()), Some(output.clone()));
            walked.push(Walked { step, fetches: Vec::new(), key, stored });
            continue;
        }
        let named = |e: String| format!("step `{}`: {e}", step.name);
        let fetches = missing(store, step)?;
        let (mut key, mut stored) = (None, None);
        if fetches.is_empty() && step.reads().all(|name| reused.contains_key(name)) {
            let (receipt, _) = prepare(store, step, &reused).map_err(named)?;
            stored = reusable(store, &receipt.key)?.map(|mut stored| {
                stored.inputs = receipt.inputs;
                Output::new(&stored, step)
            });
            if let Some(stored) = &stored {
                reused.insert(&step.name, stored.clone());
            }
            key = Some(receipt.key);
        }
        walked.push(Walked { step, fetches, key, stored });
    }
    Ok(walked)
}

/// What building `steps` needs: a fetch for each snapshot input whose files the store lacks, and a
/// build for each layer whose key has no layer in the store, or whose key waits for a fetch or
/// another build.
pub fn plan(store: &Store, steps: &[Step]) -> Result<Plan, String> {
    let receipts = store.layers()?;
    let walked = walk(store, steps, &Kept::new())?;
    let builds: Vec<(&Step, Build, &Vec<Fetch>)> =
        walked.iter().filter_map(|walked| Some((walked.step, build(walked, &receipts)?, &walked.fetches))).collect();

    // A build joins the builds whose layers it reads.
    let index: HashMap<&str, usize> =
        builds.iter().enumerate().map(|(i, (step, ..))| (step.name.as_str(), i)).collect();
    let mut parent: Vec<usize> = (0..builds.len()).collect();
    for (i, (step, ..)) in builds.iter().enumerate() {
        for j in step.reads().filter_map(|name| index.get(name)) {
            union(&mut parent, i, *j);
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut group_of: HashMap<usize, usize> = HashMap::new();
    for (i, (_, build, fetches)) in builds.iter().enumerate() {
        let g = *group_of.entry(find(&mut parent, i)).or_insert_with(|| {
            groups.push(Group::new(build.step.clone(), None));
            groups.len() - 1
        });
        groups[g].builds.push(build.clone());
        for fetch in fetches.iter() {
            add_fetch(&mut groups[g].fetches, fetch);
        }
    }
    sized(store, groups)
}

/// The build of a walked step, when the store lacks its layer.
pub(super) fn build(walked: &Walked, receipts: &[Receipt]) -> Option<Build> {
    walked.stored.is_none().then(|| build_of(walked, receipts))
}

/// The build of a walked step, whatever the store has.
pub(super) fn build_of(walked: &Walked, receipts: &[Receipt]) -> Build {
    Build {
        step: walked.step.name.clone(),
        recipe: recipe(walked.step),
        key: walked.key.clone(),
        estimate: estimate(receipts, walked.step),
        again: false,
    }
}

/// The output of each step, in dependency order: kept by reference, or else the store's layer of
/// its key. `None` while the store lacks one.
pub fn outputs(store: &Store, steps: &[Step], kept: &Kept) -> Result<Option<Vec<Output>>, String> {
    Ok(walk(store, steps, kept)?.into_iter().map(|walked| walked.stored).collect())
}

/// `group` with the fetches and the builds, as `build` gives them, of the layers `names` and of
/// the layers that they read at any depth.
pub(super) fn needs<'a>(
    walked: &'a [Walked],
    mut group: Group,
    names: &BTreeSet<&'a str>,
    mut build: impl FnMut(&Walked) -> Option<Build>,
) -> Group {
    let mut needed = names.clone();
    for Walked { step, .. } in walked.iter().rev() {
        if needed.contains(step.name.as_str()) {
            needed.extend(step.reads());
        }
    }
    for walked in walked.iter().filter(|walked| needed.contains(walked.step.name.as_str())) {
        if let Some(build) = build(walked) {
            group.builds.push(build);
            walked.fetches.iter().for_each(|fetch| add_fetch(&mut group.fetches, fetch));
        }
    }
    group
}

/// The plan of `groups`, with the size of each fetch.
pub(super) fn sized(store: &Store, mut groups: Vec<Group>) -> Result<Plan, String> {
    for fetch in groups.iter_mut().flat_map(|group| &mut group.fetches) {
        fetch.bytes = fetch_bytes(store, fetch)?;
    }
    Ok(Plan { groups })
}

impl Group {
    pub(super) fn new(id: String, cause: Option<Cause>) -> Group {
        Group { id, cause, layers: Vec::new(), drops: Vec::new(), fetches: Vec::new(), builds: Vec::new() }
    }
}

impl Plan {
    pub fn builds(&self) -> impl Iterator<Item = &Build> {
        self.groups.iter().flat_map(|group| &group.builds)
    }

    /// One fetch per version and params, with the files of every group that needs it.
    pub fn fetches(&self) -> Vec<Fetch> {
        let mut fetches = Vec::new();
        for fetch in self.groups.iter().flat_map(|group| &group.fetches) {
            add_fetch(&mut fetches, fetch);
        }
        fetches
    }
}

/// From the newest receipt of the step with the same options, or else the newest of the step.
fn estimate(receipts: &[Receipt], step: &Step) -> Option<Estimate> {
    let newest = |same_options: bool| {
        let receipts = receipts.iter().filter(|r| r.step == step.name && (!same_options || r.options == step.options));
        receipts.max_by(|a, b| a.built.cmp(&b.built))
    };
    newest(true).or_else(|| newest(false)).map(|receipt| Estimate {
        wall_ms: receipt.wall_ms,
        bytes_out: receipt.bytes_out,
        peak_rss_bytes: receipt.peak_rss_bytes,
    })
}

/// The snapshot inputs of `step` whose files the store lacks.
fn missing(store: &Store, step: &Step) -> Result<Vec<Fetch>, String> {
    let mut fetches = Vec::new();
    for input in &step.inputs {
        let Input::Snapshot { source, version, params, files } = input else { continue };
        if let Selection::Lacks(files) = selection(store, source, version, params, files)? {
            let (source, version, params) = (source.clone(), version.clone(), params.clone());
            fetches.push(Fetch { source, version, params, files, bytes: None });
        }
    }
    Ok(fetches)
}

/// Add `fetch` to `fetches`, joined with a fetch of the same version and params. A join that
/// adds files has no known size.
pub(super) fn add_fetch(fetches: &mut Vec<Fetch>, fetch: &Fetch) {
    let same = |known: &&mut Fetch| {
        (&known.source, &known.version, sorted(&known.params)) == (&fetch.source, &fetch.version, sorted(&fetch.params))
    };
    let Some(known) = fetches.iter_mut().find(same) else {
        fetches.push(fetch.clone());
        return;
    };
    let before = known.files.clone();
    if known.files.is_empty() || fetch.files.is_empty() {
        known.files.clear();
    } else {
        let names: BTreeSet<String> = known.files.drain(..).chain(fetch.files.iter().cloned()).collect();
        known.files = names.into_iter().collect();
    }
    if known.files != before || known.files != fetch.files {
        known.bytes = None;
    }
}

/// The size of the files of `fetch` in the record of its version, or else in the record of the
/// last other version in byte order that has them all. Without names, the files are those of a
/// fetch with the same params, or every file.
fn fetch_bytes(store: &Store, fetch: &Fetch) -> Result<Option<u64>, String> {
    let mut candidates: Vec<(String, Vec<String>)> = if fetch.params.is_empty() || !fetch.files.is_empty() {
        let versions = store.snapshots(&fetch.source)?.into_iter().map(|snapshot| snapshot.version);
        versions.map(|version| (version, fetch.files.clone())).collect()
    } else {
        let requests = store.requests(&fetch.source, &fetch.params)?;
        // A fetch that gave no file sizes nothing.
        let requests = requests.into_iter().filter(|request| !request.files.is_empty());
        requests.map(|request| (request.version, request.files)).collect()
    };
    candidates.sort_by(|a, b| (a.0 == fetch.version, &a.0).cmp(&(b.0 == fetch.version, &b.0)));
    for (version, names) in candidates.iter().rev() {
        let Some(snapshot) = store.snapshot(&fetch.source, version)? else { continue };
        let (files, missing) = select(&snapshot, names);
        if missing.is_empty() && !files.is_empty() {
            return Ok(Some(files.iter().map(|file| file.size).sum()));
        }
    }
    Ok(None)
}

/// The first build of the group of build `i`.
fn find(parent: &mut [usize], i: usize) -> usize {
    let mut root = i;
    while parent[root] != root {
        root = parent[root];
    }
    parent[i] = root;
    root
}

/// Join the groups of `a` and `b` under the earlier root, so that a root is its group's first build.
fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    parent[a.max(b)] = a.min(b);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::runs::Limits;
    use crate::engine::tests::{fixture, pipeline, snapshot, step, summary};
    use crate::engine::{Request, Run};

    fn lower(request: &Request) -> Result<(), String> {
        let text = std::fs::read_to_string(&request.snapshots["head"]["head.txt"]).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("lower.txt"), text.to_lowercase()).map_err(|e| e.to_string())
    }

    /// The test pipeline, and `test/lower`, which reads `head@{version}`.
    fn steps(version: &str) -> Vec<Step> {
        let mut steps = pipeline();
        let head = vec![snapshot("head", version, &["head.txt"])];
        steps.push(step("test/lower", head, "lower.txt", Run::Rust(lower)));
        steps
    }

    fn outline<'a>(plan: &'a Plan) -> Vec<(&'a str, Vec<&'a str>)> {
        let outline = |group: &'a Group| (group.id.as_str(), group.builds.iter().map(|b| b.step.as_str()).collect());
        plan.groups.iter().map(outline).collect()
    }

    #[test]
    fn a_plan_has_independent_groups_and_a_run_of_one_leaves_the_other() {
        let fixture = fixture("plan-groups");
        let plan = fixture.plan(&steps("1")).unwrap();
        let chain = ("test/upper", vec!["test/upper", "test/join", "test/count"]);
        assert_eq!(outline(&plan), [chain.clone(), ("test/lower", vec!["test/lower"])]);
        let keys: Vec<bool> = plan.builds().map(|build| build.key.is_some()).collect();
        assert_eq!(keys, [true, false, false, true], "a key waits for the layers that it reads");

        let lower = Plan { groups: plan.groups[1..].to_vec() };
        let built = fixture.run(&steps("1"), &lower, Limits::machine()).unwrap();
        assert_eq!(summary(&built), [("test/lower", false)]);
        assert_eq!(fixture.plan(&steps("1")).unwrap().groups, plan.groups[..1], "the other group is the same");

        // A version that the store lacks: a fetch, sized from the version that the store has, and a
        // build that the earlier receipt estimates.
        let plan = fixture.plan(&steps("2")).unwrap();
        assert_eq!(outline(&plan), [chain.clone(), ("test/lower", vec!["test/lower"])]);
        let fetch = Fetch {
            source: "head".into(),
            version: "2".into(),
            params: Vec::new(),
            files: vec!["head.txt".into()],
            bytes: Some(5),
        };
        assert_eq!(plan.groups[1].fetches, std::slice::from_ref(&fetch));
        assert_eq!(plan.groups[1].builds[0].key, None);
        assert_eq!(plan.groups[1].builds[0].estimate.map(|estimate| estimate.bytes_out), Some(5));

        // Two groups can need the same fetch.
        let mut both = steps("2");
        both[2].inputs = vec![snapshot("head", "2", &["head.txt"])];
        let plan = fixture.plan(&both).unwrap();
        assert_eq!(outline(&plan), [chain, ("test/lower", vec!["test/lower"])]);
        assert_eq!((&plan.groups[0].fetches, &plan.groups[1].fetches), (&vec![fetch.clone()], &vec![fetch.clone()]));
        assert_eq!(plan.fetches(), [fetch], "a run fetches it once");
    }

    #[test]
    fn an_estimate_comes_from_the_newest_receipt_with_the_same_options() {
        let fixture = fixture("plan-estimate");
        let built = fixture.build(&steps("1")).unwrap();
        let lower = built.into_iter().find(|built| built.receipt.step == "test/lower").unwrap().receipt;
        let receipt = |size: u64, built: &str, wall_ms| Receipt {
            options: serde_json::json!({ "size": size }),
            built: built.into(),
            wall_ms,
            ..lower.clone()
        };
        let receipts = [receipt(1, "2026-10-01T00:00:00Z", 1000), receipt(2, "2026-10-02T00:00:00Z", 2000)];
        let mut step = steps("1").remove(3);
        step.options = serde_json::json!({ "size": 1 });
        assert_eq!(estimate(&receipts, &step).map(|e| e.wall_ms), Some(1000), "the same options, though older");
        step.options = serde_json::json!({ "size": 3 });
        assert_eq!(estimate(&receipts, &step).map(|e| e.wall_ms), Some(2000), "else the newest of the step");
    }
}
