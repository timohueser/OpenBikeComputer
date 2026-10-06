//! Supported Cargo builds: selected profiles, tools and ordered compiler flags.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::engine::{Code, Profile, Rust};
use crate::store::{hash_file, sha256_hex};

#[derive(Default)]
pub(super) struct Context {
    native: Option<(PathBuf, Native)>,
    tools: BTreeMap<PathBuf, (String, String)>,
    used: BTreeMap<PathBuf, Option<String>>,
    #[cfg(test)]
    probes: usize,
}

struct Native {
    target: String,
    hashes: BTreeMap<String, String>,
    profiles: BTreeMap<String, String>,
    environment: BTreeMap<OsString, OsString>,
    inputs: BTreeMap<PathBuf, Option<String>>,
}

impl Context {
    pub fn preflight(&mut self, root: &Path, code: &Code) -> Result<Option<(String, String)>, String> {
        if !code.crates.is_empty() && !matches!(code.rust, Some(Rust::Prepared { .. })) {
            let native = self.native(root)?;
            return Ok(Some((native.target.clone(), digest(&native.hashes))));
        }
        Ok(None)
    }

    pub fn identity(
        &mut self,
        root: &Path,
        code: &Code,
        packages: &super::rust::Packages,
    ) -> Result<BTreeMap<String, String>, String> {
        let profile = match code.rust {
            None | Some(Rust::Native { profile: Profile::Dev }) => Profile::Dev,
            Some(Rust::Native { profile }) | Some(Rust::Prepared { profile }) => profile,
        };
        let mut hashes = BTreeMap::new();
        let target = if matches!(code.rust, Some(Rust::Prepared { .. })) {
            code.target.as_deref().ok_or("prepared Rust code requires an explicit target and builder options")?
        } else {
            let native = self.native(root)?;
            if code.target.as_ref().is_some_and(|target| target != &native.target) {
                return Err("native Rust code requires its compiler host target; use a prepared target builder".into());
            }
            hashes.extend(
                native
                    .hashes
                    .iter()
                    .filter(|(name, _)| {
                        packages.names.contains("cc") || !matches!(name.as_str(), "rust/cc" | "rust/cc-version")
                    })
                    .map(|(name, hash)| (name.clone(), hash.clone())),
            );
            hashes.insert("rust/profile-env".into(), digest(&profile_environment(profile, &native.profiles)));
            &native.target
        };
        hashes.insert("rust/target".into(), sha256_hex(target.as_bytes()));
        let text = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| format!("Cargo.toml: {e}"))?;
        hashes.insert("rust/profile".into(), digest(&profile_projection(&text, profile, packages)?));
        Ok(hashes)
    }

    fn native(&mut self, root: &Path) -> Result<&Native, String> {
        let env: BTreeMap<_, _> = std::env::vars_os().collect();
        validate_environment(&env)?;
        let environment = env
            .iter()
            .filter(|(name, _)| {
                let name = name.to_string_lossy();
                name.starts_with("CARGO_PROFILE_")
                    || matches!(
                        name.as_ref(),
                        "HOME"
                            | "PATH"
                            | "RUSTC"
                            | "CARGO"
                            | "CC"
                            | "CARGO_HOME"
                            | "RUSTUP_HOME"
                            | "RUSTUP_TOOLCHAIN"
                            | "DEVELOPER_DIR"
                            | "TOOLCHAINS"
                            | "RUSTFLAGS"
                            | "CARGO_ENCODED_RUSTFLAGS"
                            | "CARGO_INCREMENTAL"
                            | "LANG"
                            | "LC_ALL"
                            | "LC_CTYPE"
                            | "LC_MESSAGES"
                    )
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let unchanged = match &self.native {
            Some((loaded, native)) if loaded == root && native.environment == environment => native
                .inputs
                .iter()
                .map(|(path, previous)| stamp(path).map(|current| &current == previous))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .all(|same| same),
            _ => false,
        };
        if !unchanged {
            self.used.clear();
            let selection = selection_inputs(root, &env)?
                .into_iter()
                .map(|path| stamp(&path).map(|stamp| (path, stamp)))
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            let profiles = env
                .iter()
                .filter_map(|(name, value)| {
                    let name = name.to_str()?;
                    (name.starts_with("CARGO_PROFILE_") || name == "CARGO_INCREMENTAL").then(|| {
                        value
                            .to_str()
                            .map(|value| (name.to_string(), value.to_string()))
                            .ok_or_else(|| format!("{name} must be UTF-8"))
                    })
                })
                .collect::<Result<_, _>>()?;
            for path in configs(root, &env) {
                if path.is_file() {
                    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    validate_config(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                }
            }
            #[cfg(test)]
            {
                self.probes += 1;
            }
            let mut hashes = BTreeMap::new();
            let rustc = executable(root, env.get(OsStr::new("RUSTC")), "rustc")?;
            let cargo = executable(root, env.get(OsStr::new("CARGO")), "cargo")?;
            self.watch(&rustc)?;
            self.watch(&cargo)?;
            native_binary(&rustc)?;
            native_binary(&cargo)?;
            let version = output(root, &rustc, &["-vV"])?;
            let target = version.lines().find_map(|line| line.strip_prefix("host: ")).ok_or("rustc names no host")?;
            let sysroot = PathBuf::from(output(root, &rustc, &["--print", "sysroot"])?.trim());
            let installed = sysroot.join("bin/rustc").canonicalize().map_err(|e| format!("selected rustc: {e}"))?;
            if rustc != installed {
                return Err("RUSTC must select the actual sysroot compiler; remove the compiler wrapper".into());
            }
            // The rustc executable loads its compiler driver and LLVM from the selected toolchain.
            self.watch(&sysroot.join("lib"))?;
            let libraries = fs::read_dir(sysroot.join("lib")).map_err(|e| format!("compiler libraries: {e}"))?;
            for entry in libraries {
                let entry = entry.map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with("librustc_driver") || name.starts_with("libLLVM") {
                    hashes.insert(format!("rust/compiler-library/{name}"), self.tool_hash(&entry.path())?);
                }
            }
            let lld = sysroot.join("lib/rustlib").join(target).join("bin/rust-lld");
            if lld.is_file() {
                hashes.insert("rust/bundled-linker".into(), self.tool_hash(&lld)?);
            }
            self.watch(&lld)?;
            hashes.extend(self.library_hashes(&sysroot.join("lib/rustlib").join(target).join("lib"))?);
            for (name, path, version) in
                [("compiler", rustc, version.clone()), ("cargo", cargo.clone(), output(root, &cargo, &["-vV"])?)]
            {
                hashes.insert(format!("rust/{name}"), self.tool_hash(&path)?);
                hashes.insert(format!("rust/{name}-version"), sha256_hex(version.as_bytes()));
            }
            let cc = executable(root, env.get(OsStr::new("CC")), "cc")?;
            self.watch(&cc)?;
            native_binary(&cc)?;
            hashes.insert("rust/cc".into(), self.tool_hash(&cc)?);
            hashes.insert("rust/cc-version".into(), sha256_hex(output(root, &cc, &["--version"])?.as_bytes()));
            let link_driver = executable(root, None, "cc")?;
            self.watch(&link_driver)?;
            native_binary(&link_driver)?;
            hashes.insert("rust/link-driver".into(), self.tool_hash(&link_driver)?);
            hashes.insert(
                "rust/link-driver-version".into(),
                sha256_hex(output(root, &link_driver, &["--version"])?.as_bytes()),
            );
            let linker = output(root, &link_driver, &["-print-prog-name=ld"])?;
            let linker = locate(root, &OsString::from(linker.trim()))?;
            hashes.insert("rust/linker".into(), self.tool_hash(&linker)?);
            hashes.insert("rust/flags".into(), digest(&flags(&env)?));
            let mut inputs = selection;
            for (path, stamp) in &self.used {
                inputs.entry(path.clone()).or_insert_with(|| stamp.clone());
            }
            for (path, previous) in &inputs {
                if &stamp(path)? != previous {
                    return Err("Rust build tools changed during discovery; retry the plan".into());
                }
            }
            self.native =
                Some((root.to_path_buf(), Native { target: target.into(), hashes, profiles, environment, inputs }));
        }
        Ok(&self.native.as_ref().unwrap().1)
    }

    fn tool_hash(&mut self, path: &Path) -> Result<String, String> {
        self.watch(path)?;
        let fingerprint = stamp(path)?.ok_or_else(|| format!("{} is missing", path.display()))?;
        if self.tools.get(path).is_none_or(|(previous, _)| previous != &fingerprint) {
            let hash = hash_file(path)?.0;
            if stamp(path)? != Some(fingerprint.clone()) {
                return Err("Rust build tool changed while reading; retry the plan".into());
            }
            self.tools.insert(path.to_path_buf(), (fingerprint, hash));
        }
        Ok(self.tools[path].1.clone())
    }

    fn watch(&mut self, path: &Path) -> Result<(), String> {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.used.entry(path.to_path_buf()) {
            entry.insert(stamp(path)?);
        }
        Ok(())
    }

    fn library_hashes(&mut self, dir: &Path) -> Result<BTreeMap<String, String>, String> {
        self.watch(dir)?;
        let mut hashes = BTreeMap::new();
        for entry in fs::read_dir(dir).map_err(|e| format!("selected Rust sysroot libraries: {e}"))? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().is_file() {
                let name = entry.file_name().to_str().ok_or("Rust library name is not UTF-8")?.to_string();
                hashes.insert(format!("rust/sysroot-library/{name}"), self.tool_hash(&entry.path())?);
            }
        }
        Ok(hashes)
    }
}

fn stamp(path: &Path) -> Result<Option<String>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let stamp = format!("{}:{:?}", metadata.len(), metadata.modified().map_err(|e| e.to_string())?);
    #[cfg(unix)]
    let stamp = {
        use std::os::unix::fs::MetadataExt;
        format!("{stamp}:{}:{}:{}:{}", metadata.dev(), metadata.ino(), metadata.ctime(), metadata.ctime_nsec())
    };
    Ok(Some(stamp))
}

