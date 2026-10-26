//! `obc data setup`: what this machine can do, the tools and credentials that it has, and the
//! store. A missing tool or credential is part of the output, not an error.
//! `obc data source credential ID`: set the credential of a source from standard input.

use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use schemars::JsonSchema;
use serde::Serialize;

use super::{find, print_json, print_table, registry, Code, Error};
use crate::credentials;
use crate::r2::{Bucket, Credentials};
use crate::sources::{Credential, Registry, Source};
use crate::store::Store;

/// What `setup` writes.
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub store: StoreUse,
    /// What this machine can do, and the reason when it cannot.
    pub jobs: Vec<Job>,
    pub tools: Vec<Tool>,
    /// Only whether each credential is set; never its value.
    pub credentials: Vec<CredentialState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Missing,
    /// Missing, but no main job needs it.
    Optional,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoreUse {
    pub path: PathBuf,
    /// The size of every file in the store.
    pub used_bytes: u64,
    /// `null` where the system does not tell.
    pub free_bytes: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub name: String,
    pub state: Readiness,
    pub reason: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub needed_for: String,
    pub state: Readiness,
    /// The version that the tool gives, or the command that installs it; both when the version is
    /// too old.
    pub detail: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialState {
    pub name: String,
    pub needed_for: String,
    pub state: Readiness,
    /// The arguments after `obc data` of the command that sets it from standard input, such as
    /// `source credential SOURCE` for each source that needs it. `null` for the R2 key, which the
    /// page does not set.
    pub set: Option<Vec<String>>,
    /// The values that the command reads, one per line, in this order.
    pub fields: Vec<String>,
    /// The `use` of each source that it serves.
    pub uses: Vec<String>,
    /// Where a person gets it.
    pub register: Option<String>,
    /// What to copy there.
    pub hint: Option<String>,
}

/// What `source credential` wrote. It never holds a value.
#[derive(Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialSet {
    /// The file that holds the values now.
    pub file: PathBuf,
    /// The names of the values that it wrote.
    pub names: Vec<String>,
    /// The sources that the credential serves; none for a credential of `deploy`.
    pub sources: Vec<String>,
    /// The names that the environment of the command sets, for example from `tools/obc.local`:
    /// the environment wins over the file, so the new value serves only once they are removed
    /// there.
    pub overridden: Vec<String>,
}

/// A tool: its name, what needs it, the command that gives its version, the lowest major version
/// that serves, and its install command on macOS and on Linux.
struct Spec {
    name: &'static str,
    needed_for: &'static str,
    version: &'static [&'static str],
    minimum: Option<u32>,
    install: [&'static str; 2],
}

const TOOLS: [Spec; 9] = [
    Spec {
        name: "Rust",
        needed_for: "every Rust baker",
        version: &["cargo", "--version"],
        minimum: None,
        install: ["https://rustup.rs"; 2],
    },
    Spec {
        name: "C compiler",
        needed_for: "native bakers",
        version: &["cc", "--version"],
        minimum: None,
        install: ["xcode-select --install", "sudo apt install build-essential"],
    },
    Spec {
        name: "osmium-tool",
        needed_for: "cutting OSM into cells",
        version: &["osmium", "--version"],
        minimum: None,
        install: ["brew install osmium-tool", "sudo apt install osmium-tool"],
    },
    Spec {
        name: "GEOS",
        needed_for: "planner layers",
        version: &["geos-config", "--version"],
        minimum: None,
        install: ["obc doctor --install"; 2],
    },
    Spec {
        name: "uv",
        needed_for: "Python bakers",
        version: &["uv", "--version"],
        minimum: None,
        install: ["curl -LsSf https://astral.sh/uv/install.sh | sh"; 2],
    },
    Spec {
        name: "Java 21+",
        needed_for: "planner/basemap",
        version: &["java", "--version"],
        minimum: Some(21),
        install: ["brew install openjdk@21", "sudo apt install openjdk-21-jdk-headless"],
    },
    Spec {
        name: "Maven",
        needed_for: "building the planner/basemap tool",
        version: &["mvn", "--version"],
        minimum: None,
        install: ["brew install maven", "sudo apt install maven"],
    },
    Spec {
        name: "Node 22+",
        needed_for: "this page, the web apps",
        version: &["node", "--version"],
        minimum: Some(22),
        install: ["brew install node", "install Node 22 or newer with the package manager"],
    },
    Spec {
        name: "rclone",
        needed_for: "publish to prod",
        version: &["rclone", "version"],
        minimum: None,
        install: ["brew install rclone", "sudo apt install rclone"],
    },
];

