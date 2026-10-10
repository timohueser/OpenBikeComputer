//! The TUI of `obc data`. Each screen shows what a command writes with `--json`. Each change goes
//! through the shared command API. Preparation, build and apply retain their worker after exit.

mod background;
mod content;
mod execution;
mod host;
mod live;
mod local;
mod plan;
mod regions;
mod sources;

use std::io::Stdout;
use std::path::Path;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, KeyCode};
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::engine::runs::{self, Details, Outcome, Summary};
use crate::product::Product;
use crate::sources::{Kind, Refresh, State, VersionScheme};
use crate::store::{gc, Store};

use super::edit_cli::{self, Edited, Switch};
use super::runs_cli::{bytes, duration, mark, step_cells};
use super::status_cli::{self, Status};
use super::{row_text, source_listing, widths, CleanPlan, Error, SourceRow};
use live::{Fix, LiveRow};
use plan::PlanView;

/// The environment that Live and Plan show and edit.
const LIVE: &str = "live";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Live,
    Local,
    Sources,
    Store,
    Runs,
    Content,
}

/// Each screen has a stable keyboard number.
const SCREENS: [(char, &str, Screen); 6] = [
    ('1', "Live", Screen::Live),
    ('2', "Local", Screen::Local),
    ('3', "Sources", Screen::Sources),
    ('4', "Store", Screen::Store),
    ('5', "Runs", Screen::Runs),
    ('6', "Content", Screen::Content),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Help,
    Attribution,
    Source,
    Error,
    Policy,
    Clean,
    Region,
    Plan,
    Run,
    AppLogs,
    Version,
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
    LocalRefresh,
    LocalApp,
    LocalOpen,
    LocalLogs,
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
    Prepare,
    Build,
    ReviewApply,
    Apply,
    OpenRun,
    ObserveRun,
    StopRun,
    ConfirmStop,
    ReviewPrepared,
    Undo,
    Dismiss,
    SourceScope,
    SourceFilter,
    Reload,
    CustomPolicy,
    InputKey(KeyCode),
    NewRegion,
    BoxRegion,
    DeleteRegion,
    LoadAreas,
    Close,
    Quit,
    ContentConfigure,
    ContentPrepare,
    ContentReview,
    ContentPublish,
    ContentUse,
}

/// What the loop does after a key or a click.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    None,
    Quit,
    Initial,
    Areas,
    LoadAreas,
    CreateRegion(super::regions_cli::Create),
    ReviewRegionDeletion(String),
    DeleteRegion(super::regions_cli::Deletion),
    /// `obc data policy SOURCE REFRESH`.
    Policy(String, Refresh),
    /// `obc data sources --check-now`.
    CheckNow,
    /// `obc data clean`, for Clean.
    PlanClean,
    /// `obc data clean --apply` of the plan that Clean shows.
    Clean,
    /// `obc data status [--check]`, and the stored Live settings, for Live.
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
    Start(crate::operation::Kind, Box<super::build_cli::EnvPlan>),
    LocalCheck(crate::dev::Request),
    LocalReview(crate::dev::Request),
    LocalRegion(String),
    LocalLayer(String, Switch),
    LocalStart(crate::dev::Request),
    LocalControl(local::Control, crate::dev::App),
    LocalState,
    ObserveRun(String),
    StopRun(String),
    Versions(String),
    Reload,
    ContentRead,
    ContentConfigure(String),
    ContentPrepare,
    ContentReview(String),
    ContentPublish(String),
    ContentUse(String),
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
        Screen::Local => &[
            (KeyCode::Char('r'), Action::Open(Overlay::Region), "r", "region"),
            (KeyCode::Char(' '), Action::Toggle, "space", "toggle"),
            (KeyCode::Char('R'), Action::CheckNow, "R", "check Local"),
            (KeyCode::Char('f'), Action::LocalRefresh, "f", "check Live inputs"),
            (KeyCode::Char('b'), Action::Open(Overlay::Plan), "b", "review build"),
            (KeyCode::Char('s'), Action::LocalApp, "s", "start / stop"),
            (KeyCode::Char('o'), Action::LocalOpen, "o", "open"),
            (KeyCode::Char('l'), Action::LocalLogs, "l", "logs"),
        ],
        Screen::Sources => &[
            (KeyCode::Enter, Action::Open(Overlay::Source), "enter", "details"),
            (KeyCode::Char('f'), Action::SourceScope, "f", "scope"),
            (KeyCode::Char('/'), Action::SourceFilter, "/", "filter"),
            (KeyCode::Char('e'), Action::Open(Overlay::Policy), "e", "policy"),
            (KeyCode::Char('v'), Action::Open(Overlay::Version), "v", "version"),
            (KeyCode::Char('R'), Action::CheckNow, "R", "check upstream"),
            (KeyCode::Char('L'), Action::Open(Overlay::Attribution), "L", "attribution"),
        ],
        Screen::Store => &[(KeyCode::Char('c'), Action::Open(Overlay::Clean), "c", "clean")],
        Screen::Runs => &[(KeyCode::Enter, Action::OpenRun, "enter", "run")],
        Screen::Content => &[
            (KeyCode::Char('c'), Action::ContentConfigure, "c", "configure inputs"),
            (KeyCode::Char('f'), Action::ContentPrepare, "f", "prepare snapshot"),
            (KeyCode::Char('R'), Action::CheckNow, "R", "reload"),
            (KeyCode::Char('p'), Action::ContentReview, "p", "review publication"),
            (KeyCode::Char('y'), Action::ContentPublish, "y", "publish reviewed version"),
            (KeyCode::Enter, Action::ContentUse, "enter", "use version"),
        ],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    Screen(Screen),
    Row(usize),
    Action(Action),
    Region(regions::Target),
}

