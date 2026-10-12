//! `obc data update TARGET`: make a target serve its settings. It bakes what is stale, keeps the
//! rest of the serving release by reference, publishes the new release and switches to it.

use std::collections::BTreeMap;
use std::path::Path;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::{freshness, listing, print_json, Code, Error};
use crate::collection::{self, Collected};
use crate::engine::changes;
use crate::engine::plan::outputs;
use crate::engine::runs::{Context, Event, Limits, Phase, Publication, Run};
use crate::engine::state::Newer;
use crate::fetch::http::Http;
use crate::product::{Inputs, Product};
use crate::regions::{Area, Bbox, Region, Regions};
use crate::release::{self, File, Output, Release};
use crate::settings::Settings;
use crate::sources::{Refresh, Registry};
use crate::store::Store;
use crate::target::Target;

#[derive(Args)]
pub struct UpdateArgs {
    /// The target, as `data/targets.toml` names it.
    target: String,
    /// A region of `data/regions/`, or Geofabrik paths joined by `,`.
    #[arg(long, conflicts_with = "bbox")]
    region: Option<String>,
    /// A box: west,south,east,north in degrees.
    #[arg(long = "box", allow_hyphen_values = true)]
    bbox: Option<String>,
    /// Switch an optional layer on.
    #[arg(long)]
    add: Vec<String>,
    /// Switch an optional layer off.
    #[arg(long)]
    remove: Vec<String>,
    /// The refresh policy of a source for this target: SOURCE=DAYS or SOURCE=manual.
    #[arg(long, value_name = "SOURCE=DAYS|manual")]
    refresh: Vec<String>,
    /// Bake these layers again, also when they are not stale, with the stale layers that they read.
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
}

/// What an update did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Updated {
    pub run: String,
    pub target: String,
    /// The release that the target serves now.
    pub serving: String,
    /// The release that it served before; `null` when it served nothing or the update changed
    /// nothing.
    pub rollback: Option<String>,
    /// The steps that the run baked or found in the store, in dependency order.
    pub baked: Vec<Baked>,
    /// The number of outputs that the release keeps by reference.
    pub kept: usize,
    /// What the collection after the switch did; `null` without a switch.
    pub collection: Option<Collected>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Baked {
    pub step: String,
    pub key: String,
    pub reused: bool,
}

pub fn command(root: &Path, products: &[&dyn Product], args: UpdateArgs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let target = crate::target::find(root, &args.target, &store).map_err(|e| Code::Usage.error(e))?;
    if let Target::R2 { .. } = target {
        return Err(Code::Blocked
            .error(format!("`obc data update` writes only a dir target, and `{}` is an R2 bucket", args.target))
            .fix("Update a dir target, such as `obc data update local`."));
    }
    let mut run = super::api::start_run(&store, &format!("update {}", args.target))?;
    let edit = |settings: &mut Settings| edit(root, products, &args, settings);
    let result = update(root, &store, &Http::new(), (&args.target, &target), products, edit, &args.only, &mut run);
    let updated = super::api::finish_run(run, result, None)?;
    if json {
        return print_json(&updated);
    }
    for baked in &updated.baked {
        println!("{:<7} {}  {}", if baked.reused { "reused" } else { "baked" }, &baked.key[..12], baked.step);
    }
    match &updated.rollback {
        Some(rollback) => println!(
            "{} serves release {}; rollback {}; {} outputs kept",
            updated.target,
            &updated.serving[..12],
            &rollback[..12],
            updated.kept
        ),
        None => println!("{} serves release {}", updated.target, &updated.serving[..12]),
    }
    if let Some(collected) = &updated.collection {
        println!("{}", collected.text());
    }
    Ok(())
}

