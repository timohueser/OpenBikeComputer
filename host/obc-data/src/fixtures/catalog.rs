//! The Git consumer catalog changes only after its immutable archives are verified.

use super::*;

pub(crate) struct Catalog {
    pub text: String,
    pub document: toml::Value,
    pub base_url: String,
    pub prefix: String,
    pub file: LayerFile,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_update_preserves_unrelated_catalog_and_staged_work_and_refuses_a_changed_pointer() {
        let scratch = crate::store::tests::Scratch::new("fixture-catalog-apply");
        let root = &scratch.0;
        std::fs::create_dir(root.join("fixtures")).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(root).status().unwrap().success());
        std::fs::write(root.join("unrelated"), "staged owner work").unwrap();
        assert!(std::process::Command::new("git")
            .args(["add", "unrelated"])
            .current_dir(root)
            .status()
            .unwrap()
            .success());
        let staged = std::fs::read(root.join(".git/index")).unwrap();
        let old = "a".repeat(64);
        let text = format!("schema=1\nbase_url=\"https://fixtures.example/v1/\"\n\n[packages.ride]\nsummary=\"Ride\"\narchive=\"packages/{old}.tar.gz\"\nsha256=\"{old}\"\nbytes=10\nprovenance=\"Original\"\nlicense=\"ODbL-1.0\"\n\n# Owner's unchanged scenario\n[scenarios.ride]\npackages=[\"ride\"]\n");
        std::fs::write(root.join("fixtures/catalog.toml"), &text).unwrap();
        let reviewed = Catalog::read(root).unwrap();
        let digest = "b".repeat(64);
        let selected = [(
            "ride".into(),
            (
                LayerFile { path: format!("packages/{digest}.tar.gz"), size: 20, sha256: digest.clone() },
                "Ride".into(),
                "ODbL-1.0".into(),
            ),
        )]
        .into();
        reviewed.replace(root, &selected).unwrap();
        let applied = Catalog::read(root).unwrap();
        assert_eq!(applied.document["packages"]["ride"]["sha256"].as_str(), Some(digest.as_str()));
        assert!(applied.text.ends_with("# Owner's unchanged scenario\n[scenarios.ride]\npackages=[\"ride\"]\n"));
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), staged);
        assert!(reviewed.replace(root, &selected).unwrap_err().contains("catalog changed"));
        assert_eq!(Catalog::read(root).unwrap().text, applied.text);
        assert!(applied
            .replace(
                root,
                &[(
                    "new-map".into(),
                    (
                        LayerFile { path: "packages/placeholder.tar.gz".into(), size: 1, sha256: "c".repeat(64) },
                        "New map".into(),
                        "ODbL-1.0".into()
                    )
                )]
                .into()
            )
            .is_err());
    }
}

impl Catalog {
    pub fn assets(&self, root: &Path, id: &str, map: &str) -> Result<BTreeMap<String, LayerFile>, String> {
        let mut assets = BTreeMap::new();
        let tracked = self.document["packages"].get(id).and_then(|p| p.get("tracked_sources"));
        for (destination, source) in tracked.and_then(toml::Value::as_table).into_iter().flatten() {
            relative(destination)?;
            let source = source.as_str().ok_or("fixture tracked source is not a path")?;
            relative(source)?;
            if destination == map || destination.starts_with(".obc-") {
                return Err("fixture tracked asset conflicts with generated output".into());
            }
            let path = format!("fixtures/{source}");
            if !std::fs::symlink_metadata(root.join(&path)).map_err(|e| e.to_string())?.file_type().is_file()
                || !root
                    .join(&path)
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(root.join("fixtures").canonicalize().map_err(|e| e.to_string())?)
            {
                return Err("fixture tracked asset must be a regular file inside fixtures".into());
            }
            let (sha256, size) = hash_file(&root.join(&path))?;
            assets.insert(destination.clone(), LayerFile { path, sha256, size });
        }
        Ok(assets)
    }