#[derive(Clone)]
struct App {
    content: content::View,
    host: String,
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
    /// the stored Live settings.
    env: Option<Edited>,
    /// Pending settings differ from the applied settings: `undo` discards them.
    edited: bool,
    /// The id of each region, or why `data/regions/` could not be read.
    regions: Result<Vec<crate::regions::Region>, String>,
    /// The filter of Region, and whether keys type into it.
    filter: String,
    filtering: bool,
    region_editor: regions::Editor,
    source_view: sources::View,
    policy_days: Option<String>,
    versions: Option<super::versions::Versions>,
    moves: std::collections::BTreeMap<String, Option<String>>,
    reload: bool,
    /// The plan of Plan, once it is made.
    plan: Option<PlanView>,
    execution: execution::Execution,
    local: local::View,
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
    notice: Option<Error>,
    /// Last successful file write, distinct from unsaved inputs and environment Undo.
    saved: Option<String>,
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
        partial_bytes: 0,
    },
};

pub fn run(root: &Path, products: &[&dyn Product]) -> Result<std::process::ExitCode, Error> {
    let store = Store::open()?;
    let mut app = App::new(Vec::new(), list_runs(&store)?);
    if let Ok(screen) = std::env::var(crate::worker::SCREEN) {
        if let Some((_, _, selected)) = SCREENS.iter().find(|(key, _, _)| key.to_string() == screen) {
            app.screen = *selected;
        }
    }
    app.regions = crate::settings::regions(root, &store).map(|regions| regions.iter().cloned().collect());
    app.notice = app.regions.clone().err().map(|message| super::Code::InvalidData.error(message));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        stop();
        previous(info);
    }));
    let result = start()
        .inspect_err(|_| stop())
        .and_then(|mut tui| background::run_loop(root, products, &store, &mut app, &mut tui));
    stop();
    result.map(|()| {
        if app.reload {
            let index = SCREENS.iter().position(|(_, _, screen)| *screen == app.screen).unwrap_or(0);
            std::process::ExitCode::from(crate::worker::RELOAD_EXIT + index as u8)
        } else {
            std::process::ExitCode::SUCCESS
        }
    })
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
    for run in &mut runs {
        if matches!(
            crate::operation::status(store, &run.summary.id)?,
            Some(
                crate::operation::Status::Starting
                    | crate::operation::Status::Running
                    | crate::operation::Status::Stopping
            )
        ) {
            run.summary.outcome = Outcome::Running;
        }
    }
    runs.sort_by_key(|run| run.summary.outcome != Outcome::Running);
    Ok(runs)
}

impl App {
    fn new(sources: Vec<SourceRow>, runs: Vec<Details>) -> Self {
        Self {
            content: content::View::default(),
            host: host::current(),
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
            region_editor: regions::Editor::default(),
            source_view: sources::View::default(),
            policy_days: None,
            versions: None,
            moves: Default::default(),
            reload: false,
            plan: None,
            execution: execution::Execution::default(),
            local: local::View::default(),
            row: 0,
            source: 0,
            kept: 0,
            run: 0,
            scroll: 0,
            choice: 0,
            asking: false,
            notice: None,
            saved: None,
            hits: Vec::new(),
        }
    }

    /// Read the sources again; with `check_now`, after a check of upstream now.
    fn reload(&mut self, root: &Path, products: &[&dyn Product], check_now: bool) -> Result<(), Error> {
        (self.sources, self.live) = source_listing(root, products, check_now)?;
        self.regions = crate::settings::regions(root, &Store::open()?).map(|regions| regions.iter().cloned().collect());
        Ok(())
    }

    /// Read the stored Live settings and `status [--check]` again.
    fn read_live(&mut self, root: &Path, products: &[&dyn Product], check: bool) -> Result<(), Error> {
        self.edited = edit_cli::edited(root, &Store::open()?, LIVE);
        let env = edit_cli::current(root, &Store::open()?, LIVE);
        self.env = env.as_ref().ok().cloned();
        let status = status_cli::read(root, products, check);
        self.status = status.as_ref().ok().cloned();
        self.row = self.row.min(self.live_rows().len() - 1);
        env.and(status).map(drop)
    }

    fn rows(&self) -> usize {
        match self.screen {
            Screen::Live => self.live_rows().len(),
            Screen::Local => self.local.rows().len(),
            Screen::Sources => self.source_view.rows(&self.sources).len(),
            Screen::Store => self
                .store
                .as_ref()
                .map_or(0, |plan| plan.store.kept.len() + usize::from(!plan.store.objects.is_empty())),
            Screen::Runs => self.runs.len(),
            Screen::Content => self.content.versions().len(),
        }
    }

    /// The number of choices of an overlay with a selection.
    fn choices(&self) -> Option<usize> {
        match self.overlay? {
            Overlay::Policy => Some(Refresh::ALL.len()),
            Overlay::Version => self.versions.as_ref().map(|v| v.common.len() + 2),
            Overlay::Region if self.region_editor.mode.is_none() => Some(self.shown_regions().len()),
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
            (Some(Overlay::Region), _) if self.region_editor.mode.is_some() => &mut self.scroll,
            (Some(Overlay::Policy | Overlay::Region | Overlay::Version), _) => &mut self.choice,
            (Some(_), _) => &mut self.scroll,
            (None, Screen::Live) => &mut self.row,
            (None, Screen::Local) => &mut self.local.row,
            (None, Screen::Sources) => &mut self.source,
            (None, Screen::Store) => &mut self.kept,
            (None, Screen::Runs) => &mut self.run,
            (None, Screen::Content) => &mut self.content.row,
        }
    }

    /// The regions whose id has the filter of Region.
    fn shown_regions(&self) -> Vec<&str> {
        let filter = self.filter.to_lowercase();
        let regions = self.regions.iter().flatten();
        regions
            .filter(|region| region.id.contains(&filter) || region.name.to_lowercase().contains(&filter))
            .map(|region| region.id.as_str())
            .collect()
    }

