//! One finite typed task. Its completion cannot replace navigation, filters or draft inputs.

use super::{list_runs, App, Effect, Error, PlanView, Screen, Tui, LIVE, NO_PLAN, TICK};
use crate::cli::Code;
use crate::cli::{build_cli::plan_live, clean, clean_plan, edit_cli, policy, regions_cli};
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

type ViewContext = (
    Result<crate::cli::edit_cli::Edited, String>,
    Result<Vec<crate::sources::Source>, String>,
    Result<Vec<crate::regions::Region>, String>,
);

fn context(root: &Path) -> ViewContext {
    (
        crate::cli::edit_cli::current(root, super::LIVE).map_err(|error| error.message),
        crate::cli::registry(root).map(|registry| registry.sources).map_err(|error| error.message),
        crate::regions::Regions::load(root).map(|regions| regions.iter().cloned().collect()),
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
        let mut task: Option<std::thread::ScopedJoinHandle<'_, (Effect, Option<App>, Result<(), Error>)>> = None;
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
                    app.complete(effect, updated);
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
                Effect::Quit => finishing = true,
                next if !app.busy => {
                    app.busy = true;
                    let mut updated = app.clone();
                    updated.notice = None;
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
                        );
                        let before = observes.then(|| context(root));
                        let result = perform(root, products, store, &mut updated, next.clone());
                        if before.is_some_and(|before| before != context(root)) {
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
                    frame.render_widget(Line::from("Finishing the admitted check or edit before quitting…"), area);
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
                if app.screen == Screen::Runs {
                    app.runs = list_runs(store)?;
                }
                read = Instant::now();
            }
            if app.screen == Screen::Store && app.store.is_none() && !app.busy && effect == Effect::None {
                effect = Effect::PlanClean;
            }
        }
    })
}

impl App {
    /// Completion changes data, never the user's focus, filters or draft inputs.
    pub(super) fn complete(&mut self, effect: Effect, updated: App) {
        let selected = self.sources.get(self.source).map(|row| row.source.id.clone());
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
            Effect::None | Effect::Quit => {}
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
    crate::worker::check(root).map_err(|message| {
        Code::Blocked
            .error(message)
            .fix("Leave any input with Esc, press q to quit, then run obc data again to load the current Rust code.")
    })?;
    match effect {
        Effect::None | Effect::Quit => Ok(()),
        Effect::Initial => app.reload(root, products, false).and(app.read_live(root, products, false)),
        Effect::Areas => regions_cli::suggestions(store, "").map(|areas| app.region_editor.areas = Some(areas)),
        Effect::LoadAreas => regions_cli::load_areas(root, store).map(|areas| app.region_editor.areas = Some(areas)),
        Effect::CreateRegion(args) => {
            let id = args.id.clone();
            regions_cli::create(root, store, args)?;
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
        Effect::CheckNow => app.reload(root, products, true),
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
            let plan = plan_live(root, &Store::open()?, &Http::new(), &crate::cli::remote()?, products, &[], false);
            let plan = plan.inspect_err(|_| app.overlay = None)?;
            app.plan = Some(PlanView::new(plan));
            Ok(())
        }
        Effect::Select(only) => {
            let Some(view) = app.plan.as_mut() else { return Ok(()) };
            let taken = match only.is_empty() {
                true => Ok(view.all.clone()),
                false => plan_live(root, &Store::open()?, &Http::new(), &crate::cli::remote()?, products, &only, false),
            };
            view.taken = taken.inspect_err(|_| app.overlay = None)?;
            Ok(())
        }
    }
}
