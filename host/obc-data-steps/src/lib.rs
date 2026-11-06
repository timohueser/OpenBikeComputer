//! The products whose steps make the releases of `obc data`, the layers that the steps make, and the
//! producers of prepared sources.

pub mod climate_reference;
pub mod colour_reference;
pub mod device;
pub mod forest_reference;
mod functions;
pub mod planner;
mod prepared_tiles;
mod readers;
mod region_sources;
pub mod snow_reference;
mod terrain_bias;
pub mod terrain_reference;
pub mod ui;
pub mod wiki;

use obc_data::engine::{Client, Input, Layer, Run, Step};
use obc_data::prepared::Prepared;
use obc_data::product::Product;
use serde_json::Value;

pub const PRODUCTS: &[&dyn Product] = &[&device::Device, &planner::Planner];

pub use functions::FUNCTIONS;

/// The producers of the prepared sources.
pub const PREPARED: &[&dyn Prepared] = &[
    &terrain_reference::TERRAIN_REFERENCE,
    &forest_reference::FOREST_REFERENCE,
    &climate_reference::CLIMATE_REFERENCE,
    &snow_reference::SNOW_REFERENCE,
    &colour_reference::COLOUR_REFERENCE,
    &wiki::WIKI,
];

/// A Python step of `layer`: `entry`, a `tools.*` module or a script, with the argument `--step`,
/// under `uv run` with the packages of `group` of `pyproject.toml`. The hash seed is fixed, so the
/// order of a set never reaches the bytes of a layer.
pub(crate) fn python(
    layer: &'static Layer,
    name: &str,
    inputs: Vec<Input>,
    options: Value,
    (entry, group): (&str, Option<&str>),
    outputs: &[&str],
) -> Step {
    let mut argv: Vec<String> = [
        "env",
        "PYTHONHASHSEED=0",
        "uv",
        "run",
        "--locked",
        "--offline",
        "--no-default-groups",
        "--no-python-downloads",
    ]
    .map(String::from)
    .into();
    argv.extend(group.into_iter().flat_map(|group| ["--group".to_string(), group.to_string()]));
    argv.push("python".into());
    if !entry.ends_with(".py") {
        argv.push("-m".into());
    }
    argv.extend([entry.to_string(), "--step".to_string()]);
    Step {
        layer,
        name: name.into(),
        inputs,
        options,
        outputs: outputs.iter().map(|output| output.to_string()).collect(),
        run: Run::Python(argv),
        client: Client::All,
    }
}

#[cfg(test)]
mod tests {
    /// The child process of a Rust step of a test runs it here.
    #[test]
    fn obc_data_step() {
        obc_data::engine::child(&[super::FUNCTIONS, &[&crate::device::tests::FAKE]].concat());
    }

    #[test]
    fn each_function_has_a_name_of_its_own_and_is_listed() {
        let names: std::collections::BTreeSet<&str> = super::FUNCTIONS.iter().map(|function| function.name).collect();
        assert_eq!(names.len(), super::FUNCTIONS.len(), "two functions have one name");
        let declared = include_str!("functions.rs").matches(": Function =").count();
        assert_eq!(
            declared + 1,
            super::FUNCTIONS.len(),
            "FUNCTIONS lists each static of functions.rs and `obc-data/pass`"
        );
    }

    #[test]
    fn the_products_fill_each_pointer_of_a_release_once() {
        let mut keys: Vec<&str> = super::PRODUCTS.iter().map(|product| product.pointer().key).collect();
        keys.sort();
        assert_eq!(keys, obc_data::release::POINTERS);
    }

    /// The source overview gives a source that only a prepare reads the need of its producer.
    #[test]
    fn each_source_that_only_a_prepare_reads_has_one_producer() {
        use obc_data::sources::{FetchKind, Keep, Kind};
        let only_prepared = obc_data::sources::all().iter().filter(|source| {
            source.kind == Kind::Data && source.keep == Keep::Prepared && source.fetch.kind != FetchKind::Prepared
        });
        for source in only_prepared {
            let producers: Vec<&str> =
                super::PREPARED.iter().filter(|p| p.reads(source)).map(|producer| producer.name()).collect();
            assert_eq!(producers.len(), 1, "`{}` is read by {producers:?}", source.id);
        }
    }

    #[test]
    fn each_source_that_a_product_can_read_is_a_row_of_the_registry() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for product in super::PRODUCTS {
            let reads = product.reads(&root);
            assert!(reads.len() > 5, "{}", product.name());
            for id in reads {
                assert!(obc_data::sources::all().iter().any(|source| source.id == id), "{}: `{id}`", product.name());
            }
        }
    }
}