    /// Keys type into the filter of Region.
    fn typing(&self) -> bool {
        self.overlay == Some(Overlay::Region) && (self.filtering || self.region_editor.mode.is_some())
            || self.overlay.is_none() && self.screen == Screen::Sources && self.source_view.typing
            || self.overlay == Some(Overlay::Policy) && self.policy_days.is_some()
    }

    /// Whether `enter` in Policy or Region chooses another policy or region.
    fn chooses(&self) -> bool {
        match self.overlay {
            Some(Overlay::Version) => self.versions.as_ref().is_some_and(|v| {
                self.choice == 0 || self.choice == 1 && v.newest || self.choice >= 2 && self.choice < v.common.len() + 2
            }),
            Some(Overlay::Policy) => {
                let row = self.sources.get(self.source);
                row.is_some_and(|row| Refresh::ALL[self.choice] != row.source.refresh)
            }
            Some(Overlay::Region) => {
                let region = self.shown_regions().get(self.choice).map(|id| id.to_string());
                region.is_some() && region != self.current_env().map(|env| env.region.clone())
            }
            _ => false,
        }
    }

    /// Whether a key does something for the selected row.
    fn works(&self, action: Action) -> bool {
        if matches!(
            action,
            Action::ContentConfigure
                | Action::ContentPrepare
                | Action::ContentReview
                | Action::ContentPublish
                | Action::ContentUse
        ) {
            if self.busy || self.content.draft.is_some() {
                return false;
            }
            return match action {
                Action::ContentPrepare => {
                    self.content.status.as_ref().is_some_and(|status| status["configured"] == true)
                }
                Action::ContentReview | Action::ContentUse => self.content.digest().is_some(),
                Action::ContentPublish => self.content.review.is_some(),
                _ => true,
            };
        }
        if self.busy
            && matches!(
                action,
                Action::Choose
                    | Action::CheckNow
                    | Action::Clean
                    | Action::Toggle
                    | Action::Undo
                    | Action::LoadAreas
                    | Action::DeleteRegion
                    | Action::Prepare
                    | Action::Build
                    | Action::ReviewApply
                    | Action::Apply
                    | Action::ObserveRun
                    | Action::StopRun
                    | Action::ConfirmStop
                    | Action::ReviewPrepared
                    | Action::LocalRefresh
                    | Action::LocalApp
                    | Action::LocalOpen
                    | Action::LocalLogs
                    | Action::Open(Overlay::Clean | Overlay::Plan | Overlay::Version)
            )
        {
            return false;
        }
        match action {
            Action::Open(Overlay::Source | Overlay::Version) => self.source_visible(),
            Action::Reload => crate::worker::can_reload(),
            Action::Open(Overlay::Policy) => {
                self.source_visible()
                    && self.sources.get(self.source).is_some_and(|row| row.source.version == VersionScheme::Date)
            }
            Action::Open(Overlay::Clean) => self.store.as_ref().is_some_and(|plan| !plan.is_empty()),
            Action::Filter => self.regions.is_ok(),
            Action::Choose => self.chooses(),
            Action::Toggle if self.overlay == Some(Overlay::Plan) => {
                !self.asking && self.plan.as_ref().is_some_and(PlanView::toggles)
            }
            Action::Toggle if self.screen == Screen::Local => {
                matches!(self.local.rows().get(self.local.row), Some(local::Row::Layer(_)))
            }
            Action::LocalApp | Action::LocalLogs => self.local.selected_app().is_some(),
            Action::LocalOpen => {
                self.local.selected_app().is_some_and(|app| app.url().is_some() && self.local.running(app))
            }
            Action::Toggle => matches!(self.live_rows().get(self.row), Some(LiveRow::Layer(_))),
            Action::Fix => {
                self.fix().is_some_and(|fix| !matches!(fix, Fix::Plan) || self.works(Action::Open(Overlay::Plan)))
            }
            Action::Steps => self.plan.as_ref().is_some_and(|view| {
                !view.all.groups.is_empty() || (view.dev.is_some() && !view.taken.versions.is_empty())
            }),
            Action::Prepare => self.plan.is_some() && !self.asking,
            Action::Build => {
                self.plan.as_ref().is_some_and(|view| {
                    !view.taken.needs_prepare && (view.dev.is_none() || view.taken.blocked.is_empty())
                }) && !self.asking
            }
            Action::ReviewApply => {
                self.plan.as_ref().is_some_and(|view| {
                    view.taken.env == LIVE
                        && !view.taken.needs_prepare
                        && !view.taken.blocked.iter().any(|product| !product.layers.is_empty())
                }) && !self.asking
            }
            Action::Apply => self.asking && self.overlay == Some(Overlay::Plan),
            Action::OpenRun => self.runs.get(self.run).is_some(),
            Action::ObserveRun => self.execution.selected.is_some(),
            Action::StopRun => !self.asking && self.execution.can_stop(),
            Action::ConfirmStop => self.asking && self.overlay == Some(Overlay::Run) && self.execution.can_stop(),
            Action::ReviewPrepared => self.execution.prepared().is_some(),
            Action::Undo => self.edited,
            _ => true,
        }
    }

