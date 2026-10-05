//! `obc data` with the products whose steps make the releases.

use std::process::ExitCode;

use obc_data::product::Product;

const PRODUCTS: &[&dyn Product] = &[];

fn main() -> ExitCode {
    obc_data::cli::main(PRODUCTS)
}
