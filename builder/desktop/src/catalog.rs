//! The published map catalog, fetched for the desktop app. The desktop and hosted builders consume
//! the same document over HTTP and hand it to the same frontend catalog client.
//!
//! Two things a `fetch()` in the webview would not do: the window has no network capability at all,
//! so every request the app makes is Rust code in [`crate::http`], and the catalog admits nothing
//! partial, so the body is read whole and handed over as one string with the URL it came from.
//!
//! `OBC_CATALOG_URL` overrides the compiled default at run time, which is how a maintainer tests a
//! bake tree before publishing.

use serde::Serialize;

const DEFAULT_CATALOG_URL: &str = "https://maps.openbikecomputer.com/cell-catalog/catalog.json";
const COMPILED_CATALOG_URL: Option<&str> = option_env!("OBC_CATALOG_URL");

/// The manifest body plus the URL it was read from. Relative references resolve against the
/// manifest's own location, and only this side knows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedCatalog {
    pub url: String,
    pub body: String,
}

pub fn url() -> String {
    selected_url(std::env::var("OBC_CATALOG_URL").ok())
}

fn selected_url(runtime: Option<String>) -> String {
    runtime
        .filter(|value| !value.trim().is_empty())
        .or_else(|| COMPILED_CATALOG_URL.filter(|value| !value.trim().is_empty()).map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_CATALOG_URL.to_owned())
}

pub fn fetch() -> Result<FetchedCatalog, String> {
    let url = url();
    let body = crate::http::get_text(&url).map_err(|e| format!("map catalog: {e}"))?;
    Ok(FetchedCatalog { url, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_url_is_absolute_and_overridable() {
        // It must be absolute: the frontend resolves catalog references against it.
        assert_eq!(selected_url(None), "https://maps.openbikecomputer.com/cell-catalog/catalog.json");
        assert_eq!(
            selected_url(Some("https://example.invalid/catalog.json".into())),
            "https://example.invalid/catalog.json"
        );
        assert_ne!(selected_url(Some(String::new())), "", "an empty override must fall back");
    }
}
