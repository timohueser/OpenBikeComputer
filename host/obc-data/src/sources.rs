//! `data/sources.toml`, the pins of `data/env/live.toml`, and the state of each source.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::date;
use crate::fetch::upstream::Upstream;

/// In the order `obc data sources` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An input that steps read.
    Data,
    /// A file that ships to users as it is.
    Asset,
    /// Code that steps run; nothing of it ships.
    Tool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FetchKind {
    Http,
    Osm,
    Geofabrik,
    Glo30,
    Dtm,
    Capture,
    Github,
    /// A person orders or downloads the files.
    ByHand,
    /// A person installs it, or another source's build brings it.
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fetch {
    pub kind: FetchKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Only for `osm`: the source whose pin is the base day, `from=`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// How upstream names a version, and so what a pin of the source looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VersionScheme {
    /// `YYYY-MM-DD`: the only scheme that gives a pin an age.
    Date,
    Release,
    Commit,
    /// The SHA-256 of the file.
    Digest,
}

/// How old a pin may get before the source is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "RefreshRepr", into = "RefreshRepr")]
pub enum Refresh {
    Days(u16),
    /// Never stale: a person moves the pin.
    Manual,
}

impl Refresh {
    /// Every policy that `refresh` allows.
    pub const ALL: [Refresh; 5] =
        [Refresh::Days(7), Refresh::Days(30), Refresh::Days(90), Refresh::Days(365), Refresh::Manual];
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum RefreshRepr {
    Days(i64),
    Word(String),
}

impl TryFrom<RefreshRepr> for Refresh {
    type Error = String;
    fn try_from(repr: RefreshRepr) -> Result<Self, String> {
        let refresh = match repr {
            RefreshRepr::Days(days) => u16::try_from(days).ok().map(Refresh::Days),
            RefreshRepr::Word(word) => (word == "manual").then_some(Refresh::Manual),
        };
        refresh
            .filter(|refresh| Refresh::ALL.contains(refresh))
            .ok_or("`refresh` is 7, 30, 90 or 365 days, or \"manual\"".into())
    }
}

/// `7`, `30`, `90`, `365` or `manual`, as a command takes it.
impl std::str::FromStr for Refresh {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, String> {
        text.parse().map_or(RefreshRepr::Word(text.into()), RefreshRepr::Days).try_into()
    }
}

impl From<Refresh> for RefreshRepr {
    fn from(refresh: Refresh) -> Self {
        match refresh {
            Refresh::Days(days) => RefreshRepr::Days(days.into()),
            Refresh::Manual => RefreshRepr::Word("manual".into()),
        }
    }
}

impl JsonSchema for Refresh {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Refresh".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let values: Vec<serde_json::Value> =
            Refresh::ALL.iter().map(|refresh| serde_json::to_value(refresh).expect("a policy serializes")).collect();
        schemars::json_schema!({
            "description": "How old a pin may get, in days, before the source is stale; `manual` is never stale.",
            "enum": values
        })
    }
}

impl std::fmt::Display for Refresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refresh::Days(days) => write!(f, "{days} d"),
            Refresh::Manual => f.write_str("manual"),
        }
    }
}

/// What a fetch needs before upstream answers: environment variables, or a file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl Credential {
    /// Whether this machine has it: every variable set and not empty, and the file present.
    pub fn present(&self) -> bool {
        self.env.iter().all(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
            && self.file.as_deref().is_none_or(|file| expand_home(file).is_file())
    }

    pub fn describe(&self) -> String {
        self.env.iter().cloned().chain(self.file.clone()).collect::<Vec<_>>().join(" and ")
    }
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => PathBuf::from(path),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: Kind,
    /// An SPDX id or `LicenseRef-…`. Unset blocks a data source or an asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obligations: Option<String>,
    pub fetch: Fetch,
    /// Hosts the fetch reaches besides the host of `fetch.url`. `*.example.org` is any subdomain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    pub version: VersionScheme,
    pub refresh: Refresh,
    pub redistribute: bool,
    /// R2 keeps a copy, because upstream cannot give a version again.
    #[serde(default)]
    pub r2_copy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<Credential>,
}

