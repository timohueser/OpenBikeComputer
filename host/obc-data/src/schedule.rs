//! One operator-owned live timer. Unit state, not a saved preference, admits publication.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

use crate::operation::budget::{Budget, SLICE};
use crate::store::Store;

const TIMER: &str = "obc-data-live.timer";
const SERVICE: &str = "obc-data-live.service";
const PREFLIGHT: &str = "/opt/obc-data/bin/obc-data-plumbing";

#[derive(Debug, Serialize, JsonSchema)]
pub struct State {
    pub enabled: bool,
    pub active: bool,
    pub runnable: bool,
    pub blocked: Option<String>,
    pub calendar: Option<String>,
    pub time_zone: Option<String>,
    pub next: Option<String>,
    pub last_trigger: Option<String>,
    pub last_run: Option<Box<crate::cli::operation_cli::View>>,
}

struct Units {
    directory: PathBuf,
    root: PathBuf,
    store: PathBuf,
    environment: PathBuf,
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
        let units = Self {
            directory: user_directory()?,
            root: root.canonicalize().map_err(|e| e.to_string())?,
            store: store.root().canonicalize().map_err(|e| e.to_string())?,
            environment: crate::operation::launch::environment()?,
            budget: Budget::environment()?,
        };
        for location in [&units.directory, &units.root, &units.store, &units.environment] {
            path(location)?;
        }
        Ok(units)
    }

    fn service(&self) -> Result<String, String> {
        Ok(format!("[Unit]\nDescription=OpenBikeComputer live refresh admission\nOnFailure={}\n[Service]\nType=oneshot\nWorkingDirectory={}\nEnvironmentFile={}\nExecStartPre={PREFLIGHT} bake-preflight {} {}\nExecStart=/usr/bin/env OBC_DATA_STORE={} OBC_RUN_ENV_FILE={} CARGO_NET_OFFLINE=true RUSTUP_AUTO_INSTALL=0 cargo run --quiet --locked --offline --manifest-path {} -p obc-data-steps --bin obc-data -- --json auto live\nSlice={}\nUMask=0077\nRestart=no\n",
            self.budget.alert, path(&self.root)?, path(&self.environment)?, path(&self.root)?, path(&self.store)?, path(&self.store)?,
            path(&self.environment)?, path(&self.root.join("Cargo.toml"))?, SLICE))
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
        let state = installed(&self.directory, &mut show)?;
        if let Some(reason) = &state.blocked {
            return Err(reason.clone());
        }
        if !state.enabled || !state.active {
            return Ok(state);
        }
        let service = loaded(&self.directory, SERVICE, &show(SERVICE)?)?;
        if service != self.service()? {
            return Err(
                "installed live timer targets another checkout, store or host setup; review its schedule again".into(),
            );
        }
        Ok(State { runnable: true, ..state })
    }
}

fn user_directory() -> Result<PathBuf, String> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|config| config.join("systemd/user"))
        .ok_or_else(|| "configure the operator user unit directory".into())
}

fn loaded(directory: &Path, name: &str, values: &BTreeMap<String, String>) -> Result<String, String> {
    let file = directory.join(name);
    if values.get("LoadState").map(String::as_str) != Some("loaded")
        || values.get("FragmentPath") != Some(&path(&file)?)
        || values.get("DropInPaths").is_none_or(|value| !value.is_empty())
        || values.get("NeedDaemonReload").map(String::as_str) != Some("no")
    {
        return Err("installed schedule differs from the known operator units; inspect systemd setup".into());
    }
    std::fs::read_to_string(file).map_err(|e| e.to_string())
}

