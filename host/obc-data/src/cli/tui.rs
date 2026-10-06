//! The TUI of `obc data`. Each screen shows what a command writes with `--json`, and the bar
//! names that command. Each change goes through the function of its command: `policy` and
//! `clean --apply`.

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
use crate::product::Product;
use crate::sources::{Kind, Refresh, State, VersionScheme};
use crate::store::{gc, import, Store};

use super::runs_cli::{bytes, duration, mark, step_cells};
use super::{
    clean, clean_plan, live_column, live_unknown, policy, registry, row_text, source_rows, widths, CleanPlan, Error,
    SourceRow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Sources,
    Store,
    Runs,
}

/// The key of a screen is its number among the five screens of the TUI; 1 Live and 2 Local are
/// not built.
const SCREENS: [(char, &str, Screen); 3] =
    [('3', "Sources", Screen::Sources), ('4', "Store", Screen::Store), ('5', "Runs", Screen::Runs)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Help,
    Attribution,
    Policy,
    Clean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Show(Screen),
    NextScreen,
    Up,
    Down,
    Open(Overlay),
    /// `enter` on a policy of Policy.
    Choose,
    CheckNow,
    /// The question before a clean.
    Ask,
    Clean,
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
}

/// A key that works now, with its label and what it does when the bar shows it.
struct Binding {
    key: KeyCode,
    action: Action,
    bar: Option<(&'static str, &'static str)>,
}

