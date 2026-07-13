//! Compatibility paths for the normative OBCM POI id table now owned by `obc-formats`.
//!
//! Kept source-compatible through FAR; remove these aliases in the #812 final audit after all
//! downstream callers use `obc_formats::obcm` directly.

pub use obc_formats::obcm::{
    poi_category_of as category_of, poi_label_of as label_of, poi_subtype_row as subtype_row, PoiCategory, PoiSubtype,
    POI_SUBTYPES as SUBTYPES,
};
