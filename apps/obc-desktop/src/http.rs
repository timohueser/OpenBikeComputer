//! The desktop app's small native network surface. Map selection, verification
//! and assembly are shared with the web builder; Rust moves the catalog bytes
//! because the Tauri webview has no blanket network permission.

use std::io::Read;

/// Read a small text document whole.
pub fn get_text(url: &str) -> Result<String, String> {
    let mut response = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    response.body_mut().read_to_string().map_err(|e| format!("read {url}: {e}"))
}

/// Read one digest-pinned catalog object. This is deliberately not a general
/// HTTP proxy: every URL must share the configured catalog root's origin.
pub fn get_catalog_object(url: &str) -> Result<Vec<u8>, String> {
    same_catalog_origin(url, &crate::catalog::url())?;
    let mut response = ureq::get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    let mut bytes = Vec::new();
    response.body_mut().as_reader().read_to_end(&mut bytes).map_err(|e| format!("read {url}: {e}"))?;
    Ok(bytes)
}

fn same_catalog_origin(requested: &str, catalog: &str) -> Result<(), String> {
    let requested = url::Url::parse(requested).map_err(|e| format!("catalog object URL: {e}"))?;
    let catalog = url::Url::parse(catalog).map_err(|e| format!("configured catalog URL: {e}"))?;
    let local_http =
        requested.scheme() == "http" && matches!(requested.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if requested.scheme() != "https" && !local_http {
        return Err("catalog objects must use https (or loopback http for local testing)".into());
    }
    if requested.origin() != catalog.origin() {
        return Err(format!("catalog objects must stay on {}", catalog.origin().ascii_serialization()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_reads_are_https_and_same_origin() {
        let root = "https://maps.example.test/prefix/catalog.json";
        assert!(same_catalog_origin("https://maps.example.test/prefix/cells/fine/index.json", root).is_ok());
        assert!(same_catalog_origin("https://other.example.test/cell.obcm", root).is_err());
        assert!(same_catalog_origin("http://maps.example.test/cell.obcm", root).is_err());
        assert!(same_catalog_origin("file:///tmp/cell.obcm", root).is_err());
        assert!(same_catalog_origin("http://127.0.0.1:8123/cell.obcm", "http://127.0.0.1:8123/catalog.json").is_ok());
    }
}
