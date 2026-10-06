//! The TUI of `obc data`. Each screen shows what a command writes with `--json`. Each change goes
//! through the function of its command: `region`, `layer`, `undo`, `policy` and `clean --apply`.

mod live;
mod plan;

use std::io::Stdout;
use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind,
};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::engine::runs::{self, Details, Outcome, Summary};
use crate::fetch::http::Http;
use crate::product::Product;
use crate::regions::Regions;
use crate::sources::{Kind, Refresh, State, VersionScheme};
use crate::store::{gc, import, Store};

use super::build_cli::plan_live;
use super::edit_cli::{self, Edited, Switch};
use super::runs_cli::{bytes, duration, mark, step_cells};
use super::status_cli::{self, Status};
use super::{clean, clean_plan, policy, row_text, source_listing, widths, CleanPlan, Error, SourceRow};
use live::{Fix, LiveRow};
use plan::PlanView;

/// The environment that Live and Plan show and edit.
const LIVE: &str = "live";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Live,
    Sources,
    Store,
    Runs,
}

/// The key of a screen is its number among the five screens of the TUI; 2 Local is not built.
const SCREENS: [(char, &str, Screen); 4] = [
    ('1', "Live", Screen::Live),
    ('3', "Sources", Screen::Sources),
    ('4', "Store", Screen::Store),
    ('5', "Runs", Screen::Runs),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Help,
    Attribution,
    Policy,
    Clean,
    Region,
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Show(Screen),
    NextScreen,
    Up,
    Down,
    Open(Overlay),
    /// `enter` on a policy of Policy, or on a region of Region.
    Choose,
    CheckNow,
    /// The question before a clean.
    Ask,
    Clean,
    /// `space`: switch the selected optional layer of Live, or take or leave the selected move of
    /// Plan.
    Toggle,
    /// `enter` on a row of NEEDS ATTENTION.
    Fix,
    /// Type into the filter of Region.
    Filter,
    /// The fetches and builds of Plan, or its groups again.
    Steps,
    Undo,
    Close,
    Quit,
}

/// What the loop does after a key or a click.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    None,
    Quit,
    /// `obc data policy SOURCE REFRESH`.
    Policy(String, Refresh),
    /// `obc data sources --check-now`.
    CheckNow,
    /// `obc data clean`, for Clean.
    PlanClean,
    /// `obc data clean --apply` of the plan that Clean shows.
    Clean,
    /// `obc data status [--check]`, and `data/env/live.toml`, for Live.
    Status {
        check: bool,
    },
    /// `obc data region live ID`.
    Region(String),
    /// `obc data layer live NAME on|off`.
    Layer(String, Switch),
    /// `obc data undo live`.
    Undo,
    /// `obc data plan live`, for Plan.
    Plan,
    /// `obc data plan live --only MOVES`: the moves that Plan takes; every move when empty.
    Select(Vec<String>),
}

/// A key that works now, with its label and what it does when the bar shows it.
struct Binding {
    key: KeyCode,
    action: Action,
    bar: Option<(&'static str, String)>,
}

/// The keys that the bar can show on a screen with rows, besides `?`.
fn screen_keys(screen: Screen) -> &'static [(KeyCode, Action, &'static str, &'static str)] {
    match screen {
        Screen::Live => &[
            (KeyCode::Char('r'), Action::Open(Overlay::Region), "r", "region"),
            (KeyCode::Char(' '), Action::Toggle, "space", "toggle"),
            (KeyCode::Char('R'), Action::CheckNow, "R", "check R2"),
        ],
        Screen::Sources => &[
            (KeyCode::Char('e'), Action::Open(Overlay::Policy), "e", "policy"),
            (KeyCode::Char('R'), Action::CheckNow, "R", "check upstream"),
            (KeyCode::Char('L'), Action::Open(Overlay::Attribution), "L", "attribution"),
        ],
        Screen::Store => &[(KeyCode::Char('c'), Action::Open(Overlay::Clean), "c", "clean")],
        Screen::Runs => &[],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Screen(Screen),
    Row(usize),
}

#[derive(Clone)]
struct App {
    screen: Screen,
    overlay: Option<Overlay>,
    sources: Vec<SourceRow>,
    /// Source id to the versions that the live releases read; `None` when R2 could not be read.
    live: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// An effect runs.
    busy: bool,
    /// The plan of `clean`, once Store has shown.
    store: Option<CleanPlan>,
    /// Running runs first, then newest first.
    runs: Vec<Details>,
    /// What `status` wrote; `None` until it is read, or when it failed.
    status: Option<Status>,
    /// `data/env/live.toml`.
    env: Option<Edited>,
    /// `data/env/live.toml` differs from its committed version: `undo` takes the edits back.
    edited: bool,
    /// The id of each region, or why `data/regions/` could not be read.
    regions: Result<Vec<String>, String>,
    /// The filter of Region, and whether keys type into it.
    filter: String,
    filtering: bool,
    /// The plan of Plan, once it is made.
    plan: Option<PlanView>,
    row: usize,
    source: usize,
    kept: usize,
    run: usize,
    /// The first line that an overlay shows.
    scroll: usize,
    /// The selected policy of Policy, or the selected region of Region.
    choice: usize,
    /// Clean asks its question.
    asking: bool,
    /// The error of the last change.
    notice: Option<String>,
    /// Where the last frame drew each tab and row.
    hits: Vec<(Rect, Hit)>,
}

type Tui = Terminal<CrosstermBackend<Stdout>>;

/// How often Runs reads the runs again while one runs.
const TICK: Duration = Duration::from_secs(1);

/// The plan before Store has one.
static NO_PLAN: CleanPlan = CleanPlan {
    store: gc::Plan {
        kept: Vec::new(),
        snapshots: Vec::new(),
        objects: Vec::new(),
        remove_bytes: 0,
        keep_objects: 0,
        keep_bytes: 0,
    },
    import: import::Plan { dirs: Vec::new(), bytes: 0 },
};

pub fn run(root: &Path, products: &[&dyn Product]) -> Result<(), Error> {
    let store = Store::open()?;
    let (sources, live) = source_listing(root, products, false)?;
    let mut app = App::new(sources, list_runs(&store)?);
    app.live = live;
    app.regions = Regions::load(root).map(|regions| regions.iter().map(|region| region.id.clone()).collect());
    app.notice = app.regions.clone().err();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        stop();
        previous(info);
    }));
    let result =
        start().inspect_err(|_| stop()).and_then(|mut tui| run_loop(root, products, &store, &mut app, &mut tui));
    stop();
    result
}