impl Source {
    fn validate(&self) -> Result<(), String> {
        let fail = |why: &str| Err(format!("source `{}`: {why}", self.id));
        if !crate::is_kebab(&self.id) {
            return fail("the id is not lowercase kebab-case");
        }
        if let Some(licence) = &self.licence {
            if !is_licence_expression(licence) {
                return fail("`licence` is an SPDX expression: ids or `LicenseRef-…` with AND, OR, WITH and ( )");
            }
        }
        match (&self.fetch.url, self.fetch.kind) {
            (None, FetchKind::Installed) => {}
            (Some(_), FetchKind::Installed) => return fail("an `installed` fetch has no `url`"),
            (None, _) => return fail("`fetch.url` is missing"),
            (Some(url), _) if !url.starts_with("https://") => return fail("`fetch.url` is not https"),
            _ => {}
        }
        if (self.fetch.kind == FetchKind::Osm) != self.fetch.from.is_some() {
            return fail("an `osm` fetch, and only an `osm` fetch, names the source of its base day in `fetch.from`");
        }
        let host = |h: &str| {
            !h.is_empty() && h.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || ".-".contains(c))
        };
        if let Some(bad) = self.hosts.iter().find(|h| !host(h.strip_prefix("*.").unwrap_or(h))) {
            return fail(&format!("`{bad}` in `hosts` is not a host name"));
        }
        if matches!(self.refresh, Refresh::Days(_)) && self.version != VersionScheme::Date {
            return fail("a refresh in days needs `version = \"date\"`: only a date pin has an age");
        }
        if self.r2_copy && !self.redistribute {
            return fail("`r2_copy` needs `redistribute`: R2 is public");
        }
        if let Some(credential) = &self.credential {
            if credential.env.is_empty() == credential.file.is_none() {
                return fail("a credential is `env` or `file`");
            }
        }
        Ok(())
    }
}

/// The SPDX expression grammar: `id [WITH id]`, `( expr )`, joined by `AND` or `OR`.
fn is_licence_expression(text: &str) -> bool {
    let spaced = text.replace('(', " ( ").replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    let mut at = 0;
    expression(&tokens, &mut at) && at == tokens.len()
}

fn expression(tokens: &[&str], at: &mut usize) -> bool {
    loop {
        if !term(tokens, at) {
            return false;
        }
        match tokens.get(*at) {
            Some(&("AND" | "OR")) => *at += 1,
            _ => return true,
        }
    }
}

fn term(tokens: &[&str], at: &mut usize) -> bool {
    let id = |token: Option<&&str>| {
        token.is_some_and(|t| {
            !["AND", "OR", "WITH", "(", ")"].contains(t)
                && t.chars().all(|c| c.is_ascii_alphanumeric() || "-.+".contains(c))
        })
    };
    if tokens.get(*at) == Some(&"(") {
        *at += 1;
        let inner = expression(tokens, at) && tokens.get(*at) == Some(&")");
        *at += 1;
        return inner;
    }
    if !id(tokens.get(*at)) {
        return false;
    }
    *at += 1;
    if tokens.get(*at) == Some(&"WITH") {
        *at += 1;
        if !id(tokens.get(*at)) {
            return false;
        }
        *at += 1;
    }
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcesFile {
    source: Vec<Source>,
}

/// Parse and validate `data/sources.toml`.
pub fn parse_sources(text: &str) -> Result<Vec<Source>, String> {
    let file: SourcesFile = toml::from_str(text).map_err(|e| format!("data/sources.toml: {e}"))?;
    let mut seen = BTreeSet::new();
    for source in &file.source {
        source.validate()?;
        if !seen.insert(source.id.as_str()) {
            return Err(format!("source `{}` is listed twice", source.id));
        }
    }
    let from = |source: &&Source| source.fetch.from.as_deref().is_some_and(|id| !seen.contains(id));
    if let Some(source) = file.source.iter().find(from) {
        return Err(format!("source `{}`: `fetch.from` names no source", source.id));
    }
    Ok(file.source)
}

/// An environment file. `crate::env` checks `region` and `layers`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnvFile {
    pub(crate) region: Option<String>,
    #[serde(default)]
    pub(crate) layers: Vec<String>,
    #[serde(default)]
    pub(crate) pins: BTreeMap<String, String>,
}

