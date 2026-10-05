//! The plan: what a run would fetch and build, in groups that do not depend on each other.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{order, prepare, reusable, select, Codes, Input, Receipt, Step};
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub groups: Vec<Group>,
}

/// One change. A group never needs the work of another group, so each can be selected alone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// The first layer that it builds: the name that `--only` selects.
    pub id: String,
    pub fetches: Vec<Fetch>,
    /// In dependency order.
    pub builds: Vec<Build>,
    /// Apply fills the uploads, switches and removals; the engine leaves them empty.
    pub uploads: Vec<Upload>,
    pub switches: Vec<Switch>,
    pub removals: Vec<Removal>,
}

/// Files of a snapshot that the store lacks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fetch {
    pub source: String,
    pub version: String,
    /// The file names, or none for every file of the version.
    pub files: Vec<String>,
    /// The size of these files in the store's record of the version, or else in the newest other
    /// version that has them all.
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub step: String,
    /// `None` until the layers and snapshots that it reads are in the store.
    pub key: Option<String>,
    /// From the newest receipt of the step.
    pub estimate: Option<Estimate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    pub wall_ms: u64,
    pub bytes_out: u64,
    pub peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Upload {
    pub object: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    pub pointer: String,
    pub release: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Removal {
    pub object: String,
    pub bytes: u64,
}

/// What building `steps` needs: a fetch for each snapshot file that the store lacks, and a build
/// for each layer whose key has no receipt, or whose key waits for a fetch or another build.
pub fn plan(store: &Store, root: &Path, steps: &[Step]) -> Result<Plan, String> {
    let mut newest: HashMap<String, Receipt> = HashMap::new();
    for receipt in store.layers()? {
        match newest.get(&receipt.step) {
            Some(known) if known.built >= receipt.built => {}
            _ => {
                newest.insert(receipt.step.clone(), receipt);
            }
        }
    }
    let mut codes = Codes::default();
    let mut reused: HashMap<&str, Receipt> = HashMap::new();
    let mut builds: Vec<(&Step, Build, Vec<Fetch>)> = Vec::new();
    for step in order(steps)? {
        let fetches = missing(store, step)?;
        let waits = step.layers().any(|name| !reused.contains_key(name));
        let mut key = None;
        if fetches.is_empty() && !waits {
            let (receipt, _) = prepare(store, step, &reused, &codes.get(root, &step.code)?.0)
                .map_err(|e| format!("step `{}`: {e}", step.name))?;
            if let Some(stored) = reusable(store, &receipt.key)? {
                reused.insert(&step.name, stored);
                continue;
            }
            key = Some(receipt.key);
        }
        let estimate = newest.get(&step.name).map(|receipt| Estimate {
            wall_ms: receipt.wall_ms,
            bytes_out: receipt.bytes_out,
            peak_rss_bytes: receipt.peak_rss_bytes,
        });
        builds.push((step, Build { step: step.name.clone(), key, estimate }, fetches));
    }

    // A build joins the builds whose layers it reads, and the builds that fetch the same version.
    let index: HashMap<&str, usize> =
        builds.iter().enumerate().map(|(i, (step, ..))| (step.name.as_str(), i)).collect();
    let mut parent: Vec<usize> = (0..builds.len()).collect();
    let mut fetched: HashMap<(&str, &str), usize> = HashMap::new();
    for (i, (step, _, fetches)) in builds.iter().enumerate() {
        for j in step.layers().filter_map(|name| index.get(name)) {
            union(&mut parent, i, *j);
        }
        for fetch in fetches {
            let j = *fetched.entry((&fetch.source, &fetch.version)).or_insert(i);
            union(&mut parent, i, j);
        }
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut group_of: HashMap<usize, usize> = HashMap::new();
    for (i, (_, build, fetches)) in builds.iter().enumerate() {
        let g = *group_of.entry(find(&mut parent, i)).or_insert_with(|| {
            groups.push(Group {
                id: build.step.clone(),
                fetches: Vec::new(),
                builds: Vec::new(),
                uploads: Vec::new(),
                switches: Vec::new(),
                removals: Vec::new(),
            });
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
}

/// The snapshot files that `step` reads and the store lacks: no record, no such file, or no object.
fn missing(store: &Store, step: &Step) -> Result<Vec<Fetch>, String> {
    let mut fetches = Vec::new();
    for input in &step.inputs {
        let Input::Snapshot { source, version, files: selected } = input else { continue };
        let files = match store.snapshot(source, version)? {
            None => selected.clone(),
            Some(snapshot) => {
                let (files, missing) = select(&snapshot, selected);
                let absent =
                    files.iter().filter(|file| !store.object(&file.sha256).is_file()).map(|file| file.name.as_str());
                let names: BTreeSet<&str> = missing.into_iter().chain(absent).collect();
                if names.is_empty() {
                    continue;
                }
                names.into_iter().map(str::to_string).collect()
            }
        };
        fetches.push(Fetch { source: source.clone(), version: version.clone(), files, bytes: None });
    }
    Ok(fetches)
}

/// Add `fetch` to `fetches`, joined with a fetch of the same version.
fn add_fetch(fetches: &mut Vec<Fetch>, fetch: &Fetch) {
    let Some(known) = fetches.iter_mut().find(|known| known.source == fetch.source && known.version == fetch.version)
    else {
        fetches.push(fetch.clone());
        return;
    };
    if known.files.is_empty() || fetch.files.is_empty() {
        known.files.clear();
    } else {
        let names: BTreeSet<String> = known.files.drain(..).chain(fetch.files.iter().cloned()).collect();
        known.files = names.into_iter().collect();
    }
}

fn fetch_bytes(store: &Store, fetch: &Fetch) -> Result<Option<u64>, String> {
    let mut snapshots = store.snapshots(&fetch.source)?;
    snapshots.sort_by(|a, b| (a.version == fetch.version, &a.version).cmp(&(b.version == fetch.version, &b.version)));
    for snapshot in snapshots.iter().rev() {
        let (files, missing) = select(snapshot, &fetch.files);
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
        assert_eq!(outline(&plan), [chain, ("test/lower", vec!["test/lower"])]);
        let fetch =
            Fetch { source: "head".into(), version: "2".into(), files: vec!["head.txt".into()], bytes: Some(5) };
        assert_eq!(plan.groups[1].fetches, std::slice::from_ref(&fetch));
        assert_eq!(plan.groups[1].builds[0].key, None);
        assert_eq!(plan.groups[1].builds[0].estimate.map(|estimate| estimate.bytes_out), Some(5));

        // Two builds that need the same fetch are one change.
        let mut both = steps("2");
        both[2].inputs = vec![snapshot("head", "2", &["head.txt"])];
        let plan = fixture.plan(&both).unwrap();
        assert_eq!(outline(&plan), [("test/upper", vec!["test/upper", "test/lower", "test/join", "test/count"])]);
        assert_eq!(plan.groups[0].fetches, [fetch]);
    }
}
