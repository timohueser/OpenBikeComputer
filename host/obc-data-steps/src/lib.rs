//! The products whose steps make the releases of `obc data`.

pub mod maps;
pub mod planner;

use obc_data::product::Product;

pub const PRODUCTS: &[&dyn Product] = &[&maps::Maps, &planner::Planner];