/// The jobs and the tools that each needs. A credential that a job needs is in [`setup`].
const DEVICE: [&str; 3] = ["Rust", "C compiler", "osmium-tool"];
const PLANNER: [&str; 7] = ["Rust", "C compiler", "osmium-tool", "GEOS", "uv", "Java 21+", "Maven"];

/// The tools that the planner steps need and this machine lacks.
pub(super) fn planner_missing() -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let tools = tools(&std::env::var_os("PATH").unwrap_or_default(), home.as_deref());
    tools
        .into_iter()
        .filter(|tool| PLANNER.contains(&tool.name.as_str()) && tool.state != Readiness::Ready)
        .map(|tool| tool.name)
        .collect()
}

pub fn setup(root: &Path, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let free_bytes = crate::store::free_bytes(store.root())?.map(|(_, free)| free);
    let store = StoreUse { path: store.root().to_path_buf(), used_bytes: used(store.root()), free_bytes };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let tools = tools(&std::env::var_os("PATH").unwrap_or_default(), home.as_deref());
    let r2 = Bucket::from_env(Credentials::Main).is_ok();
    let mut credentials = vec![CredentialState {
        name: "R2 key (OBC_R2_*)".into(),
        needed_for: "publish to prod".into(),
        state: if r2 { Readiness::Ready } else { Readiness::Missing },
        set: None,
        fields: Vec::new(),
        uses: Vec::new(),
        register: None,
        hint: None,
    }];
    let registry = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let mut accounts: Vec<(&Credential, Vec<&Source>)> = Vec::new();
    for source in &registry.sources {
        let Some(credential) = &source.credential else { continue };
        match accounts.iter_mut().find(|(known, _)| known.describe() == credential.describe()) {
            Some((_, sources)) => sources.push(source),
            None => accounts.push((credential, vec![source])),
        }
    }
    credentials.extend(accounts.iter().map(|(credential, sources)| CredentialState {
        name: credential.describe(),
        needed_for: format!("fetch {}", sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>().join(", ")),
        state: if credential.present() { Readiness::Ready } else { Readiness::Optional },
        set: Some(["source", "credential", &sources[0].id].map(String::from).to_vec()),
        fields: credentials::place(credential).map(|place| place.names).unwrap_or_default(),
        uses: sources.iter().filter_map(|source| source.r#use.clone()).collect(),
        register: credential.register.clone(),
        hint: credential.hint.clone(),
    }));
    let missing = |names: &[&str]| -> Vec<String> {
        tools
            .iter()
            .filter(|tool| names.contains(&tool.name.as_str()) && tool.state != Readiness::Ready)
            .map(|tool| tool.name.clone())
            .collect()
    };
    let bake = |name: &str, needs: &[&str]| match missing(needs).as_slice() {
        [] => Job { name: name.into(), state: Readiness::Ready, reason: "All tools are present.".into() },
        missing => Job {
            name: name.into(),
            state: Readiness::Missing,
            reason: format!(
                "Needs {}. Install {}, then check again.",
                list(missing),
                if missing.len() == 1 { "it" } else { "them" }
            ),
        },
    };
    let mut publish = missing(&["rclone"]);
    if !r2 {
        publish.push("an R2 key".into());
    }
    let publish = Job {
        name: "Publish to prod".into(),
        state: if publish.is_empty() { Readiness::Ready } else { Readiness::Missing },
        reason: match publish.is_empty() {
            true => "rclone and the R2 key are present.".into(),
            false if r2 => format!("Needs {}. Install it, then check again.", list(&publish)),
            false => format!("Needs {}. Ask the owner for the key, then put it in tools/obc.local.", list(&publish)),
        },
    };
    let unset = accounts.iter().filter(|(credential, _)| !credential.present()).count();
    let fetch = Job {
        name: "Fetch sources that need an account".into(),
        state: if unset == 0 { Readiness::Ready } else { Readiness::Optional },
        reason: match unset {
            0 => "Every credential is set.".into(),
            _ => format!(
                "Optional: {unset} of {} credentials are not set. Credentials names the sources of each.",
                accounts.len()
            ),
        },
    };
    credentials.extend(super::deploy_cli::credentials());
    let jobs =
        vec![bake("Bake device layers here", &DEVICE), bake("Bake planner layers here", &PLANNER), publish, fetch];
    let setup = Setup { store, jobs, tools, credentials };
    if json {
        return print_json(&setup);
    }
    let mark = |state: Readiness| match state {
        Readiness::Ready => "✓",
        Readiness::Missing => "✗",
        Readiness::Optional => "○",
    };
    println!("WHAT THIS MACHINE CAN DO");
    print_table(
        &setup
            .jobs
            .iter()
            .map(|job| vec![format!("  {} {}", mark(job.state), job.name), job.reason.clone()])
            .collect::<Vec<_>>(),
    );
    println!("TOOLS");
    let tools = setup.tools.iter().map(|tool| {
        vec![format!("  {} {}", mark(tool.state), tool.name), tool.needed_for.clone(), tool.detail.clone()]
    });
    print_table(&tools.collect::<Vec<_>>());
    println!("CREDENTIALS (only whether each is set)");
    print_table(
        &setup
            .credentials
            .iter()
            .map(|c| vec![format!("  {} {}", mark(c.state), c.name), c.needed_for.clone()])
            .collect::<Vec<_>>(),
    );
    let free = setup
        .store
        .free_bytes
        .map_or_else(|| "free space unknown".into(), |free| format!("{} free", super::bytes(free)));
    println!("STORE {} · {} used · {free}", setup.store.path.display(), super::bytes(setup.store.used_bytes));
    Ok(())
}

/// Read the values of the credential of source `id` on standard input, one line each, and write
/// them where [`credentials::place`] says.
pub fn credential(root: &Path, id: &str, json: bool) -> Result<(), Error> {
    let registry = registry(root)?;
    let credential = find(&registry, id)?
        .credential
        .as_ref()
        .ok_or_else(|| Code::Usage.error(format!("source `{id}` needs no credential")))?;
    let place = credentials::place(credential).ok_or_else(|| Code::InvalidData.error("no writer knows the file"))?;
    let shares = |source: &&Source| source.credential.as_ref().is_some_and(|c| c.describe() == credential.describe());
    let sources = registry.sources.iter().filter(shares).map(|s| s.id.clone()).collect();
    let set = write(place, credential.file.is_none(), sources)?;
    print_set(&set, json)
}

/// Read the values of `place` on standard input, one line each, and write them; `env` for the
/// credentials file, whose names the environment can also set.
pub(super) fn write(place: credentials::Place, env: bool, sources: Vec<String>) -> Result<CredentialSet, Error> {
    let home = std::env::var_os("HOME").ok_or_else(|| Code::Failed.error("HOME is not set"))?;
    let home = Path::new(&home);
    let terminal = std::io::stdin().is_terminal();
    let mut values = Vec::new();
    for name in &place.names {
        if terminal {
            eprint!("{name}: ");
        }
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        values.push(line.trim().to_string());
    }
    let before = credentials::env_file(home);
    let file = credentials::write(home, &place, &values).map_err(|e| Code::Usage.error(e))?;
    let changed =
        |name: &&String| std::env::var(name).is_ok_and(|now| !now.is_empty() && before.get(*name) != Some(&now));
    let overridden = if env { place.names.iter().filter(changed).cloned().collect() } else { Vec::new() };
    Ok(CredentialSet { file, names: place.names, sources, overridden })
}

pub(super) fn print_set(set: &CredentialSet, json: bool) -> Result<(), Error> {
    if json {
        return print_json(set);
    }
    let serves = if set.sources.is_empty() { String::new() } else { format!(" for {}", list(&set.sources)) };
    println!("Wrote {} to {}{serves}.", list(&set.names), set.file.display());
    if !set.overridden.is_empty() {
        eprintln!("The environment sets {}, and it wins: remove it from tools/obc.local.", list(&set.overridden));
    }
    Ok(())
}

/// Each tool: found on `path`, or for GEOS in `~/geos-install/bin`, where `obc doctor --install`
/// builds it.
fn tools(path: &OsStr, home: Option<&Path>) -> Vec<Tool> {
    let mac = cfg!(target_os = "macos");
    TOOLS
        .iter()
        .map(|spec| {
            let mut dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
            if spec.name == "GEOS" {
                dirs.extend(home.map(|home| home.join("geos-install/bin")));
            }
            let found = dirs.iter().map(|dir| dir.join(spec.version[0])).find(|file| file.is_file());
            let install = spec.install[usize::from(!mac)];
            let (state, detail) = match found.and_then(|program| version(&program, &spec.version[1..])) {
                None => (Readiness::Missing, install.to_string()),
                Some(version) => match spec
                    .minimum
                    .filter(|&minimum| major(&version).is_some_and(|major| major < minimum))
                {
                    Some(minimum) => (Readiness::Missing, format!("{version}; needs {minimum} or newer: {install}")),
                    None => (Readiness::Ready, version),
                },
            };
            Tool { name: spec.name.into(), needed_for: spec.needed_for.into(), state, detail }
        })
        .collect()
}

/// The first line that `program ARGS` writes on standard output, or else on standard error; `None`
/// when it does not run or fails. It runs outside the repository, so that rustup does not install
/// the toolchain of `rust-toolchain.toml` for a version check.
fn version(program: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).current_dir(std::env::temp_dir()).output();
    let output = output.ok().filter(|output| output.status.success())?;
    let first = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes).lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
    };
    Some(first(&output.stdout).or_else(|| first(&output.stderr)).unwrap_or_else(|| "present".into()))
}

