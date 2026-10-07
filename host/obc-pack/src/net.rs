//! Read a small catalog document.

/// Small documents, such as a region index or a catalog manifest, are read whole: each is parsed as
/// one document and a partial one is worthless.
pub fn get_text(url: &str) -> Result<String, String> {
    let mut resp = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    resp.body_mut().read_to_string().map_err(|e| format!("read {url}: {e}"))
}
