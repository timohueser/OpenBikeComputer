//! An environment: `data/env/<name>.toml` names its region and the optional layers that are on.
//! It names no versions: the live releases record which version of each source live reads, and a
//! plan moves a source with `--move`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::regions::Regions;
use crate::store::sorted;

/// The version of each fetch: a source id with its `NAME=VALUE`s, sorted.
pub type RequestKey = (String, Vec<(String, String)>);
pub type Versions = BTreeMap<RequestKey, String>;
/// The versions of each fetch that the live layers read. Two versions of one fetch conflict: only a
/// `--move` chooses between them.
pub type LiveVersions = BTreeMap<RequestKey, BTreeSet<String>>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Env {
    pub name: String,
    /// A region id of `data/regions/`.
    pub region: String,
    /// The optional layers that are on.
    pub layers: Vec<String>,
    pub settings: Option<crate::settings::Settings>,
    /// Sources whose effective policy keeps one version across a multi-request acquisition.
    pub manual: BTreeSet<String>,
    /// The versions of each fetch that the live releases read; empty without a live release.
    pub live: LiveVersions,
    /// Exact reads with retained input copies. Credentials are needed only for a new fetch.
    pub retained: Vec<crate::input_copy::Retained>,
    /// Source id to the version of a `--move SOURCE@VERSION`. `None` for `--move SOURCE`: the
    /// newest upstream version of each request. This intent never changes during resolution.
    pub moves: BTreeMap<String, Option<String>>,
    /// Exact versions resolved by a fetch or selected request refresh.
    pub resolved: Versions,
    /// Active acquisition requests, including requests without local bytes. Held inputs are
    /// direct snapshot inputs and never enter this inventory.
    pub requests: RefCell<BTreeSet<RequestKey>>,
    /// The sources of `moves` that move because they are stale, not by a `--move`. No step list
    /// needs to read them.
    pub stale: BTreeSet<String>,
    /// Automatic refresh selections, scoped to acquisition requests.
    pub stale_requests: BTreeSet<RequestKey>,
    /// The versions of a saved plan. A step list then reads exactly these, and a fetch that they
    /// lack is not in the plan.
    pub planned: Option<Versions>,
    /// Each version that `product::version` gave: what a plan records.
    pub read: RefCell<Versions>,
    /// The sources whose live versions conflict, and that a step list read without a `--move`.
    pub refused: RefCell<BTreeSet<String>>,
    /// Failed requests in this listing pass. Products can block only the layers that need them.
    pub fetch_failures: Vec<(crate::product::Wanted, String)>,
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

    /// Local has a region only when a Local edit or preparation wrote `data/env/local.toml`. It
    /// never borrows the region of Live: that region is large. `region` replaces the file's region.
    pub fn local(root: &Path, regions: &Regions, region: Option<&str>) -> Result<(Env, String), String> {
        let mut text = match std::fs::read_to_string(Self::path(root, "local")) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.to_string()),
        };
        if let Some(region) = region {
            text = set(&text, "region", toml::Value::String(region.into()));
        }
        if text.trim().is_empty() {
            return Err("Local has no region: choose one with `obc data dev REGION`".into());
        }
        let env = Self::parse("local", &text, regions).map_err(|e| format!("data/env/local.toml: {e}"))?;
        regions.get(&env.region).expect("parse checks the region").selectable()?;
        Ok((env, text))
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

    /// The version of the fetch of `source` with `params` that a plan names: that of the saved
    /// plan; or else its `--move`; or else the version that live reads; or else the `start` of the
    /// source in the `data/sources.toml` of this build. Without params, a source
    /// that live reads per params gives the version of all of them, such as the GLO-30 tiles that
    /// one version names. `product::version` decides for a fetch that this does not name. `Err`
    /// when live reads it at more versions.
    pub fn version(&self, source: &str, params: &[(String, String)]) -> Result<Option<&str>, String> {
        let fetch = (source.to_string(), sorted(params));
        if let Some(planned) = &self.planned {
            return Ok(planned.get(&fetch).map(String::as_str));
        }
        if let Some(moved) = self.moves.get(source).filter(|_| !self.stale.contains(source)) {
            return Ok(moved.as_deref().or_else(|| self.resolved.get(&fetch).map(String::as_str)));
        }
        if let Some(version) = self.resolved.get(&fetch) {
            return Ok(Some(version));
        }
        if self.stale_requests.contains(&fetch) && self.moves.contains_key(source) {
            return Ok(None);
        }
        let start = crate::sources::all().iter().find(|s| s.id == source).and_then(|s| s.start.as_deref());
        let versions: BTreeSet<&str> = match self.live.get(&fetch) {
            Some(read) => read.iter().map(String::as_str).collect(),
            // A source with a `start` has one version for all its requests: a new request, such
            // as a tile of a new region, reads the version that live reads.
            None if params.is_empty() || start.is_some() => {
                let reads = self.live.iter().filter(|((id, _), _)| id == source).flat_map(|(_, read)| read);
                reads.map(String::as_str).collect()
            }
            None => BTreeSet::new(),
        };
        match versions.len() {
            0 => Ok(start),
            1 => Ok(versions.first().copied()),
            _ => {
                let versions = versions.into_iter().collect::<Vec<_>>().join(" and ");
                Err(format!("the live layers read `{source}` {params:?} at {versions}"))
            }
        }
    }

    /// Whether a plan moves `source` to its newest upstream version.
    pub fn moves_to_newest(&self, source: &str) -> bool {
        !self.stale.contains(source) && self.moves.get(source).is_some_and(Option::is_none)
    }

    /// The fetch to run for `wanted`. A `manual` source moves as a whole: a request without a
    /// version takes the version that the first fetch of the source in this run gave, so a model
    /// fetched tile by tile over midnight keeps one version.
    pub fn pinned(&self, wanted: &crate::product::Wanted) -> crate::product::Wanted {
        let version =
            wanted.version.clone().or_else(|| self.resolved.get(&(wanted.source.clone(), Vec::new())).cloned());
        crate::product::Wanted { version, ..wanted.clone() }
    }

    /// Record the `version` that a fetch of `wanted` gave. The first of a `manual` source is the
    /// version of the source too.
    pub fn resolve(&mut self, wanted: &crate::product::Wanted, version: String) {
        let manual = self.manual.contains(&wanted.source);
        if manual {
            self.resolved.entry((wanted.source.clone(), Vec::new())).or_insert_with(|| version.clone());
        }
        self.resolved.insert((wanted.source.clone(), sorted(&wanted.params)), version);
    }

    /// `text`, the file of this environment, with its `region` and `layers`. Comments and the other
    /// lines stay.
    pub fn edit(&self, text: &str) -> String {
        let layers = self.layers.iter().map(|layer| toml::Value::String(layer.clone())).collect();
        let text = set(text, "region", toml::Value::String(self.region.clone()));
        set(&text, "layers", toml::Value::Array(layers))
    }
}

