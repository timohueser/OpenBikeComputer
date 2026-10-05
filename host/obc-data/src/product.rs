//! A product, such as the planner or the device maps: the steps that build its release for an
//! environment. The steps of a product live in the crate that makes their bytes; the `obc data`
//! binary passes each product to `cli::main`.

use crate::engine::plan::Fetch;
use crate::engine::Step;
use crate::env::Env;
use crate::regions::Regions;
use crate::store::Store;

pub trait Product {
    /// Kebab-case. Each of its layer names starts with `<name>/`.
    fn name(&self) -> &'static str;

    /// The optional layers that `layers` of an environment can switch on.
    fn optional(&self) -> &'static [&'static str] {
        &[]
    }

    /// Its steps for `env`. A step list that reads a snapshot, such as the `.poly` of a region or
    /// the Geofabrik index, gives `Unplanned::NeedsFetch` while the store lacks it.
    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned>;
}

/// Why a product gives no steps.
#[derive(Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// The step list reads these snapshots. `obc data` fetches them and asks once more.
    NeedsFetch(Vec<Fetch>),
    /// The environment does not give what the product needs, such as a pin.
    Invalid(String),
}