fn installed(
    directory: &Path,
    show: &mut impl FnMut(&str) -> Result<BTreeMap<String, String>, String>,
) -> Result<State, String> {
    let timer = show(TIMER)?;
    let field = |name: &str| timer.get(name).filter(|value| !value.is_empty() && value.as_str() != "n/a").cloned();
    let mut state = State {
        enabled: timer.get("UnitFileState").map(String::as_str) == Some("enabled"),
        active: timer.get("ActiveState").map(String::as_str) == Some("active"),
        runnable: false,
        blocked: None,
        calendar: None,
        time_zone: None,
        next: field("NextElapseUSecRealtime"),
        last_trigger: field("LastTriggerUSec"),
        last_run: None,
    };
    if timer.get("LoadState").map(String::as_str) == Some("not-found") {
        return Ok(state);
    }
    let parsed = (|| -> Result<(), String> {
        let body = loaded(directory, TIMER, &timer)?;
        let value = body.lines().find_map(|line| line.strip_prefix("OnCalendar=")).ok_or("timer has no calendar")?;
        let (calendar, zone) = value.rsplit_once(' ').ok_or("timer has no explicit time zone")?;
        state.calendar = Some(calendar.into());
        state.time_zone = Some(zone.into());
        if body != Units::timer(calendar, zone)? || timer.get("Persistent").map(String::as_str) != Some("yes") {
            return Err("installed live timer has another calendar or catch-up policy".into());
        }
        Ok(())
    })();
    state.blocked = parsed.err();
    Ok(state)
}

fn command(args: &[&str]) -> Result<Vec<u8>, String> {
    crate::operation::launch::bounded_output(
        Command::new("systemctl").arg("--user").arg("--no-pager").args(args),
        Duration::from_secs(30),
    )
    .map_err(|reason| format!("systemd operator setup: {reason}"))
}

fn show(unit: &str) -> Result<BTreeMap<String, String>, String> {
    let (status, bytes) = crate::operation::launch::bounded_status(
        Command::new("systemctl").args(["--user", "--no-pager", "show", unit,
        "--property=LoadState,UnitFileState,ActiveState,FragmentPath,DropInPaths,NeedDaemonReload,Persistent,NextElapseUSecRealtime,LastTriggerUSec,ControlGroup"]), Duration::from_secs(30))?;
    let values = properties(&bytes)?;
    if !status.success() && values.get("LoadState").map(String::as_str) != Some("not-found") {
        return Err("systemd unit state could not be read; inspect operator setup".into());
    }
    Ok(values)
}

/// An enabled installed timer grants only the next checked publication handoff.
pub fn state(root: &Path, store: &Store, env: &str) -> Result<State, String> {
    live(env)?;
    let mut state = installed(&user_directory()?, &mut show)?;
    if state.enabled && state.blocked.is_none() {
        let admitted = (|| -> Result<bool, String> {
            let units = Units::load(root, store)?;
            let checked = units.inspect(show)?;
            if checked.runnable {
                budget_ready(&units.budget)?;
            }
            Ok(checked.runnable)
        })();
        state.runnable = admitted.as_ref().is_ok_and(|runnable| *runnable);
        state.blocked =
            admitted.err().map(|reason| format!("{reason}; update host setup or save the reviewed schedule again"));
    }
    if let Some(run) = crate::engine::runs::list(store)?.into_iter().find(|run| run.command == "auto live") {
        state.last_run =
            Some(Box::new(crate::cli::operation_cli::view(store, &run.id).map_err(|error| error.message)?));
    }
    Ok(state)
}

pub(crate) fn budget_ready(budget: &Budget) -> Result<(), String> {
    let actual = show(SLICE)?;
    if actual.get("ActiveState").map(String::as_str) != Some("active")
        || actual.get("DropInPaths").is_none_or(|value| !value.is_empty())
        || actual.get("NeedDaemonReload").map(String::as_str) != Some("no")
    {
        return Err("install and start the configured bake slice before running data work".into());
    }
    let file = actual.get("FragmentPath").ok_or("bake slice has no installed unit")?;
    if std::fs::read_to_string(file).map_err(|e| e.to_string())? != budget.unit() {
        return Err("the installed bake slice differs from the operator budget".into());
    }
    let group = actual
        .get("ControlGroup")
        .filter(|group| group.starts_with('/') && !group.split('/').any(|part| part == ".."))
        .ok_or("bake slice has no cgroup-v2 controller path")?;
    budget.verify_controllers(&Path::new("/sys/fs/cgroup").join(&group[1..]))
}