fn run_loop(root: &Path, products: &[&dyn Product], store: &Store, app: &mut App, tui: &mut Tui) -> Result<(), Error> {
    let io = |e: std::io::Error| e.to_string();
    let mut read = Instant::now();
    // Live is the first screen. Only `R` lists R2.
    let mut effect = Effect::Status { check: false };
    loop {
        match effect {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            effect => {
                app.busy = true;
                tui.draw(|frame| app.draw(frame)).map_err(io)?;
                if let Err(error) = perform(root, products, store, app, effect) {
                    app.notice = Some(error.message);
                }
                app.busy = false;
                discard_keys()?;
                // A fetch writes its progress to standard error, over the screen: draw all of it
                // again. `Terminal::clear` would ask the terminal for the cursor first.
                execute!(std::io::stdout(), terminal::Clear(terminal::ClearType::All)).map_err(io)?;
                tui.swap_buffers();
            }
        }
        tui.draw(|frame| app.draw(frame)).map_err(io)?;
        let screen = app.screen;
        effect = Effect::None;
        // A mouse move is an event too, so the tick is timed, not the wait for an event.
        if event::poll(TICK.saturating_sub(read.elapsed())).map_err(io)? {
            effect = match event::read().map_err(io)? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.key(key.code),
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    app.click(mouse.column, mouse.row);
                    Effect::None
                }
                _ => Effect::None,
            };
        }
        let running = app.runs.iter().any(|run| run.summary.outcome == Outcome::Running);
        let tick = read.elapsed() >= TICK;
        if app.screen == Screen::Runs && (screen != Screen::Runs || (tick && running)) {
            app.runs = list_runs(store)?;
        }
        if app.screen == Screen::Store && screen != Screen::Store && app.store.is_none() {
            effect = Effect::PlanClean;
        }
        if tick || app.screen != screen {
            read = Instant::now();
        }
    }
}

/// Do what `effect` names, with the function of its command.
fn perform(root: &Path, products: &[&dyn Product], store: &Store, app: &mut App, effect: Effect) -> Result<(), Error> {
    crate::worker::check(root)?;
    match effect {
        Effect::None | Effect::Quit => Ok(()),
        Effect::Policy(id, refresh) => {
            let result = policy(root, &id, refresh).map(drop);
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
                Err(error) => app.notice = Some(error.message),
            }
            result
        }
        Effect::Status { check } => app.read_live(root, products, check),
        Effect::Region(id) => {
            let result = edit_cli::region(root, products, LIVE, &id).map(drop);
            result.and(app.read_live(root, products, false))
        }
        Effect::Layer(layer, switch) => {
            let result = edit_cli::layer(root, products, LIVE, &layer, switch).map(drop);
            result.and(app.read_live(root, products, false))
        }
        Effect::Undo => edit_cli::undo(root, LIVE).and(app.read_live(root, products, false)),
        Effect::Plan => {
            let plan = plan_live(root, &Store::open()?, &Http::new(), &super::remote()?, products, &[], false);
            let plan = plan.inspect_err(|_| app.overlay = None)?;
            app.plan = Some(PlanView::new(plan));
            Ok(())
        }
        Effect::Select(only) => {
            let Some(view) = app.plan.as_mut() else { return Ok(()) };
            let taken = match only.is_empty() {
                true => Ok(view.all.clone()),
                false => plan_live(root, &Store::open()?, &Http::new(), &super::remote()?, products, &only, false),
            };
            view.taken = taken.inspect_err(|_| app.overlay = None)?;
            Ok(())
        }
    }
}

/// Drop the keys typed while the TUI waited: they are not for what it shows now.
fn discard_keys() -> Result<(), Error> {
    while event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
        event::read().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn start() -> Result<Tui, Error> {
    let io = |e: std::io::Error| e.to_string();
    terminal::enable_raw_mode().map_err(io)?;
    execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture).map_err(io)?;
    Ok(Terminal::new(CrosstermBackend::new(std::io::stdout())).map_err(io)?)
}

/// Undo `start`, also after a panic or a `start` that failed half way.
fn stop() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
}

fn list_runs(store: &Store) -> Result<Vec<Details>, Error> {
    let mut runs = runs::all_details(store)?;
    runs.sort_by_key(|run| run.summary.outcome != Outcome::Running);
    Ok(runs)
}

impl App {
    fn new(sources: Vec<SourceRow>, runs: Vec<Details>) -> Self {
        Self {
            screen: Screen::Live,
            overlay: None,
            sources,
            live: None,
            busy: false,
            store: None,
            runs,
            status: None,
            env: None,
            edited: false,
            regions: Ok(Vec::new()),
            filter: String::new(),
            filtering: false,
            plan: None,
            row: 0,
            source: 0,
            kept: 0,
            run: 0,
            scroll: 0,
            choice: 0,
            asking: false,
            notice: None,
            hits: Vec::new(),
        }
    }

    /// Read the sources again; with `check_now`, after a check of upstream now.
    fn reload(&mut self, root: &Path, products: &[&dyn Product], check_now: bool) -> Result<(), Error> {
        (self.sources, self.live) = source_listing(root, products, check_now)?;
        Ok(())
    }

    /// Read `data/env/live.toml` and `status [--check]` again.
    fn read_live(&mut self, root: &Path, products: &[&dyn Product], check: bool) -> Result<(), Error> {
        self.edited = edit_cli::edited(root, LIVE);
        let env = edit_cli::current(root, LIVE);
        self.env = env.as_ref().ok().cloned();
        let status = status_cli::read(root, products, check);
        self.status = status.as_ref().ok().cloned();
        self.row = self.row.min(self.live_rows().len() - 1);
        env.and(status).map(drop)
    }

    fn rows(&self) -> usize {
        match self.screen {
            Screen::Live => self.live_rows().len(),
            Screen::Sources => self.sources.len(),
            Screen::Store => self
                .store
                .as_ref()
                .map_or(0, |plan| plan.store.kept.len() + usize::from(!plan.store.objects.is_empty())),
            Screen::Runs => self.runs.len(),
        }
    }

    /// The number of choices of an overlay with a selection.
    fn choices(&self) -> Option<usize> {
        match self.overlay? {
            Overlay::Policy => Some(Refresh::ALL.len()),
            Overlay::Region => Some(self.shown_regions().len()),
            Overlay::Plan => self.plan.as_ref().filter(|view| !view.steps).map(|view| view.all.groups.len()),
            _ => None,
        }
    }

    /// The selected row, the choice of an overlay, or the scroll of an overlay.
    fn selected(&mut self) -> &mut usize {
        if self.overlay == Some(Overlay::Plan) && self.choices().is_some() {
            return &mut self.plan.as_mut().expect("Plan has its plan").group;
        }
        match (self.overlay, self.screen) {
            (Some(Overlay::Policy | Overlay::Region), _) => &mut self.choice,
            (Some(_), _) => &mut self.scroll,
            (None, Screen::Live) => &mut self.row,
            (None, Screen::Sources) => &mut self.source,
            (None, Screen::Store) => &mut self.kept,
            (None, Screen::Runs) => &mut self.run,
        }
    }

    /// The regions whose id has the filter of Region.
    fn shown_regions(&self) -> Vec<&str> {
        let filter = self.filter.to_lowercase();
        let regions = self.regions.iter().flatten();
        regions.map(String::as_str).filter(|id| id.contains(&filter)).collect()
    }

    /// Keys type into the filter of Region.
    fn typing(&self) -> bool {
        self.overlay == Some(Overlay::Region) && self.filtering
    }

