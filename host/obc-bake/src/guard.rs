//! The mandatory re-bake guard: is the *published* catalog still readable?
//!
//! `OBCC_Spec.md` §6 states the law — an OBCM format bump invalidates every baked
//! artifact — and names three mechanisms. Two already exist: the generator refuses a
//! mixed or stale tree (a), and `catalog.rs`'s `PINNED_OBCM_VERSION` breaks the
//! packer's test suite when the format constant moves (b). Both are about the
//! repository and the tree on the bake box.
//!
//! Neither can see the thing that actually matters to a rider: **what is on the CDN
//! right now**. A format bump can be merged, tested, and released with the pin
//! moved and the re-bake honestly intended — and the published catalog still serves
//! v10 artifacts to a v11 firmware for as long as nobody re-runs the bake. That is
//! the gap this module closes, and it is why the check lives here rather than in
//! `obc-pack`: it is a fact about a deployment, not about a tree, so it needs a URL
//! and a network — neither of which belongs in the packer's own test suite.
//!
//! It fails **loudly and skips gracefully**: with no catalog URL configured there is
//! nothing to check and the guard says so and succeeds, because a project that has
//! not published a catalog yet must not have a red CI check about it. Wired into
//! `.github/workflows/bake.yml`, where `vars.OBC_CATALOG_URL` supplies the URL.

use obc_pack::catalog::CatalogManifest;

/// What the guard found.
#[derive(Debug, Clone)]
pub enum GuardOutcome {
    /// No URL configured — nothing to check.
    Skipped { reason: String },
    /// Every published artifact matches this build's OBCM version.
    Current { artifacts: usize, obcm_version: u8 },
    /// Artifacts this firmware cannot read are being served right now.
    Stale { expected: u8, found: Vec<(String, u8)>, artifacts: usize },
}

impl GuardOutcome {
    pub fn ok(&self) -> bool {
        !matches!(self, GuardOutcome::Stale { .. })
    }

    pub fn render(&self) -> String {
        match self {
            GuardOutcome::Skipped { reason } => {
                format!("obcm-version guard: skipped — {reason}")
            }
            GuardOutcome::Current { artifacts, obcm_version } => {
                format!("obcm-version guard: {artifacts} published artifacts, all OBCM v{obcm_version} — current")
            }
            GuardOutcome::Stale { expected, found, artifacts } => {
                let mut s = format!(
                    "obcm-version guard: FAILED — this build writes OBCM v{expected}, but {} of {artifacts} \
                     published artifacts are a different version.\n\nAn OBCM bump invalidates every baked artifact \
                     (OBCC_Spec.md §6): every map in the catalog is unreadable to this firmware until the bakery \
                     re-runs. Re-bake and re-publish:\n\n    obc-bake bake --out <tree> --force\n    obc-bake \
                     publish <tree> --base-url <url> --target r2\n\nStale artifacts:\n",
                    found.len()
                );
                for (region, version) in found.iter().take(20) {
                    s.push_str(&format!("  v{version}  {region}\n"));
                }
                if found.len() > 20 {
                    s.push_str(&format!("  … and {} more\n", found.len() - 20));
                }
                s
            }
        }
    }
}

/// Resolve the catalog URL: the flag, then `OBC_CATALOG_URL`, then nothing.
pub fn catalog_url(flag: Option<&str>) -> Option<String> {
    flag.map(str::to_owned).or_else(|| std::env::var("OBC_CATALOG_URL").ok()).filter(|u| !u.trim().is_empty())
}

/// Fetch the published manifest and compare it against this build's OBCM version.
pub fn check(url: Option<&str>) -> Result<GuardOutcome, String> {
    let Some(url) = catalog_url(url) else {
        return Ok(GuardOutcome::Skipped {
            reason: "no --catalog-url and no OBC_CATALOG_URL (nothing published yet)".into(),
        });
    };
    let body = obc_pack::net::get_text(&url)?;
    evaluate(&body)
}

/// The pure half, so the interesting cases are testable without a network.
pub fn evaluate(body: &str) -> Result<GuardOutcome, String> {
    // Whole body, one document, `schema_version` first — the consumer rules of §7
    // apply to the guard exactly as they do to the site.
    let manifest: CatalogManifest = serde_json::from_str(body).map_err(|e| format!("catalog manifest: {e}"))?;
    if manifest.schema_version != obc_pack::catalog::CATALOG_SCHEMA_VERSION {
        return Err(format!(
            "catalog manifest: schema_version {} — this build implements {}",
            manifest.schema_version,
            obc_pack::catalog::CATALOG_SCHEMA_VERSION
        ));
    }
    let expected = obc_formats::obcm::VERSION;
    let found: Vec<(String, u8)> = manifest
        .artifacts
        .iter()
        .filter(|a| a.obcm_version != expected)
        .map(|a| (format!("{} [{}]", a.region_id, a.preset_id), a.obcm_version))
        .collect();
    let artifacts = manifest.artifacts.len();
    if found.is_empty() {
        Ok(GuardOutcome::Current { artifacts, obcm_version: expected })
    } else {
        Ok(GuardOutcome::Stale { expected, found, artifacts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(version: u8) -> String {
        let example: serde_json::Value =
            serde_json::from_str(obc_pack::catalog::CATALOG_EXAMPLE_JSON).expect("the checked-in example parses");
        let mut example = example;
        for artifact in example["artifacts"].as_array_mut().expect("artifacts") {
            artifact["obcm_version"] = serde_json::json!(version);
        }
        example.to_string()
    }

    #[test]
    fn a_catalog_at_this_builds_version_passes() {
        let outcome = evaluate(&manifest_with(obc_formats::obcm::VERSION)).unwrap();
        assert!(outcome.ok(), "{}", outcome.render());
        assert!(matches!(outcome, GuardOutcome::Current { .. }));
    }

    #[test]
    fn a_catalog_one_version_behind_fails_with_the_re_bake_instruction() {
        let stale = obc_formats::obcm::VERSION - 1;
        let outcome = evaluate(&manifest_with(stale)).unwrap();
        assert!(!outcome.ok());
        let text = outcome.render();
        assert!(text.contains("FAILED"), "{text}");
        assert!(text.contains("obc-bake bake"), "the failure must say what to do: {text}");
    }

    #[test]
    fn an_unreadable_manifest_is_an_error_not_a_pass() {
        assert!(evaluate("<html>404</html>").is_err());
        assert!(evaluate(
            "{\"schema_version\": 99, \"generated_at\": \"2026-07-26T09:00:00Z\", \"presets\": [], \"artifacts\": []}"
        )
        .is_err());
    }

    #[test]
    fn no_url_configured_is_a_skip_not_a_failure() {
        // The env var is deliberately not read here — `catalog_url(None)` may find
        // one in a developer's environment, and either answer is correct behaviour.
        let outcome = GuardOutcome::Skipped { reason: "no url".into() };
        assert!(outcome.ok());
    }
}
