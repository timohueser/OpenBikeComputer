//! Compatibility paths for normative OBCM flags and sentinels now owned by `obc-formats`.
//! Remove in the #812 final audit.

pub use obc_formats::obcm::{
    BRANCH_BIT, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, STYLE_DASHED_BIT,
    STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK,
};