/// `text` with the top-level `key = value`: the lines of the key and of its value replaced by one
/// line, or else a line added after the last top-level key.
fn set(text: &str, key: &str, value: toml::Value) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let top = lines.iter().position(|line| line.trim_start().starts_with('[')).unwrap_or(lines.len());
    let name = |line: &String| match line.trim_start().starts_with('#') {
        true => None,
        false => line.split_once('=').map(|(name, _)| name.trim().to_string()),
    };
    let line = format!("{key} = {value}");
    match lines[..top].iter().position(|l| name(l).as_deref() == Some(key)) {
        Some(at) => {
            // A value such as an array can go on over more lines: it ends where the lines parse.
            let whole = |end: &usize| toml::from_str::<toml::Table>(&lines[at..=*end].join("\n")).is_ok();
            let end = (at..top).find(whole).unwrap_or(at);
            lines.splice(at..=end, [line]);
        }
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
        let region =
            |id| parse_region(id, "name = \"A region\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\n").unwrap();
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
    fn a_saved_plan_then_a_move_then_live_names_the_version_of_a_fetch() {
        let area = |area: &str| vec![("area".to_string(), area.to_string())];
        let mut env = Env::default();
        let mut live = |source: &str, params: Vec<(String, String)>, versions: &[&str]| {
            env.live.insert((source.into(), params), versions.iter().map(|v| v.to_string()).collect());
        };
        live("osm", Vec::new(), &["2026-09-01"]);
        live("land", Vec::new(), &["2026-09-01", "2026-09-02"]);
        live("extract", area("a"), &["2026-09-01"]);
        live("tile", area("a"), &["1"]);
        env.moves.insert("osm".into(), Some("2026-10-01".into()));
        let version = |source, params: &[(String, String)]| env.version(source, params).map(|v| v.map(str::to_string));
        assert_eq!(version("osm", &[]), Ok(Some("2026-10-01".into())), "a move first");
        assert_eq!(version("extract", &area("a")), Ok(Some("2026-09-01".into())), "per params");
        assert_eq!(version("extract", &area("c")), Ok(None), "a new area is not read by live");
        assert_eq!(version("tile", &[]), Ok(Some("1".into())), "without params, the one version of live");
        assert!(version("land", &[]).unwrap_err().contains("2026-09-01 and 2026-09-02"), "a conflict");

        env.moves.insert("land".into(), None);
        assert_eq!(env.version("land", &[]), Ok(None), "a move chooses");
        assert!(env.moves_to_newest("land") && !env.moves_to_newest("osm") && !env.moves_to_newest("qrank"));
        env.planned = Some(Versions::from([(("land".into(), Vec::new()), "2026-09-15".into())]));
        assert_eq!((env.version("land", &[]), env.version("osm", &[])), (Ok(Some("2026-09-15")), Ok(None)));

        env.planned = None;
        env.moves.insert("extract".into(), None);
        env.stale.insert("extract".into());
        assert!(!env.moves_to_newest("extract"), "automatic selection is per request");
        env.resolved.insert(("extract".into(), area("b")), "2026-09-20".into());
        assert_eq!(env.version("extract", &area("a")), Ok(Some("2026-09-01")), "an unrelated request keeps live");
        assert_eq!(env.version("extract", &area("b")), Ok(Some("2026-09-20")), "the selected request moves");
    }

    #[test]
    fn a_start_version_serves_only_a_source_that_live_does_not_read() {
        let tile = |id: &str| vec![("tile".to_string(), id.to_string())];
        let mut env = Env::default();
        assert_eq!(env.version("hansen-gfc", &tile("a")), Ok(Some("v1.11")));
        env.live.insert(("hansen-gfc".into(), tile("a")), ["v1.12".into()].into());
        assert_eq!(env.version("hansen-gfc", &tile("b")), Ok(Some("v1.12")), "a new tile keeps the moved version");
    }

    #[test]
    fn an_edit_keeps_the_comments_and_the_other_lines() {
        let regions = regions();
        let text = "# Live.\n\n# The region.\nregion = \"monaco\"\r\n# The layers.\nlayers = [\n  # None yet.\n]\n";
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
}