/// The first number in a version line: `21` of `openjdk 21.0.5`, `22` of `v22.1.0`.
fn major(version: &str) -> Option<u32> {
    let digits: String = version.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The size of the files under `dir`; links are not followed.
fn used(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => used(&entry.path()),
            Ok(meta) if meta.is_file() => meta.len(),
            _ => 0,
        })
        .sum()
}

/// `a`, `a and b`, `a, b and c`.
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[cfg(unix)]
    #[test]
    fn a_tool_missing_from_path_or_too_old_has_its_install_command() {
        use std::os::unix::fs::PermissionsExt;
        let bin = Scratch::new("setup-path");
        for (program, line) in
            [("osmium", "osmium version 1.17.0"), ("java", "openjdk 17.0.2 2022-01-18"), ("node", "v22.1.0")]
        {
            let file = bin.0.join(program);
            std::fs::write(&file, format!("#!/bin/sh\necho '{line}'\n")).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tools = tools(bin.0.as_os_str(), None);
        let tool = |name: &str| {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            (tool.state, tool.detail.as_str())
        };
        let java = if cfg!(target_os = "macos") {
            "brew install openjdk@21"
        } else {
            "sudo apt install openjdk-21-jdk-headless"
        };
        assert_eq!(tool("osmium-tool"), (Readiness::Ready, "osmium version 1.17.0"));
        assert_eq!(tool("Node 22+"), (Readiness::Ready, "v22.1.0"));
        assert_eq!(
            tool("Java 21+"),
            (Readiness::Missing, format!("openjdk 17.0.2 2022-01-18; needs 21 or newer: {java}").as_str())
        );
        assert_eq!(tool("GEOS"), (Readiness::Missing, "obc doctor --install"));
        let maven = if cfg!(target_os = "macos") { "brew install maven" } else { "sudo apt install maven" };
        assert_eq!(tool("Maven"), (Readiness::Missing, maven));
    }
}
