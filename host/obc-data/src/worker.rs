//! The checkout boundary of a freshly built producer worker. Library callers have no worker.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::engine::code;

/// Private launcher return codes preserve one of the five terminal screens.
pub const RELOAD_EXIT: u8 = 75;
pub const SCREEN: &str = "OBC_DATA_TUI_SCREEN";

pub fn can_reload() -> bool {
    BINDING.get().is_some()
}

pub const ROOT: &str = "OBC_DATA_WORKER_ROOT";
pub const CODE: &str = "OBC_DATA_WORKER_CODE";
pub const EXE: &str = "OBC_DATA_WORKER_EXE";
pub const COMPILED_ROOT: &str = "OBC_DATA_COMPILED_ROOT";
pub const COMPILED_CODE: &str = "OBC_DATA_COMPILED_CODE";
const PRODUCERS: &[&str] = &["obc-data-steps"];
const RESTART: &str = "Rust producer code changed; quit and restart obc data to build a fresh worker";

struct Binding {
    root: PathBuf,
    code: String,
}

static BINDING: OnceLock<Binding> = OnceLock::new();

/// Cargo runtime state does not select compiler libraries or dependency fingerprints.
pub fn compiler_command(command: &mut std::process::Command) -> &mut std::process::Command {
    for name in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "DYLD_FALLBACK_LIBRARY_PATH"] {
        command.env_remove(name);
    }
    let metadata: Vec<_> = std::env::vars_os()
        .map(|(name, _)| name)
        .chain(command.get_envs().map(|(name, _)| name.to_owned()))
        .filter(|name| {
            name.to_str().is_some_and(|name| name.starts_with("CARGO_PKG_") || name.starts_with("CARGO_MANIFEST_"))
        })
        .collect();
    for name in metadata {
        command.env_remove(name);
    }
    command
}

pub fn fingerprint(root: &Path) -> Result<String, String> {
    code::compiled(root, &PRODUCERS.iter().map(|name| (*name).into()).collect::<Vec<_>>())
}

/// Require the launcher's checkout and immutable executable before reading any CLI command.
pub fn enter(compiled_root: Option<&str>, compiled_code: Option<&str>) -> Result<(), String> {
    let required = |key| std::env::var(key).map_err(|_| "start this worker through obc data".to_string());
    let root = PathBuf::from(required(ROOT)?).canonicalize().map_err(|e| e.to_string())?;
    let binding = Binding { root, code: required(CODE)? };
    stamp(&binding, compiled_root, compiled_code)?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    if crate::store::hash_file(&executable)?.0 != required(EXE)? {
        return Err("the producer executable differs from the launched worker; restart obc data".into());
    }
    verify(&binding, &binding.root)?;
    BINDING.set(binding).map_err(|_| "the producer worker is already bound".to_string())
}

fn stamp(binding: &Binding, root: Option<&str>, code: Option<&str>) -> Result<(), String> {
    if root.map(Path::new) != Some(binding.root.as_path()) || code != Some(binding.code.as_str()) {
        return Err("the compiled producer belongs to another checkout or code version; restart obc data".into());
    }
    Ok(())
}

/// Reject actions in a long-lived worker after a persistent checkout change.
pub fn check(root: &Path) -> Result<(), String> {
    BINDING.get().map_or(Ok(()), |binding| verify(binding, root))
}

/// A detached request needs an actual launcher binding, not an ambient environment value.
pub fn bound_code(root: &Path) -> Result<String, String> {
    let binding = BINDING.get().ok_or("start detached operations through the fresh producer launcher")?;
    verify(binding, root)?;
    Ok(binding.code.clone())
}

fn verify(binding: &Binding, root: &Path) -> Result<(), String> {
    if root.canonicalize().map_err(|e| e.to_string())? != binding.root || fingerprint(root)? != binding.code {
        return Err(RESTART.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{repository, write};
    use crate::store::tests::Scratch;

    #[test]
    fn the_worker_binds_its_engine_and_checkout_but_not_source_refresh_controls() {
        let scratch = Scratch::new("worker-binding");
        repository(
            &scratch.0,
            &[("obc-data-steps", "[dependencies]\nobc-data={path=\"../obc-data\"}\n"), ("obc-data", "")],
        );
        let sources = "[[source]]\nid=\"land\"\nkind=\"data\"\nlicence=\"CC0-1.0\"\nattribution=\"Land\"\nfetch={kind=\"http\",url=\"https://example.org/land\"}\nversion=\"date\"\nrefresh=7\nredistribute=true\n";
        write(&scratch.0.join("data/sources.toml"), sources);
        write(
            &scratch.0.join("obc-data/src/lib.rs"),
            "pub const SOURCES: &str = include_str!(\"../../data/sources.toml\");\n",
        );
        let binding = Binding { root: scratch.0.canonicalize().unwrap(), code: fingerprint(&scratch.0).unwrap() };
        verify(&binding, &scratch.0).unwrap();
        let credential = format!("{sources}credential={{env=[\"NEW_CREDENTIAL\"]}}\n");
        write(&scratch.0.join("data/sources.toml"), &credential);
        assert!(verify(&binding, &scratch.0).unwrap_err().contains("quit and restart"));
        write(
            &scratch.0.join("data/sources.toml"),
            &sources.replace("refresh=7", "refresh=30").replace("redistribute=true", "redistribute=false"),
        );
        verify(&binding, &scratch.0).unwrap();
        write(
            &scratch.0.join("data/sources.toml"),
            &sources.replace("attribution=\"Land\"", "attribution=\"New credit\""),
        );
        assert!(verify(&binding, &scratch.0).unwrap_err().contains("quit and restart"));
        write(&scratch.0.join("data/sources.toml"), sources);
        let manifest = scratch.0.join("Cargo.toml");
        let original = std::fs::read_to_string(&manifest).unwrap();
        write(&manifest, &(original.clone() + "\n[profile.dev.package.obc-data-steps]\nopt-level=2\n"));
        assert!(verify(&binding, &scratch.0).unwrap_err().contains("quit and restart"));
        write(&manifest, &original);
        write(&scratch.0.join(".cargo/config.toml"), "[build]\njobs=2\ntarget-dir='/tmp/build-cache'\n");
        verify(&binding, &scratch.0).unwrap();
        write(&scratch.0.join(".cargo/config.toml"), "[build]\njobs=4\ntarget-dir='/tmp/other-cache'\n");
        verify(&binding, &scratch.0).unwrap();
        write(&scratch.0.join(".cargo/config.toml"), "[build]\nrustc-wrapper='unsupported'\n");
        assert!(verify(&binding, &scratch.0).unwrap_err().contains("Cargo config [build]"));
        std::fs::remove_file(scratch.0.join(".cargo/config.toml")).unwrap();
        write(&scratch.0.join("obc-data/src/lib.rs"), "pub fn new_steps() {}\n");
        assert!(verify(&binding, &scratch.0).unwrap_err().contains("quit and restart"));
        assert!(verify(&binding, scratch.0.parent().unwrap()).is_err());
    }

    #[test]
    fn an_overwritten_artifact_or_unstamped_worker_cannot_use_the_requested_binding() {
        let binding = Binding { root: PathBuf::from("checkout-a"), code: "code-a".into() };
        stamp(&binding, Some("checkout-a"), Some("code-a")).unwrap();
        for (root, code) in [(Some("checkout-b"), Some("code-a")), (Some("checkout-a"), Some("code-b")), (None, None)] {
            assert!(stamp(&binding, root, code).unwrap_err().contains("another checkout or code version"));
        }
    }
}
