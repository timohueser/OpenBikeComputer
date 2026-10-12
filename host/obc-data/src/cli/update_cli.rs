//! `obc data update TARGET`: make a target serve its settings. It reviews what is stale, bakes it,
//! keeps the rest of the serving release by reference, publishes the new release and switches to
//! it.

mod review;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::{freshness, listing, print_json, Code, Error};
use crate::collection::{self, Collected};
use crate::engine::changes::{self, Update};
use crate::engine::plan::{outputs, Estimate};
use crate::engine::runs::{Context, Event, Limits, Phase, Publication, Run};
use crate::engine::state::Newer;
use crate::engine::{InputKind, Step};
use crate::fetch::http::Http;
use crate::product::{Inputs, Product, RequestKey};
use crate::regions::{Area, Bbox, Region, Regions};
use crate::release::{self, Fetched, FetchedFile, File, Output, Release, Serving};
use crate::settings::Settings;
use crate::sources::{Refresh, Registry, Source};
use crate::store::Store;
use crate::target::{Tag, Target};
pub use review::{MachineCheck, Review};

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
    /// Show the review, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Start only when the review has this id, as `--dry-run` showed it.
    #[arg(long, value_name = "ID", conflicts_with = "dry_run")]
    expect: Option<String>,
    /// Start also when the format of a layer changes: its readers need new data.
    #[arg(long)]
    accept_format_change: bool,
}

impl UpdateArgs {
    /// The command that starts the update of `review`.
    fn argv(&self, review: &Review) -> Vec<String> {
        let mut argv: Vec<String> = ["obc", "data", "update", &self.target].map(String::from).into();
        let mut flag = |name: &str, value: &str| argv.extend([format!("--{name}"), value.to_string()]);
        self.region.iter().for_each(|region| flag("region", region));
        self.bbox.iter().for_each(|bbox| flag("box", bbox));
        self.add.iter().for_each(|layer| flag("add", layer));
        self.remove.iter().for_each(|layer| flag("remove", layer));
        self.refresh.iter().for_each(|policy| flag("refresh", policy));
        if !self.only.is_empty() {
            flag("only", &self.only.join(","));
        }
        flag("expect", &review.id);
        if review.layers.iter().any(|layer| layer.format.is_some()) {
            argv.push("--accept-format-change".into());
        }
        argv
    }
}

/// What the person asks of an update besides its settings.
#[derive(Debug, Default)]
pub(super) struct Asked {
    pub only: Vec<String>,
    pub expect: Option<String>,
    pub accept_format_change: bool,
}

/// What an update did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Updated {
    pub run: String,
    pub target: String,
    /// The id of the review that the update did.
    pub review: String,
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
    let (http, machine) = (Http::new(), MachineCheck::this(&store)?);
    let edit = |settings: &mut Settings| edit(root, products, &args, settings);
    let target = (args.target.as_str(), &target);
    if args.dry_run {
        let mut prepared = prepare(root, &store, &http, target, products, edit, &args.only, &machine, None)?;
        prepared.review.argv = args.argv(&prepared.review);
        return review::print(&prepared.review, json);
    }
    let asked =
        Asked { only: args.only.clone(), expect: args.expect.clone(), accept_format_change: args.accept_format_change };
    let mut run = super::api::start_run(&store, &format!("update {}", args.target))?;
    let result = update(root, &store, &http, target, products, edit, &asked, &machine, &mut run);
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

/// What an update reads and decides before it changes anything.
pub(super) struct Prepared {
    pub review: Review,
    base: Option<(Serving, Tag)>,
    serving: Option<Release>,
    settings: Settings,
    sources: Vec<Source>,
    steps: Vec<Step>,
    update: Update,
}

/// Update `target` with the settings of its serving release after `edit`, when the review passes
/// [`review::check`].
#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    root: &Path,
    store: &Store,
    http: &Http,
    target: (&str, &Target),
    products: &[&dyn Product],
    edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    asked: &Asked,
    machine: &MachineCheck,
    run: &mut Run,
) -> Result<Updated, Error> {
    let prepared = prepare(root, store, http, target, products, edit, &asked.only, machine, Some(run))?;
    review::check(&prepared.review, asked.expect.as_deref(), asked.accept_format_change)?;
    apply(root, store, http, target, products, prepared, run)
}