/// Parse an environment file and check its `[pins]`.
pub(crate) fn parse_env(text: &str, sources: &[Source]) -> Result<EnvFile, String> {
    let file: EnvFile = toml::from_str(text).map_err(|e| e.to_string())?;
    for (id, pin) in &file.pins {
        let source = sources.iter().find(|s| &s.id == id).ok_or_else(|| format!("pin `{id}` names no source"))?;
        if source.version == VersionScheme::Date && date::parse(pin).is_none() {
            return Err(format!("pin `{id}` = `{pin}` is not a YYYY-MM-DD date"));
        }
    }
    Ok(file)
}

/// Parse the `[pins]` of an environment file: source id to version.
pub fn parse_pins(text: &str, sources: &[Source]) -> Result<BTreeMap<String, String>, String> {
    parse_env(text, sources).map(|file| file.pins)
}

/// The sources and the live pins of the repository at `root`.
pub struct Registry {
    pub sources: Vec<Source>,
    pub pins: BTreeMap<String, String>,
}

impl Registry {
    pub fn load(root: &Path) -> Result<Self, String> {
        let read = |path: &Path| std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()));
        let sources = parse_sources(&read(&root.join("data/sources.toml"))?)?;
        let pins = parse_pins(&read(&root.join("data/env/live.toml"))?, &sources)
            .map_err(|e| format!("data/env/live.toml: {e}"))?;
        Ok(Self { sources, pins })
    }

    /// The registry of the repository above the current directory, or else above the running
    /// program.
    pub fn live() -> Result<Self, String> {
        let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
        let root = starts.into_iter().flatten().find_map(|start| crate::find_root(&start));
        Self::load(&root.ok_or("no data/sources.toml above the current directory or the program")?)
    }
}

/// A source of the `data/sources.toml` that this build embeds. A product that carries a credit
/// takes it here, so the text has one home. Ids are constants in code: an unknown id panics.
pub fn embedded(id: &str) -> &'static Source {
    static SOURCES: LazyLock<Vec<Source>> = LazyLock::new(|| {
        parse_sources(include_str!("../../../data/sources.toml")).expect("data/sources.toml is valid")
    });
    SOURCES.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no source `{id}` in data/sources.toml"))
}

/// The credit of an embedded source, as the product must show it.
pub fn attribution(id: &str) -> &'static str {
    embedded(id).attribution.as_deref().unwrap_or_else(|| panic!("source `{id}` has no attribution"))
}

/// The state of a source or a layer. A source is only ok, stale or blocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ok,
    Stale,
    CodeChanged,
    InputChanged,
    NotApplied,
    Blocked,
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            State::Ok => "ok",
            State::Stale => "stale",
            State::CodeChanged => "code changed",
            State::InputChanged => "input changed",
            State::NotApplied => "not applied",
            State::Blocked => "blocked",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    pub state: State,
    pub reason: Option<String>,
    /// Days since the pin's date; only a date pin has one.
    pub age_days: Option<i64>,
}