fn selection_inputs(root: &Path, env: &BTreeMap<OsString, OsString>) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for dir in root.ancestors() {
        paths.extend(
            ["rust-toolchain", "rust-toolchain.toml", ".cargo/config", ".cargo/config.toml"].map(|name| dir.join(name)),
        );
    }
    for (name, fallback, files) in [
        ("CARGO_HOME", ".cargo", ["config", "config.toml"]),
        ("RUSTUP_HOME", ".rustup", ["settings.toml", "toolchains"]),
    ] {
        let home = env
            .get(OsStr::new(name))
            .map(|path| root.join(path))
            .or_else(|| env.get(OsStr::new("HOME")).map(|path| PathBuf::from(path).join(fallback)));
        if let Some(home) = home {
            paths.extend(files.map(|file| home.join(file)));
        }
    }
    for (name, default) in [("RUSTC", "rustc"), ("CARGO", "cargo"), ("CC", "cc")] {
        paths.push(located(root, env.get(OsStr::new(name)).unwrap_or(&OsString::from(default)))?);
    }
    paths.push(located(root, &OsString::from("cc"))?);
    #[cfg(target_os = "macos")]
    paths.push("/var/db/xcode_select_link".into());
    Ok(paths)
}

fn digest(value: &impl serde::Serialize) -> String {
    let value = super::super::sorted(serde_json::to_value(value).expect("build identity is serializable"));
    sha256_hex(&serde_json::to_vec(&value).expect("build identity is serializable"))
}