/// Update `target` with the settings of its serving release after `edit`.
#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    root: &Path,
    store: &Store,
    http: &Http,
    (name, target): (&str, &Target),
    products: &[&dyn Product],
    edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    only: &[String],
    run: &mut Run,
) -> Result<Updated, Error> {
    let base = release::serving(target)?;
    let serving = base.as_ref().map(|(serving, _)| Release::read(target, &serving.serving.id)).transpose()?;
    let mut settings = serving.as_ref().map(|release| release.settings.clone()).unwrap_or_default();
    edit(&mut settings)?;
    if settings.region.is_empty() {
        return Err(Code::Usage
            .error(format!("target `{name}` has no region"))
            .fix(format!("Name one: `obc data update {name} --region ID` or `--box W,S,E,N`.")));
    }
    let regions = settings.regions().map_err(|e| Code::InvalidData.error(e))?;
    let mut sources = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?.sources;
    settings.policies(&mut sources).map_err(|e| Code::Usage.error(e))?;
    let manual = sources.iter().filter(|source| source.refresh == Refresh::Manual).map(|s| s.id.clone()).collect();
    let serving_versions = serving.as_ref().map(Release::versions).unwrap_or_default();
    let mut inputs = Inputs { settings: settings.clone(), serving: serving_versions, manual, ..Inputs::default() };

    run.record(&Event::Phase { phase: Phase::Prepare })?;
    let requests = freshness::discover(root, products, &inputs, &regions, store, http, &sources)?;
    for (key, newer) in freshness::newer(store, http, &sources, &requests, &inputs.serving) {
        match newer {
            Some(version) => {
                inputs.resolved.insert(key, version);
            }
            None => {
                inputs.due.insert(key);
            }
        }
    }
    let listed = {
        let fetch = listing::fetcher(root, store, http, &sources, Some(&mut *run), true);
        listing::steps(root, products, &mut inputs, &regions, store, false, fetch)?
    };
    if !listed.blocked.is_empty() {
        let reasons: Vec<String> =
            listed.blocked.iter().map(|b| format!("{} ({}): {}", b.layer, b.step, b.reason)).collect();
        return Err(Code::Blocked.error(format!("layers cannot be planned: {}", reasons.join("; "))));
    }
    let release: BTreeMap<&str, &Output> =
        serving.iter().flat_map(|release| &release.outputs).map(|output| (output.step.as_str(), output)).collect();
    let update = changes::update(store, &listed.steps, &release, &Newer::new(), only)?;

    run.record(&Event::Phase { phase: Phase::Build })?;
    let context = Context { store, root, sources: &sources, http, kept: &update.kept, limits: Limits::machine() };
    let built = run.build(&context, &listed.steps, &update.plan).map_err(|e| Code::RunFailed.error(e))?;
    let baked =
        built.into_iter().map(|built| Baked { step: built.receipt.step, key: built.receipt.key, reused: built.reused });
    let mut outputs = outputs(store, &listed.steps, &update.kept)?
        .ok_or_else(|| Code::Failed.error("the store lacks the output of a step after the run"))?;
    outputs.sort_by(|a, b| a.step.cmp(&b.step));
    let next = compose(products, settings, outputs)?;
    let mut updated = Updated {
        run: run.id().into(),
        target: name.into(),
        serving: String::new(),
        rollback: None,
        baked: baked.collect(),
        kept: update.kept.len(),
        collection: None,
    };
    let changed = || {
        Code::Failed
            .error(format!("release.json of `{name}` changed during the update; nothing switched"))
            .fix(format!("Run `obc data update {name}` again: it keeps what this run baked."))
    };
    if let Some(serving) = serving.as_ref().filter(|serving| serving.serves_as(&next)) {
        let (current, _) = base.as_ref().expect("a serving release has a base");
        // The serving release has every change: a switch to it writes a pointer that drifts.
        if !release::drift(target, serving)?.is_empty() && !release::switch(target, base.as_ref(), &current.serving.id)?
        {
            return Err(changed());
        }
        updated.serving = current.serving.id.clone();
        return Ok(updated);
    }

    run.record(&Event::Phase { phase: Phase::Verify })?;
    for product in products {
        let failed =
            |e: String| Code::VerifyFailed.error(format!("product `{}`: {e}; nothing changed", product.name()));
        product.verify(root, serving.as_ref(), &next, store).map_err(failed)?;
    }
    run.record(&Event::Phase { phase: Phase::Upload })?;
    let id = release::publish(target, store, &next)?;
    run.record(&Event::Phase { phase: Phase::Switch })?;
    if !release::switch(target, base.as_ref(), &id)? {
        return Err(changed());
    }
    run.record(&Event::Published { mutation: Publication::Switched { release: id.clone() } })?;
    run.sync()?;
    let since = base.as_ref().map(|(base, _)| base.serving.since.as_str());
    updated.collection = Some(collected(run, target, since)?);
    updated.rollback = base.map(|(base, _)| base.serving.id);
    updated.serving = id;
    Ok(updated)
}

/// The collection of `target` after a switch. A switch stays when the collection fails: the next
/// switch collects again.
pub(super) fn collected(run: &mut Run, target: &Target, served_since: Option<&str>) -> Result<Collected, Error> {
    run.record(&Event::Phase { phase: Phase::Collect })?;
    let collected = collection::collect(target, served_since, crate::date::now());
    Ok(collected
        .unwrap_or_else(|e| Collected { skipped: Some(format!("the collection failed: {e}")), ..Collected::default() }))
}

