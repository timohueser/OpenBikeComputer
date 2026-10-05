//! An environment: `data/env/<name>.toml` names its region, the optional layers that are on, and
//! the version of each source that it is built from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::regions::Regions;
use crate::sources::{parse_env, Source};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub name: String,
    /// A region id of `data/regions/`.
    pub region: String,
    /// The optional layers that are on.
    pub layers: Vec<String>,
    /// Source id to version.
    pub pins: BTreeMap<String, String>,
}

impl Env {
    /// Read `data/env/<name>.toml` of the repository at `root`.
    pub fn load(root: &Path, name: &str, sources: &[Source], regions: &Regions) -> Result<Env, String> {
        if !crate::is_kebab(name) {
            return Err(format!("`{name}` is not an environment name"));
        }
        let path = root.join("data/env").join(format!("{name}.toml"));
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Env::parse(name, &text, sources, regions).map_err(|e| format!("data/env/{name}.toml: {e}"))
    }

    pub fn parse(name: &str, text: &str, sources: &[Source], regions: &Regions) -> Result<Env, String> {
        let file = parse_env(text, sources)?;
        let region = file.region.ok_or("it names no `region`")?;
        if regions.get(&region).is_none() {
            return Err(format!("region `{region}` is not a file in data/regions/"));
        }
        let mut seen = BTreeSet::new();
        if let Some(layer) = file.layers.iter().find(|layer| !crate::is_kebab(layer) || !seen.insert(*layer)) {
            return Err(format!("layer `{layer}` is not kebab-case, or is listed twice"));
        }
        Ok(Env { name: name.into(), region, layers: file.layers, pins: file.pins })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::parse_region;
    use crate::sources::parse_sources;

    #[test]
    fn an_environment_names_a_known_region_and_its_layers() {
        let sources = parse_sources(include_str!("../../../data/sources.toml")).unwrap();
        let regions = Regions::new(vec![parse_region("monaco", "name = \"Monaco\"\nkind = \"geofabrik\"\n").unwrap()]);
        let regions = regions.unwrap();
        let parse = |text: &str| Env::parse("test", text, &sources, &regions);

        let env = parse("region = \"monaco\"\nlayers = [\"climate\"]\n[pins]\nplanetiler = \"0.9.0\"\n").unwrap();
        assert_eq!((env.region.as_str(), env.layers.as_slice()), ("monaco", &["climate".to_string()][..]));
        assert_eq!(env.pins["planetiler"], "0.9.0");

        assert_eq!(parse("[pins]\n").unwrap_err(), "it names no `region`");
        assert_eq!(parse("region = \"atlantis\"\n").unwrap_err(), "region `atlantis` is not a file in data/regions/");
        assert!(parse("region = \"monaco\"\nlayers = [\"sun\", \"sun\"]\n").unwrap_err().contains("listed twice"));
        assert!(parse("region = \"monaco\"\nlayer = []\n").unwrap_err().contains("unknown field"));
    }

    #[test]
    fn live_is_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let sources = parse_sources(include_str!("../../../data/sources.toml")).unwrap();
        Env::load(&root, "live", &sources, &Regions::load(&root).unwrap()).unwrap();
    }
}