fn profile_projection(
    text: &str,
    profile: Profile,
    packages: &super::rust::Packages,
) -> Result<serde_json::Value, String> {
    let manifest: toml::Value = toml::from_str(text).map_err(|e| format!("Cargo.toml: {e}"))?;
    let name = match profile {
        Profile::Dev => "dev",
        Profile::Release => "release",
    };
    let mut selected = manifest
        .get("profile")
        .and_then(|profiles| profiles.get(name))
        .cloned()
        .unwrap_or(toml::Value::Table(Default::default()));
    let table = selected.as_table_mut().ok_or("selected Cargo profile must be a table")?;
    if table.contains_key("inherits") {
        return Err("data builds support dev/release without profile inheritance; remove inherits".into());
    }
    if let Some(overrides) = table.get_mut("package") {
        let overrides = overrides.as_table_mut().ok_or("Cargo profile package must be a table")?;
        if overrides
            .keys()
            .any(|name| name != "*" && !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        {
            return Err(
                "data builds support profile overrides by package name or *; replace package-ID overrides".into()
            );
        }
        overrides.retain(|name, _| (name == "*" && packages.non_workspace) || packages.names.contains(name));
        if overrides.is_empty() {
            table.remove("package");
        }
    }
    Ok(serde_json::json!({ "profile": name, "settings": selected }))
}

fn validate_environment(env: &BTreeMap<OsString, OsString>) -> Result<(), String> {
    for (name, value) in env {
        let name = name.to_string_lossy();
        if value.is_empty() {
            continue;
        }
        let refused = (name.starts_with("CARGO_PROFILE_") && !profile_setting(&name))
            || (name.starts_with("CARGO_TARGET_") && name != "CARGO_TARGET_DIR" && name != "CARGO_TARGET_TMPDIR")
            || (name.starts_with("CARGO_BUILD_") && name != "CARGO_BUILD_JOBS" && name != "CARGO_BUILD_TARGET_DIR")
            || matches!(
                name.as_ref(),
                "RUSTC_WRAPPER"
                    | "RUSTC_WORKSPACE_WRAPPER"
                    | "RUSTC_BOOTSTRAP"
                    | "CFLAGS"
                    | "CXXFLAGS"
                    | "CPPFLAGS"
                    | "LDFLAGS"
                    | "CXX"
                    | "AR"
                    | "ARFLAGS"
                    | "SDKROOT"
                    | "MACOSX_DEPLOYMENT_TARGET"
                    | "LD_LIBRARY_PATH"
                    | "LD_PRELOAD"
                    | "DYLD_LIBRARY_PATH"
                    | "DYLD_INSERT_LIBRARIES"
            )
            || name.starts_with("CC_")
            || name.starts_with("CFLAGS_")
            || name.starts_with("CXXFLAGS_")
            || name.starts_with("AR_")
            || name.starts_with("CXX_")
            || name.starts_with("RANLIB");
        let native_override = name.strip_prefix("HOST_").or_else(|| name.strip_prefix("TARGET_")).is_some_and(|name| {
            matches!(
                name,
                "CC" | "CXX" | "AR" | "RANLIB" | "CFLAGS" | "CXXFLAGS" | "CPPFLAGS" | "ARFLAGS" | "RANLIBFLAGS"
            )
        });
        if refused || native_override {
            return Err(format!(
                "data build identity does not support {name}; unset it and configure the declared build"
            ));
        }
        if name == "CARGO_INCREMENTAL" && !matches!(value.to_str(), Some("0" | "1")) {
            return Err("CARGO_INCREMENTAL must be 0 or 1".into());
        }
    }
    Ok(())
}

fn profile_setting(name: &str) -> bool {
    let key = name.strip_prefix("CARGO_PROFILE_DEV_").or_else(|| name.strip_prefix("CARGO_PROFILE_RELEASE_"));
    key.is_some_and(|key| {
        matches!(
            key.strip_prefix("BUILD_OVERRIDE_").unwrap_or(key),
            "OPT_LEVEL"
                | "DEBUG"
                | "SPLIT_DEBUGINFO"
                | "DEBUG_ASSERTIONS"
                | "OVERFLOW_CHECKS"
                | "LTO"
                | "PANIC"
                | "CODEGEN_UNITS"
                | "RPATH"
                | "INCREMENTAL"
                | "STRIP"
        )
    })
}

fn profile_environment(profile: Profile, env: &BTreeMap<String, String>) -> BTreeMap<&str, &str> {
    let prefix = match profile {
        Profile::Dev => "CARGO_PROFILE_DEV_",
        Profile::Release => "CARGO_PROFILE_RELEASE_",
    };
    env.iter()
        .filter(|(name, _)| name.starts_with(prefix) || name.as_str() == "CARGO_INCREMENTAL")
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

fn flags(env: &BTreeMap<OsString, OsString>) -> Result<Vec<String>, String> {
    let flags = if let Some(encoded) = env.get(OsStr::new("CARGO_ENCODED_RUSTFLAGS")) {
        let encoded = encoded.to_str().ok_or("CARGO_ENCODED_RUSTFLAGS must be UTF-8")?;
        if encoded.is_empty() {
            Vec::new()
        } else {
            encoded.split('\u{1f}').map(str::to_string).collect()
        }
    } else {
        env.get(OsStr::new("RUSTFLAGS"))
            .map(|flags| flags.to_str().ok_or("RUSTFLAGS must be UTF-8"))
            .transpose()?
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let mut args = flags.iter();
    while let Some(flag) = args.next() {
        if ["-A", "-D", "-F", "-W"].iter().any(|prefix| flag.starts_with(prefix)) {
            if flag.len() == 2 {
                args.next().ok_or("lint flag requires a value")?;
            }
            continue;
        }
        if flag == "--cfg" {
            args.next().ok_or("--cfg requires a value")?;
            continue;
        }
        if flag.starts_with("--cfg=") {
            continue;
        }
        let option = if flag == "-C" {
            args.next().ok_or("-C requires a value")?.as_str()
        } else {
            flag.strip_prefix("-C").ok_or("unsupported Rust flag; use declared compiler options or remove RUSTFLAGS")?
        };
        let (name, value) = option.split_once('=').unwrap_or((option, ""));
        if name == "target-cpu" && value == "native" {
            return Err("target-cpu=native is host-dependent; select an explicit target CPU".into());
        }
        if !matches!(
            name,
            "opt-level"
                | "debuginfo"
                | "debug-assertions"
                | "overflow-checks"
                | "lto"
                | "codegen-units"
                | "panic"
                | "target-cpu"
                | "target-feature"
                | "embed-bitcode"
                | "strip"
        ) || value.is_empty()
        {
            return Err(format!(
                "unsupported Rust codegen option {name}; use declared compiler options or remove RUSTFLAGS"
            ));
        }
    }
    Ok(flags)
}

fn configs(root: &Path, env: &BTreeMap<OsString, OsString>) -> Vec<PathBuf> {
    let home = env
        .get(OsStr::new("CARGO_HOME"))
        .map(|home| root.join(home))
        .or_else(|| env.get(OsStr::new("HOME")).map(|home| PathBuf::from(home).join(".cargo")));
    root.ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(home)
        .map(|dir| {
            let old = dir.join("config");
            if old.is_file() {
                old
            } else {
                dir.join("config.toml")
            }
        })
        .collect()
}

fn validate_config(text: &str) -> Result<(), String> {
    let config: toml::Value = toml::from_str(text).map_err(|_| "invalid Cargo config")?;
    for (name, value) in config.as_table().ok_or("Cargo config must be a table")? {
        if matches!(name.as_str(), "net" | "term" | "alias" | "registry" | "registries") {
            continue;
        }
        if name == "build"
            && value
                .as_table()
                .is_some_and(|table| table.keys().all(|key| matches!(key.as_str(), "jobs" | "target-dir")))
        {
            continue;
        }
        return Err(format!("data build identity does not support Cargo config [{name}]; remove its build override"));
    }
    Ok(())
}

fn locate(root: &Path, program: &OsString) -> Result<PathBuf, String> {
    let found = located(root, program)?;
    found.canonicalize().map_err(|e| format!("{}: {e}", found.display()))
}

fn located(root: &Path, program: &OsString) -> Result<PathBuf, String> {
    let path = Path::new(program);
    let found = if path.components().count() > 1 {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| root.join(dir).join(path))
            .find(|path| path.is_file())
            .ok_or_else(|| format!("{} is required for data builds", path.display()))?
    };
    Ok(found)
}

fn executable(root: &Path, override_path: Option<&OsString>, name: &str) -> Result<PathBuf, String> {
    let program = override_path.cloned().unwrap_or_else(|| name.into());
    let path = locate(root, &program)?;
    if matches!(name, "rustc" | "cargo") {
        #[cfg(unix)]
        {
            let rustup = path.parent().unwrap().join("rustup");
            use std::os::unix::fs::MetadataExt;
            let proxy = fs::metadata(&path)
                .ok()
                .zip(fs::metadata(&rustup).ok())
                .is_some_and(|(a, b)| a.dev() == b.dev() && a.ino() == b.ino());
            if proxy {
                return locate(root, &output(root, &rustup, &["which", name])?.trim().into());
            }
        }
    }
    #[cfg(target_os = "macos")]
    if name == "cc"
        && path.starts_with("/usr/bin")
        && path.file_name().is_some_and(|name| name == "clang" || name == "cc" || name == "gcc")
    {
        return locate(root, &output(root, Path::new("/usr/bin/xcrun"), &["--find", "clang"])?.trim().into());
    }
    Ok(path)
}

fn native_binary(path: &Path) -> Result<(), String> {
    let mut magic = [0; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if magic != *b"\x7fELF"
        && &magic[..2] != b"MZ"
        && ![
            [0xfe, 0xed, 0xfa, 0xce],
            [0xce, 0xfa, 0xed, 0xfe],
            [0xfe, 0xed, 0xfa, 0xcf],
            [0xcf, 0xfa, 0xed, 0xfe],
            [0xca, 0xfe, 0xba, 0xbe],
            [0xbe, 0xba, 0xfe, 0xca],
            [0xca, 0xfe, 0xba, 0xbf],
            [0xbf, 0xba, 0xfe, 0xca],
        ]
        .contains(&magic)
    {
        return Err(format!("{} must be a native build tool executable; remove script wrappers", path.display()));
    }
    Ok(())
}

fn output(root: &Path, program: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if !output.status.success() {
        return Err(format!("{} {} failed: {}", program.display(), args.join(" "), output.status));
    }
    String::from_utf8(output.stdout).map_err(|_| "build tool output is not UTF-8".into())
}

#[cfg(test)]
mod tests;