/// Read the serving release of `target`, apply `edit` to its settings, list the steps and review
/// them. Only the fetches that a step list needs to know its versions run; `run` records them.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(
    root: &Path,
    store: &Store,
    http: &Http,
    (name, target): (&str, &Target),
    products: &[&dyn Product],
    edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    only: &[String],
    machine: &MachineCheck,
    mut run: Option<&mut Run>,
) -> Result<Prepared, Error> {
    let base = release::serving(target)?;
    let serving = base.as_ref().map(|(serving, _)| read(name, target, &serving.serving.id)).transpose()?;
    let rollback = base.as_ref().and_then(|(serving, _)| serving.rollback.as_ref());
    let rollback = rollback.map(|rollback| read(name, target, &rollback.id)).transpose()?;
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

    if let Some(run) = run.as_deref_mut() {
        run.record(&Event::Phase { phase: Phase::Prepare })?;
    }
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
        let fetch = listing::fetcher(root, store, http, &sources, run, true);
        listing::steps(root, products, &mut inputs, &regions, store, false, fetch)?
    };
    if !listed.blocked.is_empty() {
        let reasons: Vec<String> =
            listed.blocked.iter().map(|b| format!("{} ({}): {}", b.layer, b.step, b.reason)).collect();
        return Err(Code::Blocked.error(format!("layers cannot be planned: {}", reasons.join("; "))));
    }
    let release: BTreeMap<&str, &Output> =
        serving.iter().flat_map(|release| &release.outputs).map(|output| (output.step.as_str(), output)).collect();
    let mut update = changes::update(store, &listed.steps, &release, &Newer::new(), only)?;
    // A step that this machine never baked has the cost of the machine that made the release.
    for build in update.plan.groups.iter_mut().flat_map(|group| group.builds.iter_mut().chain(&mut group.layers)) {
        if let (None, Some(output)) = (build.estimate, release.get(build.step.as_str())) {
            let cost = output.cost;
            build.estimate = Some(Estimate {
                wall_ms: cost.wall_ms,
                bytes_out: cost.bytes_out,
                peak_rss_bytes: cost.peak_rss_bytes,
            });
        }
    }
    let review = review::review(review::Parts {
        target: name,
        base: base.as_ref(),
        serving: serving.as_ref(),
        rollback: rollback.as_ref(),
        settings: &settings,
        steps: &listed.steps,
        only,
        plan: &update.plan,
        kept: &update.kept,
        layers: &update.layers,
        machine,
    });
    Ok(Prepared { review, base, serving, settings, sources, steps: listed.steps, update })
}

/// Bake, publish and switch as `prepared` says, then collect.
fn apply(
    root: &Path,
    store: &Store,
    http: &Http,
    (name, target): (&str, &Target),
    products: &[&dyn Product],
    prepared: Prepared,
    run: &mut Run,
) -> Result<Updated, Error> {
    let Prepared { review, base, serving, settings, sources, steps, update } = prepared;
    run.record(&Event::Phase { phase: Phase::Build })?;
    let context = Context { store, root, sources: &sources, http, kept: &update.kept, limits: Limits::machine() };
    let built = run.build(&context, &steps, &update.plan).map_err(|e| Code::RunFailed.error(e))?;
    let baked =
        built.into_iter().map(|built| Baked { step: built.receipt.step, key: built.receipt.key, reused: built.reused });
    let mut outputs = outputs(store, &steps, &update.kept)?
        .ok_or_else(|| Code::Failed.error("the store lacks the output of a step after the run"))?;
    outputs.sort_by(|a, b| a.step.cmp(&b.step));
    let fetched = reads(store, &outputs, serving.as_ref(), &sources)?;
    let next = compose(products, settings, outputs, fetched)?;
    let mut updated = Updated {
        run: run.id().into(),
        target: name.into(),
        review: review.id,
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
    run.record(&Event::Phase { phase: Phase::Collect })?;
    let since = base.as_ref().map(|(base, _)| base.serving.since.as_str());
    updated.collection = Some(collection::collect(target, since, crate::date::now()));
    updated.rollback = base.map(|(base, _)| base.serving.id);
    updated.serving = id;
    Ok(updated)
}

/// The release of `outputs`: the pointer and the named files of each product.
fn compose(
    products: &[&dyn Product],
    settings: Settings,
    outputs: Vec<Output>,
    sources: Vec<Fetched>,
) -> Result<Release, Error> {
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
        sources,
        machine: crate::machine(),
        time: crate::date::timestamp(crate::date::now()),
    };
    release.check().map_err(|e| Code::Failed.error(format!("the new release: {e}")))?;
    Ok(release)
}

