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
    pub service: Service,
    pub id: String,
    pub target: Host,
    pub expected: Ready,
    pub source: PathBuf,
    pub release: LayerFile,
    pub runtime: LayerFile,
    pub objects_url: String,
    pub site_origin: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub installed: Installed,
    pub candidate: Candidate,
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
    fn stage(&mut self, staged: &Stage) -> Result<(), String>;
    fn probe(&mut self, staged: &Stage) -> Result<Ready, String>;
}

/// The current publication supplies active slots. This operation does not activate them.
pub fn stage(candidates: &[Candidate], current: &[Installed], vps: &mut impl Vps) -> Result<Vec<Installed>, String> {
    use std::collections::BTreeSet;
    if candidates.len() != 3 || candidates.iter().map(|candidate| candidate.service).collect::<BTreeSet<_>>().len() != 3
    {
        return Err("staging needs all three planner services".into());
    }
    if current.iter().any(|installed| installed.check().is_err())
        || current.iter().map(|installed| installed.service).collect::<BTreeSet<_>>().len() != current.len()
    {
        return Err("conflicting current service slots".into());
    }
    let state = vps.inspect()?;
    if state.installed.iter().any(|installed| installed.check().is_err())
        || state.installed.iter().map(|installed| (installed.service, installed.slot)).collect::<BTreeSet<_>>().len()
            != state.installed.len()
        || current.iter().any(|active| {
            state
                .installed
                .iter()
                .any(|unit| unit.service == active.service && unit.slot == active.slot && unit.id != active.id)
        })
    {
        return Err("service slot ownership differs from the current publication".into());
    }
    for candidate in candidates {
        Installed { service: candidate.service, id: candidate.id.clone(), slot: 0 }.check()?;
        let target = &candidate.target;
        let version = |value: &str| value.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>().ok();
        if target.triple != state.host.triple
            || version(&state.host.glibc).zip(version(&target.glibc)).is_none_or(|(host, baseline)| host < baseline)
            || candidate.service != Service::Routing && (target.python.is_none() || target.python != state.host.python)
            || candidate.service == Service::Search && (target.node.is_none() || target.node != state.host.node)
        {
            return Err(format!("host prerequisites differ from the {} runtime target", candidate.service.name()));
        }
        if !matches!(
            (candidate.service, &candidate.expected),
            (Service::Routing, Ready::Routing { .. })
                | (Service::Search, Ready::Search { .. })
                | (Service::Downloads, Ready::Downloads { .. })
        ) {
            return Err("candidate readiness service differs".into());
        }
        if !current.iter().any(|unit| unit.service == candidate.service)
            && state.installed.iter().any(|unit| unit.service == candidate.service)
        {
            return Err(format!("{} has a slot with unknown publication ownership", candidate.service.name()));
        }
    }
    let mut result = Vec::new();
    for candidate in candidates {
        let active = current.iter().find(|unit| unit.service == candidate.service);
        let installed = state
            .installed
            .iter()
            .filter(|unit| unit.service == candidate.service && unit.id == candidate.id)
            .min_by_key(|unit| active != Some(*unit));
        if let Some(installed) = installed {
            if vps.probe(&Stage { installed: installed.clone(), candidate: candidate.clone() })? != candidate.expected {
                return Err(format!("installed {} reports different data", candidate.service.name()));
            }
            result.push(installed.clone());
            continue;
        }
        let installed = Installed {
            service: candidate.service,
            id: candidate.id.clone(),
            slot: active.map_or(0, |unit| 1 - unit.slot),
        };
        vps.stage(&Stage { installed: installed.clone(), candidate: candidate.clone() })?;
        if vps.probe(&Stage { installed: installed.clone(), candidate: candidate.clone() })? != candidate.expected {
            return Err(format!("candidate {} reports different data", candidate.service.name()));
        }
        result.push(installed);
    }
    Ok(result)
}