/// Validate all setup before replacing the known timer. This does not start a bake.
pub fn install(root: &Path, store: &Store, env: &str, calendar: &str, zone: &str) -> Result<State, String> {
    live(env)?;
    let units = Units::load(root, store)?;
    let timer = Units::timer(calendar, zone)?;
    crate::operation::launch::preflight()?;
    crate::operation::launch::bounded_output(
        Command::new("systemd-analyze").args(["calendar", &format!("{calendar} {zone}")]),
        Duration::from_secs(30),
    )
    .map_err(|reason| format!("validate the calendar and time zone with systemd-analyze: {reason}"))?;
    if show(&units.budget.alert)?.get("LoadState").map(String::as_str) != Some("loaded") {
        return Err("install the operator alert service before enabling the timer".into());
    }
    units.budget.disk(&units.root, 0)?;
    units.budget.disk(&units.store, 0)?;
    let output = crate::operation::launch::bounded_output(
        Command::new(PREFLIGHT).arg("bake-preflight").arg(&units.root).arg(&units.store),
        Duration::from_secs(30),
    )
    .map_err(|reason| format!("update the installed plumbing binary and host budget setup: {reason}"))?;
    if output != b"{\"ready\":true}\n" {
        return Err("installed plumbing binary does not support bake disk preflight".into());
    }
    let _admission =
        store.try_lock("schedule-live")?.ok_or("another schedule change or publication handoff is in progress")?;
    crate::commit::durable_directory(&units.directory)?;
    crate::commit::durable(&units.directory.join(SLICE), units.budget.unit().as_bytes())?;
    crate::commit::durable(&units.directory.join(SERVICE), units.service()?.as_bytes())?;
    crate::commit::durable(&units.directory.join(TIMER), timer.as_bytes())?;
    command(&["daemon-reload"])?;
    command(&["start", SLICE])?;
    budget_ready(&units.budget)?;
    command(&["enable", "--now", TIMER])?;
    let observed = units.inspect(show)?;
    if !observed.runnable {
        return Err("live timer was not enabled and active".into());
    }
    Ok(observed)
}

/// Disable only future admissions. A retained run completes build and verification.
pub fn disable(root: &Path, store: &Store, env: &str) -> Result<State, String> {
    live(env)?;
    let _ = root;
    let directory = user_directory()?;
    let _admission =
        store.try_lock("schedule-live")?.ok_or("another schedule change or publication handoff is in progress")?;
    let timer = show(TIMER)?;
    if timer.get("LoadState").map(String::as_str) == Some("not-found") {
        return installed(&directory, &mut show);
    }
    if timer.get("FragmentPath") != Some(&path(&directory.join(TIMER))?) {
        return Err("the timer is not in the configured operator unit directory".into());
    }
    command(&["disable", "--now", TIMER])?;
    let state = installed(&directory, &mut show)?;
    if state.enabled || show(TIMER)?.get("ActiveState").map(String::as_str) == Some("active") {
        return Err("live timer remains enabled or active".into());
    }
    Ok(state)
}

/// Serialize the first irreversible handoff with schedule disable, then release immediately.
pub(crate) fn handoff(root: &Path, store: &Store) -> Result<Option<crate::store::Lock>, String> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    gate(store, || {
        let units = Units::load(root, store)?;
        let state = units.inspect(show)?;
        if state.runnable {
            budget_ready(&units.budget)?;
        }
        Ok(state.runnable)
    })
}

