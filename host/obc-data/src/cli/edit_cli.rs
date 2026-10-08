//! The edits of an environment: `region ENV ID`, `layer ENV NAME on|off` and `undo ENV`. An edit
//! saves Live settings in the store or Local settings in its ignored file. It never commits.

use std::path::Path;
use std::process::Command;

use clap::ValueEnum;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{check_layers, load};
use super::{print_json, Code, Error};
use crate::env::Env;
use crate::product::Product;
use crate::store::{write_atomic, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

/// Environment settings after an edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Edited {
    pub env: String,
    pub region: String,
    pub layers: Vec<String>,
}

pub fn region(root: &Path, store: &Store, products: &[&dyn Product], name: &str, id: &str) -> Result<Edited, Error> {
    edit(root, store, products, name, true, |env| env.region = id.into())
}

pub fn layer(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    name: &str,
    layer: &str,
    switch: Switch,
) -> Result<Edited, Error> {
    edit(root, store, products, name, false, |env| match switch {
        Switch::On if !env.layers.iter().any(|on| on == layer) => env.layers.push(layer.into()),
        Switch::On => {}
        Switch::Off => env.layers.retain(|on| on != layer),
    })
}

/// Restore applied Live settings, or the committed settings of another environment.
pub fn undo(root: &Path, store: &Store, name: &str) -> Result<Edited, Error> {
    if name == "live" {
        let settings = crate::settings::undo(store)?;
        return Ok(Edited { env: name.into(), region: settings.region, layers: settings.layers });
    }
    write_atomic(&Env::path(root, name), &committed(root, name)?)?;
    current(root, store, name)
}

/// The current environment settings.
pub fn current(root: &Path, store: &Store, name: &str) -> Result<Edited, Error> {
    let env = load(root, name, store)?.env;
    Ok(Edited { env: env.name, region: env.region, layers: env.layers })
}

/// Whether Live has pending edits, or another environment differs from its committed file.
pub fn edited(root: &Path, store: &Store, name: &str) -> bool {
    if name == "live" {
        return crate::settings::edited(store).unwrap_or(true);
    }
    let now = std::fs::read(Env::path(root, name)).ok();
    committed(root, name).is_ok_and(|committed| Some(committed) != now)
}

/// `data/env/<name>.toml` as git has it in `HEAD`.
fn committed(root: &Path, name: &str) -> Result<Vec<u8>, Error> {
    if !crate::is_kebab(name) {
        return Err(Code::Usage.error(format!("`{name}` is not an environment name")));
    }
    let file = format!("data/env/{name}.toml");
    // `./` names the file from `root`, which need not be the top of the repository.
    let shown = Command::new("git").arg("-C").arg(root).args(["show", &format!("HEAD:./{file}")]).output();
    let shown = shown.map_err(|e| format!("git: {e}"))?;
    if !shown.status.success() {
        let why = String::from_utf8_lossy(&shown.stderr).trim().to_string();
        return Err(Code::Usage.error(format!("{file} has no committed version: {why}")));
    }
    Ok(shown.stdout)
}