    /// Whether `enter` in Policy or Region chooses another policy or region.
    fn chooses(&self) -> bool {
        match self.overlay {
            Some(Overlay::Policy) => {
                let row = self.sources.get(self.source);
                row.is_some_and(|row| Refresh::ALL[self.choice] != row.source.refresh)
            }
            Some(Overlay::Region) => {
                let region = self.shown_regions().get(self.choice).map(|id| id.to_string());
                region.is_some() && region != self.env.as_ref().map(|env| env.region.clone())
            }
            _ => false,
        }
    }

    /// Whether a key does something for the selected row.
    fn works(&self, action: Action) -> bool {
        match action {
            Action::Open(Overlay::Policy) => {
                self.sources.get(self.source).is_some_and(|row| row.source.version == VersionScheme::Date)
            }
            Action::Open(Overlay::Clean) => self.store.as_ref().is_some_and(|plan| !plan.is_empty()),
            Action::Filter => self.regions.is_ok(),
            Action::Choose => self.chooses(),
            Action::Toggle if self.overlay == Some(Overlay::Plan) => self.plan.as_ref().is_some_and(PlanView::toggles),
            Action::Toggle => matches!(self.live_rows().get(self.row), Some(LiveRow::Layer(_))),
            Action::Fix => self.fix().is_some(),
            Action::Steps => self.plan.as_ref().is_some_and(|view| !view.all.groups.is_empty()),
            Action::Undo => self.edited,
            _ => true,
        }
    }

    fn bindings(&self) -> Vec<Binding> {
        let bar = |key, action, label, does: &str| Binding { key, action, bar: Some((label, does.to_string())) };
        let hidden = |key, action| Binding { key, action, bar: None };
        let typing = self.typing();
        let mut keys = vec![hidden(KeyCode::Up, Action::Up), hidden(KeyCode::Down, Action::Down)];
        if !typing {
            keys.extend([hidden(KeyCode::Char('k'), Action::Up), hidden(KeyCode::Char('j'), Action::Down)]);
        }
        match self.overlay {
            Some(overlay) => {
                let mut offer = |key, action, label, does| {
                    if self.works(action) {
                        keys.push(bar(key, action, label, does));
                    }
                };
                match overlay {
                    Overlay::Region if !typing => offer(KeyCode::Char('/'), Action::Filter, "/", "filter"),
                    Overlay::Clean if self.works(Action::Open(Overlay::Clean)) => match self.asking {
                        false => offer(KeyCode::Char('a'), Action::Ask, "a", "clean"),
                        true => offer(KeyCode::Char('y'), Action::Clean, "y", "clean"),
                    },
                    Overlay::Plan => {
                        offer(KeyCode::Char(' '), Action::Toggle, "space", "toggle");
                        let steps = self.plan.as_ref().is_some_and(|view| view.steps);
                        offer(KeyCode::Char('d'), Action::Steps, "d", if steps { "changes" } else { "steps" });
                    }
                    _ => {}
                }
                offer(KeyCode::Enter, Action::Choose, "enter", "choose");
                let close = if self.asking {
                    "cancel"
                } else if typing {
                    "clear"
                } else {
                    "close"
                };
                keys.push(bar(KeyCode::Esc, Action::Close, "esc", close));
            }
            None => {
                keys.extend(SCREENS.iter().map(|&(key, _, screen)| hidden(KeyCode::Char(key), Action::Show(screen))));
                keys.push(hidden(KeyCode::Tab, Action::NextScreen));
                if self.rows() > 0 {
                    let works = screen_keys(self.screen).iter().filter(|&&(_, action, _, _)| self.works(action));
                    keys.extend(works.map(|&(key, action, label, does)| bar(key, action, label, does)));
                }
                if self.screen == Screen::Live {
                    match self.live_rows().get(self.row) {
                        Some(LiveRow::Region) => keys.push(hidden(KeyCode::Enter, Action::Open(Overlay::Region))),
                        Some(LiveRow::Attention(_)) if self.works(Action::Fix) => {
                            keys.push(bar(KeyCode::Enter, Action::Fix, "enter", "fix"))
                        }
                        _ => {}
                    }
                }
                keys.push(bar(KeyCode::Char('p'), Action::Open(Overlay::Plan), "p", "plan"));
                if self.works(Action::Undo) {
                    keys.push(bar(KeyCode::Char('u'), Action::Undo, "u", "undo"));
                }
                keys.push(bar(KeyCode::Char('?'), Action::Open(Overlay::Help), "?", "help"));
            }
        }
        if !typing {
            keys.push(hidden(KeyCode::Char('q'), Action::Quit));
        }
        keys
    }

    fn key(&mut self, key: KeyCode) -> Effect {
        self.notice = None;
        if self.typing() {
            match key {
                KeyCode::Char(c) => self.filter.push(c),
                KeyCode::Backspace => drop(self.filter.pop()),
                _ => {}
            }
            if matches!(key, KeyCode::Char(_) | KeyCode::Backspace) {
                self.choice = 0;
                return Effect::None;
            }
        }
        match self.bindings().iter().find(|binding| binding.key == key) {
            Some(binding) => self.act(binding.action),
            None => Effect::None,
        }
    }

    fn act(&mut self, action: Action) -> Effect {
        let last = match self.overlay {
            Some(_) => self.choices().map_or(usize::MAX, |choices| choices.saturating_sub(1)),
            None => self.rows().saturating_sub(1),
        };
        match action {
            Action::Show(screen) => self.screen = screen,
            Action::NextScreen => {
                let at = SCREENS.iter().position(|&(_, _, screen)| screen == self.screen).unwrap_or(0);
                self.screen = SCREENS[(at + 1) % SCREENS.len()].2;
            }
            // The drawing stops the scroll of an overlay at its end.
            Action::Up => *self.selected() = self.selected().saturating_sub(1),
            Action::Down => *self.selected() = (*self.selected() + 1).min(last),
            Action::Open(overlay) => {
                (self.overlay, self.scroll, self.asking) = (Some(overlay), 0, false);
                match overlay {
                    Overlay::Policy => {
                        let current = self.sources.get(self.source).map(|row| row.source.refresh);
                        self.choice = Refresh::ALL.iter().position(|&r| Some(r) == current).unwrap_or(0);
                    }
                    Overlay::Region => {
                        (self.filter, self.filtering) = (String::new(), false);
                        let current = self.env.as_ref().map(|env| env.region.as_str());
                        self.choice = self.shown_regions().iter().position(|&id| Some(id) == current).unwrap_or(0);
                    }
                    Overlay::Clean => return Effect::PlanClean,
                    Overlay::Plan => {
                        self.plan = None;
                        return Effect::Plan;
                    }
                    Overlay::Help | Overlay::Attribution => {}
                }
            }
            Action::Choose if self.overlay == Some(Overlay::Policy) => {
                self.overlay = None;
                return Effect::Policy(self.sources[self.source].source.id.clone(), Refresh::ALL[self.choice]);
            }
            Action::Choose => {
                let id = self.shown_regions()[self.choice].to_string();
                (self.overlay, self.filter, self.filtering) = (None, String::new(), false);
                return Effect::Region(id);
            }
            Action::CheckNow if self.screen == Screen::Live => return Effect::Status { check: true },
            Action::CheckNow => return Effect::CheckNow,
            Action::Ask => self.asking = true,
            Action::Clean => {
                (self.overlay, self.asking) = (None, false);
                return Effect::Clean;
            }
            Action::Toggle => return self.toggle(),
            Action::Fix => match self.fix() {
                Some(Fix::Source(source)) => (self.screen, self.source) = (Screen::Sources, source),
                Some(Fix::Store) => self.screen = Screen::Store,
                Some(Fix::Plan) => return self.act(Action::Open(Overlay::Plan)),
                None => {}
            },
            Action::Filter => self.filtering = true,
            Action::Steps => {
                if let Some(view) = self.plan.as_mut() {
                    view.steps = !view.steps;
                    self.scroll = 0;
                }
            }
            Action::Undo => return Effect::Undo,
            Action::Close if self.asking => self.asking = false,
            Action::Close if self.typing() => (self.filter, self.filtering, self.choice) = (String::new(), false, 0),
            Action::Close => self.overlay = None,
            Action::Quit => return Effect::Quit,
        }
        Effect::None
    }