    pub fn read(root: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(root.join("fixtures/catalog.toml")).map_err(|e| e.to_string())?;
        let document: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
        if document.get("schema").and_then(toml::Value::as_integer) != Some(1)
            || document.get("packages").and_then(toml::Value::as_table).is_none()
        {
            return Err("fixture consumer catalog has no supported schema and packages".into());
        }
        let base_url = document
            .get("base_url")
            .and_then(toml::Value::as_str)
            .ok_or("fixture catalog has no public origin")?
            .to_string();
        let uri: ureq::http::Uri = base_url.parse().map_err(|_| "fixture catalog has no HTTPS public origin")?;
        if uri.scheme_str() != Some("https")
            || uri.authority().is_none_or(|authority| authority.as_str().contains('@'))
            || uri.query().is_some()
            || !uri.path().ends_with('/')
        {
            return Err("fixture catalog needs an HTTPS public origin and prefix without credentials".into());
        }
        let prefix = uri.path().trim_start_matches('/').to_string();
        if !prefix.is_empty() {
            relative(prefix.trim_end_matches('/'))?;
        }
        let file = LayerFile {
            path: "fixtures/catalog.toml".into(),
            size: text.len() as u64,
            sha256: sha256_hex(text.as_bytes()),
        };
        Ok(Self { text, document, base_url, prefix, file })
    }

    pub fn replace(&self, root: &Path, selected: &BTreeMap<String, (LayerFile, String, String)>) -> Result<(), String> {
        for (id, (archive, _, licence)) in selected {
            if !crate::is_kebab(id) || licence.is_empty() || archive.size == 0 {
                return Err("fixture catalog update lacks a named package, selected licence and archive".into());
            }
            digest(&archive.sha256)?;
            if archive.path != format!("packages/{}.tar.gz", archive.sha256) {
                return Err("fixture catalog update needs an immutable hash-named archive".into());
            }
        }
        let current = std::fs::read(root.join(&self.file.path)).map_err(|e| e.to_string())?;
        if current != self.text.as_bytes() {
            return Err("fixture catalog changed; review a new plan before its update".into());
        }
        let mut updated = String::new();
        let mut package = None;
        let mut seen = std::collections::BTreeSet::new();
        for line in self.text.split_inclusive('\n') {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                package = trimmed.strip_prefix("[packages.").and_then(|id| id.strip_suffix(']'));
            }
            let replacement = package.and_then(|id| {
                let (archive, _, licence) = selected.get(id)?;
                let field = line.split_once('=')?.0.trim();
                let value = match field {
                    "archive" => serde_json::to_string(&archive.path).ok()?,
                    "sha256" => serde_json::to_string(&archive.sha256).ok()?,
                    "bytes" => archive.size.to_string(),
                    "license" => serde_json::to_string(licence).ok()?,
                    "provenance" => serde_json::to_string(
                        "Exact selected inputs and producer receipts: .obc-data.json in the archive",
                    )
                    .ok()?,
                    _ => return None,
                };
                seen.insert((id.to_string(), field.to_string()));
                Some(format!("{field} = {value}{}", if line.ends_with('\n') { "\n" } else { "" }))
            });
            updated.push_str(replacement.as_deref().unwrap_or(line));
        }
        for (id, (archive, summary, licence)) in selected {
            if self.document["packages"].get(id).is_none() {
                updated.push_str(&format!(
                    "\n[packages.{id}]\nsummary = {}\narchive = {}\nsha256 = {}\nbytes = {}\nprovenance = {}\nlicense = {}\n",
                    serde_json::to_string(summary).map_err(|e| e.to_string())?,
                    serde_json::to_string(&archive.path).map_err(|e| e.to_string())?,
                    serde_json::to_string(&archive.sha256).map_err(|e| e.to_string())?,
                    archive.size,
                    serde_json::to_string("Exact selected inputs and producer receipts: .obc-data.json in the archive").map_err(|e| e.to_string())?,
                    serde_json::to_string(licence).map_err(|e| e.to_string())?,
                ));
            } else if ["archive", "sha256", "bytes", "provenance", "license"]
                .iter()
                .any(|field| !seen.contains(&(id.clone(), field.to_string())))
            {
                return Err(format!("fixture catalog {id} lacks its immutable package fields"));
            }
        }
        let _: toml::Value = toml::from_str(&updated).map_err(|e| e.to_string())?;
        if std::fs::read(root.join(&self.file.path)).map_err(|e| e.to_string())? != self.text.as_bytes() {
            return Err("fixture catalog changed before replacement; review a new plan".into());
        }
        crate::store::write_atomic(&root.join(&self.file.path), updated.as_bytes())
    }
}
