//! One finite typed task. Its completion cannot replace navigation, filters or draft inputs.

use super::{list_runs, App, Effect, Error, PlanView, Screen, Tui, LIVE, NO_PLAN, TICK};
use crate::cli::Code;
use crate::cli::{build_cli::plan_live_moves, clean, clean_plan, edit_cli, policy, regions_cli};
use crate::fetch::http::Http;
use crate::regions::Regions;
use crate::{product::Product, store::Store};
use ratatui::{
    crossterm::{
        event::{self, Event, KeyEventKind, MouseButton, MouseEventKind},
        execute, terminal,
    },
    layout::Rect,
    text::Line,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

type Task<'a> = std::thread::ScopedJoinHandle<'a, (Effect, Option<App>, Result<(), Error>)>;

type ViewContext = (
    Result<crate::cli::edit_cli::Edited, String>,
    Result<Vec<crate::sources::Source>, String>,
    Result<Vec<crate::regions::Region>, String>,
    Result<Option<String>, String>,
    Result<Vec<(String, String)>, String>,
);

fn context(root: &Path, store: &Store) -> ViewContext {
    (
        crate::cli::edit_cli::current(root, super::LIVE).map_err(|error| error.message),
        crate::cli::registry(root).map(|registry| registry.sources).map_err(|error| error.message),
        crate::regions::Regions::load(root).map(|regions| regions.iter().cloned().collect()),
        match std::fs::read_to_string(crate::env::Env::path(root, "local")) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        },
        crate::local::saved(store).map(|saved| {
            saved.into_iter().map(|saved| (saved.original.product.clone(), saved.original.id())).collect()
        }),
    )
}