/// The release of `outputs`: the pointer and the named files of each product.
fn compose(products: &[&dyn Product], settings: Settings, outputs: Vec<Output>) -> Result<Release, Error> {
    let (mut pointers, mut named) = (Vec::new(), Vec::new());
    for product in products {
        let bug = |message: String| {
            let fix = format!("The code of product `{}` is wrong, not the data: correct it.", product.name());
            Code::Failed.error(message).fix(fix)
        };
        let pointer = product.pointer();
        let output = outputs.iter().find(|output| output.step == pointer.step);
        let file = output.and_then(|output| output.files.iter().find(|file| file.path == pointer.path));
        let file = file.ok_or_else(|| bug(format!("no step makes the file {} of {}", pointer.path, pointer.step)))?;
        pointers.push(File { key: pointer.key.into(), size: file.size, sha256: file.sha256.clone() });
        let files = product.named(&outputs).map_err(bug)?;
        let folder = crate::engine::digest(files.iter().map(|(path, file)| (path.as_str(), file.sha256.as_str())));
        named.extend(files.into_iter().map(|(path, file)| File {
            key: format!("releases/{folder}/{path}"),
            size: file.size,
            sha256: file.sha256,
        }));
    }
    pointers.sort();
    named.sort();
    let release = Release {
        settings,
        outputs,
        pointers,
        named,
        machine: crate::machine(),
        time: crate::date::timestamp(crate::date::now()),
    };
    release.check().map_err(|e| Code::Failed.error(format!("the new release: {e}")))?;
    Ok(release)
}

/// Apply the flags of `args` to `settings`.
fn edit(root: &Path, products: &[&dyn Product], args: &UpdateArgs, settings: &mut Settings) -> Result<(), Error> {
    let usage = |message: String| Code::Usage.error(message);
    if let Some(region) = &args.region {
        let presets = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
        match presets.get(region) {
            Some(_) => settings.select(&presets, region).map_err(usage)?,
            None => {
                let areas: Vec<String> = region.split(',').map(str::to_string).collect();
                let path = |area: &String| area.split('/').all(crate::is_kebab);
                if let Some(area) = areas.iter().find(|area| !path(area)) {
                    return Err(usage(format!("`{area}` is no region of data/regions/ and no Geofabrik path")));
                }
                let id = if areas.len() == 1 { areas[0].clone() } else { "areas".into() };
                let area = Area::Geofabrik { areas: areas.clone() };
                defined(
                    settings,
                    Region { id, name: areas.join(" + "), area, countries: Vec::new(), time_zone: None },
                )?;
            }
        }
    }
    if let Some(text) = &args.bbox {
        let numbers: Vec<f64> = text
            .split(',')
            .map(|n| n.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| usage(format!("`--box {text}` is not four numbers")))?;
        let corners: [f64; 4] = numbers.try_into().map_err(|_| usage(format!("`--box {text}` is not four numbers")))?;
        let bbox = Bbox::new(corners).map_err(usage)?;
        let name = format!("Box {text}");
        defined(
            settings,
            Region { id: "box".into(), name, area: Area::Box { bbox }, countries: Vec::new(), time_zone: None },
        )?;
    }
    for layer in &args.add {
        if !products.iter().any(|product| product.optional().contains(&layer.as_str())) {
            return Err(usage(format!("no product has the optional layer `{layer}`")));
        }
        if !settings.layers.contains(layer) {
            settings.layers.push(layer.clone());
        }
    }
    settings.layers.retain(|layer| !args.remove.contains(layer));
    settings.layers.sort();
    let sources = crate::sources::all();
    for policy in &args.refresh {
        let (id, refresh) =
            policy.split_once('=').ok_or_else(|| usage(format!("`--refresh {policy}` is not SOURCE=DAYS|manual")))?;
        if !sources.iter().any(|source| source.id == id) {
            return Err(usage(format!("no source `{id}`")));
        }
        let refresh: Refresh = refresh.parse().map_err(|e: String| usage(format!("`--refresh {policy}`: {e}")))?;
        settings.refresh.insert(id.into(), refresh);
    }
    Ok(())
}