/// Save a valid environment edit. Only an explicit region selection copies a new definition.
fn edit(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    name: &str,
    select_region: bool,
    change: impl FnOnce(&mut Env),
) -> Result<Edited, Error> {
    if name == "live" {
        let mut settings = crate::settings::current(store)?;
        let regions = crate::settings::regions(root, store)?;
        let mut env = Env {
            name: name.into(),
            region: settings.region.clone(),
            layers: settings.layers.clone(),
            ..Env::default()
        };
        change(&mut env);
        if select_region {
            settings.select(&regions, &env.region).map_err(|e| Code::Usage.error(e))?;
        } else {
            settings.env().map_err(|e| Code::Usage.error(e))?;
        }
        env.layers.sort();
        env.layers.dedup();
        check_layers(products, &env)?;
        settings.layers = env.layers;
        crate::settings::save(store, &settings)?;
        return Ok(Edited { env: name.into(), region: settings.region, layers: settings.layers });
    }
    let path = Env::path(root, name);
    // Local has no file until its first edit.
    let (mut env, text, regions) = if name == "local" && !path.exists() {
        let regions = crate::settings::regions(root, store).map_err(|e| Code::InvalidData.error(e))?;
        (Env { name: name.into(), ..Env::default() }, String::new(), regions)
    } else {
        let loaded = load(root, name, store)?;
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        (loaded.env, text, loaded.regions)
    };
    change(&mut env);
    let refused = |e: String| Code::Usage.error(e).fix("Nothing changed.");
    if env.region.is_empty() {
        return Err(refused("Local has no region: choose one with `obc data region local REGION`".into()));
    }
    let text = env.edit(&text);
    let env = Env::parse(name, &text, &regions).map_err(|e| refused(format!("data/env/{name}.toml: {e}")))?;
    regions.get(&env.region).expect("parse checks the region").selectable().map_err(refused)?;
    check_layers(products, &env).map_err(|e| refused(e.message))?;
    write_atomic(&path, text.as_bytes())?;
    Ok(Edited { env: env.name, region: env.region, layers: env.layers })
}

pub fn print(edited: Edited, json: bool) -> Result<(), Error> {
    if json {
        return print_json(&edited);
    }
    let layers = if edited.layers.is_empty() { "—".into() } else { edited.layers.join(", ") };
    println!("{} settings · region {} · layers {layers}", edited.env, edited.region);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::write;
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

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Ok(Vec::new().into())
        }
    }

    #[test]
    fn live_edits_capture_definitions_in_the_store_and_undo_restores_applied_settings() {
        let scratch = Scratch::new("cli-edit");
        let root = scratch.0.join("repository");
        let store = Store::at(scratch.0.join("store"));
        write(&root.join("data/sources.toml"), include_str!("../../../../data/sources.toml"));
        let area = "name = \"A region\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\n";
        for region in ["monaco", "europe/andorra"] {
            write(
                &root.join(format!("data/regions/{region}.toml")),
                &format!("{area}countries = [\"AD\"]\ntime_zone = \"Europe/Andorra\"\n"),
            );
        }
        write(&root.join("data/regions/bare.toml"), area);
        let products: &[&dyn Product] = &[&Optional];
        assert!(region(&root, &store, products, "live", "atlantis").unwrap_err().message.contains("atlantis"));
        assert!(region(&root, &store, products, "live", "bare").unwrap_err().message.contains("time_zone"));
        assert!(layer(&root, &store, products, "local", "sun", Switch::On).unwrap_err().message.contains("no region"));
        region(&root, &store, products, "local", "monaco").unwrap();
        assert!(root.join("data/env/local.toml").is_file());
        region(&root, &store, products, "live", "monaco").unwrap();
        let applied = crate::settings::current(&store).unwrap();
        crate::settings::applied(&store, &applied).unwrap();
        assert!(!edited(&root, &store, "live"));
        assert!(layer(&root, &store, products, "live", "snow", Switch::On).is_err());
        assert_eq!(crate::settings::current(&store).unwrap(), applied);
        region(&root, &store, products, "live", "europe/andorra").unwrap();
        layer(&root, &store, products, "live", "climate", Switch::On).unwrap();
        assert!(edited(&root, &store, "live"));
        let pending = crate::settings::current(&store).unwrap();
        assert_eq!(pending.layers, ["climate"]);
        assert_eq!(pending.definitions.len(), 1);
        let saved = pending.definitions["europe/andorra"].clone();
        write(&root.join("data/regions/europe/andorra.toml"), &saved.replace("A region", "Changed preset"));
        assert_eq!(crate::settings::current(&store).unwrap().definitions["europe/andorra"], saved);
        layer(&root, &store, products, "live", "sun", Switch::On).unwrap();
        assert_eq!(crate::settings::current(&store).unwrap().definitions["europe/andorra"], saved);
        assert!(!root.join("data/env/live.toml").exists());
        undo(&root, &store, "live").unwrap();
        assert_eq!(crate::settings::current(&store).unwrap(), applied);
    }
}
