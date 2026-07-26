//! The published map catalog (`OBCC_Spec.md`), fetched for the desktop app.
//!
//! The desktop tier serves pre-baked maps too — it is not a builder *instead of* a
//! catalog, it is a builder *as well as* one (#894's table). So this is the same
//! document the hosted site reads, over the same HTTP, and the frontend parses it
//! with the same `parseCatalog`.
//!
//! Two things this does that a `fetch()` in the webview would not:
//!
//! * The window has no network capability at all, by design. Every request the app
//!   makes is Rust code in [`crate::http`], where what it reaches is reviewable.
//! * §7 of the spec admits nothing partial, so the body is read whole here and
//!   handed over as one string with the URL it came from — the frontend needs that
//!   URL as the base for resolving preview references.
//!
//! **The default URL is a placeholder until the tier it points at exists.** B1
//! (#898) bakes the artifacts and C6 (#905) decides where they are served from;
//! until then a fetch fails with a plain "not found", which is the honest state of
//! the world rather than a silent empty catalog. `OBC_CATALOG_URL` overrides it at
//! run time, which is also how a bakery tests its own tree before publishing.

use serde::Serialize;

const DEFAULT_CATALOG_URL: &str = "https://timohueser.github.io/OpenBikeComputer/builder/data/catalog.json";

/// The manifest body plus the URL it was read from — §2 resolves a preview
/// reference against the manifest's own location, and only this side knows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedCatalog {
    pub url: String,
    pub body: String,
}

pub fn url() -> String {
    std::env::var("OBC_CATALOG_URL").unwrap_or_else(|_| DEFAULT_CATALOG_URL.to_string())
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
        // Absolute matters: the frontend resolves preview references against it.
        assert!(url().starts_with("https://"), "the default catalog URL must be absolute");
        std::env::set_var("OBC_CATALOG_URL", "https://example.invalid/catalog.json");
        assert_eq!(url(), "https://example.invalid/catalog.json");
        std::env::remove_var("OBC_CATALOG_URL");
    }
}
