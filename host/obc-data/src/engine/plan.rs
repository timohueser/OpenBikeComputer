//! The plan: what a run would fetch and build, in groups that do not depend on each other.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{order, prepare, recipe, reusable, select, selection, Codes, Input, Receipt, Selection, Step};
use crate::store::{sorted, Store};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub groups: Vec<Group>,
}

/// One change: builds that read each other's layers, and the fetches that they need. A group
/// never needs a build of another group, so each can be selected alone. Two groups can need the
/// same fetch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanGroup")]
pub struct Group {
    /// The step of its first build. It names the group only in the plan that it comes from.
    pub id: String,
    pub fetches: Vec<Fetch>,
    /// In dependency order.
    pub builds: Vec<Build>,
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
    /// The code hash.
    pub code: String,
    /// The snapshot inputs whose files the store lacks.
    pub fetches: Vec<Fetch>,
    /// `None` until the store has every snapshot and layer that the step reads.
    pub key: Option<String>,
    /// The layer of `key` in the store, with all of its objects.
    pub stored: Option<Receipt>,
}

/// Each step in dependency order, with its key and its stored layer. `plan` and
/// `release::release` reuse the same layers.
pub(super) fn walk<'a>(store: &Store, root: &Path, steps: &'a [Step]) -> Result<Vec<Walked<'a>>, String> {
    let mut codes = Codes::default();
    let mut reused: HashMap<&str, Receipt> = HashMap::new();
    let mut walked = Vec::new();
    for step in order(steps)? {
        let named = |e: String| format!("step `{}`: {e}", step.name);
        let code = codes.get(root, &step.code).map_err(named)?.0.clone();
        let fetches = missing(store, step)?;
        let (mut key, mut stored) = (None, None);
        if fetches.is_empty() && step.layers().all(|name| reused.contains_key(name)) {
            let (receipt, _) = prepare(store, step, &reused, &code).map_err(named)?;
            stored = reusable(store, &receipt.key)?;
            if let Some(stored) = &stored {
                reused.insert(&step.name, stored.clone());
            }
            key = Some(receipt.key);
        }
        walked.push(Walked { step, code, fetches, key, stored });
    }
    Ok(walked)
}

/// What building `steps` needs: a fetch for each snapshot input whose files the store lacks, and a
/// build for each layer whose key has no layer in the store, or whose key waits for a fetch or
/// another build.
pub fn plan(store: &Store, root: &Path, steps: &[Step]) -> Result<Plan, String> {
    let receipts = store.layers()?;
    let mut builds: Vec<(&Step, Build, Vec<Fetch>)> = Vec::new();
    for Walked { step, code, fetches, key, stored } in walk(store, root, steps)? {
        if stored.is_none() {
            let (recipe, estimate) = (recipe(step, &code), estimate(&receipts, step));
            builds.push((step, Build { step: step.name.clone(), recipe, key, estimate }, fetches));
        }
    }

    // A build joins the builds whose layers it reads.
    let index: HashMap<&str, usize> =
        builds.iter().enumerate().map(|(i, (step, ..))| (step.name.as_str(), i)).collect();
    let mut parent: Vec<usize> = (0..builds.len()).collect();
    for (i, (step, ..)) in builds.iter().enumerate() {
        for j in step.layers().filter_map(|name| index.get(name)) {
            union(&mut parent, i, *j);
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut group_of: HashMap<usize, usize> = HashMap::new();
    for (i, (_, build, fetches)) in builds.iter().enumerate() {
        let g = *group_of.entry(find(&mut parent, i)).or_insert_with(|| {
            groups.push(Group { id: build.step.clone(), fetches: Vec::new(), builds: Vec::new() });
            groups.len() - 1
        });
        groups[g].builds.push(build.clone());
        for fetch in fetches {
            add_fetch(&mut groups[g].fetches, fetch);
        }
    }
    for fetch in groups.iter_mut().flat_map(|group| &mut group.fetches) {
        fetch.bytes = fetch_bytes(store, fetch)?;
    }
    Ok(Plan { groups })
}

impl Plan {
    /// Whether a run of `self` does the same work as a run of `other`: the same groups, fetches,
    /// recipes and keys. Estimates and fetch sizes may differ.
    pub fn same_work(&self, other: &Plan) -> bool {
        let work = |plan: &Plan| {
            let mut plan = plan.clone();
            for group in &mut plan.groups {
                group.fetches.iter_mut().for_each(|fetch| fetch.bytes = None);
                group.builds.iter_mut().for_each(|build| build.estimate = None);
            }
            plan
        };
        work(self) == work(other)
    }

    /// The plan with only the groups `ids` names.
    pub fn only(&self, ids: &[String]) -> Result<Plan, String> {
        if let Some(id) = ids.iter().find(|id| !self.groups.iter().any(|group| &group.id == *id)) {
            return Err(format!("the plan has no group `{id}`"));
        }
        Ok(Plan { groups: self.groups.iter().filter(|group| ids.contains(&group.id)).cloned().collect() })
    }

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
fn add_fetch(fetches: &mut Vec<Fetch>, fetch: &Fetch) {
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
    use crate::engine::tests::{fixture, pipeline, snapshot, step, steps_crate, summary};
    use crate::engine::{Request, Run};

    fn lower(request: &Request) -> Result<(), String> {
        let text = std::fs::read_to_string(&request.snapshots["head"]["head.txt"]).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("lower.txt"), text.to_lowercase()).map_err(|e| e.to_string())
    }

    /// The test pipeline, and `test/lower`, which reads `head@{version}`.
    fn steps(version: &str) -> Vec<Step> {
        let mut steps = pipeline();
        let head = vec![snapshot("head", version, &["head.txt"])];
        steps.push(step("test/lower", head, steps_crate(), "lower.txt", Run::Rust(lower)));
        steps
    }

    fn outline<'a>(plan: &'a Plan) -> Vec<(&'a str, Vec<&'a str>)> {
        let outline = |group: &'a Group| (group.id.as_str(), group.builds.iter().map(|b| b.step.as_str()).collect());
        plan.groups.iter().map(outline).collect()
    }

    #[test]
    fn a_plan_has_independent_groups_and_only_selects_some() {
        let fixture = fixture("plan-groups");
        let plan = fixture.plan(&steps("1")).unwrap();
        let chain = ("test/upper", vec!["test/upper", "test/join", "test/count"]);
        assert_eq!(outline(&plan), [chain.clone(), ("test/lower", vec!["test/lower"])]);
        let keys: Vec<bool> = plan.builds().map(|build| build.key.is_some()).collect();
        assert_eq!(keys, [true, false, false, true], "a key waits for the layers that it reads");
        assert_eq!(plan.only(&["nothing".into()]).unwrap_err(), "the plan has no group `nothing`");

        let lower = plan.only(&["test/lower".into()]).unwrap();
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
