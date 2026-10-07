//! Local uses the saved environment, per-product pins and shared Plan/Run controls.

use super::*;
use crate::dev::{self, App as LocalApp, Request};
use std::time::Instant;

#[derive(Clone, Default)]
pub(super) struct View {
    pub env: Option<Edited>,
    pub optional: Vec<String>,
    pub plan: Option<crate::cli::EnvPlan>,
    pub request: Option<Request>,
    pub state: Option<dev::State>,
    pub checked: Option<Instant>,
    pub row: usize,
    pub app: Option<LocalApp>,
    pub logs: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Row {
    Region,
    Layer(String),
    App(LocalApp),
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Control {
    Start,
    Stop,
    Open,
    Logs,
}

impl View {
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Region];
        rows.extend(self.optional.iter().cloned().map(Row::Layer));
        rows.extend(LocalApp::ALL.into_iter().map(Row::App));
        rows.push(Row::Pending);
        rows
    }

    pub fn selected_app(&self) -> Option<LocalApp> {
        match self.rows().get(self.row) {
            Some(Row::App(app)) => Some(*app),
            _ => None,
        }
    }

    pub fn running(&self, app: LocalApp) -> bool {
        self.state
            .as_ref()
            .and_then(|state| state.apps.get(&app))
            .is_some_and(|state| matches!(state.status.as_str(), "ready" | "starting"))
    }

    pub fn request(&self, refresh_live: bool) -> Request {
        let app = self.selected_app().or(self.app).unwrap_or(LocalApp::WebPlanner);
        let reviewed = (!refresh_live)
            .then(|| {
                self.request.as_ref().filter(|request| request.app == app).and_then(|request| request.reviewed.clone())
            })
            .flatten();
        Request { region: None, refresh_live, app, inputs_only: false, reviewed }
    }
}

impl App {
    pub(super) fn current_env(&self) -> Option<&Edited> {
        if self.screen == Screen::Local {
            self.local.env.as_ref()
        } else {
            self.env.as_ref()
        }
    }

    pub(super) fn draw_local(&mut self, frame: &mut Frame, area: Rect) {
        let rows = self.local.rows();
        let mut lines = Vec::new();
        let mut at = Vec::new();
        for row in &rows {
            if matches!(row, Row::App(LocalApp::WebPlanner)) {
                let prepared =
                    self.local.state.as_ref().and_then(|state| state.region.as_deref()).unwrap_or("not started");
                lines.extend([Line::default(), Line::from(format!("APPS · prepared region {prepared}")).dim()]);
                if let (Some(state), Some(env)) = (&self.local.state, &self.local.env) {
                    if state.region.as_deref().is_some_and(|region| region != env.region) || state.layers != env.layers
                    {
                        lines.push(Line::from("Selected changes need preparation; running apps keep their view."));
                    }
                }
            }
            if matches!(row, Row::Pending) {
                lines.extend([Line::default(), Line::from("LOCAL BAKE · selected inputs").dim()]);
            }
            at.push(lines.len());
            match row {
                Row::Region => lines.push(Line::from(format!(
                    "region  {}",
                    self.local.env.as_ref().map_or("—", |env| env.region.as_str())
                ))),
                Row::Layer(name) => {
                    let on = self.local.env.as_ref().is_some_and(|env| env.layers.contains(name));
                    lines.push(Line::from(format!("[{}] {name}", if on { "x" } else { " " })));
                }
                Row::App(app) => {
                    let state = self.local.state.as_ref().and_then(|state| state.apps.get(app));
                    let status = state.map_or("stopped", |state| state.status.as_str());
                    lines.push(Line::from(format!("{:<13} {status}", app.name())));
                    if let Some(message) = state.and_then(|state| state.message.as_ref()) {
                        lines.push(Line::from(format!("  {message}")));
                    }
                    if status == "ready" {
                        if let Some(url) = app.url() {
                            lines.push(Line::from(format!("  {url}")));
                        }
                    }
                }
                Row::Pending => {
                    if let Some(plan) = &self.local.plan {
                        lines.push(Line::from(format!(
                            "{} · {} changes",
                            self.local.request(false).app.name(),
                            plan.groups.len()
                        )));
                        for blocked in &plan.blocked {
                            lines.push(Line::from(format!("  {}", blocked.reason)));
                        }
                        if plan.needs_prepare {
                            lines.push(Line::from("Check Live inputs to plan Local · network access"));
                        }
                    } else {
                        lines.push(Line::from("Local check pending"));
                    }
                }
            }
        }
        let checked = self
            .local
            .checked
            .map_or("not checked".into(), |time| format!("last check {}s ago", time.elapsed().as_secs()));
        lines.push(Line::from(format!("{checked} · preparation leaves stopped apps stopped")).dim());
        self.draw_lines(frame, area, Line::from("LOCAL").bold(), lines, &at, self.local.row);
    }

    pub(super) fn read_local(
        &mut self,
        root: &Path,
        products: &[&dyn Product],
        store: &Store,
        request: &Request,
    ) -> Result<(), Error> {
        let product = products
            .iter()
            .find(|product| product.name() == "planner")
            .ok_or_else(|| super::super::Code::Blocked.error("This worker has no Local planner"))?;
        let plan = product.dev_check(root, store, request)?;
        self.local.env = Some(Edited { env: "local".into(), region: plan.region.clone(), layers: plan.layers.clone() });
        self.local.optional =
            products.iter().flat_map(|product| product.optional()).map(|name| (*name).to_string()).collect();
        self.local.optional.sort();
        self.local.optional.dedup();
        self.local.plan = Some(plan);
        self.local.request = Some(request.clone());
        self.local.state = dev::state(store)?;
        self.local.checked = Some(Instant::now());
        Ok(())
    }
}

pub(super) fn start(root: &Path, store: &Store, request: Request) -> Result<crate::cli::operation_cli::Handle, Error> {
    crate::cli::operation_cli::start(
        root,
        store,
        crate::operation::Request {
            kind: crate::operation::Kind::DevPrepare,
            env: "local".into(),
            only: Vec::new(),
            moves: Vec::new(),
            plan: None,
            dev: Some(request),
        },
        None,
    )
}

pub(super) fn ensure_environment(root: &Path) -> Result<(), Error> {
    let file = crate::env::Env::path(root, "local");
    match std::fs::read(&file) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let (_, body) = crate::env::Env::local(root, &Regions::load(root)?)?;
            crate::store::write_atomic(&file, body.as_bytes()).map_err(Into::into)
        }
        Err(error) => Err(error.to_string().into()),
    }
}
