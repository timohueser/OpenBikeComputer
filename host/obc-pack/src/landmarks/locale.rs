//! Local fallback from pinned administrative P131/P37 and country P17/P37 claims.
use super::*;
pub const LANGUAGE_BYTES: &[u8] = include_bytes!("../../../../specs/content-languages.json");
pub const MAX_DEPTH: usize = 8;
pub const MAX_LOCALES: usize = 64;

pub fn languages() -> Vec<(String, String)> {
    serde_json::from_slice(LANGUAGE_BYTES).expect("checked content language mapping")
}

pub fn default_language(
    entity: &Value,
    locales: &BTreeMap<String, Value>,
    available: &[TextVariant],
) -> (String, Vec<String>) {
    let supported = languages();
    let choose = |ids: &BTreeSet<String>| {
        let mut sources = Vec::new();
        let mut choices = BTreeSet::new();
        for id in ids {
            if let Some(locale) = locales.get(id) {
                let official = entity_ids(locale, "P37");
                for (code, language) in &supported {
                    if official.contains(language) && available.iter().any(|v| v.language == *code) {
                        choices.insert(code.clone());
                        sources.push(id.clone());
                    }
                }
            }
        }
        supported.iter().find(|(code, _)| choices.contains(code)).map(|(code, _)| {
            sources.sort();
            sources.dedup();
            (code.clone(), sources)
        })
    };
    let mut pending: BTreeSet<_> = entity_ids(entity, "P131").into_iter().collect();
    let mut visited = BTreeSet::new();
    for _ in 0..MAX_DEPTH {
        pending = pending.into_iter().filter(|id| !visited.contains(id)).take(MAX_LOCALES - visited.len()).collect();
        if let Some(result) = choose(&pending) {
            return result;
        }
        let mut next = BTreeSet::new();
        for id in pending {
            if visited.insert(id.clone()) {
                if let Some(locale) = locales.get(&id) {
                    next.extend(entity_ids(locale, "P131"));
                }
            }
        }
        next.retain(|id| !visited.contains(id));
        pending = next;
    }
    if let Some(result) = choose(&entity_ids(entity, "P17").into_iter().collect()) {
        return result;
    }
    let code = supported
        .iter()
        .find(|(code, _)| available.iter().any(|v| v.language == *code))
        .expect("a usable supported article")
        .0
        .clone();
    (code, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn claim(id: &str) -> Value {
        serde_json::json!({"mainsnak":{"datavalue":{"value":{"id":id}}}})
    }
    #[test]
    fn administrative_language_precedes_country_and_missing_metadata_keeps_content() {
        let variant = |language: &str| TextVariant {
            language: language.into(),
            text_pages: vec!["Text.".into()],
            attribution: Attribution {
                source_url: String::new(),
                revision: String::new(),
                license_url: String::new(),
                original_notices: String::new(),
            },
        };
        let available = [variant("de"), variant("fr")];
        let entity = serde_json::json!({"claims":{"P131":[claim("Q10")], "P17":[claim("Q20")]}});
        let mut locales = BTreeMap::from([
            ("Q10".into(), serde_json::json!({"claims":{"P37":[claim("Q150")], "P131":[claim("Q10")]}})),
            ("Q20".into(), serde_json::json!({"claims":{"P37":[claim("Q188")]}})),
        ]);
        assert_eq!(default_language(&entity, &locales, &available), ("fr".into(), vec!["Q10".into()]));
        locales.get_mut("Q10").unwrap()["claims"]["P37"] = serde_json::json!([]);
        assert_eq!(default_language(&entity, &locales, &available), ("de".into(), vec!["Q20".into()]));
        assert_eq!(default_language(&entity, &BTreeMap::new(), &available), ("de".into(), vec![]));
    }
    #[test]
    fn content_language_mapping_matches_wire_order() {
        assert_eq!(
            languages().iter().map(|(code, _)| code.as_bytes()).collect::<Vec<_>>(),
            obc_formats::articles::LANGUAGES.iter().map(|code| code.as_slice()).collect::<Vec<_>>()
        );
    }
}