/// The release `id` of the target `name`.
pub(super) fn read(name: &str, target: &Target, id: &str) -> Result<Release, Error> {
    Release::read(target, id).map_err(|e| {
        let fix = match target {
            Target::Dir(dir) => format!(
                "An older `obc data` can have written it: move the folder aside with `mv {0} {0}.old`, then run \
                 `obc data update {name} --region ID` again.",
                dir.display()
            ),
            Target::R2 { .. } => "Correct the release of the bucket, or switch it to its rollback.".into(),
        };
        Code::InvalidData.error(format!("target `{name}`: {e}")).fix(fix)
    })
}

/// The files of each fetch that `outputs` read: from the snapshot record in the store, else from
/// the serving release, whose kept outputs read them. A target copies the files of a source that
/// `sources` lets it give to others.
fn reads(
    store: &Store,
    outputs: &[Output],
    serving: Option<&Release>,
    sources: &[Source],
) -> Result<Vec<Fetched>, Error> {
    // The names of the files of each fetch and version.
    let mut names: BTreeMap<(RequestKey, String), BTreeSet<String>> = BTreeMap::new();
    for output in outputs {
        for input in output.inputs.iter().filter(|input| input.kind == InputKind::Snapshot) {
            let read = output.snapshots.get(&input.name).ok_or_else(|| {
                Code::Failed.error(format!("step `{}` reads {} and has no version of it", output.step, input.name))
            })?;
            let fetch = ((input.name.clone(), read.params.clone()), read.version.clone());
            names.entry(fetch).or_default().extend(input.files.iter().cloned());
        }
    }
    let mut fetched = Vec::new();
    for (((source, params), version), names) in names {
        let recorded = store.snapshot(&source, &version)?.map(|snapshot| {
            let files = snapshot.files.into_iter();
            files.map(|file| FetchedFile { name: file.name, size: file.size, sha256: file.sha256 }).collect()
        });
        let served = serving.iter().flat_map(|release| &release.sources);
        let earlier = served.into_iter().find(|f| (&f.source, &f.version, &f.params) == (&source, &version, &params));
        let mut files = [recorded, earlier.map(|fetched| fetched.files.clone())]
            .into_iter()
            .flatten()
            .map(|files: Vec<FetchedFile>| {
                files.into_iter().filter(|file| names.contains(&file.name)).collect::<Vec<_>>()
            })
            .find(|files| files.len() == names.len())
            .ok_or_else(|| {
                Code::Failed
                    .error(format!("neither the store nor the serving release has the files of {source}@{version}"))
            })?;
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let copy = sources.iter().any(|declared| declared.id == source && declared.copied());
        fetched.push(Fetched { source, version, params, files, copy });
    }
    Ok(fetched)
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
            // The file `format-2` gives the layer `test` a new format.
            static FORMAT_2: crate::engine::Layer = crate::engine::Layer { format: 2, ..crate::engine::tests::TEST };
            if root.join("format-2").exists() {
                steps.iter_mut().filter(|step| step.layer.name == "test").for_each(|step| step.layer = &FORMAT_2);
            }
            Ok(steps.into())
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: POINTERS[0], step: "test/join", path: "joined.txt" }
        }
    }

    /// A product whose pointer is the count of the pipeline. The file `earth` gives it a step that
    /// reads `natural-earth`, a source whose versions have no order.
    struct Counter;

    impl Product for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }

        fn steps(&self, root: &Path, inputs: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            if !root.join("earth").exists() {
                return Ok(Vec::new().into());
            }
            let version = crate::product::version(inputs, store, "natural-earth", &[]).map_err(Unplanned::Failed)?;
            let version = version.map_err(|wanted| Unplanned::NeedsFetch(vec![wanted]))?;
            static EARTH: crate::engine::Layer = crate::engine::Layer { name: "earth", ..crate::engine::tests::TEST };
            let read = vec![snapshot("natural-earth", &version, &["earth.zip"])];
            let mut step =
                crate::engine::tests::step("counter/earth", read, "earth.zip", crate::engine::Run::Rust(earth));
            step.layer = &EARTH;
            Ok(vec![step].into())
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
        asked(fixture, target, http, (&Asked::default(), &MachineCheck::default()), edit)
    }

    /// An update with `asked` on `machine`.
    fn asked(
        fixture: &Fixture,
        target: &Target,
        http: &Http,
        (asked, machine): (&Asked, &MachineCheck),
        edit: impl FnOnce(&mut Settings) -> Result<(), Error>,
    ) -> Result<Updated, Error> {
        let mut run = Run::create(&fixture.store, "update local").unwrap();
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let updated =
            super::update(&root, &fixture.store, http, ("local", target), &products, edit, asked, machine, &mut run);
        run.finish(updated.as_ref().err().map(|error| error.message.as_str())).unwrap();
        updated
    }

    fn earth(request: &crate::engine::Request) -> Result<(), String> {
        let copied = std::fs::copy(&request.snapshots["natural-earth"]["earth.zip"], request.output.join("earth.zip"));
        copied.map(|_| ()).map_err(|e| e.to_string())
    }

    fn asked_update(fixture: &Fixture, target: &Target, asked: &Asked) -> Result<Updated, Error> {
        self::asked(fixture, target, &Http::new(), (asked, &MachineCheck::default()), |_| Ok(()))
    }

    /// The review of an update without flags, as `--dry-run` gives it.
    fn review(fixture: &Fixture, target: &Target) -> Review {
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let machine = MachineCheck::default();
        let prepared =
            prepare(&root, &fixture.store, &Http::new(), ("local", target), &products, |_| Ok(()), &[], &machine, None);
        prepared.unwrap().review
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
        assert_eq!(
            (serving.serving.id, serving.rollback.map(|role| role.id)),
            (moved.serving.clone(), Some(revised.serving))
        );

        // An older `obc data` wrote a manifest without `sources`.
        let key = release::manifest_key(&moved.serving);
        let mut older: serde_json::Value = serde_json::from_slice(&target.get(&key).unwrap().unwrap()).unwrap();
        older.as_object_mut().unwrap().remove("sources");
        let bytes = serde_json::to_vec(&older).unwrap();
        let id = crate::store::sha256_hex(&bytes);
        let any = crate::target::Condition::Any;
        target.put(&release::manifest_key(&id), &bytes, &any).unwrap();
        let named =
            release::Serving { serving: release::Role { id, since: "2026-10-12T00:00:00Z".into() }, rollback: None };
        target.put(release::SERVING, &serde_json::to_vec(&named).unwrap(), &any).unwrap();
        let refused = update(&fixture, &target, |_| Ok(())).unwrap_err();
        assert_eq!(refused.code, Code::InvalidData, "{}", refused.message);
        assert!(refused.fix.contains("local.old"), "{}", refused.fix);
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

    /// Releases A, B and C of `target`, from `head` 1, 2 and 3: C serves, and B is its rollback.
    /// The target copies the files of `head`.
    fn three_releases(fixture: &Fixture, target: &Target) -> (String, String, Updated) {
        let mut head = crate::fetch::tests::source("https://example.org/head.txt", "date");
        head.id = "head".into();
        fixture.with_sources(&[head]);
        let a = update(fixture, target, region).unwrap().serving;
        fixture.fetched_version("head", "2", "head.txt", b"2\n");
        let b = update(fixture, target, |_| Ok(())).unwrap().serving;
        fixture.fetched_version("head", "3", "head.txt", b"3\n");
        (a, b, update(fixture, target, |_| Ok(())).unwrap())
    }

    fn keys(target: &Target) -> std::collections::BTreeSet<String> {
        target.list().unwrap().into_iter().map(|listed| listed.key).collect()
    }

    /// A collection after the grace: `now` is later than the time of every file.
    fn late(target: &Target) -> Collected {
        let now = crate::date::now();
        collection::collect(target, Some(&crate::date::timestamp(now)), now + collection::GRACE + 60)
    }

    #[test]
    fn a_collection_after_the_grace_deletes_dropped_releases_and_orphans_and_keeps_older_keys() {
        use std::collections::BTreeSet;
        let fixture = fixture("update-collect");
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        let (a, b, c) = three_releases(&fixture, &target);
        let collected = c.collection.unwrap();
        assert_eq!((collected.deferred, collected.keys), (vec![a.clone()], 0), "a stopped serving just now");

        let files = |id: &str| -> BTreeSet<String> {
            Release::read(&target, id).unwrap().files().into_iter().map(|file| file.key).collect()
        };
        let mut only_a: BTreeSet<String> = &(&files(&a) - &files(&b)) - &files(&c.serving);
        assert!(only_a.contains("sources/head/1/head.txt"), "the copy of a source file: {only_a:?}");
        assert!(!files(&a).iter().any(|key| key.starts_with("sources/tail/")), "tail is not declared");
        only_a.insert(release::manifest_key(&a));
        let older = ["reference/v1/x", "cell-catalog/cells/x", "planner/objects/x", "reference/v1/.x.part"];
        let orphans = ["objects/0000", "objects/.1.2.part", ".release.json.1.tmp", "cell-catalog/.catalog.json.1.tmp"];
        for key in older.iter().chain(&orphans) {
            write(&dir.join(key), key);
        }
        let before = keys(&target);
        assert!(before.contains("sources/head/1/head.txt"), "the publish copies it");

        let epoch = crate::date::timestamp(0);
        let young = collection::collect(&target, Some(&epoch), crate::date::now());
        assert_eq!((young.deferred, young.keys), (vec![a.clone()], 0), "its files changed within the grace");

        // A collection that stopped after one file resumes from the manifest.
        let first = only_a.iter().find(|key| key.starts_with("objects/")).unwrap().clone();
        target.remove(&first).unwrap();
        let late = late(&target);
        assert_eq!((late.releases, late.stopped), (vec![a], None));
        assert_eq!(late.keys as usize, only_a.len() - 1 + orphans.len());
        let gone: BTreeSet<String> = only_a.into_iter().chain(orphans.map(String::from)).collect();
        assert_eq!(keys(&target), &before - &gone);
    }

    #[test]
    fn a_collection_deletes_nothing_while_a_pointer_drifts_or_a_manifest_cannot_be_read() {
        let fixture = fixture("update-collect-guards");
        let dir = fixture.scratch.0.join("local");
        let target = Target::Dir(dir.clone());
        three_releases(&fixture, &target);
        let before = keys(&target);
        let pointer = dir.join(POINTERS[0]);
        let saved = std::fs::read(&pointer).unwrap();
        std::fs::write(&pointer, b"another publisher").unwrap();
        let drifted = late(&target);
        assert_eq!(drifted.stopped.as_deref(), Some("cell-catalog/catalog.json is not of the serving release"));
        assert_eq!((drifted.keys, keys(&target)), (0, before.clone()));

        std::fs::write(&pointer, saved).unwrap();
        let unreadable = release::manifest_key(&"f".repeat(64));
        write(&dir.join(&unreadable), "not a manifest");
        let unknown = late(&target);
        assert!(unknown.stopped.unwrap().starts_with("the files of a release are not known"));
        assert_eq!((unknown.keys, keys(&target).len()), (0, before.len() + 1));
    }

    #[test]
    fn an_update_starts_only_when_its_review_is_the_one_that_was_read() {
        let fixture = fixture("update-review");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        fixture.fetched_version("head", "2", "head.txt", b"newer\n");
        let review = review(&fixture, &target);
        let moved = &review.sources[0];
        assert_eq!((moved.source.as_str(), moved.from.as_deref(), moved.to.as_str()), ("head", Some("1"), "2"));
        assert_eq!((review.layers.len(), review.layers[0].text.as_str()), (1, "head 1 → 2"));
        assert_eq!(review.after.rollback.as_ref(), Some(&first.serving));

        let expect = |id: &str| Asked { expect: Some(id.into()), ..Asked::default() };
        let run = |asked: &Asked| asked_update(&fixture, &target, asked);
        let refused = run(&expect("000000000000")).unwrap_err();
        assert_eq!(refused.code, Code::Usage, "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving, "nothing changed");
        let updated = run(&expect(&review.id)).unwrap();
        assert_eq!((&updated.review, updated.rollback.as_ref()), (&review.id, Some(&first.serving)));
    }

    #[test]
    fn a_format_change_starts_only_with_its_acknowledgement() {
        let fixture = fixture("update-format");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let first = update(&fixture, &target, region).unwrap();
        write(&fixture.root().join("format-2"), "");
        let format = review(&fixture, &target).layers[0].format.map(|format| (format.from, format.to));
        assert_eq!(format, Some((1, 2)));
        let refused = asked_update(&fixture, &target, &Asked::default()).unwrap_err();
        assert_eq!(refused.code, Code::Usage);
        assert!(refused.message.contains("test 1 → 2 (read by tests)"), "{}", refused.message);
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving, "nothing changed");
        let accepted = Asked { accept_format_change: true, ..Asked::default() };
        assert_eq!(asked_update(&fixture, &target, &accepted).unwrap().rollback, Some(first.serving));
    }

    #[test]
    fn a_machine_without_room_for_the_update_refuses_it_before_it_starts() {
        let fixture = fixture("update-machine");
        fixture.with_sources(&[]);
        let target = Target::Dir(fixture.scratch.0.join("local"));
        let (root, products): (_, [&dyn Product; 2]) = (fixture.root(), [&Pipeline, &Counter]);
        let machine = MachineCheck::default();
        let first =
            prepare(&root, &fixture.store, &Http::new(), ("local", &target), &products, region, &[], &machine, None);
        assert_eq!(first.unwrap().review.cost.unknown, ["test/upper", "test/join", "test/count"], "no receipts");
        let first = update(&fixture, &target, region).unwrap();

        write(&fixture.root().join("revised"), "test/join");
        let full = MachineCheck { memory_bytes: None, store_free_bytes: Some(0), refusals: Vec::new() };
        let layers = fixture.store.layers().unwrap().len();
        let refused = asked(&fixture, &target, &Http::new(), (&Asked::default(), &full), |_| Ok(())).unwrap_err();
        assert_eq!(refused.code, Code::Blocked);
        assert!(
            refused.message.starts_with("this machine cannot do the update: the store gets"),
            "{}",
            refused.message
        );
        assert_eq!(release::serving(&target).unwrap().unwrap().0.serving.id, first.serving);
        assert_eq!(fixture.store.layers().unwrap().len(), layers, "nothing baked");
    }

    #[test]
    fn a_source_whose_versions_have_no_order_moves_only_after_a_refresh() {
        use crate::fetch::tests::{serve, whole};
        use crate::store::sha256_hex;
        let (actual, _) = serve(|_, _| whole(b"earth 2\n"));
        let actual = actual.replacen("file.bin", "earth.zip", 1);
        let fixture = fixture("update-refresh");
        let url = actual.replacen("http://", "https://", 1);
        write(
            &fixture.root().join("data/sources.toml"),
            &format!(
                "[[source]]\nid = \"natural-earth\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Earth\"\n\
                 fetch = {{ kind = \"http\", url = \"{url}\" }}\nversion = \"digest\"\nredistribute = true\n"
            ),
        );
        write(&fixture.root().join("earth"), "");
        let (one, two) = (sha256_hex(b"earth 1\n"), sha256_hex(b"earth 2\n"));
        fixture.fetched_version("natural-earth", &one, "earth.zip", b"earth 1\n");
        let (target, http) = (Target::Dir(fixture.scratch.0.join("local")), Http::loopback(&actual));
        let first = update_with(&fixture, &target, &http, region).unwrap();

        let registry = crate::sources::Registry::load(&fixture.root()).unwrap();
        let request =
            crate::fetch::Request { refresh: false, source: &registry.sources[0], version: None, params: vec![] };
        assert_eq!(crate::fetch::fetch(&fixture.root(), &fixture.store, &http, &request).unwrap().version, two);
        let same = update_with(&fixture, &target, &http, |_| Ok(())).unwrap();
        assert_eq!((&same.serving, steps(&same)), (&first.serving, Vec::new()), "a newer file in the store");

        let refreshed = crate::cli::source_cli::refresh(&fixture.root(), &fixture.store, &http, "natural-earth");
        assert_eq!(refreshed.unwrap().version, two);
        let moved = &review(&fixture, &target).sources[0];
        assert_eq!((moved.source.as_str(), moved.from.as_ref(), &moved.to), ("natural-earth", Some(&one), &two));
        assert_eq!(steps(&update_with(&fixture, &target, &http, |_| Ok(())).unwrap()), ["counter/earth"]);
    }
}
