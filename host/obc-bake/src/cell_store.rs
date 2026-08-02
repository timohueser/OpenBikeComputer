use std::path::{Path, PathBuf};

use obc_pack::catalog::CellSource;
use obc_pack::grid::CellId;
use serde::{Deserialize, Serialize};

pub(crate) const ARTIFACT_EXT: &str = ".obcm";
pub(crate) const SIDECAR_EXT: &str = ".obcm.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct CellSidecar {
    pub(crate) schema_revision: u32,
    pub(crate) built_at: String,
    pub(crate) sources: Vec<CellSource>,
    pub(crate) partial: bool,
    /// The terrain revision this cell's nav ascents were integrated from
    /// (`OBCC_Spec.md` §13.4). Skipped when absent so a terrain-less tree's sidecars
    /// stay byte-identical to the ones baked before terrain existed — the field
    /// appearing would otherwise rewrite every sidecar in the store for nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terrain_revision: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CellState {
    pub(crate) pack_key: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) sidecar: CellSidecar,
}

pub(crate) fn paths(out: &Path, band: &str, id: CellId) -> (PathBuf, PathBuf, PathBuf) {
    let width = obc_pack::grid::id_width(id.log2);
    let dir = out.join("cells").join(band).join(format!("{:0width$}", id.i));
    let stem = format!("{:0width$}", id.j);
    (
        dir.join(format!("{stem}{ARTIFACT_EXT}")),
        dir.join(format!("{stem}{SIDECAR_EXT}")),
        dir.join(format!(".{stem}.cell.json")),
    )
}

pub(crate) fn read_current(out: &Path, band: &str, id: CellId, pack_key: &str) -> Result<Option<CellState>, String> {
    let (artifact, sidecar, state_path) = paths(out, band, id);
    let Ok(text) = std::fs::read_to_string(&state_path) else { return Ok(None) };
    let Ok(state) = serde_json::from_str::<CellState>(&text) else { return Ok(None) };
    if state.pack_key != pack_key || !artifact.is_file() || !sidecar.is_file() {
        return Ok(None);
    }
    let (bytes, sha256) = crate::hash::file(&artifact)?;
    Ok((bytes == state.bytes && sha256 == state.sha256).then_some(state))
}