pub(super) fn run_loop(
    root: &Path,
    products: &[&dyn Product],
    store: &Store,
    app: &mut App,
    tui: &mut Tui,
) -> Result<(), Error> {
    let io = |error: std::io::Error| error.to_string();
    std::thread::scope(|scope| {
        let mut task: Option<Task<'_>> = None;
        let mut effect = Effect::Initial;
        let mut finishing = false;
        let mut read = Instant::now();
        loop {
            if task.as_ref().is_some_and(|task| task.is_finished()) {
                let (effect, updated, result) = task.take().unwrap().join().map_err(|_| {
                    Code::Failed.error("The background check or edit stopped unexpectedly; restart obc data.")
                })?;
                let notice = result.err().or(updated.as_ref().and_then(|app| app.notice.clone()));
                if let Some(updated) = updated {
                    app.complete(effect.clone(), updated);
                }
                if notice.is_some() && matches!(effect, Effect::Select(_)) {
                    if let Some(view) = &mut app.plan {
                        view.restore_selection();
                    }
                }
                if notice.is_some() {
                    app.notice = notice;
                }
                app.busy = false;
                // Fetch progress can write over the terminal. The admitted task has ended.
                execute!(std::io::stdout(), terminal::Clear(terminal::ClearType::All)).map_err(io)?;
                tui.swap_buffers();
            }
            if finishing && !app.busy {
                return Ok(());
            }
            match effect {
                Effect::None => {}
                Effect::Quit | Effect::Reload => finishing = true,
                next if !app.busy => {
                    app.busy = true;
                    let mut updated = app.clone();
                    updated.notice = None;
                    if matches!(next, Effect::LocalCheck(_) | Effect::LocalReview(_)) {
                        updated.local.checked = Some(Instant::now());
                    }
                    task = Some(scope.spawn(move || {
                        let observes = matches!(
                            next,
                            Effect::Initial
                                | Effect::CheckNow
                                | Effect::Status { .. }
                                | Effect::Areas
                                | Effect::LoadAreas
                                | Effect::ReviewRegionDeletion(_)
                                | Effect::Plan
                                | Effect::Select(_)
                                | Effect::PlanClean
                                | Effect::LocalCheck(_)
                                | Effect::LocalReview(_)
                        );
                        let before = observes.then(|| context(root, store));
                        let result = perform(root, products, store, &mut updated, next.clone());
                        if before.is_some_and(|before| before != context(root, store)) {
                            return (
                                next,
                                None,
                                Err(Code::PlanOutdated
                                    .error("The live region, sources or saved coverage changed during this check.")
                                    .fix("Run the check again. The previous rows and your inputs stay unchanged.")),
                            );
                        }
                        (next, Some(updated), result)
                    }));
                }
                _ => {}
            }
            tui.draw(|frame| {
                app.draw(frame);
                if finishing {
                    let area = Rect { y: frame.area().bottom().saturating_sub(1), height: 1, ..frame.area() };
                    frame.render_widget(
                        Line::from(if app.reload {
                            "Finishing the admitted check or edit before reloading current code…"
                        } else {
                            "Finishing the admitted check or edit before quitting…"
                        }),
                        area,
                    );
                }
            })
            .map_err(io)?;
            effect = Effect::None;
            if event::poll(Duration::from_millis(50)).map_err(io)? {
                effect = match event::read().map_err(io)? {
                    Event::Key(key) if !finishing && key.kind == KeyEventKind::Press => app.key(key.code),
                    Event::Mouse(mouse) if !finishing && mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                        app.click(mouse.column, mouse.row)
                    }
                    _ => Effect::None,
                };
            }
            if read.elapsed() >= TICK {
                let selected = app.runs.get(app.run).map(|run| run.summary.id.clone());
                app.runs = list_runs(store)?;
                for view in app.execution.active.iter().chain(app.execution.view.iter()) {
                    if !matches!(
                        view.operation,
                        Some(
                            crate::operation::Status::AwaitingOwner { .. }
                                | crate::operation::Status::UnknownOwner { .. }
                                | crate::operation::Status::Finished { .. }
                        )
                    ) {
                        continue;
                    }
                    if let Some(run) = app.runs.iter_mut().find(|run| run.summary.id == view.run.summary.id) {
                        *run = view.run.clone();
                    }
                }
                app.run = selected
                    .and_then(|id| app.runs.iter().position(|run| run.summary.id == id))
                    .unwrap_or(0)
                    .min(app.runs.len().saturating_sub(1));
                if !app.busy && effect == Effect::None {
                    if app.screen == Screen::Local && app.overlay.is_none() {
                        effect = Effect::LocalState;
                    } else if let Some(id) = app.observed_run() {
                        effect = Effect::ObserveRun(id);
                    }
                }
                read = Instant::now();
            }
            if app.screen == Screen::Live
                && app.overlay.is_none()
                && app.schedule.state.is_none()
                && !app.busy
                && effect == Effect::None
            {
                effect = Effect::ScheduleRead;
            }
            if app.screen == Screen::Local
                && app.overlay.is_none()
                && app.local.checked.is_none()
                && !app.busy
                && effect == Effect::None
            {
                effect = Effect::LocalCheck(app.local.request(false));
            }
            if app.screen == Screen::Store && app.store.is_none() && !app.busy && effect == Effect::None {
                effect = Effect::PlanClean;
            }
        }
    })
}

impl App {
    pub(super) fn observed_run(&self) -> Option<String> {
        let shown = (self.overlay == Some(super::Overlay::Run)).then(|| self.execution.selected.clone()).flatten();
        shown.or_else(|| self.execution.handle.as_ref().map(|handle| handle.run.clone())).or_else(|| {
            self.runs
                .iter()
                .find(|run| run.summary.outcome == crate::engine::runs::Outcome::Running)
                .map(|run| run.summary.id.clone())
        })
    }

