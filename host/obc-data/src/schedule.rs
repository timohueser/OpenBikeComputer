//! One operator-owned live timer. Unit state, not a saved preference, admits publication.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use crate::operation::budget::{Budget, SLICE};
use crate::store::Store;

const TIMER: &str = "obc-data-live.timer";
const SERVICE: &str = "obc-data-live.service";

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct State {
    pub enabled: bool,
    pub calendar: Option<String>,
    pub time_zone: Option<String>,
    pub next: Option<String>,
    pub last_trigger: Option<String>,
}

struct Units {
    directory: PathBuf,
    root: PathBuf,
    store: PathBuf,
    environment: PathBuf,
    worker: PathBuf,
    budget: Budget,
}

fn live(env: &str) -> Result<(), String> {
    if env != "live" {
        return Err("installed schedules support live only; other environments build and verify manually".into());
    }
    if !cfg!(target_os = "linux") {
        return Err("live schedules need the configured Linux systemd host".into());
    }
    Ok(())
}

fn path(path: &Path) -> Result<String, String> {
    let value = path.to_str().ok_or("schedule paths need UTF-8")?;
    if !path.is_absolute() || value.bytes().any(|c| c.is_ascii_whitespace() || b"%\\\"$".contains(&c)) {
        return Err("schedule paths need absolute paths without whitespace or systemd expansions".into());
    }
    Ok(value.into())
}

fn properties(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    text.lines()
        .map(|line| {
            line.split_once('=')
                .map(|(key, value)| (key.into(), value.into()))
                .ok_or_else(|| "systemd returned an invalid property".into())
        })
        .collect()
}

impl Units {
    fn load(root: &Path, store: &Store) -> Result<Self, String> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or("configure the operator user unit directory")?;
        let units = Self {
            directory: config.join("systemd/user"),
            root: root.canonicalize().map_err(|e| e.to_string())?,
            store: store.root().canonicalize().map_err(|e| e.to_string())?,
            environment: crate::operation::launch::environment()?,
            worker: std::env::current_exe().map_err(|e| e.to_string())?.canonicalize().map_err(|e| e.to_string())?,
            budget: Budget::environment()?,
        };
        for location in [&units.directory, &units.root, &units.store, &units.environment, &units.worker] {
            path(location)?;
        }
        Ok(units)
    }

    fn service(&self, worker: &Path) -> Result<String, String> {
        Ok(format!("[Unit]\nDescription=OpenBikeComputer live refresh admission\nOnFailure={}\n[Service]\nType=oneshot\nWorkingDirectory={}\nEnvironmentFile={}\nExecStart=/usr/bin/env OBC_DATA_STORE={} OBC_RUN_ENV_FILE={} {} --json auto live\nSlice={}\nUMask=0077\nRestart=no\n",
            self.budget.alert, path(&self.root)?, path(&self.environment)?, path(&self.store)?,
            path(&self.environment)?, path(worker)?, SLICE))
    }

    fn timer(calendar: &str, zone: &str) -> Result<String, String> {
        if calendar.trim() != calendar
            || calendar.is_empty()
            || calendar.len() > 512
            || calendar.chars().any(char::is_control)
            || calendar.contains('%')
            || zone.is_empty()
            || !zone.bytes().all(|c| c.is_ascii_alphanumeric() || b"/_+-".contains(&c))
        {
            return Err("use a valid calendar and an explicit time-zone name".into());
        }
        Ok(format!("[Unit]\nDescription=OpenBikeComputer live refresh\n[Timer]\nOnCalendar={calendar} {zone}\nPersistent=true\nUnit={SERVICE}\n[Install]\nWantedBy=timers.target\n"))
    }

    fn inspect(&self, mut show: impl FnMut(&str) -> Result<BTreeMap<String, String>, String>) -> Result<State, String> {
        let timer = show(TIMER)?;
        if timer.get("LoadState").is_some_and(|value| value == "not-found") {
            return Ok(State { enabled: false, calendar: None, time_zone: None, next: None, last_trigger: None });
        }
        let loaded = |name: &str, values: &BTreeMap<String, String>| -> Result<String, String> {
            let file = self.directory.join(name);
            if values.get("LoadState").map(String::as_str) != Some("loaded")
                || values.get("FragmentPath") != Some(&path(&file)?)
                || values.get("DropInPaths").is_none_or(|value| !value.is_empty())
                || values.get("NeedDaemonReload").map(String::as_str) != Some("no")
            {
                return Err("installed schedule differs from the known operator units; inspect systemd setup".into());
            }
            std::fs::read_to_string(file).map_err(|e| e.to_string())
        };
        let body = loaded(TIMER, &timer)?;
        let value = body.lines().find_map(|line| line.strip_prefix("OnCalendar=")).ok_or("timer has no calendar")?;
        let (calendar, zone) = value.rsplit_once(' ').ok_or("timer has no explicit time zone")?;
        if body != Self::timer(calendar, zone)? || timer.get("Persistent").map(String::as_str) != Some("yes") {
            return Err("installed live timer has another calendar or catch-up policy".into());
        }
        let service = loaded(SERVICE, &show(SERVICE)?)?;
        let command = service.lines().find_map(|line| line.strip_prefix("ExecStart=")).ok_or("timer has no worker")?;
        let worker = command
            .strip_suffix(" --json auto live")
            .and_then(|line| line.rsplit_once(' ').map(|(_, worker)| worker))
            .ok_or("timer has another worker command")?;
        if service != self.service(Path::new(worker))?
            || crate::store::hash_file(Path::new(worker))? != crate::store::hash_file(&self.worker)?
        {
            return Err(
                "installed live timer targets another checkout, store or worker; review its schedule again".into()
            );
        }
        let field = |name: &str| timer.get(name).filter(|value| !value.is_empty() && value.as_str() != "n/a").cloned();
        Ok(State {
            enabled: timer.get("UnitFileState").map(String::as_str) == Some("enabled")
                && timer.get("ActiveState").map(String::as_str) == Some("active"),
            calendar: Some(calendar.into()),
            time_zone: Some(zone.into()),
            next: field("NextElapseUSecRealtime"),
            last_trigger: field("LastTriggerUSec"),
        })
    }
}
