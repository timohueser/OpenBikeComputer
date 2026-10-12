//! The products whose steps make the releases of `obc data`, the layers that the steps make, and the
//! producers of prepared sources.

pub mod device;
pub mod planner;
mod readers;
mod region_sources;
pub mod terrain_reference;
pub mod ui;
pub mod wiki;

use obc_data::engine::{Client, Input, Layer, Run, Step};
use obc_data::prepared::Prepared;
use obc_data::product::Product;
use serde_json::Value;

pub const PRODUCTS: &[&dyn Product] = &[&device::Device, &planner::Planner];

/// The producers of the prepared sources.
pub const PREPARED: &[&dyn Prepared] = &[&terrain_reference::TERRAIN_REFERENCE, &wiki::WIKI];

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
    #[test]
    fn the_products_fill_each_pointer_of_a_release_once() {
        let mut keys: Vec<&str> = super::PRODUCTS.iter().map(|product| product.pointer().key).collect();
        keys.sort();
        assert_eq!(keys, obc_data::release::POINTERS);
    }
}
