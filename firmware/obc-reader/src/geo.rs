//! Compatibility paths for neutral scene geometry now owned by `obc-map-scene`.

pub use obc_map_scene::{cos_lat, delta_m, ground_dist_m, ground_dist_m_cl};

#[inline]
pub fn seg_dist_m_cl(a: (i32, i32), b: (i32, i32), cl: f32) -> f32 {
    ground_dist_m_cl(a, b, cl)
}

#[inline]
pub fn seg_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    ground_dist_m(a, b)
}
