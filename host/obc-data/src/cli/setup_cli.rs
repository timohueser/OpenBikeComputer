//! `obc data setup`: what this machine can do, the tools and credentials that it has, and the
//! store. A missing tool or credential is part of the output, not an error.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use schemars::JsonSchema;
use serde::Serialize;

use super::{print_json, print_table, Code, Error};
use crate::r2::{Bucket, Credentials};
use crate::sources::Registry;
use crate::store::Store;

/// What `setup` writes.
#[derive(Serialize, JsonSchema)]
pub struct Setup {
    pub store: StoreUse,
    /// What this machine can do, and the reason when it cannot.
    pub jobs: Vec<Job>,
    pub tools: Vec<Tool>,
    /// Only whether each credential is set; never its value.
    pub credentials: Vec<Credential>,
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
pub struct StoreUse {
    pub path: PathBuf,
    /// The size of every file in the store.
    pub used_bytes: u64,
    /// `null` where the system does not tell.
    pub free_bytes: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
pub struct Job {
    pub name: String,
    pub state: Readiness,
    pub reason: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Tool {
    pub name: String,
    pub needed_for: String,
    pub state: Readiness,
    /// The version that the tool gives, or the command that installs it.
    pub detail: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Credential {
    pub name: String,
    pub needed_for: String,
    pub state: Readiness,
}

/// A tool: its name, what needs it, the command that gives its version, and its install command
/// on macOS and on Linux.
struct Spec {
    name: &'static str,
    needed_for: &'static str,
    version: &'static [&'static str],
    install: [&'static str; 2],
}

const TOOLS: [Spec; 8] = [
    Spec {
        name: "Rust",
        needed_for: "every Rust baker",
        version: &["cargo", "--version"],
        install: ["https://rustup.rs"; 2],
    },
    Spec {
        name: "C compiler",
        needed_for: "native bakers",
        version: &["cc", "--version"],
        install: ["xcode-select --install", "sudo apt install build-essential"],
    },
    Spec {
        name: "osmium-tool",
        needed_for: "cutting OSM into cells",
        version: &["osmium", "--version"],
        install: ["brew install osmium-tool", "sudo apt install osmium-tool"],
    },
    Spec {
        name: "GEOS",
        needed_for: "planner layers",
        version: &["geos-config", "--version"],
        install: ["obc doctor --install"; 2],
    },
    Spec {
        name: "uv",
        needed_for: "Python bakers",
        version: &["uv", "--version"],
        install: ["curl -LsSf https://astral.sh/uv/install.sh | sh"; 2],
    },
    Spec {
        name: "Java",
        needed_for: "planner/basemap",
        version: &["java", "--version"],
        install: ["brew install openjdk@21", "sudo apt install openjdk-21-jre-headless"],
    },
    Spec {
        name: "Node",
        needed_for: "this page, the web apps",
        version: &["node", "--version"],
        install: ["brew install node", "install Node 22 or newer with the package manager"],
    },
    Spec {
        name: "rclone",
        needed_for: "publish to prod",
        version: &["rclone", "version"],
        install: ["brew install rclone", "sudo apt install rclone"],
    },
];

/// The jobs and the tools that each needs. A credential that a job needs is in [`setup`].
const DEVICE: [&str; 3] = ["Rust", "C compiler", "osmium-tool"];
const PLANNER: [&str; 6] = ["Rust", "C compiler", "osmium-tool", "GEOS", "uv", "Java"];

pub fn setup(root: &Path, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let free_bytes = crate::store::free_bytes(store.root())?.map(|(_, free)| free);
    let store = StoreUse { path: store.root().to_path_buf(), used_bytes: used(store.root()), free_bytes };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let tools = tools(&std::env::var_os("PATH").unwrap_or_default(), home.as_deref());
    let r2 = Bucket::from_env(Credentials::Main).is_ok();
    let mut credentials = vec![Credential {
        name: "R2 key (OBC_R2_*)".into(),
        needed_for: "publish to prod".into(),
        state: if r2 { Readiness::Ready } else { Readiness::Missing },
    }];
    let registry = Registry::load(root).map_err(|e| Code::InvalidData.error(e))?;
    let mut accounts: Vec<(String, Vec<&str>, bool)> = Vec::new();
    for source in &registry.sources {
        let Some(credential) = &source.credential else { continue };
        let name = credential.describe();
        match accounts.iter_mut().find(|(known, ..)| *known == name) {
            Some((_, sources, _)) => sources.push(&source.id),
            None => accounts.push((name, vec![&source.id], credential.present())),
        }
    }
    credentials.extend(accounts.iter().map(|(name, sources, present)| Credential {
        name: name.clone(),
        needed_for: format!("fetch {}", sources.join(", ")),
        state: if *present { Readiness::Ready } else { Readiness::Optional },
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
    let unset = accounts.iter().filter(|(.., present)| !present).count();
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
            let version = found.and_then(|program| version(&program, &spec.version[1..]));
            Tool {
                name: spec.name.into(),
                needed_for: spec.needed_for.into(),
                state: if version.is_some() { Readiness::Ready } else { Readiness::Missing },
                detail: version.unwrap_or_else(|| spec.install[usize::from(!mac)].into()),
            }
        })
        .collect()
}

/// The first line that `program ARGS` writes on standard output, or else on standard error; `None`
/// when it does not run or fails.
fn version(program: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok().filter(|output| output.status.success())?;
    let first = |bytes: &[u8]| {
        String::from_utf8_lossy(bytes).lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
    };
    Some(first(&output.stdout).or_else(|| first(&output.stderr)).unwrap_or_else(|| "present".into()))
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
    fn a_tool_missing_from_path_has_its_install_command() {
        use std::os::unix::fs::PermissionsExt;
        let bin = Scratch::new("setup-path");
        let osmium = bin.0.join("osmium");
        std::fs::write(&osmium, "#!/bin/sh\necho 'osmium version 1.17.0'\n").unwrap();
        std::fs::set_permissions(&osmium, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = tools(bin.0.as_os_str(), None);
        let tool = |name: &str| tools.iter().find(|tool| tool.name == name).unwrap();
        assert_eq!(
            (tool("osmium-tool").state, tool("osmium-tool").detail.as_str()),
            (Readiness::Ready, "osmium version 1.17.0")
        );
        let java = if cfg!(target_os = "macos") {
            "brew install openjdk@21"
        } else {
            "sudo apt install openjdk-21-jre-headless"
        };
        assert_eq!((tool("Java").state, tool("Java").detail.as_str()), (Readiness::Missing, java));
        assert_eq!((tool("GEOS").state, tool("GEOS").detail.as_str()), (Readiness::Missing, "obc doctor --install"));
    }
}