    /// Completion changes data, never the user's focus, filters or draft inputs.
    pub(super) fn complete(&mut self, effect: Effect, updated: App) {
        let selected = self.sources.get(self.source).map(|row| row.source.id.clone());
        let live_row = self.live_rows().get(self.row).cloned();
        let schedule_result = matches!(effect, Effect::ScheduleRead | Effect::ScheduleChange(_));
        self.saved = updated.saved.clone();
        match effect {
            Effect::Initial => {
                self.sources = updated.sources;
                self.live = updated.live;
                self.status = updated.status;
                self.env = updated.env;
                self.edited = updated.edited;
            }
            Effect::Policy(_, _) | Effect::CheckNow => {
                self.sources = updated.sources;
                self.live = updated.live;
                if matches!(effect, Effect::CheckNow)
                    && self.overlay == Some(super::Overlay::Version)
                    && updated.versions.as_ref().is_some_and(|versions| Some(&versions.source) == selected.as_ref())
                {
                    self.versions = updated.versions;
                    if let Some(versions) = &self.versions {
                        self.choice = self.choice.min(versions.common.len() + 1);
                    }
                }
                self.source =
                    selected.and_then(|id| self.sources.iter().position(|row| row.source.id == id)).unwrap_or(0);
            }
            Effect::Region(_) | Effect::Undo => {
                self.status = updated.status;
                self.env = updated.env;
                self.edited = updated.edited;
                self.sources = updated.sources;
                self.live = updated.live;
                self.source =
                    selected.and_then(|id| self.sources.iter().position(|row| row.source.id == id)).unwrap_or(0);
                self.row = self.row.min(self.live_rows().len().saturating_sub(1));
            }
            Effect::Status { .. } | Effect::Layer(_, _) => {
                self.status = updated.status;
                self.env = updated.env;
                self.edited = updated.edited;
                self.row = self.row.min(self.live_rows().len().saturating_sub(1));
            }
            Effect::ConfigRead => self.config.review = updated.config.review,
            Effect::ConfigCommit(_, _) => {
                self.config.review = updated.config.review;
                self.edited = updated.edited;
                self.asking = false;
            }
            Effect::PlanClean | Effect::Clean => self.store = updated.store,
            Effect::Plan => self.plan = updated.plan,
            Effect::Areas | Effect::LoadAreas => self.region_editor.areas = updated.region_editor.areas,
            Effect::CreateRegion(args) => {
                self.regions = updated.regions;
                if self.region_editor.draft().as_ref() == Some(&args) && updated.region_editor.mode.is_none() {
                    self.region_editor.mode = None;
                }
                self.choice = self.choice.min(self.shown_regions().len().saturating_sub(1));
            }
            Effect::DeleteRegion(plan) => {
                self.regions = updated.regions;
                if self.region_editor.mode == Some(super::regions::Mode::Delete)
                    && self.region_editor.deletion.as_ref() == Some(&plan)
                {
                    self.region_editor.mode = updated.region_editor.mode;
                }
                self.choice = self.choice.min(self.shown_regions().len().saturating_sub(1));
            }
            Effect::ReviewRegionDeletion(_) => self.region_editor.deletion = updated.region_editor.deletion,
            Effect::Select(_) => {
                if let (Some(current), Some(updated)) = (&mut self.plan, updated.plan) {
                    current.taken = updated.taken;
                }
            }
            Effect::Start(_, _) => {
                if let Some(handle) = updated.execution.handle {
                    let id = handle.run.clone();
                    self.execution.handle = Some(handle);
                    self.execution.active = None;
                    if self.overlay == Some(super::Overlay::Plan) {
                        self.execution.selected = Some(id);
                        self.execution.view = None;
                        (self.overlay, self.scroll) = (Some(super::Overlay::Run), 0);
                    }
                }
            }
            Effect::ObserveRun(id) | Effect::StopRun(id) | Effect::ReconcileRun(id) => {
                if let Some(view) = updated.execution.view.filter(|view| view.run.summary.id == id) {
                    if self.execution.selected.as_ref() == Some(&id) {
                        self.execution.dev_request = updated.execution.dev_request.clone();
                        self.execution.view = Some(view.clone());
                    }
                    if self.execution.handle.as_ref().is_some_and(|handle| handle.run == id) {
                        self.execution.active = Some(view.clone());
                    }
                    if let Some(run) = self.runs.iter_mut().find(|run| run.summary.id == id) {
                        *run = view.run.clone();
                    }
                    if matches!(
                        view.operation,
                        Some(crate::operation::Status::Finished { .. } | crate::operation::Status::Stopped)
                    ) && self.execution.handle.as_ref().is_some_and(|handle| handle.run == id)
                    {
                        self.execution.handle = None;
                    }
                }
            }
            next @ (Effect::LocalCheck(_) | Effect::LocalReview(_)) => {
                let review = matches!(next, Effect::LocalReview(_));
                let request = match next {
                    Effect::LocalCheck(request) | Effect::LocalReview(request) => request,
                    _ => unreachable!(),
                };
                if self.local.request(false).app == request.app {
                    let selected = self.local.rows().get(self.local.row).cloned();
                    self.local.env = updated.local.env;
                    self.local.optional = updated.local.optional;
                    self.local.plan = updated.local.plan;
                    self.local.request = updated.local.request;
                    self.local.checked = updated.local.checked;
                    self.local.state = updated.local.state;
                    self.local.row = selected
                        .and_then(|selected| self.local.rows().iter().position(|row| row == &selected))
                        .unwrap_or(self.local.row.min(self.local.rows().len().saturating_sub(1)));
                    if self.screen == Screen::Local && self.overlay == Some(super::Overlay::Plan) {
                        self.asking = review
                            && self
                                .local
                                .plan
                                .as_ref()
                                .is_some_and(|plan| !plan.needs_prepare && plan.blocked.is_empty());
                        self.plan = self.local.plan.clone().map(|plan| PlanView::local(plan, request));
                    }
                }
            }
            Effect::LocalRegion(_) | Effect::LocalLayer(_, _) => {
                self.local.env = updated.local.env;
                self.local.plan = updated.local.plan;
                self.local.request = updated.local.request;
                self.local.checked = updated.local.checked;
                self.local.state = updated.local.state;
            }
            Effect::LocalStart(request) => {
                if let Some(handle) = updated.execution.handle {
                    let id = handle.run.clone();
                    self.execution.dev_request = Some((id.clone(), request));
                    self.execution.handle = Some(handle);
                    self.execution.active = None;
                    if self.screen == Screen::Local && matches!(self.overlay, None | Some(super::Overlay::Plan)) {
                        self.execution.selected = Some(id);
                        self.execution.view = None;
                        self.overlay = Some(super::Overlay::Run);
                        self.scroll = 0;
                    }
                }
            }
            Effect::LocalControl(super::local::Control::Logs, _) => self.local.logs = updated.local.logs,
            Effect::LocalControl(_, _) | Effect::LocalState => self.local.state = updated.local.state,
            Effect::Versions(id) => {
                if self.sources.get(self.source).is_some_and(|row| row.source.id == id) {
                    self.versions = updated.versions;
                    if self.overlay == Some(super::Overlay::Version) {
                        if let Some(versions) = &self.versions {
                            self.choice = match self.moves.get(&versions.source) {
                                None => 0,
                                Some(None) => 1,
                                Some(Some(version)) => {
                                    versions.common.iter().position(|known| known == version).map_or(0, |at| at + 2)
                                }
                            };
                        }
                    }
                }
            }
            Effect::ConfigRead => {
                app.config.review = Some(super::super::config_cli::review(root)?);
                Ok(())
            }
            Effect::ConfigCommit(review, message) => {
                let committed = super::super::config_cli::commit(root, &review, &message)?;
                app.saved = Some(format!("Committed bake configuration {} · nothing pushed", committed.commit));
                app.config.review = None;
                app.edited = edit_cli::edited(root, LIVE);
                Ok(())
            }
            Effect::ScheduleRead => {
                self.schedule.state = updated.schedule.state;
                if self.schedule.calendar.is_empty() {
                    self.schedule.calendar = updated.schedule.calendar;
                }
                if self.schedule.zone.is_empty() {
                    self.schedule.zone = updated.schedule.zone;
                }
            }
            Effect::ScheduleChange(_) => {
                self.schedule.state = updated.schedule.state;
                if updated.schedule.pending.is_none() {
                    self.schedule.pending = None;
                    self.asking = false;
                }
            }
            Effect::None | Effect::Quit | Effect::Reload => {}
        }
        if schedule_result {
            if let Some(at) = live_row.and_then(|row| self.live_rows().iter().position(|shown| *shown == row)) {
                self.row = at;
            }
        }
    }
}