    /// A click on a tab shows its screen. A click on a row selects it.
    fn click(&mut self, x: u16, y: u16) {
        let hit = self.hits.iter().find(|(area, _)| area.contains(Position { x, y })).map(|&(_, hit)| hit);
        match hit {
            Some(Hit::Screen(screen)) => {
                self.overlay = None;
                self.screen = screen;
            }
            Some(Hit::Row(row)) if self.overlay.is_none() => *self.selected() = row,
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.hits.clear();
        let [tabs, body, bar] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
        let mut x = tabs.x;
        for &(key, name, screen) in &SCREENS {
            let text = format!(" {key} {name} ");
            let width = text.chars().count() as u16;
            let style = if screen == self.screen { Style::new().reversed() } else { Style::new() };
            let area = Rect { x, width, ..tabs }.intersection(tabs);
            frame.render_widget(Span::styled(text, style), area);
            self.hits.push((area, Hit::Screen(screen)));
            x += width + 1;
        }
        let body = Rect { y: body.y + 1, height: body.height.saturating_sub(1), ..body };
        match self.screen {
            Screen::Live => self.draw_live(frame, body),
            Screen::Sources => self.draw_sources(frame, body),
            Screen::Store => self.draw_store(frame, body),
            Screen::Runs => self.draw_runs(frame, body),
        }
        self.draw_bar(frame, bar);
        if let Some(overlay) = self.overlay {
            self.draw_overlay(frame, body, overlay);
        }
    }

    fn draw_sources(&mut self, frame: &mut Frame, area: Rect) {
        let header = ["SOURCE", "LICENCE", "R2 COPY", "LIVE", "AGE", "POLICY", "STATE"];
        let mut table = vec![header.map(String::from).to_vec()];
        table.extend(self.sources.iter().map(SourceRow::cells));
        let widths = widths(&table);
        let mut lines = Vec::new();
        let mut at = Vec::new();
        let mut end = 0;
        for (i, (row, cells)) in self.sources.iter().zip(&table[1..]).enumerate() {
            if row.source.kind == Kind::Tool && (i == 0 || self.sources[i - 1].source.kind != Kind::Tool) {
                lines.extend([Line::default(), Line::from("tools").dim()]);
            }
            at.push(lines.len());
            let (last, rest) = cells.split_last().expect("a row has cells");
            let prefix = row_text(&[rest, &[String::new()]].concat(), &widths);
            let line = Line::from(vec![Span::raw(prefix), Span::styled(last.clone(), state_style(row.state))]);
            lines.push(if i == self.source { line.reversed() } else { line });
            if i == self.source {
                lines.extend(expansion(row));
                end = lines.len() - 1;
            }
        }
        self.draw_lines(frame, area, Line::from(row_text(&table[0], &widths)), lines, &at, end);
    }

    fn draw_store(&mut self, frame: &mut Frame, area: Rect) {
        let mut table = vec![["ENTRY", "SIZE", "KEPT BECAUSE"].map(String::from).to_vec()];
        let plan = &self.store.as_ref().unwrap_or(&NO_PLAN).store;
        table
            .extend(plan.kept.iter().map(|kept| vec![kept.entry.clone(), bytes(kept.bytes), kept.because.join(" · ")]));
        let unused = !plan.objects.is_empty();
        if unused {
            let objects = gc::objects_text(plan.objects.len() as u64);
            table.push(vec![objects, bytes(plan.remove_bytes), "unused".into()]);
        }
        let widths = widths(&table);
        let mut lines: Vec<Line> = table[1..].iter().map(|cells| Line::from(row_text(cells, &widths))).collect();
        if unused {
            let last = lines.len() - 1;
            lines[last] = lines[last].clone().yellow();
        }
        if let Some(line) = lines.get_mut(self.kept) {
            *line = line.clone().reversed();
        }
        let at: Vec<usize> = (0..lines.len()).collect();
        self.draw_lines(frame, area, Line::from(row_text(&table[0], &widths)), lines, &at, self.kept);
    }

    fn draw_runs(&mut self, frame: &mut Frame, area: Rect) {
        let mut table = vec![["WHEN", "COMMAND", "", "TOOK", "MOVED"].map(String::from).to_vec()];
        table.extend(self.runs.iter().map(|run| {
            let run = &run.summary;
            let when = run.started.get(..16).unwrap_or(&run.started).replace('T', " ");
            let took = run.wall_ms.map_or("—".into(), duration);
            vec![when, run.command.clone(), mark(run.outcome).into(), took, moved(run)]
        }));
        let widths = widths(&table);
        let mut lines = Vec::new();
        for (i, cells) in table[1..].iter().enumerate() {
            let line = Line::from(row_text(cells, &widths));
            lines.push(if i == self.run { line.reversed() } else { line });
        }
        let height = (lines.len() as u16 + 1).min(area.height / 2);
        let [list, steps] = Layout::vertical([Constraint::Length(height + 1), Constraint::Fill(1)]).areas(area);
        let at: Vec<usize> = (0..lines.len()).collect();
        self.draw_lines(frame, list, Line::from(row_text(&table[0], &widths)), lines, &at, self.run);
        if let Some(run) = self.runs.get(self.run) {
            frame.render_widget(Paragraph::new(steps_lines(run)), steps);
        }
    }

    /// Draw `header`, and under it `lines` scrolled so that line `end` shows. Row `i` starts at
    /// line `at[i]`.
    fn draw_lines(&mut self, frame: &mut Frame, area: Rect, header: Line, lines: Vec<Line>, at: &[usize], end: usize) {
        let [head, area] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        frame.render_widget(header.dim(), head);
        let offset = (end + 1).saturating_sub(area.height as usize);
        for (row, &line) in at.iter().enumerate() {
            if let Some(y) = line.checked_sub(offset).filter(|&y| y < area.height as usize) {
                self.hits.push((Rect { y: area.y + y as u16, height: 1, ..area }, Hit::Row(row)));
            }
        }
        frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), area);
    }

    fn draw_bar(&self, frame: &mut Frame, area: Rect) {
        if self.busy {
            frame.render_widget(Line::from("working…"), area);
            return;
        }
        let mut spans = Vec::new();
        for (label, does) in self.bindings().into_iter().filter_map(|binding| binding.bar) {
            spans.extend([Span::from(label).bold(), Span::from(format!(" {does}   "))]);
        }
        if let Some(notice) = &self.notice {
            spans.push(Span::styled(notice.clone(), Color::Red));
        }
        frame.render_widget(Line::from(spans), area);
    }

    fn draw_overlay(&mut self, frame: &mut Frame, body: Rect, overlay: Overlay) {
        let id = self.sources.get(self.source).map_or("", |row| row.source.id.as_str());
        // `focus` is the line of the choice, which always shows; `footer` shows under the lines.
        let (title, lines, footer, focus) = match overlay {
            Overlay::Help => (" HELP ".to_string(), help(), Vec::new(), None),
            Overlay::Attribution => (" ATTRIBUTION ".into(), attribution(&self.sources), Vec::new(), None),
            Overlay::Policy => (format!(" POLICY · {id} "), self.policy_lines(), Vec::new(), Some(self.choice)),
            Overlay::Clean => (" CLEAN ".into(), self.clean_lines(), Vec::new(), None),
            Overlay::Region => (format!(" REGION · {LIVE} "), self.region_lines(), Vec::new(), Some(self.choice + 1)),
            Overlay::Plan => {
                let (lines, footer, focus) = self.plan_lines();
                (format!(" PLAN · {LIVE} "), lines, footer, focus)
            }
        };
        let width = body.width * 4 / 5;
        let wrapped = |lines| Paragraph::new(lines).wrap(Wrap { trim: false });
        let (paragraph, footer) = (wrapped(lines), wrapped(footer));
        let height = paragraph.line_count(width.saturating_sub(2));
        let footer_height = footer.line_count(width.saturating_sub(2));
        let outer = Constraint::Length((height + footer_height + 2) as u16);
        let area = body.centered(Constraint::Length(width), outer).intersection(body);
        let block = Block::bordered().title(title);
        let [top, bottom] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(footer_height as u16)]).areas(block.inner(area));
        let shown = top.height as usize;
        let scroll = match focus {
            Some(line) => (line + 1).saturating_sub(shown),
            None => {
                self.scroll = self.scroll.min(height.saturating_sub(shown));
                self.scroll
            }
        };
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        frame.render_widget(paragraph.scroll((scroll as u16, 0)), top);
        frame.render_widget(footer, bottom);
    }

    fn region_lines(&self) -> Vec<Line<'static>> {
        let filter = match (self.filtering, self.filter.is_empty()) {
            (false, true) => Span::from("filter").dim(),
            (true, _) => Span::from(format!("{}▏", self.filter)),
            (false, false) => Span::from(self.filter.clone()),
        };
        if let Err(error) = &self.regions {
            return vec![Line::styled(error.clone(), Color::Red)];
        }
        let mut lines = vec![Line::from(vec![Span::from("/ ").bold(), filter])];
        let current = self.env.as_ref().map(|env| env.region.as_str());
        for (i, id) in self.shown_regions().into_iter().enumerate() {
            let line = Line::from(format!("{} {id}", if Some(id) == current { "●" } else { " " }));
            lines.push(if i == self.choice { line.reversed() } else { line });
        }
        lines
    }

    fn policy_lines(&self) -> Vec<Line<'static>> {
        let current = self.sources[self.source].source.refresh;
        let lines = Refresh::ALL.iter().enumerate().map(|(i, &refresh)| {
            let mark = if refresh == current { "●" } else { " " };
            let line = Line::from(format!("{mark} {refresh}"));
            if i == self.choice {
                line.reversed()
            } else {
                line
            }
        });
        lines.collect()
    }

    fn clean_lines(&self) -> Vec<Line<'static>> {
        let plan = self.store.as_ref().unwrap_or(&NO_PLAN);
        if plan.is_empty() {
            return vec![Line::from("nothing to clean")];
        }
        let gc = &plan.store;
        let mut lines: Vec<Line> =
            gc.snapshots.iter().map(|snapshot| Line::from(format!("snapshot {snapshot}"))).collect();
        let objects = gc::objects_text(gc.objects.len() as u64);
        lines.push(Line::from(format!("{objects}  {}", bytes(gc.remove_bytes))));
        for dir in plan.import.dirs.iter().filter(|dir| dir.files > 0) {
            lines.push(Line::from(format!("move {}  {} files  {}", dir.dir.display(), dir.files, bytes(dir.bytes))));
        }
        if self.asking {
            lines.extend([Line::default(), Line::from(plan.question()).bold()]);
        }
        lines
    }
}

