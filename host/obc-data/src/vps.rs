//! The three planner services staged before a publication commit.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::engine::LayerFile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Routing,
    Search,
    Downloads,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::Search => "search",
            Self::Downloads => "downloads",
        }
    }
}

/// Actual host prerequisites. Absent interpreters cannot satisfy a runtime target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub triple: String,
    pub glibc: String,
    pub node: Option<String>,
    pub python: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub service: Service,
    pub id: String,
    pub slot: u8,
}

impl Installed {
    pub fn check(&self) -> Result<(), String> {
        if self.slot > 1
            || self.id.len() != 64
            || !self.id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid service slot or identity".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub host: Host,
    pub installed: Vec<Installed>,
}

/// A verified view of existing release metadata and objects, valid during `Vps::stage`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub installed: Installed,
    pub source: PathBuf,
    pub release: LayerFile,
    pub runtime: LayerFile,
    pub objects_url: String,
    pub site_origin: String,
}

/// Identities reported by the opened service data, rather than its requested configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "service", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ready {
    Routing { package: String },
    Search { grid: String, model: BTreeMap<String, String> },
    Downloads { catalog: String },
}

/// Staging starts and checks a candidate. It does not activate traffic or retire another slot.
pub trait Vps {
    fn inspect(&mut self) -> Result<State, String>;
    fn stage(&mut self, candidate: &Candidate) -> Result<(), String>;
    fn probe(&mut self, installed: &Installed) -> Result<Ready, String>;
}
