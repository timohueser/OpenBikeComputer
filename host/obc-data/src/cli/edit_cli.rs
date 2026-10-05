//! The edits of an environment: `region ENV ID`, `layer ENV NAME on|off` and `undo ENV`. An edit
//! writes `data/env/ENV.toml` and nothing else: it never commits.

use std::path::Path;
use std::process::Command;

use clap::ValueEnum;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{check_layers, load};
use super::{print_json, Code, Error};
use crate::env::Env;
use crate::product::Product;
use crate::store::write_atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

/// An environment file after an edit.
#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Edited {
    pub env: String,
    pub region: String,
    pub layers: Vec<String>,
}

pub fn region(root: &Path, products: &[&dyn Product], name: &str, id: &str, json: bool) -> Result<(), Error> {
    print(edit(root, products, name, |env| env.region = id.into())?, json)
}

pub fn layer(
    root: &Path,
    products: &[&dyn Product],
    name: &str,
    layer: &str,
    switch: Switch,
    json: bool,
) -> Result<(), Error> {
    let edited = edit(root, products, name, |env| match switch {
        Switch::On if !env.layers.iter().any(|on| on == layer) => env.layers.push(layer.into()),
        Switch::On => {}
        Switch::Off => env.layers.retain(|on| on != layer),
    })?;
    print(edited, json)
}

/// Write `data/env/<name>.toml` as git has it in `HEAD`.
pub fn undo(root: &Path, name: &str, json: bool) -> Result<(), Error> {
    if !crate::is_kebab(name) {
        return Err(Code::Usage.error(format!("`{name}` is not an environment name")));
    }
    let file = format!("data/env/{name}.toml");
    let shown = Command::new("git").arg("-C").arg(root).args(["show", &format!("HEAD:{file}")]).output();
    let shown = shown.map_err(|e| format!("git: {e}"))?;
    if !shown.status.success() {
        let why = String::from_utf8_lossy(&shown.stderr).trim().to_string();
        return Err(Code::Usage.error(format!("{file} has no committed version: {why}")));
    }
    write_atomic(&Env::path(root, name), &shown.stdout)?;
    let env = load(root, name)?.env;
    print(Edited { env: env.name, region: env.region, layers: env.layers }, json)
}

/// Change the environment `name` and write its file, when the result is valid.
fn edit(root: &Path, products: &[&dyn Product], name: &str, change: impl FnOnce(&mut Env)) -> Result<Edited, Error> {
    let loaded = load(root, name)?;
    let path = Env::path(root, name);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut env = loaded.env;
    change(&mut env);
    let text = env.edit(&text);
    let refused = |e: String| Code::Usage.error(e).fix("Nothing changed.");
    let env = Env::parse(name, &text, &loaded.regions).map_err(|e| refused(format!("data/env/{name}.toml: {e}")))?;
    check_layers(products, &env).map_err(|e| refused(e.message))?;
    write_atomic(&path, text.as_bytes())?;
    Ok(Edited { env: env.name, region: env.region, layers: env.layers })
}

fn print(edited: Edited, json: bool) -> Result<(), Error> {
    if json {
        return print_json(&edited);
    }
    let layers = if edited.layers.is_empty() { "—".into() } else { edited.layers.join(", ") };
    println!("data/env/{}.toml · region {} · layers {layers}", edited.env, edited.region);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::write;
    use crate::engine::Step;
    use crate::product::Unplanned;
    use crate::regions::Regions;
    use crate::store::tests::Scratch;
    use crate::store::Store;

    struct Optional;

    impl Product for Optional {
        fn name(&self) -> &'static str {
            "test"
        }

        fn optional(&self) -> &'static [&'static str] {
            &["climate", "sun"]
        }

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            Ok(Vec::new())
        }
    }

    const LIVE: &str = "# Live.\nregion = \"monaco\"\nlayers = []\n";

    fn git(root: &Path, args: &[&str]) {
        let done = Command::new("git").arg("-C").arg(root).args(args).output().unwrap();
        assert!(done.status.success(), "{}", String::from_utf8_lossy(&done.stderr));
    }

    #[test]
    fn an_edit_writes_the_environment_file_and_undo_restores_the_committed_one() {
        let scratch = Scratch::new("cli-edit");
        let root = scratch.0.join("repository");
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        for region in ["monaco", "europe/andorra"] {
            write(&root.join(format!("data/regions/{region}.toml")), "name = \"A region\"\nkind = \"geofabrik\"\n");
        }
        write(&root.join("data/env/live.toml"), LIVE);
        let file = || std::fs::read_to_string(root.join("data/env/live.toml")).unwrap();
        let products: &[&dyn Product] = &[&Optional];

        assert!(region(&root, products, "live", "atlantis", false).unwrap_err().message.contains("atlantis"));
        let snow = layer(&root, products, "live", "snow", Switch::On, false).unwrap_err();
        assert_eq!(snow.code, Code::Usage, "{}", snow.message);
        assert_eq!(file(), LIVE, "a refused edit changes nothing");

        region(&root, products, "live", "europe/andorra", false).unwrap();
        for (name, switch) in [("sun", Switch::On), ("climate", Switch::On), ("sun", Switch::On), ("sun", Switch::Off)]
        {
            layer(&root, products, "live", name, switch, false).unwrap();
        }
        assert_eq!(file(), "# Live.\nregion = \"europe/andorra\"\nlayers = [\"climate\"]\n");

        let refused = undo(&root, "live", false).unwrap_err();
        assert!(refused.message.contains("has no committed version"), "{}", refused.message);
        git(&root, &["init", "--quiet"]);
        write(&root.join("data/env/live.toml"), LIVE);
        git(&root, &["add", "data/env/live.toml"]);
        git(&root, &["-c", "user.name=test", "-c", "user.email=test@example.org", "commit", "--quiet", "-m", "live"]);
        region(&root, products, "live", "europe/andorra", false).unwrap();
        undo(&root, "live", false).unwrap();
        assert_eq!(file(), LIVE);
    }
}
