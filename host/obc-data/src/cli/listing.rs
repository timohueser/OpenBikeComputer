//! The steps of every product for the settings of a target. A product whose step list reads a
//! snapshot that the store lacks gets it fetched and is asked again.

use std::collections::BTreeMap;
use std::path::Path;

use super::{fetched, Code, Error};
use crate::engine::runs::Run;
use crate::engine::Step;
use crate::fetch::{self, http::Http, Request};
use crate::product::{BlockedLayer, Inputs, Product, Unplanned, Wanted};
use crate::regions::Regions;
use crate::sources::Source;
use crate::store::Store;

/// The fetch rounds of one product: a loop guard, far above the two that the planner needs.
const ROUNDS: usize = 8;

/// The sources that a read-only command fetches to list the steps: small metadata, never bulk data.
const METADATA: [&str; 3] = ["geofabrik-poly", "geofabrik-index", "copernicus-glo-30-tiles"];

/// The steps of every product, and the steps that cannot be planned.
pub(super) struct Listed {
    pub steps: Vec<Step>,
    pub blocked: Vec<BlockedLayer>,
}

/// Fetch what a product names, and give the version fetched; with the code of a failed fetch:
/// `fetch_failed` or `blocked`. A fetch without a version takes the newest version upstream.
/// Without `bulk`, only [`METADATA`] is fetched. With `run`, the run records each fetch.
pub(super) fn fetcher<'a>(
    root: &'a Path,
    store: &'a Store,
    http: &'a Http,
    sources: &'a [Source],
    mut run: Option<&'a mut Run>,
    bulk: bool,
) -> impl FnMut(&Wanted) -> Result<String, Error> + 'a {
    move |wanted| {
        if !bulk && !METADATA.contains(&wanted.source.as_str()) {
            return Err(Code::Blocked
                .error(format!("source `{}` is not in the store, and only an update fetches data", wanted.source))
                .fix("Run `obc data update TARGET`: it fetches what the steps read."));
        }
        let source = sources
            .iter()
            .find(|source| source.id == wanted.source)
            .ok_or_else(|| Code::InvalidData.error(format!("no source `{}` in data/sources.toml", wanted.source)))?;
        let unnamed = fetch::params(source).into_iter().find(|name| wanted.params.iter().all(|(n, _)| n != name));
        if let Some(name) = unnamed.filter(|_| wanted.version.is_none()) {
            let message = format!("source `{}` is fetched per `{name}=`: it has no one newest version", source.id);
            return Err(Code::Failed.error(message).fix(format!(
                "The step list of a product names this fetch: correct the product, or name a `start` of `{}`.",
                source.id
            )));
        }
        let request =
            Request { refresh: false, source, version: wanted.version.clone(), params: wanted.params.clone() };
        let snapshot = match run.as_deref_mut() {
            Some(run) => run.fetch_request(root, store, http, &request, &[]),
            None => fetch::fetch(root, store, http, &request),
        };
        fetched(source, snapshot).map(|snapshot| snapshot.version)
    }
}

/// The steps of every product. A product whose step list reads snapshots that the store lacks gets
/// them fetched, as [`product_steps`] says. A product that does not suit the settings blocks every
/// layer that it has. With `observe`, a failed fetch blocks its product, and the steps run no tool
/// check.
pub(super) fn steps(
    root: &Path,
    products: &[&dyn Product],
    inputs: &mut Inputs,
    regions: &Regions,
    store: &Store,
    observe: bool,
    mut fetch: impl FnMut(&Wanted) -> Result<String, Error>,
) -> Result<Listed, Error> {
    let offered = |layer: &&String| products.iter().any(|product| product.optional().contains(&layer.as_str()));
    if let Some(layer) = inputs.settings.layers.iter().find(|layer| !offered(layer)) {
        return Err(Code::Usage.error(format!("no product has the optional layer `{layer}`")));
    }
    inputs.fetch_failures.clear();
    let mut listed = Listed { steps: Vec::new(), blocked: Vec::new() };
    for product in products {
        let blocked =
            |reason: String| BlockedLayer { step: product.name().into(), layer: product.name().into(), reason };
        let before = inputs.fetch_failures.len();
        match product_steps(root, *product, inputs, regions, store, &mut fetch, observe) {
            Ok(Ok(steps)) => {
                listed.blocked.extend(steps.blocked);
                add_steps(products, *product, &mut listed.steps, steps.steps)?;
            }
            Ok(Err(reason)) => listed.blocked.push(blocked(reason)),
            Err(error)
                if observe
                    && inputs.fetch_failures.len() > before
                    && matches!(error.code, Code::Blocked | Code::FetchFailed) =>
            {
                listed.blocked.push(blocked(format!("{}; {}", error.message, error.fix)))
            }
            Err(error) => return Err(error),
        }
    }
    Ok(listed)
}

