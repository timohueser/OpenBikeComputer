//! Definition-only deletion with a reviewed digest and freshly checked references.

use crate::cli::api::{Code, Error};
use crate::regions::{Area, Regions};
use crate::store::sha256_hex;
use schemars::JsonSchema;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct Deletion {
    pub(super) region: String,
    pub(super) sha256: String,
    pub(super) used_by: Vec<String>,
}

pub(super) fn definition_path(root: &Path, id: &str) -> Result<PathBuf, Error> {
    if !id.split('/').all(crate::is_kebab) {
        return Err(Code::Usage.error("region id parts are lowercase kebab-case"));
    }
    let path = root.join("data/regions").join(id).with_extension("toml");
    for component in path.ancestors().take_while(|path| *path != root) {
        if let Ok(metadata) = std::fs::symlink_metadata(component) {
            if metadata.file_type().is_symlink() {
                return Err(Code::InvalidData.error(format!("{} is a symlink", component.display())));
            }
        }
    }
    Ok(path)
}

pub(super) fn deletion(root: &Path, id: &str) -> Result<Deletion, Error> {
    let regions = Regions::load(root).map_err(|e| Code::InvalidData.error(e))?;
    if regions.get(id).is_none() {
        return Err(Code::Usage.error(format!("no region `{id}`")));
    }
    let path = definition_path(root, id)?;
    let mut used_by = Vec::new();
    for region in regions.iter() {
        if let Area::Union { union } = &region.area {
            if union.iter().any(|member| member == id) {
                used_by.push(format!("region:{}", region.id));
            }
        }
    }
    for dir in ["data/env", "fixtures", "tools/planner-regions"] {
        references(root, &root.join(dir), id, &mut used_by)?;
    }
    used_by.sort();
    used_by.dedup();
    Ok(Deletion { region: id.into(), sha256: sha256_hex(&std::fs::read(path).map_err(|e| e.to_string())?), used_by })
}

fn references(root: &Path, path: &Path, id: &str, out: &mut Vec<String>) -> Result<(), Error> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string().into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(Code::InvalidData.error(format!("{} is a symlink", path.display())));
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
            references(root, &entry.map_err(|e| e.to_string())?.path(), id, out)?;
        }
        return Ok(());
    }
    let found = match path.extension().and_then(|ext| ext.to_str()) {
        Some("json" | "toml") => {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let value: serde_json::Value = if path.extension().is_some_and(|ext| ext == "json") {
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            } else {
                let value: toml::Value = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                serde_json::to_value(value).map_err(|e| e.to_string())?
            };
            names(&value, id)
        }
        Some("sh") => std::fs::read_to_string(path)
            .map_err(|e| e.to_string())?
            .split(|c: char| c.is_whitespace() || "\"'()$;".contains(c))
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .windows(2)
            .any(|words| words == ["box", id]),
        _ => false,
    };
    if found {
        out.push(path.strip_prefix(root).expect("below root").to_string_lossy().into_owned());
    }
    Ok(())
}

fn names(value: &serde_json::Value, id: &str) -> bool {
    match value {
        serde_json::Value::String(name) => name == id,
        serde_json::Value::Array(values) => values.iter().any(|value| names(value, id)),
        serde_json::Value::Object(fields) => fields.values().any(|value| names(value, id)),
        _ => false,
    }
}

pub(super) fn remove(root: &Path, expected: &Deletion) -> Result<(), Error> {
    let current = deletion(root, &expected.region)?;
    if current.sha256 != expected.sha256 || !current.used_by.is_empty() {
        return Err(Code::PlanOutdated.error("the region definition or its references changed; review deletion again"));
    }
    std::fs::remove_file(definition_path(root, &expected.region)?).map_err(|e| e.to_string().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::write;
    use crate::store::tests::Scratch;

    #[test]
    fn deletion_rechecks_definition_and_each_reference_and_keeps_baked_data() {
        let scratch = Scratch::new("region-delete");
        let root = &scratch.0;
        let path = root.join("data/regions/unused.toml");
        let definition = "name = \"Unused\"\nkind = \"box\"\nbox = [7,47,8,48]\n";
        write(&path, definition);
        write(&root.join("store/objects/kept"), "payload");
        let preview = deletion(root, "unused").unwrap();
        assert!(preview.used_by.is_empty());
        for (reference, text) in [
            ("data/env/local.toml", "region = \"unused\"\n"),
            ("data/regions/union.toml", "name = \"Union\"\nkind = \"union\"\nunion = [\"unused\",\"unused\"]\n"),
            ("fixtures/data/coverage.json", "{\"region\":\"unused\"}"),
            ("fixtures/build.sh", "BBOX=\"$(box unused)\""),
            ("tools/planner-regions/recipe.json", "{\"region\":\"unused\"}"),
        ] {
            let reference = root.join(reference);
            write(&reference, text);
            assert_eq!(deletion(root, "unused").unwrap().used_by.len(), 1);
            assert_eq!(remove(root, &preview).unwrap_err().code, Code::PlanOutdated);
            assert!(path.is_file());
            std::fs::remove_file(reference).unwrap();
        }
        write(&path, &(definition.to_string() + "# changed\n"));
        assert_eq!(remove(root, &preview).unwrap_err().code, Code::PlanOutdated);
        let updated = deletion(root, "unused").unwrap();
        remove(root, &updated).unwrap();
        assert!(!path.exists());
        assert!(root.join("store/objects/kept").is_file());
    }
}
