//! Fixture publication pins a non-secret destination distinct from production data.

use super::Credentials;
use std::path::PathBuf;

#[derive(PartialEq, Eq, serde::Serialize)]
enum Target {
    Directory(PathBuf),
    Bucket { endpoint: String, bucket: String },
}

impl Target {
    fn configured(credentials: Credentials, var: &impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let prefix = credentials.prefix();
        let value = |suffix| var(&format!("{prefix}_{suffix}")).filter(|value| !value.is_empty());
        if let Some(path) = var(&format!("{prefix}_LOCAL_DIR")) {
            if path.is_empty() {
                return Err(format!("{prefix}_LOCAL_DIR is empty"));
            }
            if value("BUCKET").is_some() {
                return Err(format!("{prefix}_LOCAL_DIR and {prefix}_BUCKET are both set"));
            }
            let path = PathBuf::from(path).canonicalize().map_err(|e| {
                format!("prepare the configured {prefix}_LOCAL_DIR directory before fixture publication: {e}")
            })?;
            if !path.is_dir() {
                return Err(format!("{prefix}_LOCAL_DIR is not a directory"));
            }
            return Ok(Self::Directory(path));
        }
        let bucket = value("BUCKET").ok_or_else(|| format!("configure {prefix}_BUCKET to identify its destination"))?;
        if bucket.is_empty()
            || !bucket.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
        {
            return Err(format!("{prefix}_BUCKET must be a plain lowercase bucket name"));
        }
        let endpoint = value("ENDPOINT")
            .or_else(|| value("ACCOUNT_ID").map(|account| format!("https://{account}.r2.cloudflarestorage.com")));
        let endpoint = endpoint.ok_or_else(|| format!("configure {prefix}_ENDPOINT or {prefix}_ACCOUNT_ID"))?;
        let uri: ureq::http::Uri = endpoint.parse().map_err(|_| format!("{prefix}_ENDPOINT is not an HTTPS origin"))?;
        if uri.scheme_str().is_none_or(|scheme| !scheme.eq_ignore_ascii_case("https"))
            || uri.authority().is_none_or(|authority| authority.as_str().contains('@'))
            || uri.query().is_some()
            || !matches!(uri.path(), "" | "/")
        {
            return Err(format!("{prefix}_ENDPOINT must be an HTTPS origin without credentials or a path"));
        }
        let authority = uri.authority().expect("checked above").as_str().to_ascii_lowercase();
        let authority = authority.strip_suffix(":443").unwrap_or(&authority);
        Ok(Self::Bucket { endpoint: format!("https://{authority}"), bucket })
    }

    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Directory(left), Self::Directory(right)) => left.starts_with(right) || right.starts_with(left),
            _ => self == other,
        }
    }
}

fn destination(var: impl Fn(&str) -> Option<String>) -> Result<String, String> {
    let fixtures = Target::configured(Credentials::Fixtures, &var)?;
    let main = Target::configured(Credentials::Main, &var)?;
    if fixtures.overlaps(&main) {
        return Err("fixture publication needs a destination separate from the main data bucket or directory".into());
    }
    Ok(crate::store::sha256_hex(&serde_json::to_vec(&fixtures).map_err(|e| e.to_string())?))
}

/// No access key, secret or network operation is needed to compare the configured destinations.
pub fn fixture_destination() -> Result<String, String> {
    destination(|name| std::env::var(name).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn aliases_and_overlapping_local_roots_cannot_select_the_main_destination() {
        let mut configured: BTreeMap<&str, String> = [
            ("OBC_R2_BUCKET", "maps".into()),
            ("OBC_R2_ACCOUNT_ID", "account".into()),
            ("OBC_FIXTURE_R2_BUCKET", "maps".into()),
            ("OBC_FIXTURE_R2_ENDPOINT", "https://ACCOUNT.r2.cloudflarestorage.com:443/".into()),
        ]
        .into();
        let observe = |values: &BTreeMap<&str, String>| destination(|name| values.get(name).cloned());
        assert!(observe(&configured).unwrap_err().contains("separate"));
        configured.insert("OBC_FIXTURE_R2_BUCKET", "fixtures".into());
        let selected = observe(&configured).unwrap();
        configured.insert("OBC_FIXTURE_R2_ENDPOINT", "https://account.r2.cloudflarestorage.com".into());
        assert_eq!(observe(&configured).unwrap(), selected, "origin aliases keep the same non-secret target");
        configured.insert("OBC_FIXTURE_R2_SECRET_ACCESS_KEY", "rotated".into());
        assert_eq!(observe(&configured).unwrap(), selected, "secret rotation is not a destination change");
        configured.insert("OBC_FIXTURE_R2_ENDPOINT", "https://account.r2.cloudflarestorage.com/path".into());
        assert!(observe(&configured).is_err());

        let scratch = crate::store::tests::Scratch::new("fixture-targets");
        let child = scratch.0.join("fixtures");
        std::fs::create_dir_all(&child).unwrap();
        let main = scratch.0.canonicalize().unwrap();
        let configured: BTreeMap<_, _> = [
            ("OBC_R2_LOCAL_DIR", main.to_str().unwrap().to_string()),
            ("OBC_FIXTURE_R2_LOCAL_DIR", child.join("..").to_str().unwrap().to_string()),
        ]
        .into();
        assert!(observe(&configured).unwrap_err().contains("separate"));
        let configured = BTreeMap::from([
            ("OBC_R2_LOCAL_DIR", main.to_str().unwrap().to_string()),
            ("OBC_FIXTURE_R2_LOCAL_DIR", child.to_str().unwrap().to_string()),
        ]);
        assert!(observe(&configured).unwrap_err().contains("separate"));
    }
}