fn state_style(state: State) -> Style {
    Style::new().fg(match state {
        State::Ok => Color::Green,
        State::Blocked => Color::Red,
        State::NotApplied => Color::Cyan,
        State::Stale | State::CodeChanged | State::InputChanged => Color::Yellow,
    })
}

/// The lines under the selected source.
fn expansion(row: &SourceRow) -> Vec<Line<'static>> {
    let line =
        |label: &str, value: String| Line::from(vec![Span::from(format!("    {label:<13}")).dim(), value.into()]);
    let s = &row.source;
    let mut lines = Vec::new();
    if s.kind != Kind::Tool {
        let licence = [s.licence.clone().or(Some("not recorded".into())), s.obligations.clone(), s.licence_url.clone()];
        lines.push(line("licence", licence.into_iter().flatten().collect::<Vec<_>>().join(" · ")));
        lines.push(line("attribution", s.attribution.clone().unwrap_or_else(|| "—".into())));
    }
    let snapshots: Vec<String> =
        row.snapshots.iter().map(|snapshot| format!("{} {}", snapshot.version, bytes(snapshot.bytes))).collect();
    lines.push(line("snapshots", if snapshots.is_empty() { "—".into() } else { snapshots.join(" · ") }));
    if let Some(reason) = &row.reason {
        lines.push(line("reason", reason.clone()));
    }
    lines
}

fn moved(run: &Summary) -> String {
    let sizes = [(run.bytes_fetched, "fetched"), (run.bytes_built, "built")];
    let moved: Vec<String> =
        sizes.into_iter().filter(|&(size, _)| size > 0).map(|(size, what)| format!("{} {what}", bytes(size))).collect();
    if moved.is_empty() {
        "—".into()
    } else {
        moved.join(" · ")
    }
}

