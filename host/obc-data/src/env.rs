//! An environment: `data/env/<name>.toml` names its region and the optional layers that are on.
//! It names no versions: the live releases record which version of each source live reads, and a
//! plan moves a source with `--move`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::regions::Regions;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub name: String,
    /// A region id of `data/regions/`.
    pub region: String,
    /// The optional layers that are on.
    pub layers: Vec<String>,
    /// Source id to the version that the live releases read; empty without a live release.
    pub live: BTreeMap<String, String>,
    /// Source id to the version of a `--move SOURCE@VERSION`. `None` for `--move SOURCE`: the
    /// newest upstream version, until a fetch names it.
    pub moves: BTreeMap<String, Option<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvFile {
    region: Option<String>,
    #[serde(default)]
    layers: Vec<String>,
    /// Only to refuse it with a reason.
    pins: Option<toml::Table>,
}

impl Env {
    /// `data/env/<name>.toml` of the repository at `root`.
    pub fn path(root: &Path, name: &str) -> PathBuf {
        root.join("data/env").join(format!("{name}.toml"))
    }

    /// Read `data/env/<name>.toml` of the repository at `root`.
    pub fn load(root: &Path, name: &str, regions: &Regions) -> Result<Env, String> {
        if !crate::is_kebab(name) {
            return Err(format!("`{name}` is not an environment name"));
        }
        let path = Env::path(root, name);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Env::parse(name, &text, regions).map_err(|e| format!("data/env/{name}.toml: {e}"))
    }

    pub fn parse(name: &str, text: &str, regions: &Regions) -> Result<Env, String> {
        let file: EnvFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.pins.is_some() {
            return Err("an environment has no `[pins]`: the live releases record the version of each source, \
                        and `obc data plan ENV --move SOURCE@VERSION` moves one"
                .into());
        }
        let region = file.region.ok_or("it names no `region`")?;
        if regions.get(&region).is_none() {
            return Err(format!("region `{region}` is not a file in data/regions/"));
        }
        let mut seen = BTreeSet::new();
        if let Some(layer) = file.layers.iter().find(|layer| !crate::is_kebab(layer) || !seen.insert(*layer)) {
            return Err(format!("layer `{layer}` is not kebab-case, or is listed twice"));
        }
        Ok(Env { name: name.into(), region, layers: file.layers, ..Env::default() })
    }

    /// The version of `source` that a plan names: its `--move`, or else the version that live
    /// reads. `product::version` decides for a source that this does not name.
    pub fn version(&self, source: &str) -> Option<&str> {
        match self.moves.get(source) {
            Some(moved) => moved.as_deref(),
            None => self.live.get(source).map(String::as_str),
        }
    }

    /// Whether a plan moves `source` to its newest upstream version.
    pub fn moves_to_newest(&self, source: &str) -> bool {
        self.moves.get(source).is_some_and(Option::is_none)
    }

    /// `text`, the file of this environment, with its `region` and `layers`. Comments and the other
    /// lines stay.
    pub fn edit(&self, text: &str) -> String {
        let layers = self.layers.iter().map(|layer| toml::Value::String(layer.clone())).collect();
        let text = set(text, "region", toml::Value::String(self.region.clone()));
        set(&text, "layers", toml::Value::Array(layers))
    }
}

/// `text` with the top-level `key = value`: the line of the key replaced, or else a line added
/// after the last top-level key.
fn set(text: &str, key: &str, value: toml::Value) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let top = lines.iter().position(|line| line.trim_start().starts_with('[')).unwrap_or(lines.len());
    let name = |line: &String| match line.trim_start().starts_with('#') {
        true => None,
        false => line.split_once('=').map(|(name, _)| name.trim().to_string()),
    };
    let line = format!("{key} = {value}");
    match lines[..top].iter().position(|l| name(l).as_deref() == Some(key)) {
        Some(at) => lines[at] = line,
        None => {
            let after = lines[..top].iter().rposition(|l| name(l).is_some());
            lines.insert(after.map_or(top, |at| at + 1), line);
        }
    }
    crate::sources::join(text, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regions::parse_region;

    fn regions() -> Regions {
        let region = |id| parse_region(id, "name = \"A region\"\nkind = \"geofabrik\"\n").unwrap();
        Regions::new(vec![region("monaco"), region("europe/andorra")]).unwrap()
    }

    #[test]
    fn an_environment_names_a_known_region_and_its_layers_and_no_pins() {
        let regions = regions();
        let parse = |text: &str| Env::parse("test", text, &regions);

        let env = parse("region = \"monaco\"\nlayers = [\"climate\"]\n").unwrap();
        assert_eq!((env.region.as_str(), env.layers.as_slice()), ("monaco", &["climate".to_string()][..]));

        assert_eq!(parse("layers = []\n").unwrap_err(), "it names no `region`");
        assert_eq!(parse("region = \"atlantis\"\n").unwrap_err(), "region `atlantis` is not a file in data/regions/");
        assert!(parse("region = \"monaco\"\nlayers = [\"sun\", \"sun\"]\n").unwrap_err().contains("listed twice"));
        assert!(parse("region = \"monaco\"\nlayer = []\n").unwrap_err().contains("unknown field"));
        let pins = parse("region = \"monaco\"\n[pins]\nplanetiler = \"0.9.0\"\n").unwrap_err();
        assert!(pins.contains("no `[pins]`") && pins.contains("--move SOURCE@VERSION"), "{pins}");
    }

    #[test]
    fn a_move_comes_before_the_version_of_live() {
        let mut env = Env::default();
        env.live.insert("osm".into(), "2026-09-01".into());
        env.live.insert("land".into(), "2026-09-01".into());
        env.moves.insert("osm".into(), Some("2026-10-01".into()));
        env.moves.insert("land".into(), None);
        assert_eq!((env.version("osm"), env.version("land"), env.version("qrank")), (Some("2026-10-01"), None, None));
        assert!(env.moves_to_newest("land") && !env.moves_to_newest("osm") && !env.moves_to_newest("qrank"));
    }

    #[test]
    fn an_edit_keeps_the_comments_and_the_other_lines() {
        let regions = regions();
        let text = "# Live.\n\n# The region.\nregion = \"monaco\"\r\n# The layers.\nlayers = []\n";
        let mut env = Env::parse("live", text, &regions).unwrap();
        (env.region, env.layers) = ("europe/andorra".into(), vec!["climate".into(), "sun".into()]);
        let edited = env.edit(text);
        let expected = "# Live.\r\n\r\n# The region.\r\nregion = \"europe/andorra\"\r\n# The layers.\r\nlayers = [\"climate\", \"sun\"]\r\n";
        assert_eq!(edited, expected);
        assert_eq!(Env::parse("live", &edited, &regions).unwrap().layers, env.layers);
        assert_eq!(
            env.edit("# Only a comment.\n"),
            "# Only a comment.\nregion = \"europe/andorra\"\nlayers = [\"climate\", \"sun\"]\n"
        );
    }

    #[test]
    fn live_is_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Env::load(&root, "live", &Regions::load(&root).unwrap()).unwrap();
    }
}
