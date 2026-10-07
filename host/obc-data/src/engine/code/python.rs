//! Locked package exports and the interpreter selected by the same offline uv project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::Python;
use crate::store::sha256_hex;

fn output(command: &mut Command, label: &str) -> Result<String, String> {
    let result = command.output().map_err(|error| format!("{label}: {error}; prepare the Python runtime first"))?;
    if !result.status.success() {
        return Err(format!(
            "{label}: {}; prepare the selected offline Python runtime first",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    String::from_utf8(result.stdout).map_err(|error| format!("{label}: {error}"))
}

pub(super) struct Identity {
    pub hashes: BTreeMap<String, String>,
    pub executable: PathBuf,
    pub packages: BTreeMap<String, String>,
}

fn interpreter_command(root: &Path) -> Command {
    let mut command = Command::new("uv");
    command.current_dir(root).args(["python", "find", "--system", "--offline", "--no-python-downloads"]);
    if let Some(request) = std::env::var_os("UV_PYTHON") {
        command.arg(request);
    }
    command
}

pub(super) fn packages(root: &Path, group: Option<&str>) -> Result<BTreeMap<String, String>, String> {
    let mut export = Command::new("uv");
    export.current_dir(root).args([
        "export",
        "--locked",
        "--offline",
        "--no-default-groups",
        "--no-emit-project",
        "--no-header",
        "--no-annotate",
    ]);
    if let Some(group) = group {
        export.args(["--group", group]);
    }
    let packages = output(&mut export, "uv locked export")?;
    let packages = normalized(&packages);
    Ok(BTreeMap::from([(format!("python/packages/{}", group.unwrap_or("base")), sha256_hex(packages.as_bytes()))]))
}

pub(crate) fn executable(root: &Path) -> Result<PathBuf, String> {
    Ok(PathBuf::from(output(&mut interpreter_command(root), "uv offline interpreter")?.trim()))
}

pub(super) fn identity(root: &Path, runtime: &Python) -> Result<Identity, String> {
    let packages = packages(root, runtime.group.as_deref())?;
    let mut hashes = packages.clone();
    let executable = executable(root)?;
    let script = "import json,sys,sysconfig; print(json.dumps({'implementation':sys.implementation.name,'version':list(sys.version_info[:3]),'abi':sysconfig.get_config_var('SOABI')}))";
    let interpreter = output(Command::new(&executable).args(["-c", script]), "Python runtime identity")?;
    let interpreter: serde_json::Value =
        serde_json::from_str(&interpreter).map_err(|error| format!("Python identity: {error}"))?;
    let interpreter = serde_json::to_vec(&super::super::sorted(interpreter)).map_err(|error| error.to_string())?;
    hashes.insert("python/runtime".into(), sha256_hex(&interpreter));
    Ok(Identity { executable, hashes, packages })
}

fn normalized(export: &str) -> String {
    let mut packages = Vec::new();
    let mut package = String::new();
    for line in export.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let continued = line.ends_with('\\');
        if !package.is_empty() {
            package.push(' ');
        }
        package.push_str(line.trim_end_matches('\\').trim());
        if !continued {
            packages.push(std::mem::take(&mut package));
        }
    }
    if !package.is_empty() {
        packages.push(package);
    }
    packages.sort();
    packages.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::write;
    use crate::store::tests::Scratch;

    #[test]
    fn locked_export_changes_only_for_the_selected_group_and_transitive_packages() {
        let scratch = Scratch::new("python-code-groups");
        let root = &scratch.0;
        let project = |alpha: &str, beta: &str, gamma: &str| {
            write(&root.join("pyproject.toml"), &format!(
                "[project]\nname = \"identity-fixture\"\nversion = \"0\"\nrequires-python = \">=3.12\"\ndependencies = []\n[dependency-groups]\nselected = [\"alpha=={alpha}\"]\ntests = [\"beta=={beta}\"]\n[tool.uv]\npackage = false\ndefault-groups = []\n[[tool.uv.index]]\nurl = \"https://example.org/simple\"\ndefault = true\n"
            ));
            let mut lock = format!(
                "version = 1\nrevision = 3\nrequires-python = \">=3.12\"\n[[package]]\nname = \"identity-fixture\"\nversion = \"0\"\nsource = {{ virtual = \".\" }}\n[package.dev-dependencies]\nselected = [{{ name = \"alpha\" }}]\ntests = [{{ name = \"beta\" }}]\n[package.metadata.requires-dev]\nselected = [{{ name = \"alpha\", specifier = \"=={alpha}\" }}]\ntests = [{{ name = \"beta\", specifier = \"=={beta}\" }}]\n"
            );
            for (name, version) in [("alpha", alpha), ("beta", beta), ("gamma", gamma)] {
                lock += &format!("[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = {{ registry = \"https://example.org/simple\" }}\n");
                if name == "alpha" {
                    lock += "dependencies = [{ name = \"gamma\" }]\n";
                }
                lock += &format!("sdist = {{ url = \"https://example.org/{name}-{version}.tar.gz\", hash = \"sha256:{}\", size = 1 }}\n", "a".repeat(64));
            }
            write(&root.join("uv.lock"), &lock);
        };
        write(&root.join(".python-version"), "3.12\n");
        let runtime = Python { group: Some("selected".into()) };
        project("1.0.0", "1.0.0", "1.0.0");
        let before = identity(root, &runtime).unwrap().hashes;
        let packaged = super::super::Code { python_packages: Some("selected".into()), ..Default::default() };
        let selected = super::super::files(root, &packaged).unwrap();
        assert_eq!(
            selected.keys().map(String::as_str).collect::<Vec<_>>(),
            [super::super::SOURCE_BINDING, "python/packages/selected"]
        );
        assert_eq!(selected["python/packages/selected"], before["python/packages/selected"]);
        assert!(!selected.contains_key("python/runtime"), "packaged dependencies select no host interpreter");
        project("1.0.0", "2.0.0", "1.0.0");
        assert_eq!(
            identity(root, &runtime).unwrap().hashes,
            before,
            "an unrelated test group changes no producer input"
        );
        project("2.0.0", "2.0.0", "1.0.0");
        assert_ne!(identity(root, &runtime).unwrap().hashes, before);
        project("1.0.0", "1.0.0", "2.0.0");
        assert_ne!(identity(root, &runtime).unwrap().hashes, before, "transitive packages are producer inputs");
        assert!(!root.join(".venv").exists(), "identity discovery neither syncs nor installs an environment");
    }
    #[test]
    fn execution_binds_the_discovered_base_despite_active_and_project_environments() {
        let scratch = Scratch::new("python-code-interpreter");
        let root = &scratch.0;
        write(&root.join(".python-version"), "3.12\n");
        write(&root.join("pyproject.toml"), "[project]\nname = \"runtime-fixture\"\nversion = \"0\"\nrequires-python = \">=3.12\"\ndependencies = []\n[tool.uv]\npackage = false\ndefault-groups = []\n");
        write(&root.join("uv.lock"), "version = 1\nrevision = 3\nrequires-python = \">=3.12\"\n[[package]]\nname = \"runtime-fixture\"\nversion = \"0\"\nsource = { virtual = \".\" }\n");
        let base = PathBuf::from(output(&mut interpreter_command(root), "fixture interpreter").unwrap().trim());
        let active = root.join("active");
        let project = root.join("project");
        for path in [&active, &project] {
            let result = Command::new(&base).args(["-m", "venv", "--without-pip"]).arg(path).output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        }
        let code = super::super::Code { python: Some(Python::default()), ..Default::default() };
        let files = super::super::files(root, &code).unwrap();
        let expected = super::super::hash(&files);
        assert!(!root.join(".venv").exists(), "identity discovery creates no environment");
        let script = "import json,sys,sysconfig; print(json.dumps({'implementation':sys.implementation.name,'version':list(sys.version_info[:3]),'abi':sysconfig.get_config_var('SOABI'),'base':sys._base_executable,'prefix':sys.prefix}))";
        let argv: Vec<_> = [
            "uv",
            "run",
            "--locked",
            "--offline",
            "--no-default-groups",
            "--no-python-downloads",
            "python",
            "-c",
            script,
        ]
        .map(String::from)
        .into();
        for override_project in [false, true] {
            let mut find = interpreter_command(root);
            find.env_remove("UV_PROJECT_ENVIRONMENT").env("VIRTUAL_ENV", &active);
            if override_project {
                find.env("UV_PROJECT_ENVIRONMENT", &project);
            }
            assert_eq!(PathBuf::from(output(&mut find, "fixture discovery").unwrap().trim()), base);
            let mut command = super::super::super::process::command(root, &argv, Some((&code, &expected))).unwrap();
            assert_eq!(
                command.get_envs().find(|(key, _)| *key == "UV_NO_SYNC").unwrap().1,
                Some(std::ffi::OsStr::new("0"))
            );
            command.env_remove("UV_PROJECT_ENVIRONMENT").env("VIRTUAL_ENV", &active);
            if override_project {
                command.env("UV_PROJECT_ENVIRONMENT", &project);
            }
            let result = output(&mut command, "fixture execution").unwrap();
            let mut result: serde_json::Value = serde_json::from_str(&result).unwrap();
            let actual_base = PathBuf::from(result.as_object_mut().unwrap().remove("base").unwrap().as_str().unwrap());
            assert_eq!(actual_base.canonicalize().unwrap(), base.canonicalize().unwrap());
            let prefix = result.as_object_mut().unwrap().remove("prefix").unwrap();
            let expected_prefix = if override_project { project.clone() } else { root.join(".venv") };
            assert_eq!(
                PathBuf::from(prefix.as_str().unwrap()).canonicalize().unwrap(),
                expected_prefix.canonicalize().unwrap()
            );
            let bytes = serde_json::to_vec(&super::super::super::sorted(result)).unwrap();
            assert_eq!(sha256_hex(&bytes), files["python/runtime"], "actual execution matches its runtime fingerprint");
        }
        assert!(super::super::super::process::command(root, &argv, Some((&code, "changed")))
            .unwrap_err()
            .contains("changed; plan again"));
        let ordinary = super::super::super::process::command(root, &argv, None).unwrap();
        assert!(
            !ordinary.get_envs().any(|(key, _)| key == "UV_PYTHON"),
            "only declared Python commands receive a binding"
        );
    }
}