fn steps_lines(run: &Details) -> Vec<Line<'static>> {
    const BAR: usize = 24;
    let built = |step: &&runs::RunStep| step.receipt.as_ref().filter(|_| !step.reused).map(|r| r.wall_ms);
    let longest = run.steps.iter().filter_map(|step| built(&step)).max().unwrap_or(0).max(1);
    let mut table = vec![["STEP", "DURATION", "", "Δ LAST RUN", "PEAK RAM", "OUTPUT"].map(String::from).to_vec()];
    for step in &run.steps {
        let eighths = built(&step).map_or(0, |ms| (ms * BAR as u64 * 8 / longest) as usize);
        let partial = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"][eighths % 8];
        let bar = "█".repeat(eighths / 8) + partial;
        let [took, change, ram, output] = step_cells(step);
        table.push(vec![step.step.clone(), format!("{bar:<BAR$}"), took, change, ram, output]);
    }
    let widths = widths(&table);
    let mut lines: Vec<Line> = table.iter().map(|cells| Line::from(row_text(cells, &widths))).collect();
    lines[0] = lines[0].clone().dim();
    lines.extend(run.error.clone().map(|error| Line::styled(error, Color::Red)));
    lines
}

/// Each attribution of a data source or an asset, with the sources that carry it.
fn attribution(sources: &[SourceRow]) -> Vec<Line<'static>> {
    let mut credits: Vec<(&str, Vec<&str>)> = Vec::new();
    for s in sources.iter().map(|row| &row.source).filter(|s| s.kind != Kind::Tool) {
        let Some(text) = s.attribution.as_deref() else { continue };
        match credits.iter_mut().find(|(credit, _)| *credit == text) {
            Some((_, ids)) => ids.push(&s.id),
            None => credits.push((text, vec![&s.id])),
        }
    }
    let line = |(text, ids): (&str, Vec<&str>)| {
        Line::from(vec![Span::from(text.to_string()), Span::from(format!("  {}", ids.join(", "))).dim()])
    };
    credits.into_iter().map(line).collect()
}