/// Do what `effect` names, with the function of its command.
pub(super) fn perform(
    root: &Path,
    products: &[&dyn Product],
    store: &Store,
    app: &mut App,
    effect: Effect,
) -> Result<(), Error> {
    if matches!(effect, Effect::Start(_, _) | Effect::LocalStart(_)) {
        app.execution.handle = None;
    }
    if !matches!(
        effect,
        Effect::ConfigRead
            | Effect::ConfigCommit(_, _)
            | Effect::ScheduleRead
            | Effect::ScheduleChange(super::schedule::Change::Disable)
            | Effect::ObserveRun(_)
            | Effect::StopRun(_)
            | Effect::ReconcileRun(_)
            | Effect::LocalState
            | Effect::LocalControl(
                super::local::Control::Stop | super::local::Control::Open | super::local::Control::Logs,
                _
            )
    ) {
        crate::worker::check(root).map_err(|message| {
            Code::Blocked
                .error(message)
                .fix("Leave any input with Esc, then press F6 to reload current code. Run controls remain available.")
        })?;
    }
    match effect {
        Effect::None | Effect::Quit | Effect::Reload => Ok(()),
        Effect::Versions(id) => {
            let row = app
                .sources
                .iter()
                .find(|row| row.source.id == id)
                .ok_or_else(|| Code::Usage.error("The selected source no longer exists."))?;
            app.versions = Some(super::super::versions::read(store, row)?);
            Ok(())
        }
        Effect::ScheduleRead => {
            app.schedule.state = Some(crate::schedule::state(root, store, LIVE).map(std::sync::Arc::new));
            if app.schedule.calendar.is_empty() {
                if let Some(Ok(state)) = &app.schedule.state {
                    app.schedule.calendar = state.calendar.clone().unwrap_or_else(|| "daily".into());
                    app.schedule.zone = state.time_zone.clone().unwrap_or_else(|| "UTC".into());
                }
            }
            Ok(())
        }
        Effect::ScheduleChange(change) => {
            match change {
                super::schedule::Change::Install { calendar, zone } => {
                    app.schedule.state =
                        Some(Ok(std::sync::Arc::new(crate::schedule::install(root, store, LIVE, &calendar, &zone)?)));
                }
                super::schedule::Change::Disable => {
                    app.schedule.state = Some(Ok(std::sync::Arc::new(crate::schedule::disable(root, store, LIVE)?)));
                }
                super::schedule::Change::Budget => {
                    crate::schedule::setup_budget(root, store, LIVE)?;
                    app.schedule.state = Some(crate::schedule::state(root, store, LIVE).map(std::sync::Arc::new));
                }
            }
            app.schedule.pending = None;
            Ok(())
        }
        Effect::Initial => app.reload(root, products, false).and(app.read_live(root, products, false)),
        Effect::Areas => regions_cli::suggestions(store, "")
            .map(|areas| app.region_editor.areas = Some(areas))
            .map_err(super::regions::area_list_error),
        Effect::LoadAreas => regions_cli::load_areas(root, store).map(|areas| app.region_editor.areas = Some(areas)),
        Effect::CreateRegion(args) => {
            let id = args.id.clone();
            let areas = args.bbox.is_none();
            regions_cli::create(root, store, args).map_err(|error| super::regions::creation_error(error, areas))?;
            app.saved = Some(format!("Saved data/regions/{id}.toml · review and commit before apply"));
            app.regions = Regions::load(root).map(|regions| regions.iter().cloned().collect());
            app.region_editor.mode = None;
            Ok(())
        }
        Effect::ReviewRegionDeletion(id) => {
            regions_cli::deletion(root, &id).map(|deletion| app.region_editor.deletion = Some(deletion))
        }
        Effect::DeleteRegion(deletion) => {
            regions_cli::remove(root, &deletion)?;
            app.saved = Some(format!("Deleted data/regions/{}.toml · review and commit before apply", deletion.region));
            app.regions = Regions::load(root).map(|regions| regions.iter().cloned().collect());
            app.region_editor.mode = None;
            Ok(())
        }
        Effect::Policy(id, refresh) => {
            let result = policy(root, &id, refresh)
                .map(|_| app.saved = Some("Saved data/sources.toml · review and commit before apply".into()));
            let reloaded = app.reload(root, products, false);
            result.and(reloaded)
        }
        Effect::CheckNow => {
            app.reload(root, products, true)?;
            if app.overlay == Some(super::Overlay::Version) {
                if let Some(row) = app.sources.get(app.source) {
                    app.versions = Some(super::super::versions::read(store, row)?);
                }
            }
            Ok(())
        }
        Effect::PlanClean => {
            let result = clean_plan(root, products, store).map(|plan| app.store = Some(plan));
            if result.is_err() {
                app.overlay = None;
            }
            result
        }
        Effect::Clean => {
            let confirmed = app.store.as_ref().unwrap_or(&NO_PLAN);
            let result = clean(root, products, store, &confirmed.store).map(drop);
            match clean_plan(root, products, store) {
                Ok(plan) => app.store = Some(plan),
                Err(error) => app.notice = Some(error),
            }
            result
        }
        Effect::Status { check } => app.read_live(root, products, check),
        Effect::Region(id) => {
            let result = edit_cli::region(root, products, LIVE, &id).map(drop);
            result.and(app.read_live(root, products, false)).and(app.reload(root, products, false))
        }
        Effect::Layer(layer, switch) => {
            let result = edit_cli::layer(root, products, LIVE, &layer, switch).map(drop);
            result.and(app.read_live(root, products, false))
        }
        Effect::Undo => {
            edit_cli::undo(root, LIVE).and(app.read_live(root, products, false)).and(app.reload(root, products, false))
        }
        Effect::Plan => {
            let plan = plan_live_moves(
                root,
                &Store::open()?,
                &Http::new(),
                &crate::cli::remote()?,
                products,
                &[],
                &app.move_args(),
                false,
            );
            let plan = plan.inspect_err(|_| app.overlay = None)?;
            app.plan = Some(PlanView::new(plan));
            Ok(())
        }
        Effect::LocalCheck(request) | Effect::LocalReview(request) => app.read_local(root, products, store, &request),
        Effect::LocalRegion(region) => {
            super::local::ensure_environment(root)?;
            edit_cli::region(root, products, "local", &region)?;
            app.local.request = None;
            app.read_local(root, products, store, &app.local.request(false))
        }
        Effect::LocalLayer(layer, switch) => {
            super::local::ensure_environment(root)?;
            edit_cli::layer(root, products, "local", &layer, switch)?;
            app.local.request = None;
            app.read_local(root, products, store, &app.local.request(false))
        }
        Effect::LocalStart(request) => {
            let handle = super::local::start(root, store, request)?;
            app.saved = Some(format!("Started Local run {} · no app starts automatically", handle.run));
            app.execution.handle = Some(handle);
            Ok(())
        }
        Effect::LocalState => {
            app.local.state = crate::dev::state(store)?;
            Ok(())
        }
        Effect::LocalControl(control, selected) => {
            match control {
                super::local::Control::Start => {
                    app.local.state = Some(crate::dev::start(root, store, &crate::dev::prepared(store)?, selected)?);
                }
                super::local::Control::Stop => app.local.state = crate::dev::stop_app(store, selected)?,
                super::local::Control::Open => crate::dev::open(store, selected)?,
                super::local::Control::Logs => app.local.logs = crate::dev::logs(store, selected)?,
            }
            Ok(())
        }
        Effect::Select(only) => {
            let moves = app.move_args();
            let Some(view) = app.plan.as_mut() else { return Ok(()) };
            let taken = match only.is_empty() {
                true => Ok(view.all.clone()),
                false => plan_live_moves(
                    root,
                    &Store::open()?,
                    &Http::new(),
                    &crate::cli::remote()?,
                    products,
                    &only,
                    &moves,
                    false,
                ),
            };
            view.taken = taken.inspect_err(|_| app.overlay = None)?;
            Ok(())
        }
        Effect::Start(kind, plan) => {
            let handle = super::execution::start(root, store, kind, &plan)?;
            app.saved = Some(format!("Started run {} · Esc hides it · quitting does not stop it", handle.run));
            app.execution.handle = Some(handle);
            Ok(())
        }
        next @ (Effect::ObserveRun(_) | Effect::ReconcileRun(_) | Effect::StopRun(_)) => {
            let id = match &next {
                Effect::ObserveRun(id) | Effect::ReconcileRun(id) | Effect::StopRun(id) => id,
                _ => unreachable!(),
            };
            let view = match &next {
                Effect::ReconcileRun(_) => crate::cli::operation_cli::reconcile(store, id)?,
                Effect::StopRun(_) => {
                    crate::operation::stop(store, id).map_err(|message| crate::cli::Code::Blocked.error(message))?;
                    crate::cli::operation_cli::view(store, id)?
                }
                _ => crate::cli::operation_cli::view(store, id)?,
            };
            app.execution.dev_request = crate::operation::read(store, id)?
                .and_then(|control| control.request.dev)
                .map(|request| (id.clone(), request));
            app.execution.view = Some(std::sync::Arc::new(view));
            Ok(())
        }
    }
}
