//! Locked package exports and the interpreter selected by the same offline uv project.

use std::collections::BTreeMap;
use std::path::Path;
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

pub(super) fn identity(root: &Path, runtime: &Python) -> Result<BTreeMap<String, String>, String> {
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
    if let Some(group) = &runtime.group {
        export.args(["--group", group]);
    }
    let packages = output(&mut export, "uv locked export")?;
    let packages = normalized(&packages);
    let interpreter = output(
        Command::new("uv").current_dir(root).args(["python", "find", "--offline", "--no-python-downloads"]),
        "uv offline interpreter",
    )?;
    let script = "import json,sys,sysconfig; print(json.dumps({'implementation':sys.implementation.name,'version':list(sys.version_info[:3]),'abi':sysconfig.get_config_var('SOABI')}))";
    let interpreter = output(Command::new(interpreter.trim()).args(["-c", script]), "Python runtime identity")?;
    let interpreter: serde_json::Value =
        serde_json::from_str(&interpreter).map_err(|error| format!("Python identity: {error}"))?;
    let interpreter = serde_json::to_vec(&super::super::sorted(interpreter)).map_err(|error| error.to_string())?;
    Ok(BTreeMap::from([
        ("python/runtime".into(), sha256_hex(&interpreter)),
        (format!("python/packages/{}", runtime.group.as_deref().unwrap_or("base")), sha256_hex(packages.as_bytes())),
    ]))
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
        let before = identity(root, &runtime).unwrap();
        project("1.0.0", "2.0.0", "1.0.0");
        assert_eq!(identity(root, &runtime).unwrap(), before, "an unrelated test group changes no producer input");
        project("2.0.0", "2.0.0", "1.0.0");
        assert_ne!(identity(root, &runtime).unwrap(), before);
        project("1.0.0", "1.0.0", "2.0.0");
        assert_ne!(identity(root, &runtime).unwrap(), before, "transitive packages are producer inputs");
        assert!(!root.join(".venv").exists(), "identity discovery neither syncs nor installs an environment");
    }
}