/// The keys that the bar can show on a screen with rows, besides `?`.
fn screen_keys(screen: Screen) -> &'static [(KeyCode, Action, &'static str, &'static str)] {
    match screen {
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
    /// Sources shows a check of upstream from now, not from the last hour.
    checked_now: bool,
    /// The check of upstream runs.
    checking: bool,
    /// The plan of `clean`, once Store has shown.
    store: Option<CleanPlan>,
    /// Running runs first, then newest first.
    runs: Vec<Details>,
    source: usize,
    kept: usize,
    run: usize,
    /// The first line that an overlay shows.
    scroll: usize,
    /// The selected policy of Policy.
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
    let live = live_column(products, &store);
    let mut app = App::new(source_rows(&registry(root)?, live.as_ref().ok(), false)?, list_runs(&store)?);
    app.notice = live.as_ref().err().map(live_unknown);
    app.live = live.ok();
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
    loop {
        tui.draw(|frame| app.draw(frame)).map_err(io)?;
        let screen = app.screen;
        let mut effect = Effect::None;
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
        let result = match effect {
            Effect::None => continue,
            Effect::Quit => return Ok(()),
            Effect::Policy(id, refresh) => {
                let result = policy(root, &id, refresh).map(drop);
                let reloaded = app.reload(root, false);
                result.and(reloaded)
            }
            Effect::CheckNow => {
                app.checking = true;
                tui.draw(|frame| app.draw(frame)).map_err(io)?;
                let result = app.reload(root, true);
                app.checking = false;
                discard_keys()?;
                result
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
                    Err(error) => app.notice = Some(error.message),
                }
                result
            }
        };
        if let Err(error) = result {
            app.notice = Some(error.message);
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
            screen: Screen::Sources,
            overlay: None,
            sources,
            live: None,
            checked_now: false,
            checking: false,
            store: None,
            runs,
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
    fn reload(&mut self, root: &Path, check_now: bool) -> Result<(), Error> {
        self.sources = source_rows(&registry(root)?, self.live.as_ref(), check_now)?;
        self.checked_now = check_now;
        Ok(())
    }

    fn rows(&self) -> usize {
        match self.screen {
            Screen::Sources => self.sources.len(),
            Screen::Store => self
                .store
                .as_ref()
                .map_or(0, |plan| plan.store.kept.len() + usize::from(!plan.store.objects.is_empty())),
            Screen::Runs => self.runs.len(),
        }
    }

    /// The selected row, the choice of an overlay, or the scroll of an overlay.
    fn selected(&mut self) -> &mut usize {
        match (self.overlay, self.screen) {
            (Some(Overlay::Policy), _) => &mut self.choice,
            (Some(_), _) => &mut self.scroll,
            (None, Screen::Sources) => &mut self.source,
            (None, Screen::Store) => &mut self.kept,
            (None, Screen::Runs) => &mut self.run,
        }
    }

    fn run_id(&self) -> Option<&str> {
        self.runs.get(self.run).map(|run| run.summary.id.as_str())
    }

    /// Whether `enter` in Policy chooses another policy.
    fn chooses(&self) -> bool {
        let row = self.sources.get(self.source);
        self.overlay == Some(Overlay::Policy) && row.is_some_and(|row| Refresh::ALL[self.choice] != row.source.refresh)
    }

    /// Whether a key of the screen does something for the selected row.
    fn works(&self, action: Action) -> bool {
        match action {
            Action::Open(Overlay::Policy) => {
                self.sources.get(self.source).is_some_and(|row| row.source.version == VersionScheme::Date)
            }
            Action::Open(Overlay::Clean) => self.store.as_ref().is_some_and(|plan| !plan.is_empty()),
            _ => true,
        }
    }

    fn bindings(&self) -> Vec<Binding> {
        let bar = |key, action, label, does| Binding { key, action, bar: Some((label, does)) };
        let hidden = |key, action| Binding { key, action, bar: None };
        let mut keys = vec![
            hidden(KeyCode::Up, Action::Up),
            hidden(KeyCode::Char('k'), Action::Up),
            hidden(KeyCode::Down, Action::Down),
            hidden(KeyCode::Char('j'), Action::Down),
        ];
        match self.overlay {
            Some(overlay) => {
                if self.chooses() {
                    keys.push(bar(KeyCode::Enter, Action::Choose, "enter", "choose"));
                }
                if overlay == Overlay::Clean && self.works(Action::Open(Overlay::Clean)) {
                    keys.push(match self.asking {
                        false => bar(KeyCode::Char('a'), Action::Ask, "a", "clean"),
                        true => bar(KeyCode::Char('y'), Action::Clean, "y", "clean"),
                    });
                }
                keys.push(bar(KeyCode::Esc, Action::Close, "esc", if self.asking { "cancel" } else { "close" }));
            }
            None => {
                keys.extend(SCREENS.iter().map(|&(key, _, screen)| hidden(KeyCode::Char(key), Action::Show(screen))));
                keys.push(hidden(KeyCode::Tab, Action::NextScreen));
                if self.rows() > 0 {
                    let works = screen_keys(self.screen).iter().filter(|&&(_, action, _, _)| self.works(action));
                    keys.extend(works.map(|&(key, action, label, does)| bar(key, action, label, does)));
                }
                keys.push(bar(KeyCode::Char('?'), Action::Open(Overlay::Help), "?", "help"));
            }
        }
        keys.push(hidden(KeyCode::Char('q'), Action::Quit));
        keys
    }

    fn key(&mut self, key: KeyCode) -> Effect {
        self.notice = None;
        match self.bindings().iter().find(|binding| binding.key == key) {
            Some(binding) => self.act(binding.action),
            None => Effect::None,
        }
    }

    fn act(&mut self, action: Action) -> Effect {
        let last = match self.overlay {
            Some(Overlay::Policy) => Refresh::ALL.len() - 1,
            // The drawing stops the scroll of an overlay at its end.
            Some(_) => usize::MAX,
            None => self.rows().saturating_sub(1),
        };
        match action {
            Action::Show(screen) => self.screen = screen,
            Action::NextScreen => {
                let at = SCREENS.iter().position(|&(_, _, screen)| screen == self.screen).unwrap_or(0);
                self.screen = SCREENS[(at + 1) % SCREENS.len()].2;
            }
            Action::Up => *self.selected() = self.selected().saturating_sub(1),
            Action::Down => *self.selected() = (*self.selected() + 1).min(last),
            Action::Open(overlay) => {
                (self.overlay, self.scroll, self.asking) = (Some(overlay), 0, false);
                let current = self.sources.get(self.source).map(|row| row.source.refresh);
                self.choice = Refresh::ALL.iter().position(|&r| Some(r) == current).unwrap_or(0);
                if overlay == Overlay::Clean {
                    return Effect::PlanClean;
                }
            }
            Action::Choose => {
                self.overlay = None;
                return Effect::Policy(self.sources[self.source].source.id.clone(), Refresh::ALL[self.choice]);
            }
            Action::CheckNow => return Effect::CheckNow,
            Action::Ask => self.asking = true,
            Action::Clean => {
                (self.overlay, self.asking) = (None, false);
                return Effect::Clean;
            }
            Action::Close if self.asking => self.asking = false,
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

    /// The command that shows the same, or does the same.
    fn command(&self) -> String {
        let id = self.sources.get(self.source).map_or("", |row| row.source.id.as_str());
        match (self.overlay, self.screen) {
            (Some(Overlay::Help), _) => "obc data --help".into(),
            (Some(Overlay::Attribution), _) => "obc data sources --json".into(),
            (Some(Overlay::Policy), _) => match Refresh::ALL[self.choice] {
                Refresh::Days(days) => format!("obc data policy {id} {days}"),
                Refresh::Manual => format!("obc data policy {id} manual"),
            },
            (Some(Overlay::Clean), _) if self.asking => "obc data clean --apply".into(),
            (Some(Overlay::Clean), _) | (None, Screen::Store) => "obc data clean".into(),
            (None, Screen::Sources) if self.checked_now => "obc data sources --check-now".into(),
            (None, Screen::Sources) => "obc data sources".into(),
            (None, Screen::Runs) => self.run_id().map_or("obc data runs".into(), |id| format!("obc data runs {id}")),
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
        if self.checking {
            frame.render_widget(Line::from("checking upstream…"), area);
            return;
        }
        let mut spans = Vec::new();
        for (label, does) in self.bindings().iter().filter_map(|binding| binding.bar) {
            spans.extend([Span::from(label).bold(), Span::from(format!(" {does}   "))]);
        }
        if let Some(notice) = &self.notice {
            spans.push(Span::styled(notice.clone(), Color::Red));
        }
        let keys = Line::from(spans);
        let command = self.command();
        if keys.width() + 1 + command.chars().count() <= area.width as usize {
            frame.render_widget(Line::from(command).dim().right_aligned(), area);
        }
        frame.render_widget(keys, area);
    }

    fn draw_overlay(&mut self, frame: &mut Frame, body: Rect, overlay: Overlay) {
        let id = self.sources.get(self.source).map_or("", |row| row.source.id.as_str());
        let (title, lines) = match overlay {
            Overlay::Help => (" HELP ".to_string(), help()),
            Overlay::Attribution => (" ATTRIBUTION ".into(), attribution(&self.sources)),
            Overlay::Policy => (format!(" POLICY · {id} "), self.policy_lines()),
            Overlay::Clean => (" CLEAN ".into(), self.clean_lines()),
        };
        let width = body.width * 4 / 5;
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).block(Block::bordered().title(title));
        // `line_count` adds the top and bottom border but wraps at the width it is given.
        let height = paragraph.line_count(width.saturating_sub(2));
        let area = body.centered(Constraint::Length(width), Constraint::Length(height as u16)).intersection(body);
        self.scroll = self.scroll.min(height.saturating_sub(area.height as usize));
        frame.render_widget(Clear, area);
        frame.render_widget(paragraph.scroll((self.scroll as u16, 0)), area);
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
    lines.push(line("Policy", "enter choose".into()));
    lines.push(line("Clean", "a clean, then y".into()));
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
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

    fn app() -> App {
        let sources = parse_sources(SOURCES).unwrap().into_iter().map(|source| SourceRow {
            live: Some(vec![if source.kind == Kind::Tool { "0.10.2" } else { "2024-01-02" }.into()]),
            upstream: (source.kind == Kind::Data).then(|| "2024-01-09".into()),
            age_days: None,
            state: State::Ok,
            reason: None,
            snapshots: vec![Stored { version: "2024-01-02".into(), bytes: 1_000_000 }],
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
        app
    }

    fn view(app: &App) -> (Screen, Option<Overlay>, [usize; 4], bool) {
        (app.screen, app.overlay, [app.source, app.kept, app.run, app.choice], app.asking)
    }

    fn opened(overlay: Overlay, source: usize, choice: usize) -> App {
        let mut app = App { source, ..app() };
        app.act(Action::Open(overlay));
        App { choice, ..app }
    }

    #[test]
    fn no_key_does_two_things_and_each_key_in_the_bar_acts() {
        let mut states = vec![app(), App::new(Vec::new(), Vec::new()), App { source: 1, ..app() }];
        for overlay in [Overlay::Help, Overlay::Attribution, Overlay::Policy, Overlay::Clean] {
            states.push(opened(overlay, 0, 0));
            states.push(opened(overlay, 1, 1));
        }
        states.push(App { asking: true, ..opened(Overlay::Clean, 0, 0) });
        states.push(App { store: Some(CleanPlan::default()), ..opened(Overlay::Clean, 0, 0) });
        states.push(App { screen: Screen::Store, ..app() });
        states.push(App { screen: Screen::Store, store: Some(CleanPlan::default()), ..app() });
        states.push(App { screen: Screen::Runs, ..app() });
        for app in states {
            let keys = app.bindings();
            for binding in &keys {
                assert_eq!(keys.iter().filter(|other| other.key == binding.key).count(), 1, "{:?}", binding.key);
            }
            for binding in keys.iter().filter(|binding| binding.bar.is_some()) {
                let mut after = app.clone();
                let effect = after.act(binding.action);
                assert!(effect != Effect::None || view(&after) != view(&app), "{:?} in {:?}", binding.key, view(&app));
            }
        }
    }

    #[test]
    fn enter_in_policy_sets_another_policy_of_a_date_source() {
        let mut app = app();
        assert_eq!(app.key(KeyCode::Char('e')), Effect::None);
        assert_eq!((app.overlay, app.choice), (Some(Overlay::Policy), 0));
        app.key(KeyCode::Down);
        assert_eq!(app.command(), "obc data policy osm 30");
        assert_eq!(app.key(KeyCode::Enter), Effect::Policy("osm".into(), Refresh::Days(30)));
        let mut tool = App { source: 1, ..app };
        tool.key(KeyCode::Char('e'));
        assert_eq!(tool.overlay, None, "a release version has no age");
    }

    #[test]
    fn a_clean_needs_a_then_y() {
        let mut app = App { screen: Screen::Store, ..app() };
        assert_eq!(app.key(KeyCode::Char('c')), Effect::PlanClean, "the plan of now");
        assert_eq!((app.overlay, app.command().as_str()), (Some(Overlay::Clean), "obc data clean"));
        for key in [KeyCode::Enter, KeyCode::Char('y')] {
            assert_eq!(app.key(key), Effect::None);
        }
        app.key(KeyCode::Char('a'));
        assert_eq!(app.command(), "obc data clean --apply");
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
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut click = |app: &mut App, hit: Hit| {
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let (area, _) = *app.hits.iter().find(|(_, drawn)| *drawn == hit).unwrap();
            app.click(area.x, area.y)
        };
        click(&mut app, Hit::Row(1));
        assert_eq!(view(&app), (Screen::Sources, None, [1, 0, 0, 0], false));
        click(&mut app, Hit::Screen(Screen::Store));
        assert_eq!((app.screen, app.overlay), (Screen::Store, None));
    }
}
