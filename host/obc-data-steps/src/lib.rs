//! The products whose steps make the releases of `obc data`.

pub mod maps;
pub mod planner;

use obc_data::engine::{Client, Code, Input, Python, Run, Step};
use obc_data::product::Product;
use serde_json::Value;

pub const PRODUCTS: &[&dyn Product] = &[&maps::Maps, &planner::Planner];

/// The request protocol imported by every Python step.
pub(crate) const PYTHON: [&str; 1] = ["tools/step_request.py"];

/// A Python step: `entry`, a `tools.*` module or a script, with the argument `--step`, under `uv
/// run` with the packages of `group` of `pyproject.toml`. `files` is its code besides [`PYTHON`]:
/// each Python file that it imports, and each file that it reads from the repository. A credit
/// that it writes comes in its options, so `data/sources.toml` is no code of it. The hash seed is
/// fixed, so the order of a set never reaches the bytes of a layer.
pub(crate) fn python(
    name: &str,
    inputs: Vec<Input>,
    options: Value,
    (entry, group): (&str, Option<&str>),
    files: &[&str],
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
    let paths = PYTHON.iter().chain(files).map(|path| path.to_string()).collect();
    Step {
        name: name.into(),
        inputs,
        options,
        code: Code { paths, python: Some(Python { group: group.map(str::to_string) }), ..Default::default() },
        outputs: outputs.iter().map(|output| output.to_string()).collect(),
        run: Run::Command(argv),
        client: Client::All,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_plumbing_knows_the_prefix_of_each_product() {
        let prefixes: Vec<&str> = super::PRODUCTS.iter().map(|product| product.prefix()).collect();
        assert_eq!(prefixes, obc_data::live::PRODUCT_PREFIXES);
    }
}