/// Select `region`, a definition that no file of `data/regions/` holds.
fn defined(settings: &mut Settings, region: Region) -> Result<(), Error> {
    let id = region.id.clone();
    let regions = Regions::new(vec![region]).map_err(|e| Code::Usage.error(e))?;
    settings.select(&regions, &id).map_err(|e| Code::Usage.error(e))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline, snapshot, write, Fixture};
    use crate::product::{Pointer, Steps, Unplanned};
    use crate::release::POINTERS;

    /// The test pipeline: `test/upper` reads the version of `head` that the update chose.
    struct Pipeline;

    impl Product for Pipeline {
        fn name(&self) -> &'static str {
            "test"
        }

        fn optional(&self) -> &'static [&'static str] {
            &["extra"]
        }

        fn steps(&self, root: &Path, inputs: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            let head = crate::product::version(inputs, store, "head", &[]).map_err(Unplanned::Failed)?;
            let head = head.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            let mut steps = pipeline();
            steps[2].inputs = vec![snapshot("head", &head, &["head.txt"])];
            crate::engine::tests::revise(root, &mut steps);
            Ok(steps.into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[0], step: "test/join", path: "joined.txt" }
        }
    }

    /// A product without steps, whose pointer is the count of the pipeline.
    struct Counter;

    impl Product for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }

        fn steps(&self, _: &Path, _: &Inputs, _: &Regions, _: &Store) -> Result<Steps, Unplanned> {
            Ok(Vec::new().into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[1], step: "test/count", path: "count/lines.txt" }
        }
    }

    pub(in crate::cli) fn update(
        fixture: &Fixture,
        target: &Target,
        edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    ) -> Result<Updated, Error> {
        update_with(fixture, target, &Http::new(), edit)
    }

    fn update_with(
        fixture: &Fixture,
        target: &Target,
        http: &Http,
        edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    ) -> Result<Updated, Error> {
        let mut run = Run::create(&fixture.store, "update local").unwrap();
        let root = fixture.root();
        let updated =
            super::update(&root, &fixture.store, http, ("local", target), &[&Pipeline, &Counter], edit, &[], &mut run);
        run.finish(updated.as_ref().err().map(|error| error.message.as_str())).unwrap();
        updated
    }

    pub(in crate::cli) fn region(settings: &mut Settings) -> Result<(), Error> {
        let area = Area::Box { bbox: Bbox::new([7.0, 47.0, 8.0, 48.0]).unwrap() };
        defined(settings, Region { id: "box".into(), name: "Box".into(), area, countries: Vec::new(), time_zone: None })
    }

    fn steps(updated: &Updated) -> Vec<&str> {
        updated.baked.iter().map(|baked| baked.step.as_str()).collect()
    }

    #[test]
    fn an_update_bakes_what_is_stale_keeps_the_rest_by_reference_and_remembers_its_settings() {
        let fixture = fixture("update-local");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let refused = update(&fixture, &target, |_| Ok(())).unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (Code::Usage, "target `local` has no region"));

        let first = update(&fixture, &target, |settings| {
            settings.layers.push("extra".into());
            region(settings)
        })
        .unwrap();
        assert_eq!((steps(&first), first.rollback.as_ref()), (vec!["test/upper", "test/join", "test/count"], None));
        let release = Release::read(&target, &first.serving).unwrap();
        assert_eq!(
            (release.settings.region.as_str(), release.settings.layers.as_slice()),
            ("box", &["extra".to_string()][..])
        );
        assert!(release::drift(&target, &release).unwrap().is_empty());

        let same = update(&fixture, &target, |_| Ok(())).unwrap();
        assert_eq!((&same.serving, steps(&same), same.kept), (&first.serving, Vec::new(), 3), "nothing is stale");

        write(&fixture.root().join("revised"), "test/join");
        let revised = update(&fixture, &target, |_| Ok(())).unwrap();
        assert_eq!((steps(&revised), revised.kept), (vec!["test/join", "test/count"], 1));
        assert_eq!(revised.rollback.as_ref(), Some(&first.serving));
        assert_eq!(Release::read(&target, &revised.serving).unwrap().settings, release.settings, "remembered");

        // A newer version of a source in the store moves the steps that read it.
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let moved = update(&fixture, &target, |_| Ok(())).unwrap();
        assert_eq!(steps(&moved), ["test/upper", "test/join", "test/count"]);
        let serving = release::serving(&target).unwrap().unwrap().0;
        assert_eq!((serving.serving.id, serving.rollback.map(|role| role.id)), (moved.serving, Some(revised.serving)));
    }

    #[test]
    fn a_due_source_moves_to_the_newer_version_that_upstream_names() {
        use crate::fetch::tests::{serve, whole};
        // Upstream gives `head.txt` with the day 2026-10-05.
        let (actual, _) = serve(|_, _| whole(b"newer head\n"));
        let actual = actual.replacen("file.bin", "head.txt", 1);
        let fixture = fixture("update-due");
        let url = actual.replacen("http://", "https://", 1);
        write(
            &fixture.root().join("data/sources.toml"),
            &format!(
                "[[source]]\nid = \"head\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Head\"\n\
                 fetch = {{ kind = \"http\", url = \"{url}\" }}\nversion = \"date\"\nrefresh = 1\nredistribute = true\n"
            ),
        );
        fixture.fetched_version("head", "2026-01-01", "head.txt", b"head\n");
        let (target, http) = (Target::Dir(fixture.scratch.0.join("local")), Http::loopback(&actual));
        let first = update_with(&fixture, &target, &http, region).unwrap();
        let read = |id: &str| Release::read(&target, id).unwrap().versions()[&("head".to_string(), Vec::new())].clone();
        assert_eq!(read(&first.serving), "2026-01-01");

        let moved = update_with(&fixture, &target, &http, |_| Ok(())).unwrap();
        assert_eq!(steps(&moved), ["test/upper", "test/join", "test/count"]);
        assert_eq!(read(&moved.serving), "2026-10-05", "the policy is due, and upstream names a newer day");
    }

    #[test]
    fn a_clean_keeps_what_each_target_serves_and_its_rollback_and_reads_every_target() {
        let fixture = fixture("update-clean");
        fixture.with_sources(&[]);
        let dir = fixture.scratch.0.join("local");
        write(
            &fixture.root().join("data/targets.toml"),
            &format!("[local]\nkind = \"dir\"\ndir = \"{}\"\n", dir.display()),
        );
        let target = Target::Dir(dir.clone());
        update(&fixture, &target, region).unwrap();
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        update(&fixture, &target, |_| Ok(())).unwrap();
        let roots = crate::cli::roots(&fixture.root(), &fixture.store).unwrap();
        for version in ["1", "2"] {
            assert_eq!(roots.served[&("head".to_string(), version.to_string())], ["local"], "head {version}");
        }

        std::fs::write(dir.join(release::SERVING), b"not json").unwrap();
        let refused = crate::cli::roots(&fixture.root(), &fixture.store).unwrap_err();
        assert!(refused.message.starts_with("target `local`"), "a clean deletes nothing: {}", refused.message);
    }

    #[test]
    fn a_collection_deletes_after_the_grace_only_what_a_dropped_release_alone_names() {
        use crate::collection::{collect, GRACE};
        use crate::date::{now, timestamp};
        use std::collections::BTreeSet;
        let fixture = fixture("update-collect");
        fixture.with_sources(&[]);
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        let a = update(&fixture, &target, region).unwrap().serving;
        fixture.fetched_version("head", "2", "head.txt", b"2\n");
        let b = update(&fixture, &target, |_| Ok(())).unwrap().serving;
        fixture.fetched_version("head", "3", "head.txt", b"3\n");
        let c = update(&fixture, &target, |_| Ok(())).unwrap();
        let collected = c.collection.unwrap();
        assert_eq!((collected.deferred, collected.keys), (vec![a.clone()], 0), "a stopped serving just now");

        let files = |id: &str| -> BTreeSet<String> {
            Release::read(&target, id).unwrap().files().into_iter().map(|file| file.key).collect()
        };
        let mut only_a: BTreeSet<String> = &(&files(&a) - &files(&b)) - &files(&c.serving);
        only_a.insert(release::manifest_key(&a));
        let older = ["reference/v1/x", "cell-catalog/cells/x", "planner/objects/x", "reference/v1/.x.part"];
        let orphans = ["objects/0000", "objects/.1.2.part", ".release.json.1.tmp", "cell-catalog/.catalog.json.1.tmp"];
        for key in older.iter().chain(&orphans) {
            write(&dir.join(key), key);
        }
        let keys = || target.list().unwrap().into_iter().map(|listed| listed.key).collect::<BTreeSet<_>>();
        let before = keys();

        let young = collect(&target, Some(&timestamp(0)), now()).unwrap();
        assert_eq!((young.deferred, young.keys), (vec![a.clone()], 0), "its files changed within the grace");

        // A collection that stopped after one file resumes from the manifest.
        let first = only_a.iter().find(|key| key.starts_with("objects/")).unwrap().clone();
        target.remove(&first).unwrap();
        let late = collect(&target, Some(&timestamp(now())), now() + GRACE + 60).unwrap();
        assert_eq!(late.releases, [a]);
        assert_eq!(late.keys as usize, only_a.len() - 1 + orphans.len());
        let gone: BTreeSet<String> = only_a.into_iter().chain(orphans.map(String::from)).collect();
        assert_eq!(keys(), &before - &gone);
    }
}
