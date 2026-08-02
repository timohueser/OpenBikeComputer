//! The mandatory re-bake guard: is the *published* catalog still readable?
//!
//! `OBCC_Spec.md` §10 states the law — an OBCM format bump invalidates every baked
//! cell — and the generator already refuses a mixed or stale tree. That protects the
//! repository and the tree on the bake box.
//!
//! Neither can see the thing that actually matters to a rider: **what is on the CDN
//! right now**. A format bump can be merged, tested, and released with the pin
//! moved and the re-bake honestly intended — and the published catalog still serves
//! old cells to newer firmware for as long as nobody re-runs the bake. That is
//! the gap this module closes, and it is why the check lives here rather than in
//! `obc-pack`: it is a fact about a deployment, not about a tree, so it needs a URL
//! and a network — neither of which belongs in the packer's own test suite.
//!
//! It fails **loudly and skips gracefully**: with no catalog URL configured there is
//! nothing to check and the guard says so and succeeds, because a project that has
//! not published a catalog yet must not have a red CI check about it. Wired into
//! `.github/workflows/bake.yml`, where `vars.OBC_CATALOG_URL` supplies the URL.

use obc_pack::catalog::Catalog;

/// What the guard found.
#[derive(Debug, Clone)]
pub enum GuardOutcome {
    /// No URL configured — nothing to check.
    Skipped { reason: String },
    /// Every published cell matches this build's OBCM version.
    Current { cells: usize, obcm_version: u8 },
    /// Cells this firmware cannot read are being served right now.
    Stale { expected: u8, found: Vec<(String, u8)>, cells: usize },
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
            GuardOutcome::Current { cells, obcm_version } => {
                format!("obcm-version guard: {cells} published cells, all OBCM v{obcm_version} — current")
            }
            GuardOutcome::Stale { expected, found, cells } => {
                let mut s = format!(
                    "obcm-version guard: FAILED — this build writes OBCM v{expected}, but {} of {cells} \
                     published cells are a different version.\n\nAn OBCM bump invalidates every baked cell \
                     (OBCC_Spec.md §10): every assembly from the catalog is unreadable to this firmware until the bakery \
                     re-runs. Re-bake and re-publish:\n\n    obc-bake bake --out <tree> --force\n    obc-bake \
                     publish <tree> --base-url <url> --target r2\n\nStale schemas:\n",
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
    let root: Catalog = serde_json::from_str(body).map_err(|e| format!("catalog: {e}"))?;
    if root.schema_version != obc_pack::catalog::CATALOG_SCHEMA_VERSION {
        return Err(format!(
            "catalog: schema_version {} — this build implements {}",
            root.schema_version,
            obc_pack::catalog::CATALOG_SCHEMA_VERSION
        ));
    }
    let expected = obc_formats::obcm::VERSION;
    let cells: usize = root.cell_index.iter().map(|band| band.cell_count as usize).sum();
    if root.schema.obcm_version == expected {
        Ok(GuardOutcome::Current { cells, obcm_version: expected })
    } else {
        let found = vec![(
            format!("schema `{}` revision {} ({cells} cells)", root.schema.id, root.schema.revision),
            root.schema.obcm_version,
        )];
        Ok(GuardOutcome::Stale { expected, found, cells })
    }
}

// --- the cell-store lockstep guard (OBCA §6.3) ------------------------------------

/// What the cell-store guard found in a bake tree.
#[derive(Debug, Clone, Default)]
pub struct CellStoreOutcome {
    pub cells: usize,
    pub revision: u32,
    pub partial: Vec<String>,
    /// What the terrain track holds, when the tree publishes one (§13).
    pub terrain: Option<TerrainStoreOutcome>,
    /// Every violation, named. Empty means the store is in lockstep.
    pub problems: Vec<String>,
}

/// What the terrain half of the guard found — reported separately because it *is* a
/// separate store, with a separate revision, that a separate command bakes.
#[derive(Debug, Clone, Default)]
pub struct TerrainStoreOutcome {
    pub cells: usize,
    pub known_empty: u32,
    pub dataset: String,
    pub revision: u32,
    /// The terrain revision the OBCM cells recorded sampling, if any (§13.4).
    pub network_terrain_revision: Option<u32>,
}

impl CellStoreOutcome {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        if self.problems.is_empty() {
            let _ = writeln!(
                s,
                "cell-store guard: {} cells, all at schema revision {} and OBCM v{} — lockstep",
                self.cells,
                self.revision,
                obc_formats::obcm::VERSION
            );
            if !self.partial.is_empty() {
                let _ = writeln!(
                    s,
                    "  {} cell(s) are `partial` — co-bake the neighbouring extract to complete them \
                     (OBCA_Spec.md §3.7):",
                    self.partial.len()
                );
                for id in self.partial.iter().take(10) {
                    let _ = writeln!(s, "    {id}");
                }
                if self.partial.len() > 10 {
                    let _ = writeln!(s, "    … and {} more", self.partial.len() - 10);
                }
            }
            s.push_str(&self.render_terrain());
            return s;
        }
        let _ = writeln!(
            s,
            "cell-store guard: FAILED — {} problem(s).\n\nA cell store is lockstep: every cell in an assembly must \
             share one OBCM version and one schema revision, because assembly copies chunk bytes between files and \
             that is only meaningful within one revision (OBCA_Spec.md §5, §6.3). Re-bake the store:\n\n    \
             obc-bake bake --out <tree> --base-url <url> --force <region…>\n",
            self.problems.len()
        );
        for p in self.problems.iter().take(30) {
            let _ = writeln!(s, "  {p}");
        }
        if self.problems.len() > 30 {
            let _ = writeln!(s, "  … and {} more", self.problems.len() - 30);
        }
        s.push_str(&self.render_terrain());
        s
    }

    fn render_terrain(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let Some(t) = &self.terrain else {
            return s;
        };
        let _ = writeln!(
            s,
            "terrain guard: {} cell(s) + {} known-empty, {} at terrain revision {} — its own lockstep, unaffected by \
             the OBCM store's",
            t.cells, t.known_empty, t.dataset, t.revision
        );
        let _ = writeln!(
            s,
            "  the cell store's nav ascents were baked against terrain revision {}",
            t.network_terrain_revision.map_or("none".to_string(), |r| r.to_string())
        );
        s
    }
}

/// Check a **cell bake tree** for the two things that make a store assemblable, and
/// for the one thing D3 says a bake must never do.
///
/// - **Schema-revision lockstep.** Every cell sidecar states the revision it was cut
///   at; they must all agree with each other *and* with `schema.json`'s `_meta`. A
///   mixed store is not a store that is partly stale — it is one whose cells cannot be
///   grafted into a single file at all, so this is fatal with no override
///   (`OBCC_Spec.md` §10).
/// - **OBCM lockstep.** Every cell's own header must carry this build's OBCM version.
/// - **No silent downgrade (D3).** A cell whose recorded state says it was canonical
///   but whose published sidecar now says `partial` means a narrower bake overwrote a
///   covering one — the exact failure `OBCA_Spec.md` §3.7 forbids, and the reason
///   [`crate::cells`] refuses it at install time. Checking it again here catches a
///   store assembled by hand or merged from two machines.
///
/// Runs over a tree rather than a URL — unlike [`check`], which is about a deployment
/// — so it needs no network and is what a bake box runs before it publishes.
pub fn check_cell_store(tree: &std::path::Path) -> Result<CellStoreOutcome, String> {
    #[derive(serde::Deserialize)]
    struct Sidecar {
        schema_revision: u32,
        partial: bool,
        #[serde(default)]
        terrain_revision: Option<u32>,
    }
    #[derive(serde::Deserialize)]
    struct State {
        sidecar: Sidecar,
    }
    #[derive(serde::Deserialize)]
    struct SchemaMeta {
        revision: u32,
    }
    #[derive(serde::Deserialize)]
    struct SchemaDoc {
        #[serde(rename = "_meta")]
        meta: SchemaMeta,
    }

    let schema_path = tree.join("schema.json");
    let schema: SchemaDoc = serde_json::from_str(
        &std::fs::read_to_string(&schema_path).map_err(|e| format!("{}: {e}", schema_path.display()))?,
    )
    .map_err(|e| format!("{}: {e}", schema_path.display()))?;

    let mut out = CellStoreOutcome { revision: schema.meta.revision, ..Default::default() };
    let cells_root = tree.join("cells");
    if !cells_root.is_dir() {
        return Err(format!("{}: no `cells/` directory — is this a cell bake tree?", cells_root.display()));
    }
    // Read the terrain declaration before the cells, so the per-cell `terrain_revision` has
    // something to be checked against as it is read.
    let terrain_doc = match tree.join(crate::terrain::TERRAIN_DOC).is_file() {
        true => Some(crate::terrain::read_terrain_doc(&tree.join(crate::terrain::TERRAIN_DOC))?),
        false => None,
    };
    let mut baked_against: Option<Option<u32>> = None;
    for band_dir in sorted_dir(&cells_root)? {
        if !band_dir.is_dir() || name_of(&band_dir).starts_with('.') {
            continue;
        }
        if name_of(&band_dir) == crate::terrain::TERRAIN_DIR {
            // The other artifact class, on the other revision track. Checked below, against
            // `terrain.json` — never against `schema.json`, which is the whole point.
            continue;
        }
        for i_dir in sorted_dir(&band_dir)? {
            if !i_dir.is_dir() || name_of(&i_dir).starts_with('.') {
                continue;
            }
            for path in sorted_dir(&i_dir)? {
                let name = name_of(&path);
                if name.starts_with('.') || !name.ends_with(".obcm") {
                    continue;
                }
                out.cells += 1;
                let sidecar_path = path.with_file_name(format!("{name}.json"));
                let sidecar: Sidecar =
                    match std::fs::read_to_string(&sidecar_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
                        Some(s) => s,
                        None => {
                            out.problems.push(format!("{}: no readable sidecar", sidecar_path.display()));
                            continue;
                        }
                    };
                if sidecar.schema_revision != schema.meta.revision {
                    out.problems.push(format!(
                        "{}: cut at schema revision {} but the store is revision {}",
                        path.display(),
                        sidecar.schema_revision,
                        schema.meta.revision
                    ));
                }
                // §13.4: the store as a whole sampled one terrain revision, or none.
                match baked_against {
                    None => baked_against = Some(sidecar.terrain_revision),
                    Some(have) if have != sidecar.terrain_revision => out.problems.push(format!(
                        "{}: baked against terrain revision {} but the store was baked against {} — half the nav \
                         graph's ascents would come from a different raster than the other half",
                        path.display(),
                        name_revision(sidecar.terrain_revision),
                        name_revision(have)
                    )),
                    Some(_) => {}
                }
                match crate::verify::header_of(&path) {
                    Ok((version, _)) if version != obc_formats::obcm::VERSION => out.problems.push(format!(
                        "{}: OBCM v{version}, this build writes v{}",
                        path.display(),
                        obc_formats::obcm::VERSION
                    )),
                    Ok(_) => {}
                    Err(e) => out.problems.push(e),
                }
                if sidecar.partial {
                    let id = format!("{}/{}/{}", name_of(&band_dir), name_of(&i_dir), name.trim_end_matches(".obcm"));
                    out.partial.push(id);
                    // D3: was this square covered before? Then a narrower bake took
                    // coverage away, which no run is allowed to do.
                    let state_path = path.with_file_name(format!(".{}.cell.json", name.trim_end_matches(".obcm")));
                    if let Some(state) =
                        std::fs::read_to_string(&state_path).ok().and_then(|t| serde_json::from_str::<State>(&t).ok())
                    {
                        if !state.sidecar.partial {
                            out.problems.push(format!(
                                "{}: published as `partial`, but this tree recorded it as canonical — a covering bake \
                                 existed and was replaced by a narrower one (OBCA_Spec.md §3.7)",
                                path.display()
                            ));
                        }
                    }
                }
            }
        }
    }
    if out.cells == 0 {
        return Err(format!("{}: no cells — refusing to call an empty store lockstep", cells_root.display()));
    }
    out.terrain = check_terrain_store(tree, terrain_doc, baked_against.unwrap_or(None), &mut out.problems)?;
    Ok(out)
}

fn name_revision(revision: Option<u32>) -> String {
    revision.map_or("none".to_string(), |r| r.to_string())
}

/// The terrain track's own lockstep, and the one coupling to the OBCM store (§13.2, §13.4).
///
/// Two separate statements, and it matters that they are separate:
///
/// - **Terrain lockstep** is `(dataset_version, posting, cell size, terrain_revision)` and nothing
///   else. Not the OBCM version, not the schema revision — so a schema bump can never make this
///   half of the guard fail, which is exactly the property epic #1068 needs from the catalog.
/// - **The coupling** runs one way. The network band's cells sample OBCT at bake time, so their
///   `Ascent M` values are a function of a particular terrain revision. When the terrain is
///   re-baked and those cells are not, the router's numbers and the drawn profile stop being the
///   same surface — silently, because every file still parses. That is what this names, loudly,
///   *before* a publish rather than after one.
fn check_terrain_store(
    tree: &std::path::Path,
    doc: Option<crate::terrain::TerrainDoc>,
    network_terrain_revision: Option<u32>,
    problems: &mut Vec<String>,
) -> Result<Option<TerrainStoreOutcome>, String> {
    #[derive(serde::Deserialize)]
    struct TerrainSidecar {
        terrain_revision: u32,
        dataset_version: String,
    }
    #[derive(serde::Deserialize)]
    struct KnownEmpty {
        terrain_revision: u32,
        known_empty: Vec<serde_json::Value>,
    }

    let dir = tree.join("cells").join(crate::terrain::TERRAIN_DIR);
    let Some(doc) = doc else {
        if dir.is_dir() {
            problems.push(format!(
                "{}: terrain cells with no `{}` — a terrain cell is only interpretable beside the dataset, pairing \
                 and revision it was baked at (OBCC_Spec.md §13.1)",
                dir.display(),
                crate::terrain::TERRAIN_DOC
            ));
        }
        if let Some(revision) = network_terrain_revision {
            problems.push(format!(
                "the cell store's nav ascents were baked against terrain revision {revision}, but this tree publishes \
                 no terrain — the raster the router's numbers came from would not ship with them"
            ));
        }
        return Ok(None);
    };

    let mut outcome = TerrainStoreOutcome {
        dataset: format!("{} {}", doc.dataset_id, doc.dataset_version),
        revision: doc.revision,
        network_terrain_revision,
        ..Default::default()
    };
    if dir.is_dir() {
        for i_dir in sorted_dir(&dir)? {
            if !i_dir.is_dir() || name_of(&i_dir).starts_with('.') {
                continue;
            }
            for path in sorted_dir(&i_dir)? {
                let name = name_of(&path);
                if name.starts_with('.') || !name.ends_with(".obcd") {
                    continue;
                }
                outcome.cells += 1;
                let sidecar_path = path.with_file_name(format!("{name}.json"));
                let sidecar: TerrainSidecar =
                    match std::fs::read_to_string(&sidecar_path).ok().and_then(|t| serde_json::from_str(&t).ok()) {
                        Some(s) => s,
                        None => {
                            problems.push(format!("{}: no readable terrain sidecar", sidecar_path.display()));
                            continue;
                        }
                    };
                if sidecar.terrain_revision != doc.revision {
                    problems.push(format!(
                        "{}: baked at terrain revision {} but the terrain store is revision {} — one raster per \
                         revision (OBCC_Spec.md §13.2)",
                        path.display(),
                        sidecar.terrain_revision,
                        doc.revision
                    ));
                }
                if sidecar.dataset_version != doc.dataset_version {
                    problems.push(format!(
                        "{}: baked from dataset version `{}` but the terrain store is `{}` — a mixed-dataset raster \
                         has a discontinuity at every seam between the two",
                        path.display(),
                        sidecar.dataset_version,
                        doc.dataset_version
                    ));
                }
            }
        }
    }
    let empty_path = dir.join(".known-empty.json");
    if empty_path.is_file() {
        match std::fs::read_to_string(&empty_path).ok().and_then(|t| serde_json::from_str::<KnownEmpty>(&t).ok()) {
            Some(state) if state.terrain_revision != doc.revision => problems.push(format!(
                "{}: known-empty state is terrain revision {} but the store is revision {}",
                empty_path.display(),
                state.terrain_revision,
                doc.revision
            )),
            Some(state) => outcome.known_empty = state.known_empty.len() as u32,
            None => problems.push(format!("{}: unreadable known-empty state", empty_path.display())),
        }
    }
    if outcome.cells == 0 && outcome.known_empty == 0 {
        problems.push(format!(
            "{}: `{}` declares a terrain store with no cells and no known-empty coverage",
            tree.display(),
            crate::terrain::TERRAIN_DOC
        ));
    }

    // §13.4 — the one coupling, and the only place in this guard where the two tracks meet.
    if network_terrain_revision != Some(doc.revision) {
        problems.push(format!(
            "STALE network band: its cells' nav ascents were baked against terrain revision {}, but this tree \
             publishes terrain revision {}. The router's baked ascents and the published raster are two different \
             surfaces (OBCC_Spec.md §13.4) — re-bake the cells against the current terrain:\n      obc-bake bake \
             --out <tree> --force <region…>",
            name_revision(network_terrain_revision),
            doc.revision
        ));
    }
    Ok(Some(outcome))
}

fn name_of(path: &std::path::Path) -> String {
    path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

fn sorted_dir(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("{}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    entries.sort();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_with(version: u8) -> String {
        let example: serde_json::Value =
            serde_json::from_str(obc_pack::catalog::CATALOG_EXAMPLE_JSON).expect("the checked-in example parses");
        let mut example = example;
        example["schema"]["obcm_version"] = serde_json::json!(version);
        example.to_string()
    }

    #[test]
    fn a_catalog_at_this_builds_version_passes() {
        let outcome = evaluate(&catalog_with(obc_formats::obcm::VERSION)).unwrap();
        assert!(outcome.ok(), "{}", outcome.render());
        assert!(matches!(outcome, GuardOutcome::Current { .. }));
    }

    #[test]
    fn a_catalog_one_version_behind_fails_with_the_re_bake_instruction() {
        let stale = obc_formats::obcm::VERSION - 1;
        let outcome = evaluate(&catalog_with(stale)).unwrap();
        assert!(!outcome.ok());
        let text = outcome.render();
        assert!(text.contains("FAILED"), "{text}");
        assert!(text.contains("obc-bake bake"), "the failure must say what to do: {text}");
    }

    #[test]
    fn an_unreadable_manifest_is_an_error_not_a_pass() {
        assert!(evaluate("<html>404</html>").is_err());
        assert!(evaluate(
            "{\"schema_version\": 99, \"generated_at\": \"2026-07-26T09:00:00Z\", \"schema\": {}, \"skins\": [], \"regions\": [], \"cell_index\": []}"
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