    fn bindings(&self) -> Vec<Binding> {
        let bar = |key, action, label, does: &str| Binding { key, action, bar: Some((label, does.to_string())) };
        let hidden = |key, action| Binding { key, action, bar: None };
        let input = |key, label, text: &str| bar(key, Action::InputKey(key), label, text);
        if self.overlay == Some(Overlay::Region) && self.region_editor.mode.is_some() {
            let mut keys = vec![input(KeyCode::Esc, "esc", "back")];
            match self.region_editor.mode {
                Some(regions::Mode::Delete) => {
                    keys.push(hidden(KeyCode::Up, Action::Up));
                    keys.push(hidden(KeyCode::Down, Action::Down));
                    if !self.busy && self.region_editor.deletion.as_ref().is_some_and(|plan| plan.used_by.is_empty()) {
                        keys.insert(0, input(KeyCode::Char('y'), "y", "delete reviewed definition"));
                    }
                }
                _ => {
                    keys.insert(0, input(KeyCode::Tab, "tab", "field"));
                    if self.region_editor.mode == Some(regions::Mode::Areas) {
                        keys.insert(1, input(KeyCode::F(3), "F3", "selected / all"));
                    }
                    if !self.busy {
                        keys.insert(1, input(KeyCode::F(2), "F2", "save"));
                        if self.region_editor.mode == Some(regions::Mode::Areas) {
                            keys.insert(2, input(KeyCode::F(5), "F5", "load area list"));
                        }
                    }
                }
            }
            return keys;
        }
        if self.overlay == Some(Overlay::Policy) && self.policy_days.is_some() {
            let mut keys = vec![input(KeyCode::Esc, "esc", "keep policy")];
            if !self.busy {
                keys.insert(0, input(KeyCode::Enter, "enter", "save"));
            }
            return keys;
        }
        if self.overlay.is_none() && self.screen == Screen::Sources && self.source_view.typing {
            return vec![
                input(KeyCode::Enter, "enter", "done"),
                input(KeyCode::Delete, "delete", "clear"),
                input(KeyCode::Esc, "esc", "done"),
            ];
        }
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
                    Overlay::Version => offer(KeyCode::Char('R'), Action::CheckNow, "R", "check upstream"),
                    Overlay::Policy if !typing => offer(KeyCode::Char('c'), Action::CustomPolicy, "c", "custom days"),
                    Overlay::Region if !typing => {
                        offer(KeyCode::Char('/'), Action::Filter, "/", "filter");
                        offer(KeyCode::Char('n'), Action::NewRegion, "n", "new areas");
                        offer(KeyCode::Char('b'), Action::BoxRegion, "b", "new box");
                        offer(KeyCode::Char('d'), Action::DeleteRegion, "d", "delete");
                        offer(KeyCode::F(5), Action::LoadAreas, "F5", "load area list");
                    }
                    Overlay::Clean if self.works(Action::Open(Overlay::Clean)) => match self.asking {
                        false => offer(KeyCode::Char('a'), Action::Ask, "a", "clean"),
                        true => offer(KeyCode::Char('y'), Action::Clean, "y", "clean"),
                    },
                    Overlay::Plan => {
                        if self.asking {
                            offer(
                                KeyCode::Char('y'),
                                Action::Apply,
                                "y",
                                if self.plan.as_ref().is_some_and(|view| view.dev.is_some()) {
                                    "prepare Local"
                                } else {
                                    "apply to live"
                                },
                            );
                        } else {
                            offer(KeyCode::Char(' '), Action::Toggle, "space", "source move");
                            let steps = self.plan.as_ref().is_some_and(|view| view.steps);
                            offer(KeyCode::Char('d'), Action::Steps, "d", if steps { "changes" } else { "steps" });
                            offer(KeyCode::Char('f'), Action::Prepare, "f", "prepare inputs");
                            offer(
                                KeyCode::Char('b'),
                                Action::Build,
                                "b",
                                if self.plan.as_ref().is_some_and(|view| view.dev.is_some()) {
                                    "review Local build"
                                } else {
                                    "build only"
                                },
                            );
                            offer(KeyCode::Char('a'), Action::ReviewApply, "a", "review apply");
                        }
                    }
                    Overlay::Run if self.asking => offer(KeyCode::Char('y'), Action::ConfirmStop, "y", "stop run"),
                    Overlay::Run => {
                        offer(KeyCode::Char('R'), Action::ObserveRun, "R", "observe");
                        offer(KeyCode::Char('x'), Action::StopRun, "x", "stop");
                        offer(KeyCode::Char('p'), Action::ReviewPrepared, "p", "review prepared plan");
                    }
                    _ => {}
                }
                offer(KeyCode::Enter, Action::Choose, "enter", "choose");
                let close = if self.asking {
                    "cancel"
                } else if overlay == Overlay::Run {
                    "hide run"
                } else if typing {
                    "done"
                } else {
                    "close"
                };
                keys.push(bar(KeyCode::Esc, Action::Close, "esc", close));
            }
            None => {
                keys.extend(SCREENS.iter().map(|&(key, _, screen)| hidden(KeyCode::Char(key), Action::Show(screen))));
                keys.push(hidden(KeyCode::Tab, Action::NextScreen));
                if self.rows() > 0 || self.screen == Screen::Sources {
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
                if self.screen != Screen::Local && self.works(Action::Open(Overlay::Plan)) {
                    keys.push(bar(KeyCode::Char('p'), Action::Open(Overlay::Plan), "p", "plan"));
                }
                if self.screen != Screen::Local && self.works(Action::Undo) {
                    keys.push(bar(KeyCode::Char('u'), Action::Undo, "u", "undo environment"));
                }
                keys.push(bar(KeyCode::Char('?'), Action::Open(Overlay::Help), "?", "help"));
            }
        }
        if !typing {
            if self.works(Action::Reload) {
                keys.push(bar(KeyCode::F(6), Action::Reload, "F6", "reload current code"));
            }
            keys.push(if self.overlay == Some(Overlay::Run) {
                bar(KeyCode::Char('q'), Action::Quit, "q", "quit view")
            } else {
                hidden(KeyCode::Char('q'), Action::Quit)
            });
            if self.notice.is_some() {
                keys.push(bar(KeyCode::Char('!'), Action::Open(Overlay::Error), "!", "error details"));
                if self.overlay != Some(Overlay::Run) {
                    keys.push(bar(KeyCode::Char('x'), Action::Dismiss, "x", "dismiss error"));
                }
            }
        }
        keys
    }

    fn key(&mut self, key: KeyCode) -> Effect {
        if self.screen == Screen::Content && self.overlay.is_none() && self.content.draft.is_some() {
            return self.content.key(key, self.busy);
        }
        if self.overlay == Some(Overlay::Policy) && self.policy_days.is_some() {
            match key {
                KeyCode::Esc => self.policy_days = None,
                KeyCode::Char(c) => self.policy_days.as_mut().unwrap().push(c),
                KeyCode::Backspace => {
                    self.policy_days.as_mut().unwrap().pop();
                }
                KeyCode::Delete => self.policy_days.as_mut().unwrap().clear(),
                KeyCode::Enter if !self.busy => match self.policy_days.as_ref().unwrap().parse::<Refresh>() {
                    Ok(refresh) => {
                        self.policy_days = None;
                        self.overlay = None;
                        return Effect::Policy(self.sources[self.source].source.id.clone(), refresh);
                    }
                    Err(message) => {
                        self.notice = Some(
                            super::Code::Usage
                                .error(message)
                                .fix("Enter 1..65535 whole days, or press Esc to keep the current policy."),
                        )
                    }
                },
                _ => {}
            }
            return Effect::None;
        }
        if self.overlay == Some(Overlay::Region) {
            if let Some(effect) = self.region_editor.key(key, self.busy) {
                return effect;
            }
        }
        if self.overlay.is_none() && self.screen == Screen::Sources && self.source_view.typing {
            match key {
                KeyCode::Char(c) => self.source_view.filter.push(c),
                KeyCode::Backspace => {
                    self.source_view.filter.pop();
                }
                KeyCode::Delete => self.source_view.filter.clear(),
                KeyCode::Esc | KeyCode::Enter => self.source_view.typing = false,
                _ => {}
            }
            return Effect::None;
        }
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
        if matches!(
            action,
            Action::Open(Overlay::Plan)
                | Action::Prepare
                | Action::Build
                | Action::ReviewApply
                | Action::Apply
                | Action::ObserveRun
                | Action::StopRun
                | Action::ConfirmStop
                | Action::ReviewPrepared
                | Action::LocalRefresh
                | Action::LocalApp
                | Action::LocalOpen
                | Action::LocalLogs
                | Action::Open(Overlay::Version)
        ) && !self.works(action)
        {
            return Effect::None;
        }
        let last = match self.overlay {
            Some(_) => self.choices().map_or(usize::MAX, |choices| choices.saturating_sub(1)),
            None => self.rows().saturating_sub(1),
        };
        match action {
            Action::ContentConfigure => self.content.draft = Some(String::new()),
            Action::ContentPrepare => return Effect::ContentPrepare,
            Action::ContentReview => {
                if let Some(digest) = self.content.digest() {
                    return Effect::ContentReview(digest);
                }
            }
            Action::ContentPublish => {
                if let Some(review) = &self.content.review {
                    if let Some(digest) = review["snapshot"].as_str() {
                        return Effect::ContentPublish(digest.into());
                    }
                }
            }
            Action::ContentUse => {
                if let Some(digest) = self.content.digest() {
                    self.content.review = None;
                    return Effect::ContentUse(digest);
                }
            }
            Action::Reload if self.works(Action::Reload) => {
                self.reload = true;
                return Effect::Reload;
            }
            Action::Reload => {}
            Action::Show(screen) => {
                self.screen = screen;
                if screen == Screen::Local {
                    self.local.checked = None;
                }
            }
            Action::NextScreen => {
                let at = SCREENS.iter().position(|&(_, _, screen)| screen == self.screen).unwrap_or(0);
                self.screen = SCREENS[(at + 1) % SCREENS.len()].2;
                if self.screen == Screen::Local {
                    self.local.checked = None;
                }
            }
            // The drawing stops the scroll of an overlay at its end.
            Action::Up if self.overlay.is_none() && self.screen == Screen::Sources => self.source_move(false),
            Action::Down if self.overlay.is_none() && self.screen == Screen::Sources => self.source_move(true),
            Action::Up | Action::Down => {
                let at = *self.selected();
                *self.selected() = if action == Action::Up { at.saturating_sub(1) } else { (at + 1).min(last) };
                if self.screen == Screen::Local && self.overlay.is_none() {
                    if let Some(app) = self.local.selected_app() {
                        if self.local.app != Some(app) {
                            self.local.app = Some(app);
                            self.local.checked = None;
                            self.local.plan = None;
                        }
                    }
                }
            }
            Action::Open(overlay) => {
                (self.overlay, self.scroll, self.asking) = (Some(overlay), 0, false);
                match overlay {
                    Overlay::Policy => {
                        self.policy_days = None;
                        let current = self.sources.get(self.source).map(|row| row.source.refresh);
                        self.choice = Refresh::ALL.iter().position(|&r| Some(r) == current).unwrap_or(0);
                    }
                    Overlay::Region => {
                        (self.filter, self.filtering) = (String::new(), false);
                        let current = self.current_env().map(|env| env.region.as_str());
                        self.choice = self.shown_regions().iter().position(|&id| Some(id) == current).unwrap_or(0);
                    }
                    Overlay::Version => {
                        self.choice = 0;
                        self.versions = None;
                        return Effect::Versions(self.sources[self.source].source.id.clone());
                    }
                    Overlay::Clean => return Effect::PlanClean,
                    Overlay::Plan if self.screen == Screen::Local => {
                        let request = self.local.request(false);
                        return Effect::LocalCheck(request);
                    }
                    Overlay::Plan => {
                        self.plan = None;
                        return Effect::Plan;
                    }
                    Overlay::Help
                    | Overlay::Attribution
                    | Overlay::Source
                    | Overlay::Error
                    | Overlay::Run
                    | Overlay::AppLogs => {}
                }
            }
            Action::Choose if self.overlay == Some(Overlay::Version) => return self.version_choose(),
            Action::Choose if self.overlay == Some(Overlay::Policy) => {
                self.overlay = None;
                return Effect::Policy(self.sources[self.source].source.id.clone(), Refresh::ALL[self.choice]);
            }
            Action::Choose => {
                let id = self.shown_regions()[self.choice].to_string();
                (self.overlay, self.filter, self.filtering) = (None, String::new(), false);
                return if self.screen == Screen::Local { Effect::LocalRegion(id) } else { Effect::Region(id) };
            }
            Action::CheckNow if self.screen == Screen::Local => return Effect::LocalCheck(self.local.request(false)),
            Action::LocalRefresh => {
                let mut request = self.local.request(true);
                request.inputs_only = true;
                return Effect::LocalStart(request);
            }
            Action::LocalApp | Action::LocalOpen | Action::LocalLogs => {
                if let Some(app) = self.local.selected_app() {
                    let control = match action {
                        Action::LocalOpen => local::Control::Open,
                        Action::LocalLogs => {
                            self.overlay = Some(Overlay::AppLogs);
                            local::Control::Logs
                        }
                        _ if self.local.running(app) => local::Control::Stop,
                        _ => local::Control::Start,
                    };
                    return Effect::LocalControl(control, app);
                }
            }
            Action::CheckNow if self.screen == Screen::Runs => return self.act(Action::OpenRun),
            Action::CheckNow if self.screen == Screen::Content => return Effect::ContentRead,
            Action::CheckNow if self.screen == Screen::Live => return Effect::Status { check: true },
            Action::CheckNow => return Effect::CheckNow,
            Action::Ask => self.asking = true,
            Action::Clean => {
                (self.overlay, self.asking) = (None, false);
                return Effect::Clean;
            }
            Action::Toggle if self.screen == Screen::Local && self.overlay.is_none() => {
                if let Some(local::Row::Layer(layer)) = self.local.rows().get(self.local.row) {
                    let on = self.local.env.as_ref().is_some_and(|env| env.layers.contains(layer));
                    return Effect::LocalLayer(layer.clone(), if on { Switch::Off } else { Switch::On });
                }
            }
            Action::Toggle => return self.toggle(),
            Action::Fix => match self.fix() {
                Some(Fix::Source(source)) => (self.screen, self.source) = (Screen::Sources, source),
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
            Action::Prepare | Action::Build | Action::Apply => {
                if let Some(request) = self.plan.as_ref().and_then(|view| view.dev.clone()) {
                    if action == Action::Build {
                        self.asking = false;
                        return Effect::LocalReview(request);
                    }
                    let mut request = request;
                    request.inputs_only = action == Action::Prepare;
                    request.reviewed = if request.inputs_only {
                        None
                    } else {
                        Some(Box::new(self.plan.as_ref().unwrap().taken.clone()))
                    };
                    self.asking = false;
                    return Effect::LocalStart(request);
                }
                let kind = match action {
                    Action::Prepare => crate::operation::Kind::Prepare,
                    Action::Build => crate::operation::Kind::Build,
                    _ => crate::operation::Kind::Apply,
                };
                self.asking = false;
                return Effect::Start(kind, Box::new(self.plan.as_ref().expect("the action has a plan").taken.clone()));
            }
            Action::ReviewApply => self.asking = true,
            Action::OpenRun => {
                if let Some(run) = self.runs.get(self.run) {
                    let id = run.summary.id.clone();
                    self.execution.selected = Some(id.clone());
                    (self.overlay, self.scroll) = (Some(Overlay::Run), 0);
                    return Effect::ObserveRun(id);
                }
            }
            Action::StopRun => self.asking = true,
            Action::ObserveRun | Action::ConfirmStop => {
                self.asking = false;
                if let Some(id) = self.execution.selected.clone() {
                    return if action == Action::ConfirmStop { Effect::StopRun(id) } else { Effect::ObserveRun(id) };
                }
            }
            Action::ReviewPrepared => {
                if let Some(plan) = self.execution.prepared() {
                    self.plan = Some(
                        match self
                            .execution
                            .dev_request
                            .clone()
                            .filter(|(id, _)| Some(id) == self.execution.selected.as_ref())
                        {
                            Some((_, mut request)) if plan.env == "local" => {
                                request.reviewed = Some(Box::new(plan.clone()));
                                self.local.plan = Some(plan.clone());
                                self.local.request = Some(request.clone());
                                self.local.app = Some(request.app);
                                self.local.checked = None;
                                PlanView::local(plan, request)
                            }
                            _ => PlanView::new(plan),
                        },
                    );
                    (self.overlay, self.scroll, self.asking) = (Some(Overlay::Plan), 0, false);
                }
            }
            Action::Undo => return Effect::Undo,
            Action::Dismiss => self.notice = None,
            Action::SourceScope => self.source_view.scope.toggle(),
            Action::SourceFilter => self.source_view.typing = true,
            Action::CustomPolicy => {
                self.policy_days = Some(match self.sources[self.source].source.refresh {
                    Refresh::Days(days) => days.to_string(),
                    Refresh::Manual => String::new(),
                });
            }
            Action::InputKey(key) => return self.key(key),
            Action::NewRegion => {
                self.scroll = 0;
                self.region_editor.open(regions::Mode::Areas);
                if self.region_editor.areas.is_none() {
                    return Effect::Areas;
                }
            }
            Action::BoxRegion => {
                self.scroll = 0;
                self.region_editor.open(regions::Mode::Box);
            }
            Action::LoadAreas => return Effect::LoadAreas,
            Action::DeleteRegion => {
                self.scroll = 0;
                if let Some(id) = self.shown_regions().get(self.choice) {
                    let id = id.to_string();
                    self.region_editor.open(regions::Mode::Delete);
                    self.region_editor.deletion = None;
                    return Effect::ReviewRegionDeletion(id);
                }
            }
            Action::Close if self.asking => self.asking = false,
            Action::Close if self.typing() => self.filtering = false,
            Action::Close => self.overlay = None,
            Action::Quit => return Effect::Quit,
        }
        Effect::None
    }

    /// A click on a tab shows its screen. A click on a row selects it.
    fn click(&mut self, x: u16, y: u16) -> Effect {
        let hit = self.hits.iter().rev().find(|(area, _)| area.contains(Position { x, y })).map(|&(_, hit)| hit);
        match hit {
            Some(Hit::Action(action)) => return self.act(action),
            Some(Hit::Region(target)) => self.region_editor.click(target),
            Some(Hit::Screen(screen)) => {
                self.overlay = None;
                self.screen = screen;
                if screen == Screen::Local {
                    self.local.checked = None;
                }
            }
            Some(Hit::Row(row)) if self.overlay.is_none() && self.screen == Screen::Sources => {
                if let Some((index, _)) = self.source_view.rows(&self.sources).get(row) {
                    self.source = *index;
                }
            }
            Some(Hit::Row(row)) if self.overlay.is_none() && self.screen == Screen::Runs => {
                if self.run == row {
                    return self.act(Action::OpenRun);
                }
                self.run = row;
            }
            Some(Hit::Row(row)) if self.overlay.is_none() => {
                *self.selected() = row;
                if self.screen == Screen::Local {
                    if let Some(app) = self.local.selected_app() {
                        if self.local.app != Some(app) {
                            self.local.app = Some(app);
                            self.local.checked = None;
                            self.local.plan = None;
                        }
                    }
                }
            }
            _ => {}
        }
        Effect::None
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.hits.clear();
        let error_height = self.notice.as_ref().map_or(0, |error| {
            Paragraph::new(format!("Error: {}\nFix: {}", error.message, error.fix))
                .wrap(Wrap { trim: false })
                .line_count(frame.area().width)
                .min(6) as u16
        });
        let saved_height = self.saved.as_ref().map_or(0, |saved| {
            Paragraph::new(saved.as_str()).wrap(Wrap { trim: false }).line_count(frame.area().width).min(3) as u16
        });
        let [tabs, body, saved_area, error_area, bar] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(saved_height),
            Constraint::Length(error_height),
            Constraint::Length(if self.busy { 1 } else { self.bar_rows(frame.area().width).len() as u16 }),
        ])
        .areas(frame.area());
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
        if let Some(run) = self.runs.iter().find(|run| run.summary.outcome == Outcome::Running) {
            let text = format!(" ◐ {}", run.summary.command);
            let area = Rect { x, width: tabs.right().saturating_sub(x), ..tabs };
            frame.render_widget(Span::from(text), area);
        }
        let gap = u16::from(self.busy || self.bar_rows(frame.area().width).len() < 2);
        let body = Rect { y: body.y + gap, height: body.height.saturating_sub(gap), ..body };
        match self.screen {
            Screen::Live => self.draw_live(frame, body),
            Screen::Local => self.draw_local(frame, body),
            Screen::Sources => self.draw_sources(frame, body),
            Screen::Store => self.draw_store(frame, body),
            Screen::Runs => self.draw_runs(frame, body),
            Screen::Content => self.content.draw(frame, body),
        }
        self.draw_bar(frame, bar);
        if let Some(overlay) = self.overlay {
            self.draw_overlay(frame, body, overlay);
        }
        if let Some(error) = &self.notice {
            frame.render_widget(
                Paragraph::new(format!("Error: {}\nFix: {}", error.message, error.fix))
                    .red()
                    .wrap(Wrap { trim: false }),
                error_area,
            );
        }
        if let Some(saved) = &self.saved {
            frame.render_widget(Paragraph::new(saved.as_str()).dim().wrap(Wrap { trim: false }), saved_area);
        }
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

    fn bar_rows(&self, width: u16) -> Vec<Vec<Binding>> {
        let mut rows = vec![Vec::new()];
        let mut used = 0;
        for binding in self.bindings().into_iter().filter(|binding| binding.bar.is_some()) {
            let (label, does) = binding.bar.as_ref().unwrap();
            let size = (label.len() + does.chars().count() + 4) as u16;
            if used > 0 && used + size > width {
                rows.push(Vec::new());
                used = 0;
            }
            used += size;
            rows.last_mut().unwrap().push(binding);
        }
        rows
    }

    fn draw_bar(&mut self, frame: &mut Frame, area: Rect) {
        if self.busy {
            let text = if self.typing() {
                "Checking or editing…  esc leave input · then q finish and quit"
            } else {
                "Checking or editing…  navigation available · q finish and quit"
            };
            frame.render_widget(Line::from(text), area);
            return;
        }
        for (row, bindings) in self.bar_rows(area.width).into_iter().enumerate() {
            let mut spans = Vec::new();
            let mut x = area.x;
            let line = Rect { y: area.y + row as u16, height: 1, ..area }.intersection(area);
            for binding in bindings {
                let (label, does) = binding.bar.unwrap();
                let width = (label.len() + does.chars().count() + 4) as u16;
                self.hits.push((Rect { x, width, ..line }.intersection(line), Hit::Action(binding.action)));
                x += width;
                spans.extend([Span::from(label).bold(), Span::from(format!(" {does}   "))]);
            }
            frame.render_widget(Line::from(spans), line);
        }
    }

    fn draw_overlay(&mut self, frame: &mut Frame, body: Rect, overlay: Overlay) {
        let id = self.sources.get(self.source).map_or("", |row| row.source.id.as_str());
        // `focus` is the line of the choice, which always shows; `footer` shows under the lines.
        let (title, lines, footer, focus) = match overlay {
            Overlay::Help => (" HELP ".to_string(), help(), Vec::new(), None),
            Overlay::Attribution => (" ATTRIBUTION ".into(), attribution(&self.sources), Vec::new(), None),
            Overlay::Source => (format!(" SOURCE · {id} "), self.source_lines(), Vec::new(), None),
            Overlay::Version => (format!(" VERSION · {id} "), self.version_lines(), Vec::new(), Some(self.choice + 2)),
            Overlay::Error => (
                " ERROR ".into(),
                self.notice
                    .as_ref()
                    .map(|error| {
                        vec![
                            Line::from(error.message.clone()),
                            Line::default(),
                            Line::from(format!("Fix: {}", error.fix)),
                        ]
                    })
                    .unwrap_or_default(),
                Vec::new(),
                None,
            ),
            Overlay::Policy => (format!(" POLICY · {id} "), self.policy_lines(), Vec::new(), Some(self.choice)),
            Overlay::Clean => (" CLEAN ".into(), self.clean_lines(), Vec::new(), None),
            Overlay::Region => (
                format!(" REGION · {} ", if self.screen == Screen::Local { "local" } else { LIVE }),
                self.region_lines(),
                Vec::new(),
                if self.region_editor.mode.is_some() { self.region_editor.focus() } else { Some(self.choice + 3) },
            ),
            Overlay::Plan => {
                if self.asking && self.plan.as_ref().is_some_and(|view| view.dev.is_some()) {
                    let view = self.plan.as_ref().unwrap();
                    let request = view.dev.as_ref().unwrap();
                    let mut lines = vec![
                        Line::from(format!(
                            "Prepare {} for {} on {}?",
                            request.app.name(),
                            view.taken.region,
                            self.host
                        ))
                        .bold(),
                        Line::from("The exact source versions and pending work stay pinned."),
                        Line::from("Only already running apps update. Stopped apps remain stopped."),
                    ];
                    if !view.taken.versions.is_empty() {
                        lines.extend([Line::default(), Line::from("INPUT VERSIONS").dim()]);
                        lines.extend(view.versions());
                    }
                    (" CONFIRM LOCAL PREPARATION ".into(), lines, Vec::new(), None)
                } else if self.asking {
                    let plan = &self.plan.as_ref().expect("confirmation has a plan").taken;
                    let mut lines = vec![
                        Line::from(super::apply_cli::question(plan)).bold(),
                        Line::from(format!("Execution: {} · environment: {}", self.host, plan.env)),
                        Line::from("The exact reviewed plan is retained. Apply never commits configuration."),
                        Line::default(),
                    ];
                    lines.extend(super::build_cli::replaced(plan).into_iter().map(Line::from));
                    lines.extend(super::build_cli::removals(plan).into_iter().map(Line::from));
                    (" CONFIRM APPLY ".into(), lines, Vec::new(), None)
                } else {
                    let (lines, footer, focus) = self.plan_lines();
                    (
                        format!(" PLAN · {} ", self.plan.as_ref().map_or(LIVE, |view| view.taken.env.as_str())),
                        lines,
                        footer,
                        focus,
                    )
                }
            }
            Overlay::Run if self.asking => {
                let id = self.execution.selected.clone().unwrap_or_default();
                let lines = vec![
                    Line::from(format!("Stop run {id}?")).bold(),
                    Line::from("Admitted work finishes; no new step starts. An apply stops before its next phase."),
                ];
                (" STOP RUN ".into(), lines, Vec::new(), None)
            }
            Overlay::Run => (" RUN ".into(), self.run_lines(), Vec::new(), None),
            Overlay::AppLogs => {
                (" LOCAL APP LOGS ".into(), self.local.logs.iter().cloned().map(Line::from).collect(), Vec::new(), None)
            }
        };
        let width = if matches!(overlay, Overlay::Plan | Overlay::Run) { body.width } else { body.width * 4 / 5 };
        let wrapped = |lines| Paragraph::new(lines).wrap(Wrap { trim: false });
        let mut offsets = Vec::with_capacity(lines.len());
        let mut physical = 0;
        for line in &lines {
            let height = wrapped(vec![line.clone()]).line_count(width.saturating_sub(2));
            offsets.push((physical, height));
            physical += height;
        }
        let focus = focus.and_then(|line| offsets.get(line)).map(|(start, height)| start + height - 1);
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
        if overlay == Overlay::Region && self.region_editor.mode.is_some() {
            for (line, target) in self.region_editor.targets() {
                if let Some(&(start, height)) = offsets.get(line) {
                    let from = start.max(scroll);
                    let to = (start + height).min(scroll + shown);
                    if from < to {
                        self.hits.push((
                            Rect { y: top.y + (from - scroll) as u16, height: (to - from) as u16, ..top },
                            Hit::Region(target),
                        ));
                    }
                }
            }
        }
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        frame.render_widget(paragraph.scroll((scroll as u16, 0)), top);
        frame.render_widget(footer, bottom);
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
        State::Unused => Color::DarkGray,
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
    if let Some(phase) = run.phase {
        lines.insert(0, Line::from(format!("{phase:?} · {} remote writes acknowledged", run.published.len())));
    }
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
        line("u", "restore applied Live region, layers and policies".into()),
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
    lines.push(line("", "f prepare inputs · b build only · a review apply · y confirm".into()));
    lines.push(line("Run", "R observe · x stop, then y · p review prepared plan".into()));
    lines.push(line("", "esc hides the run; q quits the view without stopping work".into()));
    if crate::worker::can_reload() {
        lines.push(line("F6", "finish the check, restore terminal and launch current Rust code".into()));
    }
    lines.push(line("Version", "enter chooses a pending move for ALL active requests; p reviews it".into()));
    lines.push(line("Policy", "enter choose".into()));
    lines.push(line("Clean", "a clean, then y".into()));
    lines
}

#[cfg(test)]
mod tests;