/// The steps of one product, after the fetches that its step list needs: it is asked again while
/// each round names only new fetches, at most [`ROUNDS`] times. `Ok(Err(reason))` when the
/// product does not suit the settings (`Unplanned::Invalid`).
fn product_steps(
    root: &Path,
    product: &dyn Product,
    inputs: &mut Inputs,
    regions: &Regions,
    store: &Store,
    fetch: &mut impl FnMut(&Wanted) -> Result<String, Error>,
    observe: bool,
) -> Result<Result<crate::product::Steps, String>, Error> {
    let declarations = |inputs: &Inputs| match observe {
        true => product.status_steps(root, inputs, regions, store),
        false => product.steps(root, inputs, regions, store),
    };
    let name = product.name();
    let (mut failure, earlier) = (None, inputs.fetch_failures.len());
    let mut listed = declarations(inputs);
    // A fetch can name the next one, such as the `.poly` that gives the box of a capture.
    let mut fetched: Vec<Wanted> = Vec::new();
    for _ in 0..ROUNDS {
        let Err(Unplanned::NeedsFetch(fetches)) = &listed else { break };
        if fetches.iter().any(|wanted| fetched.contains(wanted)) {
            break;
        }
        for wanted in fetches {
            let wanted = &inputs.pinned(wanted);
            let version = match fetch(wanted) {
                Ok(version) => version,
                Err(error) if matches!(error.code, Code::FetchFailed | Code::Blocked) => {
                    inputs.fetch_failures.push((wanted.clone(), error.message.clone()));
                    // These owners share a capture. A sibling must not restart a failed one.
                    if error.code == Code::FetchFailed
                        && matches!(wanted.source.as_str(), "wikidata" | "wikipedia" | "commons")
                    {
                        return Err(error);
                    }
                    failure = Some(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            inputs.resolve(wanted, version);
        }
        fetched.extend(fetches.iter().cloned());
        listed = declarations(inputs);
    }
    match listed {
        Ok(steps) => Ok(Ok(steps)),
        Err(Unplanned::NeedsFetch(fetches)) => {
            if let Some(mut error) = failure {
                // A national terrain model is a fetch per tile: the count of each source, not a line each.
                let failures = &inputs.fetch_failures[earlier..];
                if failures.len() > 1 {
                    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
                    for (wanted, _) in failures {
                        *counts.entry(&wanted.source).or_default() += 1;
                    }
                    let counts: Vec<_> = counts.iter().map(|(source, n)| format!("{source} {n}")).collect();
                    let failed = failures.len();
                    error.message = format!("{}; {failed} fetches failed: {}", error.message, counts.join(", "));
                }
                return Err(error);
            }
            let wanted = fetches
                .iter()
                .map(|f| format!("{}@{}", f.source, f.version.as_deref().unwrap_or("newest")))
                .collect::<Vec<_>>();
            Err(product_bug(name, format!("product `{name}` still needs {} after the fetch", wanted.join(", "))))
        }
        Err(Unplanned::Invalid(reason)) => Ok(Err(reason)),
        Err(Unplanned::Failed(e)) => Err(Code::Failed.error(format!("product `{name}`: {e}"))),
    }
}

/// An error in the code of product `name`, not in `data/`.
fn product_bug(name: &str, message: String) -> Error {
    let fix = format!("The code of product `{name}` is wrong, not the data: correct the product in its crate.");
    Code::Failed.error(message).fix(fix)
}

/// Add the steps of `product` to `all`. Each step is named `<product>/…` after a product. A step
/// that two products list, such as the landmark content that the device maps and the planner
/// read, has the same declaration in both and is listed once. A Rust function is not compared: two
/// addresses of one function can differ.
fn add_steps(
    products: &[&dyn Product],
    product: &dyn Product,
    all: &mut Vec<Step>,
    steps: Vec<Step>,
) -> Result<(), Error> {
    let name = product.name();
    for step in steps {
        if !products.iter().any(|owner| step.name.starts_with(&format!("{}/", owner.name()))) {
            return Err(product_bug(name, format!("step `{}` of product `{name}` names no product", step.name)));
        }
        match all.iter().find(|listed| listed.name == step.name) {
            None => all.push(step),
            Some(listed) if same(listed, &step) => {}
            Some(_) => {
                let message =
                    format!("step `{}` of product `{name}` differs from the step of another product", step.name);
                return Err(product_bug(name, message));
            }
        }
    }
    Ok(())
}

/// Whether two steps have the same recipe, outputs and kind of run, with the same arguments.
fn same(a: &Step, b: &Step) -> bool {
    use crate::engine::Run;
    let run = |step: &Step| match &step.run {
        Run::Rust(_) => (0, None),
        Run::Command(argv) => (1, Some(argv.clone())),
        Run::Python(argv) => (2, Some(argv.clone())),
    };
    crate::engine::recipe(a) == crate::engine::recipe(b) && a.outputs == b.outputs && run(a) == run(b)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::engine::snapshot_files;
    use crate::engine::tests::{fixture, pipeline};
    use crate::product::{Pointer, Steps};

    /// The test pipeline, listed once the store has the snapshot `index@1`.
    pub(crate) struct Indexed;

    impl Product for Indexed {
        fn name(&self) -> &'static str {
            "test"
        }

        fn optional(&self) -> &'static [&'static str] {
            &["extra"]
        }

        fn steps(&self, root: &Path, _: &Inputs, _: &Regions, store: &Store) -> Result<Steps, Unplanned> {
            match snapshot_files(store, "index", "1", &[], &[]).map_err(Unplanned::Failed)? {
                Some(_) => {
                    let mut steps = pipeline();
                    crate::engine::tests::revise(root, &mut steps);
                    Ok(steps.into())
                }
                None => Err(Unplanned::NeedsFetch(vec![Wanted {
                    source: "index".into(),
                    version: Some("1".into()),
                    params: Vec::new(),
                }])),
            }
        }

        fn pointer(&self) -> Pointer {
            Pointer { key: crate::release::POINTERS[0], step: "test/join", path: "joined.txt" }
        }
    }

    fn inputs(layers: &[&str]) -> Inputs {
        let mut inputs = Inputs::default();
        inputs.settings.layers = layers.iter().map(|layer| layer.to_string()).collect();
        inputs
    }

    #[test]
    fn a_step_list_that_reads_a_snapshot_is_listed_again_after_its_fetch() {
        let (fixture, empty) = (fixture("listing-two-stage"), fixture("listing-two-stage-empty"));
        let regions = Regions::new(Vec::new()).unwrap();
        let fetches = Cell::new(0);
        let fetch = |wanted: &Wanted| {
            fixture.fetched(&wanted.source, "index.txt", b"index\n");
            fetches.set(fetches.get() + 1);
            Ok("1".into())
        };
        let list = |store: &Store, layers: &[&str], fetch: &mut dyn FnMut(&Wanted) -> Result<String, Error>| {
            steps(&fixture.root(), &[&Indexed], &mut inputs(layers), &regions, store, false, fetch)
        };
        let listed = list(&fixture.store, &["extra"], &mut { fetch }).unwrap();
        assert_eq!((listed.steps.len(), fetches.get()), (3, 1));
        let listed = list(&fixture.store, &[], &mut |_| unreachable!()).unwrap();
        assert_eq!(listed.steps.len(), 3, "the store has it now");

        let err = list(&empty.store, &[], &mut |_| Ok("1".into())).err().unwrap();
        assert_eq!(err.message, "product `test` still needs index@1 after the fetch");
        assert!(err.fix.contains("product `test`"), "a product bug points at its code: {}", err.fix);
        let err = list(&empty.store, &[], &mut |_| Err(Code::Blocked.error("credential missing"))).err().unwrap();
        assert_eq!(err.code, Code::Blocked, "a failed fetch keeps its code");
        let err = list(&fixture.store, &["snow"], &mut |_| Ok("1".into())).err().unwrap();
        assert_eq!(err.message, "no product has the optional layer `snow`");
    }

    #[test]
    fn a_read_only_listing_fetches_no_bulk_data() {
        let fixture = fixture("listing-read-only");
        let (root, http) = (fixture.root(), Http::new());
        let mut fetch = fetcher(&root, &fixture.store, &http, &[], None, false);
        let extract = Wanted { source: "geofabrik-extracts".into(), version: None, params: Vec::new() };
        let refused = fetch(&extract).unwrap_err();
        assert_eq!(refused.code, Code::Blocked, "{}", refused.message);
    }

    #[test]
    fn a_step_that_two_products_list_is_listed_once_and_has_one_declaration() {
        struct Reader;
        impl Product for Reader {
            fn name(&self) -> &'static str {
                "reader"
            }
            fn steps(&self, _: &Path, _: &Inputs, _: &Regions, _: &Store) -> Result<Steps, Unplanned> {
                Ok(Vec::new().into())
            }
            fn pointer(&self) -> Pointer {
                Pointer { key: crate::release::POINTERS[1], step: "test/count", path: "count/lines.txt" }
            }
        }
        let products: &[&dyn Product] = &[&Indexed, &Reader];
        let mut all = Vec::new();
        add_steps(products, &Indexed, &mut all, pipeline()).unwrap();
        add_steps(products, &Reader, &mut all, pipeline()).unwrap();
        assert_eq!(all.len(), 3);
        let mut other = pipeline();
        other[1].outputs = vec!["other.txt".into()];
        let err = add_steps(products, &Reader, &mut all, other).unwrap_err();
        assert_eq!(err.message, "step `test/join` of product `reader` differs from the step of another product");
    }
}