fn gate(store: &Store, enabled: impl FnOnce() -> Result<bool, String>) -> Result<Option<crate::store::Lock>, String> {
    let lock =
        store.try_lock("schedule-live")?.ok_or("a schedule change is in progress; no publication handoff started")?;
    if !enabled()? {
        return Ok(None);
    }
    Ok(Some(lock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_timer_state_binds_calendar_launcher_context_and_persistent_policy() {
        let scratch = crate::store::tests::Scratch::new("live-timer-state");
        let units = Units {
            directory: scratch.0.clone(),
            root: "/checkout".into(),
            store: "/store".into(),
            environment: "/operator.env".into(),
            budget: Budget {
                cpu_percent: 100,
                memory_bytes: 1024,
                minimum_free: 512,
                alert: "operator-alert.service".into(),
            },
        };
        std::fs::write(units.directory.join(TIMER), Units::timer("weekly", "Europe/Berlin").unwrap()).unwrap();
        std::fs::write(units.directory.join(SERVICE), units.service().unwrap()).unwrap();
        let show = |name: &str| {
            Ok(BTreeMap::from([
                ("LoadState".into(), "loaded".into()),
                ("FragmentPath".into(), path(&units.directory.join(name)).unwrap()),
                ("DropInPaths".into(), String::new()),
                ("NeedDaemonReload".into(), "no".into()),
                ("Persistent".into(), "yes".into()),
                ("UnitFileState".into(), "enabled".into()),
                ("ActiveState".into(), "active".into()),
                ("NextElapseUSecRealtime".into(), "next calendar occurrence".into()),
                ("LastTriggerUSec".into(), "previous occurrence".into()),
            ]))
        };
        let active = units.inspect(show).unwrap();
        assert!(active.enabled);
        assert_eq!(active.calendar.as_deref(), Some("weekly"));
        assert_eq!(active.time_zone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(active.next.as_deref(), Some("next calendar occurrence"));
        let disabled = units
            .inspect(|name| {
                let mut values = show(name)?;
                values.insert("ActiveState".into(), "inactive".into());
                Ok(values)
            })
            .unwrap();
        assert!(disabled.enabled);
        assert!(!disabled.active && !disabled.runnable);
        assert!(units
            .inspect(|name| {
                let mut values = show(name)?;
                values.insert("DropInPaths".into(), "/another.override".into());
                Ok(values)
            })
            .is_err());
        assert!(units
            .inspect(|name| {
                let mut values = show(name)?;
                values.insert("NeedDaemonReload".into(), "yes".into());
                Ok(values)
            })
            .is_err());
        let original = units.service().unwrap();
        std::fs::write(
            units.directory.join(SERVICE),
            original.replace("OBC_DATA_STORE=/store", "OBC_DATA_STORE=/another-store"),
        )
        .unwrap();
        assert!(units.inspect(show).unwrap_err().contains("another checkout"));
        let timer = std::fs::read(units.directory.join(TIMER)).unwrap();
        assert!(Units::timer("weekly\nUnit=another.service", "UTC").is_err());
        assert!(Units::timer("weekly", "UTC\nUnit=another.service").is_err());
        assert_eq!(std::fs::read(units.directory.join(TIMER)).unwrap(), timer);
        assert!(live("local").unwrap_err().contains("live only"));
    }

    #[test]
    fn disable_consent_leaves_active_work_and_handoff_is_serialized_without_a_queue() {
        let scratch = crate::store::tests::Scratch::new("live-timer-handoff");
        let store = Store::at(&scratch.0);
        let mut run = crate::engine::runs::Run::create(&store, "auto live").unwrap();
        run.record(&crate::engine::runs::Event::Phase { phase: crate::engine::runs::Phase::Build }).unwrap();
        run.sync().unwrap();
        let before = std::fs::read(store.run(run.id())).unwrap();
        assert!(gate(&store, || Ok(false)).unwrap().is_none());
        assert!(store.is_locked(&format!("run-{}", run.id())).unwrap(), "disable cannot stop current work");
        assert_eq!(std::fs::read(store.run(run.id())).unwrap(), before);
        let handoff = gate(&store, || Ok(true)).unwrap().unwrap();
        assert!(store.try_lock("schedule-live").unwrap().is_none(), "disable serializes with the first handoff");
        assert!(gate(&store, || Ok(true)).err().unwrap().contains("in progress"));
        drop(handoff);
        run.record(&crate::engine::runs::Event::Phase { phase: crate::engine::runs::Phase::Verify }).unwrap();
        run.finish(None).unwrap();
        assert_eq!(crate::engine::runs::list(&store).unwrap()[0].outcome, crate::engine::runs::Outcome::Ok);
    }
}