fn help() -> Vec<Line<'static>> {
    let line = |keys: &str, does: String| Line::from(vec![Span::from(format!("{keys:<12}")).bold(), does.into()]);
    let numbers: Vec<String> = SCREENS.iter().map(|(key, _, _)| key.to_string()).collect();
    let mut lines = vec![
        line(&format!("{} tab", numbers.join(" ")), "screens".into()),
        line("↑ ↓ j k", "move".into()),
        line("p", "plan".into()),
        line("u", format!("undo the edits of data/env/{LIVE}.toml")),
        line("esc", "close".into()),
        line("q", "quit".into()),
        line("?", "help".into()),
        Line::default(),
    ];
    for &(_, name, screen) in &SCREENS {
        let keys: Vec<String> = screen_keys(screen).iter().map(|(_, _, key, does)| format!("{key} {does}")).collect();
        if !keys.is_empty() {
            lines.push(line(name, keys.join(" · ")));
        }
    }
    lines.push(line("Region", "/ filter · enter choose".into()));
    lines.push(line("Plan", "space take or leave a move · d steps".into()));
    lines.push(line("Policy", "enter choose".into()));
    lines.push(line("Clean", "a clean, then y".into()));
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use serde_json::{json, Value};

    use super::*;
    use crate::cli::build_cli::EnvPlan;
    use crate::cli::status_cli::{Attention, AttentionKind, LayerStatus, ProductStatus};
    use crate::cli::Stored;
    use crate::sources::parse_sources;
    use crate::store::gc::Kept;

    const SOURCES: &str = r#"
        [[source]]
        id = "osm"
        kind = "data"
        licence = "ODbL-1.0"
        attribution = "© OpenStreetMap contributors"
        fetch = { kind = "http", url = "https://planet.openstreetmap.org/pbf/planet-{yymmdd}.osm.pbf" }
        version = "date"
        refresh = 7
        redistribute = true

        [[source]]
        id = "planetiler"
        kind = "tool"
        fetch = { kind = "github", url = "https://api.github.com/repos/onthegomap/planetiler" }
        version = "release"
        refresh = "manual"
        redistribute = true
    "#;

    const REGION: &str = "europe/germany/baden-wuerttemberg";

    /// What `status --json` writes: `maps` is unknown, and `planner` has the optional layer `sun`
    /// on, which live lacks.
    fn status() -> Status {
        let layer = |layer: &str, state, reason: &str| LayerStatus {
            layer: layer.into(),
            state,
            reason: Some(reason.into()).filter(|reason: &String| !reason.is_empty()),
        };
        let product =
            |product: &str, release: &str, applied: Option<&str>, bytes, optional: &[&str], layers| ProductStatus {
                product: product.into(),
                release: Some(release.repeat(8)),
                applied: applied.map(str::to_string),
                bytes: Some(bytes),
                optional: optional.iter().map(|layer| layer.to_string()).collect(),
                layers,
            };
        let planner = vec![
            layer("planner/basemap", State::Stale, "osm: 120 d > 90 d"),
            layer("planner/routing", State::CodeChanged, "host/route-build/src/main.rs"),
            layer("planner/overlays", State::InputChanged, "planner/routing"),
            layer("planner/sun", State::NotApplied, "missing in live"),
        ];
        let attention =
            |kind, about: &str, reason: &str| Attention { kind, about: about.into(), reason: reason.into() };
        Status {
            from: "https://maps.openbikecomputer.com".into(),
            products: vec![
                product("maps", "3f9a2c1e", None, 980_000_000, &[], None),
                product(
                    "planner",
                    "8b0d47a5",
                    Some("2026-10-02T09:14:05Z"),
                    2_370_000_000,
                    &["climate", "sun"],
                    Some(planner),
                ),
            ],
            attention: vec![
                attention(AttentionKind::Stale, "osm", "120 d > 90 d"),
                attention(AttentionKind::OldCache, "/home/rider/obc-bake", "12 files, 1.2 GB"),
                attention(AttentionKind::Unreachable, "maps", "a fetch that the step list needs failed"),
            ],
            check: None,
        }
    }

    /// What `plan live --json` writes: the edit of `sun`, two stale sources and a change of code.
    fn plan() -> EnvPlan {
        let build = |step: &str, minutes: u64, bytes_out: u64| {
            let estimate = json!({"wall_ms": minutes * 60_000, "bytes_out": bytes_out, "peak_rss_bytes": null});
            json!({"step": step, "recipe": "recipe", "key": null, "estimate": estimate})
        };
        let fetch = |source: &str, version: &str, bytes: u64| json!([{"source": source, "version": version, "params": [], "files": [], "bytes": bytes}]);
        let group = |id: &str, cause: Value, fetches: Value, builds: Vec<Value>| json!({"id": id, "cause": cause, "layers": builds, "drops": [], "fetches": fetches, "builds": builds});
        let (basemap, routing) =
            (build("planner/basemap", 41, 324_000_000), build("planner/routing", 37, 1_040_000_000));
        let groups = [
            group("layers", json!({"kind": "layers"}), json!([]), vec![build("planner/sun", 55, 30_000_000)]),
            group(
                "move:land",
                json!({"kind": "move", "source": "land", "from": ["2024-01-01"], "to": "2024-01-03"}),
                fetch("land", "2024-01-03", 950_000_000),
                vec![basemap.clone()],
            ),
            group(
                "move:osm",
                json!({"kind": "move", "source": "osm", "from": ["2024-01-02"], "to": "2024-01-09"}),
                fetch("osm", "2024-01-09", 710_000_000),
                vec![basemap, routing.clone()],
            ),
            group(
                "code:planner/routing",
                json!({"kind": "code", "paths": ["host/route-build"], "crates": []}),
                json!([]),
                vec![routing, build("planner/overlays", 7, 63_000_000)],
            ),
        ];
        serde_json::from_value(json!({
            "env": "live",
            "region": REGION,
            "layers": ["sun"],
            "moves": {"land": "2024-01-03", "osm": "2024-01-09"},
            "versions": [],
            "live": [{"product": "planner", "release": "8b0d47a5".repeat(8)}],
            "edits": [{"kind": "layers", "product": "planner", "on": ["sun"], "off": []}],
            "only": [],
            "groups": groups,
            "blocked": [{"product": "maps", "reason": "source `wikidata` is blocked", "layers": []}],
            "remove": [{"key": "planner/objects/aa", "bytes": 1_100_000_000}, {"key": "planner/objects/bb", "bytes": 5_000_000}],
            "listed": false,
        }))
        .unwrap()
    }

    fn app() -> App {
        let sources = parse_sources(SOURCES).unwrap().into_iter().map(|source| SourceRow {
            live: Some(vec![if source.kind == Kind::Tool { "0.10.2" } else { "2024-01-02" }.into()]),
            upstream: (source.kind == Kind::Data).then(|| "2024-01-09".into()),
            age_days: None,
            state: State::Ok,
            reason: None,
            snapshots: vec![Stored { version: "2024-01-02".into(), bytes: 1_000_000 }],
            requests: Vec::new(),
            credential_missing: false,
            source,
        });
        let run = |id: &str| Details {
            summary: Summary {
                id: id.into(),
                command: "build test".into(),
                started: "2024-01-10T12:00:00Z".into(),
                outcome: Outcome::Ok,
                wall_ms: Some(1000),
                bytes_fetched: 0,
                bytes_built: 10,
            },
            error: None,
            fetches: Vec::new(),
            steps: Vec::new(),
        };
        let mut app = App::new(sources.collect(), vec![run("2024-01-10-120000"), run("2024-01-09-120000")]);
        let store = gc::Plan {
            kept: vec![Kept { entry: "osm@2024-01-02".into(), bytes: 1_000_000, because: vec!["live maps".into()] }],
            snapshots: vec!["osm@2023-12-01".into()],
            objects: vec![("ab".repeat(32), 1_000_000)],
            remove_bytes: 1_000_000,
            ..gc::Plan::default()
        };
        app.store = Some(CleanPlan { store, ..CleanPlan::default() });
        app.status = Some(status());
        app.env = Some(Edited { env: LIVE.into(), region: REGION.into(), layers: vec!["sun".into()] });
        app.edited = true;
        app.regions = Ok(["europe/andorra", REGION, "monaco"].map(String::from).to_vec());
        app
    }

    /// Plan over an empty screen, with group `group` selected.
    fn planned(group: usize, steps: bool, skipped: &[&str]) -> App {
        let skipped = skipped.iter().map(|id| id.to_string()).collect();
        let plan = PlanView { group, steps, skipped, ..PlanView::new(plan()) };
        App { screen: Screen::Runs, runs: Vec::new(), overlay: Some(Overlay::Plan), plan: Some(plan), ..app() }
    }

    fn view(app: &App) -> String {
        let plan = app.plan.as_ref().map(|plan| (plan.group, plan.steps, plan.skipped.clone()));
        let selected = [app.row, app.source, app.kept, app.run, app.choice];
        format!("{:?}", (app.screen, app.overlay, selected, app.asking, (&app.filter, app.filtering), plan))
    }

    fn opened(overlay: Overlay, source: usize, choice: usize) -> App {
        let mut app = App { screen: Screen::Sources, source, ..app() };
        app.act(Action::Open(overlay));
        App { choice, ..app }
    }

    /// The text of each line of the screen.
    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let lines = buffer.content.chunks(width as usize);
        lines.map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn no_key_does_two_things_and_each_key_in_the_bar_acts() {
        let mut states = vec![app(), App::new(Vec::new(), Vec::new())];
        states.extend((0..app().live_rows().len()).map(|row| App { row, ..app() }));
        for screen in [Screen::Sources, Screen::Store, Screen::Runs] {
            states.push(App { screen, ..app() });
        }
        states.push(App { screen: Screen::Sources, source: 1, ..app() });
        for overlay in [Overlay::Help, Overlay::Attribution, Overlay::Policy, Overlay::Clean, Overlay::Region] {
            states.push(opened(overlay, 0, 0));
            states.push(opened(overlay, 1, 1));
        }
        states.push(App { asking: true, ..opened(Overlay::Clean, 0, 0) });
        states.push(App { store: Some(CleanPlan::default()), ..opened(Overlay::Clean, 0, 0) });
        states.push(App { screen: Screen::Store, store: Some(CleanPlan::default()), ..app() });
        states.push(App { filtering: true, ..opened(Overlay::Region, 0, 0) });
        states.push(App { filtering: true, filter: "mon".into(), ..opened(Overlay::Region, 0, 0) });
        for group in 0..4 {
            states.extend([
                planned(group, false, &[]),
                planned(group, true, &[]),
                planned(group, false, &["move:land"]),
            ]);
        }
        for app in states {
            let keys = app.bindings();
            for binding in &keys {
                assert_eq!(keys.iter().filter(|other| other.key == binding.key).count(), 1, "{:?}", binding.key);
            }
            for binding in keys.iter().filter(|binding| binding.bar.is_some()) {
                let mut after = app.clone();
                let effect = after.act(binding.action);
                assert!(effect != Effect::None || view(&after) != view(&app), "{:?} in {}", binding.key, view(&app));
            }
        }
    }

    #[test]
    fn live_shows_each_product_the_optional_layers_and_what_needs_attention() {
        let mut app = app();
        let drawn = screen(&mut app, 80, 18);
        let live = [
            " 1 Live   3 Sources   4 Store   5 Runs",
            "",
            "LIVE from https://maps.openbikecomputer.com",
            "region  europe/germany/baden-wuerttemberg",
            "",
            "PRODUCT  RELEASE           APPLIED     SIZE      STATE",
            "maps     release 3f9a2c1e  —           980.0 MB  unknown",
            "planner  release 8b0d47a5  2026-10-02  2.37 GB   not applied",
            "",
            "OPTIONAL LAYERS",
            "[ ] climate",
            "[x] sun      not applied",
            "",
            "NEEDS ATTENTION",
            "stale        osm                   120 d > 90 d",
            "old cache    /home/rider/obc-bake  12 files, 1.2 GB",
            "unreachable  maps                  a fetch that the step list needs failed",
            "r region   R check R2   p plan   u undo   ? help",
        ];
        assert_eq!(drawn, live, "{drawn:#?}");
        assert_eq!(app.key(KeyCode::Char('R')), Effect::Status { check: true }, "only `R` lists R2");
        // 0 region, 1 maps, 2 planner, 3 climate, 4 sun, 5 stale, 6 old cache, 7 unreachable.
        app.row = 3;
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::Layer("climate".into(), Switch::On));
        app.row = 4;
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::Layer("sun".into(), Switch::Off));
        assert_eq!(app.key(KeyCode::Char('u')), Effect::Undo);
        app.row = 7;
        assert_eq!(app.key(KeyCode::Enter), Effect::None, "an unreachable product has no fix");
        (app.row, app.source) = (5, 1);
        app.key(KeyCode::Enter);
        assert_eq!((app.screen, app.source), (Screen::Sources, 0), "the stale source");
    }

    #[test]
    fn the_region_picker_filters_and_sets_the_region_of_live() {
        let mut app = app();
        app.key(KeyCode::Char('r'));
        assert_eq!((app.overlay, app.choice), (Some(Overlay::Region), 1), "the region of live");
        assert_eq!(app.key(KeyCode::Enter), Effect::None, "live has the region already");
        app.key(KeyCode::Char('/'));
        assert_eq!(app.key(KeyCode::Char('q')), Effect::None);
        assert!(app.shown_regions().is_empty(), "`q` types: it does not quit");
        app.key(KeyCode::Backspace);
        "and".chars().for_each(|c| drop(app.key(KeyCode::Char(c))));
        assert_eq!(app.shown_regions(), ["europe/andorra"]);
        assert_eq!(app.key(KeyCode::Enter), Effect::Region("europe/andorra".into()));
        assert_eq!(app.overlay, None);

        let mut broken = App { regions: Err("data/regions/monaco.toml: no `kind`".into()), ..app };
        broken.key(KeyCode::Char('r'));
        assert_eq!(broken.region_lines(), [Line::styled("data/regions/monaco.toml: no `kind`", Color::Red)]);
        assert!(broken.bindings().iter().all(|binding| binding.key != KeyCode::Char('/')), "nothing to filter");
    }

    #[test]
    fn plan_takes_or_leaves_only_a_move_and_always_shows_what_r2_loses() {
        let mut app = App { screen: Screen::Runs, runs: Vec::new(), ..app() };
        assert_eq!(app.key(KeyCode::Char('p')), Effect::Plan);
        app.plan = Some(PlanView::new(plan()));
        let drawn = screen(&mut app, 100, 18);
        let changes = [
            "          ┌ PLAN · live ─────────────────────────────────────────────────────────────────┐",
            "          │     CHANGE                             FETCH     TIME     OUTPUT             │",
            "          │[x]  planner +sun                                 55m 00s  30.0 MB            │",
            "          │[x]  move land 2024-01-01 → 2024-01-03  950.0 MB  41m 00s  324.0 MB           │",
            "          │[x]  move osm 2024-01-02 → 2024-01-09   710.0 MB  1h 18m   1.36 GB            │",
            "          │[x]  code of host/route-build                     44m 00s  1.10 GB            │",
        ];
        let footer = [
            "          │                                                                              │",
            "          │REMOVE FROM R2  2 keys, 1.10 GB                                               │",
            "          │⚠ blocked maps: source `wikidata` is blocked                                  │",
            "          │⚠ R2 was not listed, so leftovers are unknown                                 │",
            "          │TOTAL  fetch 1.66 GB · build 4 layers, 2h 20m · output 1.46 GB                │",
            "          └──────────────────────────────────────────────────────────────────────────────┘",
        ];
        assert_eq!(drawn[4..16], [&changes[..], &footer[..]].concat(), "{drawn:#?}");
        assert_eq!(drawn[17], "d steps   esc close", "only keys");
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::None, "an edit always goes");
        app.key(KeyCode::Down);
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(vec!["move:osm".into()]));
        app.key(KeyCode::Down);
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(vec!["none".into()]), "the last move too");
        app.key(KeyCode::Up);
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(vec!["move:land".into()]));
        app.key(KeyCode::Down);
        assert_eq!(app.key(KeyCode::Char(' ')), Effect::Select(Vec::new()));
        for key in [KeyCode::Enter, KeyCode::Char('a')] {
            assert_eq!(app.key(key), Effect::None);
        }
        app.key(KeyCode::Char('d'));
        let drawn = screen(&mut app, 100, 22);
        let steps = [
            "          │FETCH                                                                         │",
            "          │  land  2024-01-03  950.0 MB                                                  │",
            "          │  osm   2024-01-09  710.0 MB                                                  │",
            "          │BUILD                                                                         │",
            "          │  planner/sun       55m 00s                                                   │",
            "          │  planner/basemap   41m 00s                                                   │",
            "          │  planner/routing   37m 00s                                                   │",
            "          │  planner/overlays  7m 00s                                                    │",
        ];
        assert_eq!(drawn[5..19], [&steps[..], &footer[..]].concat(), "{drawn:#?}");
    }

    #[test]
    fn enter_in_policy_sets_another_policy_of_a_date_source() {
        let mut app = App { screen: Screen::Sources, ..app() };
        assert_eq!(app.key(KeyCode::Char('e')), Effect::None);
        assert_eq!((app.overlay, app.choice), (Some(Overlay::Policy), 0));
        app.key(KeyCode::Down);
        assert_eq!(app.key(KeyCode::Enter), Effect::Policy("osm".into(), Refresh::Days(30)));
        let mut tool = App { source: 1, ..app };
        tool.key(KeyCode::Char('e'));
        assert_eq!(tool.overlay, None, "a release version has no age");
    }

    #[test]
    fn a_clean_needs_a_then_y() {
        let mut app = App { screen: Screen::Store, ..app() };
        assert_eq!(app.key(KeyCode::Char('c')), Effect::PlanClean, "the plan of now");
        assert_eq!(app.overlay, Some(Overlay::Clean));
        for key in [KeyCode::Enter, KeyCode::Char('y')] {
            assert_eq!(app.key(key), Effect::None);
        }
        app.key(KeyCode::Char('a'));
        assert!(app.asking);
        app.key(KeyCode::Esc);
        assert_eq!((app.overlay, app.asking), (Some(Overlay::Clean), false));
        app.key(KeyCode::Char('a'));
        assert_eq!(app.key(KeyCode::Char('y')), Effect::Clean);
        assert_eq!(app.overlay, None);
        let mut empty = App { store: Some(CleanPlan::default()), ..app };
        empty.act(Action::Open(Overlay::Clean));
        assert_eq!(empty.key(KeyCode::Char('a')), Effect::None, "an empty plan has nothing to clean");
    }

    #[test]
    fn a_click_selects_a_row_or_shows_a_screen() {
        let mut app = App { screen: Screen::Sources, ..app() };
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut click = |app: &mut App, hit: Hit| {
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let (area, _) = *app.hits.iter().find(|(_, drawn)| *drawn == hit).unwrap();
            app.click(area.x, area.y)
        };
        click(&mut app, Hit::Row(1));
        assert_eq!((app.screen, app.overlay, app.source), (Screen::Sources, None, 1));
        click(&mut app, Hit::Screen(Screen::Store));
        assert_eq!((app.screen, app.overlay), (Screen::Store, None));
    }
}
