//! The Git consumer catalog changes only after its immutable archives are verified.

use super::*;

pub(crate) struct Catalog {
    pub text: String,
    pub document: toml::Value,
    pub base_url: String,
    pub prefix: String,
    pub file: LayerFile,
}

impl Catalog {
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