/// `text`, an environment file, with `id = "version"` in its `[pins]`: the pin replaced, or added
/// after the last pin. Comments and the order of the other lines stay.
pub fn set_pin(text: &str, id: &str, version: &str) -> String {
    let pin = format!("{id} = \"{version}\"");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = |line: &str| {
        line.trim().strip_prefix("[pins]").is_some_and(|rest| rest.trim().is_empty() || rest.trim().starts_with('#'))
    };
    match lines.iter().position(|line| header(line)) {
        None => lines.extend([String::new(), "[pins]".into(), pin]),
        Some(table) => {
            let end = lines[table + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map_or(lines.len(), |i| table + 1 + i);
            let key = |line: &str| line.split_once('=').map(|(key, _)| key.trim().to_string());
            match (table + 1..end).find(|&i| key(&lines[i]).as_deref() == Some(id)) {
                Some(i) => lines[i] = pin,
                None => {
                    let last = (table + 1..end).rev().find(|&i| key(&lines[i]).is_some()).unwrap_or(table);
                    lines.insert(last + 1, pin);
                }
            }
        }
    }
    join(text, lines)
}

/// `text`, `data/sources.toml`, with the `refresh` of source `id` replaced. Comments and the other
/// lines stay.
pub fn set_refresh(text: &str, id: &str, refresh: Refresh) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = |line: &String| line.trim_start().starts_with('[');
    let names = |line: &String| {
        toml::from_str::<toml::Table>(line).is_ok_and(|table| table.get("id").and_then(|v| v.as_str()) == Some(id))
    };
    let at = lines.iter().position(names).ok_or_else(|| format!("no source `{id}`"))?;
    let start = lines[..at].iter().rposition(header).map_or(0, |i| i + 1);
    let end = lines[at..].iter().position(header).map_or(lines.len(), |i| at + i);
    let key = |line: &String| line.split_once('=').is_some_and(|(key, _)| key.trim() == "refresh");
    let line = (start..end).find(|&i| key(&lines[i])).ok_or_else(|| format!("source `{id}` has no `refresh`"))?;
    lines[line] = format!("refresh = {}", toml::Value::try_from(refresh).map_err(|e| e.to_string())?);
    Ok(join(text, lines))
}

/// `lines` with the line end of `text`.
fn join(text: &str, lines: Vec<String>) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    lines.join(newline) + newline
}

