//! The products whose steps make the releases of `obc data`.

pub mod maps;
pub mod planner;

use obc_data::product::Product;

pub const PRODUCTS: &[&dyn Product] = &[&maps::Maps, &planner::Planner];

#[cfg(test)]
mod tests {
    #[test]
    fn the_plumbing_knows_the_prefix_of_each_product() {
        let prefixes: Vec<&str> = super::PRODUCTS.iter().map(|product| product.prefix()).collect();
        assert_eq!(prefixes, obc_data::live::PRODUCT_PREFIXES);
    }
}
