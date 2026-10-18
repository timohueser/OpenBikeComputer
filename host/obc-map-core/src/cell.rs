//! The output layout of a cell and the band facts that the cutter and the step list share.

use crate::grid::{id_width, Band, CellId};

pub fn cell_path(band: &Band, cell: &CellId) -> String {
    let w = id_width(cell.log2);
    format!("cells/{}/{:0w$}/{:0w$}.obcm", band.id, cell.i, cell.j, w = w)
}

/// Whether a selected drawing level can carry generated contour geometry.
pub fn has_contours(config: &crate::config::Config, band: &Band) -> bool {
    let classes = [crate::config::ContourClass::Major, crate::config::ContourClass::Index];
    let min_lod = classes.into_iter().filter_map(|class| config.contour_style(class)).map(|style| style.min_lod).min();
    config.contours.enabled && min_lod.is_some_and(|min| band.lods.iter().any(|&lod| lod >= min))
}