/// The state of `source` at `today`, from its pin, the newest upstream version, its policy, its
/// licence and its credential. A pin is stale when upstream has a newer version, or when it is
/// before `base`, the pin of the source that `fetch.from` names.
pub fn status(
    source: &Source,
    pin: Option<&str>,
    base: Option<&str>,
    upstream: &Upstream,
    today: i64,
    credential_present: bool,
) -> Status {
    let age_days = pin.filter(|_| source.version == VersionScheme::Date).and_then(date::parse).map(|day| today - day);
    let (state, reason) = if source.kind != Kind::Tool && source.licence.is_none() {
        (State::Blocked, Some("no licence recorded".to_string()))
    } else if let Some(credential) = source.credential.as_ref().filter(|_| !credential_present) {
        (State::Blocked, Some(format!("credential missing: {}", credential.describe())))
    } else if let Some(base) = base.filter(|&base| pin.is_some_and(|pin| pin < base)) {
        let from = source.fetch.from.as_deref().unwrap_or_default();
        (State::Stale, Some(format!("before the `{from}` pin {base}")))
    } else {
        match (source.refresh, age_days) {
            (Refresh::Days(max), Some(age)) if age > i64::from(max) => match upstream {
                Upstream::Newest(newest) if Some(newest.as_str()) > pin => {
                    (State::Stale, Some(format!("{age} d > {max} d, upstream {newest}")))
                }
                Upstream::Newest(_) => (State::Ok, None),
                Upstream::CannotCheck => {
                    (State::Ok, Some(format!("{age} d > {max} d, upstream unknown: it cannot be checked")))
                }
                Upstream::Failed(_) => {
                    (State::Ok, Some(format!("{age} d > {max} d, upstream unknown: the check failed")))
                }
            },
            _ => (State::Ok, None),
        }
    };
    Status { state, reason, age_days }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSM: &str = r#"
        [[source]]
        id = "osm"
        kind = "data"
        licence = "ODbL-1.0"
        fetch = { kind = "http", url = "https://planet.openstreetmap.org/pbf/planet-{yymmdd}.osm.pbf" }
        version = "date"
        refresh = 7
        redistribute = true
    "#;

    fn osm() -> Source {
        parse_sources(OSM).unwrap().remove(0)
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        let err = parse_sources(&format!("{OSM}licence_text = \"x\"\n")).unwrap_err();
        assert!(err.contains("licence_text"), "{err}");
    }

    #[test]
    fn a_refresh_in_days_needs_a_date_version_and_a_known_policy() {
        let release = OSM.replace("version = \"date\"", "version = \"release\"");
        assert!(parse_sources(&release).unwrap_err().contains("needs `version = \"date\"`"));
        assert!(parse_sources(&OSM.replace("refresh = 7", "refresh = 14")).is_err());
        assert!(parse_sources(&OSM.replace("refresh = 7", "refresh = \"manual\"")).is_ok());
    }

    #[test]
    fn a_licence_is_an_spdx_expression() {
        for good in [
            "MIT",
            "OFL-1.1 AND MIT",
            "(MIT OR Apache-2.0) AND LicenseRef-x",
            "GPL-2.0-only WITH Classpath-exception-2.0",
        ] {
            assert!(is_licence_expression(good), "{good}");
        }
        for bad in ["", "Open data", "MIT AND", "(MIT", "MIT)", "MIT WITH", "AND MIT", "MIT OR OR GPL-3.0-only"] {
            assert!(!is_licence_expression(bad), "{bad}");
        }
    }

    #[test]
    fn an_r2_copy_needs_redistribution() {
        let text = OSM.replace("redistribute = true", "redistribute = false\nr2_copy = true");
        assert!(parse_sources(&text).unwrap_err().contains("r2_copy"));
    }

    #[test]
    fn a_pin_is_stale_when_older_than_its_policy_and_upstream_is_newer() {
        let today = date::parse("2024-01-10").unwrap();
        let newer = Upstream::Newest("2024-01-09".into());
        let fresh = status(&osm(), Some("2024-01-03"), None, &newer, today, true);
        assert_eq!((fresh.state, fresh.age_days), (State::Ok, Some(7)));
        let old = status(&osm(), Some("2024-01-02"), None, &newer, today, true);
        assert_eq!((old.state, old.reason.as_deref()), (State::Stale, Some("8 d > 7 d, upstream 2024-01-09")));
        let failed = status(&osm(), Some("2024-01-02"), None, &Upstream::Failed("offline".into()), today, true);
        assert_eq!(
            (failed.state, failed.reason.as_deref()),
            (State::Ok, Some("8 d > 7 d, upstream unknown: the check failed"))
        );
        let same = Upstream::Newest("2024-01-02".into());
        assert_eq!(status(&osm(), Some("2024-01-02"), None, &same, today, true).state, State::Ok);
        let manual = Source { refresh: Refresh::Manual, ..osm() };
        assert_eq!(status(&manual, Some("2020-01-01"), None, &newer, today, true).state, State::Ok);
        assert_eq!(status(&osm(), None, None, &newer, today, true).state, State::Ok);
        let replication = Source { fetch: Fetch { from: Some("osm-planet".into()), ..osm().fetch }, ..osm() };
        let behind = status(&replication, Some("2024-01-08"), Some("2024-01-09"), &same, today, true);
        assert_eq!(
            (behind.state, behind.reason.as_deref()),
            (State::Stale, Some("before the `osm-planet` pin 2024-01-09"))
        );
        assert_eq!(status(&replication, Some("2024-01-09"), Some("2024-01-09"), &same, today, true).state, State::Ok);
    }

    #[test]
    fn an_osm_fetch_names_the_source_of_its_base() {
        let diffs = |from: &str| {
            let fetch = format!("fetch = {{ kind = \"osm\", url = \"https://h/day/\"{from} }}");
            let diffs = OSM.replace("id = \"osm\"", "id = \"diffs\"");
            format!(
                "{OSM}{}",
                diffs
                    .lines()
                    .map(|l| if l.trim().starts_with("fetch") { fetch.as_str() } else { l })
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };
        assert!(parse_sources(&diffs(", from = \"osm\"")).is_ok());
        assert!(parse_sources(&diffs("")).unwrap_err().contains("fetch.from"));
        assert!(parse_sources(&diffs(", from = \"land\"")).unwrap_err().contains("names no source"));
        assert!(parse_sources(&OSM.replace(".pbf\"", ".pbf\", from = \"osm\"")).unwrap_err().contains("fetch.from"));
    }

    #[test]
    fn a_missing_licence_or_credential_blocks() {
        let today = date::parse("2024-01-10").unwrap();
        let unlicensed = Source { licence: None, ..osm() };
        assert_eq!(
            status(&unlicensed, Some("2024-01-09"), None, &Upstream::CannotCheck, today, true).state,
            State::Blocked
        );
        let tool = Source { kind: Kind::Tool, ..unlicensed };
        assert_eq!(status(&tool, None, None, &Upstream::CannotCheck, today, true).state, State::Ok);
        let keyed = Source { credential: Some(Credential { env: vec!["KEY".into()], file: None }), ..osm() };
        let blocked = status(&keyed, None, None, &Upstream::CannotCheck, today, false);
        assert_eq!((blocked.state, blocked.reason.as_deref()), (State::Blocked, Some("credential missing: KEY")));
    }

    #[test]
    fn a_pin_names_a_source_and_a_date_source_pins_a_date() {
        let sources = [osm()];
        assert!(parse_pins("[pins]\nosm = \"2024-01-01\"\n", &sources).is_ok());
        assert!(parse_pins("[pins]\nosm = \"latest\"\n", &sources).unwrap_err().contains("not a YYYY-MM-DD"));
        assert!(parse_pins("[pins]\nland = \"2024-01-01\"\n", &sources).unwrap_err().contains("names no source"));
    }

    #[test]
    fn a_pin_is_replaced_or_added_to_the_pins_table() {
        let text = "# live\n[pins]\nosm = \"2024-01-01\"\nland = \"2024-01-01\"\n\n[other]\nx = 1\n";
        let replaced = set_pin(text, "osm", "2024-02-01");
        assert_eq!(replaced, text.replace("osm = \"2024-01-01\"", "osm = \"2024-02-01\""));
        let added = set_pin(text, "qrank", "2024-02-01");
        assert!(added.contains("land = \"2024-01-01\"\nqrank = \"2024-02-01\"\n\n[other]"), "{added}");
        assert_eq!(set_pin("# empty\n", "osm", "2024-02-01"), "# empty\n\n[pins]\nosm = \"2024-02-01\"\n");
        let windows = "[pins] # live\r\nosm = \"2024-01-01\"\r\n";
        assert_eq!(set_pin(windows, "osm", "2024-02-01"), "[pins] # live\r\nosm = \"2024-02-01\"\r\n");
    }

    #[test]
    fn a_policy_is_replaced_in_its_source_only() {
        let land = OSM.replace("\"osm\"", "\"land\"");
        let text = format!("# sources\n{OSM}{land}");
        let edited = set_refresh(&text, "land", Refresh::Manual).unwrap();
        let manual = land.replace("        refresh = 7", "refresh = \"manual\"");
        assert_eq!(edited, format!("# sources\n{OSM}{manual}\n"));
        assert_eq!(parse_sources(&edited).unwrap()[1].refresh, Refresh::Manual);
        assert_eq!(set_refresh(&text, "qrank", Refresh::Manual).unwrap_err(), "no source `qrank`");
        assert_eq!("30".parse(), Ok(Refresh::Days(30)));
        assert!("14".parse::<Refresh>().is_err());
    }

    #[test]
    fn the_checked_in_registry_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let registry = Registry::load(&root).unwrap();
        assert!(registry.sources.iter().any(|s| s.id == "osm-planet"));
        assert_eq!(embedded("osm-planet"), registry.sources.iter().find(|s| s.id == "osm-planet").unwrap());
    }
}
