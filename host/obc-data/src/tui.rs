//! The TUI of `obc data`. Each screen shows what a command writes with `--json`, and the bar
//! names that command. The only change it makes is a `refresh`, through the same function.

use std::path::Path;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use obc_data::engine::runs::{self, Details, Outcome, Summary};
use obc_data::sources::{Kind, State};
use obc_data::store::Store;

use crate::runs_cli::{bytes, duration, mark, step_cells};
use crate::{refresh, registry, row_text, source_rows, widths, Error, SourceRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Sources,
    Runs,
}

/// The key of a screen is its number among the five screens of the TUI; 1 Live, 2 Local and
/// 4 Store are not built.
const SCREENS: [(char, &str, Screen); 2] = [('3', "Sources", Screen::Sources), ('5', "Runs", Screen::Runs)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Help,
    Attribution,
    Pin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Show(Screen),
    NextScreen,
    Up,
    Down,
    Pin,
    Attribution,
    Refresh,
    Help,
    Close,
    Quit,
}

/// What the loop does after a key or a click.
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    None,
    Quit,
    /// `obc data refresh SOURCE`.
    Refresh(String),
}

/// A key that works now, with its label and what it does when the bar shows it.
struct Binding {
    key: KeyCode,
    action: Action,
    bar: Option<(&'static str, &'static str)>,
}

/// The keys that the bar shows on a screen with rows, besides `?`.
fn screen_keys(screen: Screen) -> &'static [(KeyCode, Action, &'static str, &'static str)] {
    match screen {
        Screen::Sources => &[
            (KeyCode::Enter, Action::Pin, "enter", "pin"),
            (KeyCode::Char('L'), Action::Attribution, "L", "attribution"),
        ],
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
    /// Running runs first, then newest first.
    runs: Vec<Summary>,
    /// The selected run, once it is read.
    details: Option<Details>,
    source: usize,
    run: usize,
    /// The first line that an overlay shows.
    scroll: usize,
    /// The error of the last refresh.
    notice: Option<String>,
    /// Where the last frame drew each tab and row.
    hits: Vec<(Rect, Hit)>,
}

pub fn run(root: &Path) -> Result<(), Error> {
    let store = Store::open()?;
    let mut app = App::new(source_rows(&registry(root)?)?, list_runs(&store)?);
    let mut terminal = start()?;
    let result = run_loop(root, &store, &mut app, &mut terminal);
    stop();
    result
}

fn run_loop(root: &Path, store: &Store, app: &mut App, terminal: &mut DefaultTerminal) -> Result<(), Error> {
    loop {
        if app.details.as_ref().map(|d| d.summary.id.as_str()) != app.run_id() {
            app.details = app.run_id().map(|id| runs::details(store, id)).transpose()?;
        }
        terminal.draw(|frame| app.draw(frame)).map_err(|e| e.to_string())?;
        let screen = app.screen;
        if !event::poll(Duration::from_secs(1)).map_err(|e| e.to_string())? {
            if app.screen == Screen::Runs && app.runs.iter().any(|r| r.outcome == Outcome::Running) {
                (app.runs, app.details) = (list_runs(store)?, None);
            }
            continue;
        }
        let effect = match event::read().map_err(|e| e.to_string())? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.key(key.code),
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                app.click(mouse.column, mouse.row)
            }
            _ => Effect::None,
        };
        if app.screen == Screen::Runs && screen != Screen::Runs {
            (app.runs, app.details) = (list_runs(store)?, None);
        }
        match effect {
            Effect::None => {}
            Effect::Quit => return Ok(()),
            Effect::Refresh(id) => {
                // The fetch writes its progress to the terminal.
                stop();
                let result = refresh(root, &id, &[], "live", false);
                *terminal = start()?;
                app.notice = result.err().map(|e| e.message);
                app.sources = source_rows(&registry(root)?)?;
            }
        }
    }
}

fn start() -> Result<DefaultTerminal, Error> {
    let terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    execute!(std::io::stdout(), EnableMouseCapture).map_err(|e| e.to_string())?;
    Ok(terminal)
}

fn stop() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

fn list_runs(store: &Store) -> Result<Vec<Summary>, Error> {
    let mut runs = runs::list(store)?;
    runs.sort_by_key(|run| run.outcome != Outcome::Running);
    Ok(runs)
}

impl App {
    fn new(sources: Vec<SourceRow>, runs: Vec<Summary>) -> Self {
        let (screen, overlay, details, notice, hits) = (Screen::Sources, None, None, None, Vec::new());
        Self { screen, overlay, sources, runs, details, source: 0, run: 0, scroll: 0, notice, hits }
    }

    fn rows(&self) -> usize {
        match self.screen {
            Screen::Sources => self.sources.len(),
            Screen::Runs => self.runs.len(),
        }
    }

    /// The selected row, or the scroll of an overlay.
    fn selected(&mut self) -> &mut usize {
        match (self.overlay, self.screen) {
            (Some(_), _) => &mut self.scroll,
            (None, Screen::Sources) => &mut self.source,
            (None, Screen::Runs) => &mut self.run,
        }
    }

    fn run_id(&self) -> Option<&str> {
        self.runs.get(self.run).map(|run| run.id.as_str())
    }

    /// The newest upstream version of the selected source, when it is not the pin.
    fn newer(&self) -> Option<&str> {
        let row = self.sources.get(self.source)?;
        row.upstream.as_deref().filter(|&upstream| row.pin.as_deref() != Some(upstream))
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
                if overlay == Overlay::Pin && self.newer().is_some() {
                    keys.push(bar(KeyCode::Enter, Action::Refresh, "enter", "refresh"));
                }
                keys.push(bar(KeyCode::Esc, Action::Close, "esc", "close"));
            }
            None => {
                keys.extend(SCREENS.iter().map(|&(key, _, screen)| hidden(KeyCode::Char(key), Action::Show(screen))));
                keys.push(hidden(KeyCode::Tab, Action::NextScreen));
                if self.rows() > 0 {
                    keys.extend(
                        screen_keys(self.screen)
                            .iter()
                            .map(|&(key, action, label, does)| bar(key, action, label, does)),
                    );
                }
                keys.push(bar(KeyCode::Char('?'), Action::Help, "?", "help"));
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
        // The drawing stops the scroll of an overlay at its end.
        let last = if self.overlay.is_some() { usize::MAX } else { self.rows().saturating_sub(1) };
        match action {
            Action::Show(screen) => self.screen = screen,
            Action::NextScreen => {
                let at = SCREENS.iter().position(|&(_, _, screen)| screen == self.screen).unwrap_or(0);
                self.screen = SCREENS[(at + 1) % SCREENS.len()].2;
            }
            Action::Up => *self.selected() = self.selected().saturating_sub(1),
            Action::Down => *self.selected() = (*self.selected() + 1).min(last),
            Action::Pin => (self.overlay, self.scroll) = (Some(Overlay::Pin), 0),
            Action::Attribution => (self.overlay, self.scroll) = (Some(Overlay::Attribution), 0),
            Action::Help => (self.overlay, self.scroll) = (Some(Overlay::Help), 0),
            Action::Close => self.overlay = None,
            Action::Refresh => {
                self.overlay = None;
                return Effect::Refresh(self.sources[self.source].source.id.clone());
            }
            Action::Quit => return Effect::Quit,
        }
        Effect::None
    }

    /// A click on a tab shows its screen. A click on a row selects it; a second click is `enter`.
    fn click(&mut self, x: u16, y: u16) -> Effect {
        let hit = self.hits.iter().find(|(area, _)| area.contains(Position { x, y })).map(|&(_, hit)| hit);
        match hit {
            Some(Hit::Screen(screen)) => {
                self.overlay = None;
                self.screen = screen;
            }
            Some(Hit::Row(row)) if self.overlay.is_none() => {
                if *self.selected() == row {
                    return self.key(KeyCode::Enter);
                }
                *self.selected() = row;
            }
            _ => {}
        }
        Effect::None
    }

    /// The command that shows the same, or does the same.
    fn command(&self) -> String {
        match (self.overlay, self.screen) {
            (Some(Overlay::Help), _) => "obc data --help".into(),
            (Some(Overlay::Pin), _) if self.newer().is_some() => {
                format!("obc data refresh {}", self.sources[self.source].source.id)
            }
            (Some(_), _) => "obc data sources --json".into(),
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
            Screen::Runs => self.draw_runs(frame, body),
        }
        self.draw_bar(frame, bar);
        if let Some(overlay) = self.overlay {
            self.draw_overlay(frame, body, overlay);
        }
    }

    fn draw_sources(&mut self, frame: &mut Frame, area: Rect) {
        let header = ["SOURCE", "LICENCE", "R2 COPY", "LIVE PIN", "AGE", "POLICY", "STATE"];
        let mut table = vec![header.map(String::from).to_vec()];
        table.extend(self.sources.iter().map(|row| {
            let s = &row.source;
            vec![
                s.id.clone(),
                row.licence(),
                if s.r2_copy { "yes" } else { "no" }.into(),
                row.short(row.pin.as_deref()),
                row.age_days.map_or("—".into(), |age| format!("{age} d")),
                s.refresh.to_string(),
                row.state.to_string(),
            ]
        }));
        let widths = widths(&table);
        let mut lines = vec![Line::from(row_text(&table[0], &widths)).dim()];
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
        self.draw_lines(frame, area, lines, &at, end);
    }

    fn draw_runs(&mut self, frame: &mut Frame, area: Rect) {
        let mut table = vec![["WHEN", "COMMAND", "", "TOOK", "MOVED"].map(String::from).to_vec()];
        table.extend(self.runs.iter().map(|run| {
            let when = run.started.get(..16).unwrap_or(&run.started).replace('T', " ");
            let took = run.wall_ms.map_or("—".into(), duration);
            vec![when, run.command.clone(), mark(run.outcome).into(), took, moved(run)]
        }));
        let widths = widths(&table);
        let mut lines = vec![Line::from(row_text(&table[0], &widths)).dim()];
        for (i, cells) in table[1..].iter().enumerate() {
            let line = Line::from(row_text(cells, &widths));
            lines.push(if i == self.run { line.reversed() } else { line });
        }
        let height = (lines.len() as u16).min(area.height / 2);
        let [list, steps] = Layout::vertical([Constraint::Length(height + 1), Constraint::Fill(1)]).areas(area);
        let at: Vec<usize> = (1..lines.len()).collect();
        self.draw_lines(frame, list, lines, &at, self.run + 1);
        if let Some(details) = self.details.as_ref().filter(|d| Some(d.summary.id.as_str()) == self.run_id()) {
            frame.render_widget(Paragraph::new(steps_lines(details)), steps);
        }
    }

    /// Draw `lines` scrolled so that line `end` shows, and note where row `i` starts: `at[i]`.
    fn draw_lines(&mut self, frame: &mut Frame, area: Rect, lines: Vec<Line>, at: &[usize], end: usize) {
        let offset = (end + 1).saturating_sub(area.height as usize);
        for (row, &line) in at.iter().enumerate() {
            if let Some(y) = line.checked_sub(offset).filter(|&y| y < area.height as usize) {
                self.hits.push((Rect { y: area.y + y as u16, height: 1, ..area }, Hit::Row(row)));
            }
        }
        frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), area);
    }

    fn draw_bar(&self, frame: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        for (label, does) in self.bindings().iter().filter_map(|binding| binding.bar) {
            spans.extend([Span::from(label).bold(), Span::from(format!(" {does}   "))]);
        }
        if let Some(notice) = &self.notice {
            spans.push(Span::styled(notice.clone(), Color::Red));
        }
        frame.render_widget(Line::from(spans), area);
        frame.render_widget(Line::from(self.command()).dim().right_aligned(), area);
    }

    fn draw_overlay(&mut self, frame: &mut Frame, body: Rect, overlay: Overlay) {
        let (title, lines) = match overlay {
            Overlay::Help => (" HELP ".to_string(), help()),
            Overlay::Attribution => (" ATTRIBUTION ".into(), attribution(&self.sources)),
            Overlay::Pin => {
                let row = &self.sources[self.source];
                (format!(" PIN · {} ", row.source.id), pin(row, self.newer()))
            }
        };
        let width = body.width * 4 / 5;
        let inner = width.saturating_sub(2).max(1) as usize;
        let height = lines.iter().map(|line| line.width().div_ceil(inner).max(1)).sum::<usize>() + 2;
        let area = body.centered(Constraint::Length(width), Constraint::Length(height as u16)).intersection(body);
        self.scroll = self.scroll.min(height - area.height as usize);
        frame.render_widget(Clear, area);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((self.scroll as u16, 0));
        frame.render_widget(paragraph.block(Block::bordered().title(title)), area);
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
        let licence = [Some(row.licence()), s.obligations.clone(), s.licence_url.clone()];
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

fn pin(row: &SourceRow, newer: Option<&str>) -> Vec<Line<'static>> {
    let mut versions: Vec<(String, String)> =
        row.snapshots.iter().map(|snapshot| (snapshot.version.clone(), bytes(snapshot.bytes))).collect();
    if let Some(pin) = row.pin.clone().filter(|pin| !versions.iter().any(|(version, _)| version == pin)) {
        versions.push((pin, "—".into()));
        versions.sort_by(|a, b| b.0.cmp(&a.0));
    }
    let width = versions.iter().map(|(version, _)| version.len()).chain(newer.map(str::len)).max().unwrap_or(0);
    let mut lines: Vec<Line> = versions
        .into_iter()
        .map(|(version, size)| {
            let mark = if row.pin.as_deref() == Some(&version) { "●" } else { " " };
            Line::from(format!("{mark} {version:<width$}  {size}"))
        })
        .collect();
    lines.extend(newer.map(|newer| Line::from(format!("  {newer:<width$}  upstream")).reversed()));
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
    lines.push(line("Pin", "enter refresh".into()));
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::Stored;
    use obc_data::sources::parse_sources;

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
            pin: Some(if source.kind == Kind::Tool { "0.10.2" } else { "2024-01-02" }.into()),
            upstream: (source.kind == Kind::Data).then(|| "2024-01-09".into()),
            age_days: None,
            state: State::Ok,
            reason: None,
            snapshots: vec![Stored { version: "2024-01-02".into(), bytes: 1_000_000 }],
            source,
        });
        let run = |id: &str| Summary {
            id: id.into(),
            command: "build test".into(),
            started: "2024-01-10T12:00:00Z".into(),
            outcome: Outcome::Ok,
            wall_ms: Some(1000),
            bytes_fetched: 0,
            bytes_built: 10,
        };
        App::new(sources.collect(), vec![run("2024-01-10-120000"), run("2024-01-09-120000")])
    }

    fn view(app: &App) -> (Screen, Option<Overlay>, usize, usize) {
        (app.screen, app.overlay, app.source, app.run)
    }

    #[test]
    fn no_key_does_two_things_and_each_key_in_the_bar_acts() {
        let mut states = vec![app(), App::new(Vec::new(), Vec::new())];
        for overlay in [Overlay::Help, Overlay::Attribution, Overlay::Pin] {
            states.push(App { overlay: Some(overlay), ..app() });
        }
        states.push(App { overlay: Some(Overlay::Pin), source: 1, ..app() });
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
    fn enter_on_a_newer_upstream_version_refreshes_the_source() {
        let mut app = App { overlay: Some(Overlay::Pin), ..app() };
        assert_eq!(app.command(), "obc data refresh osm");
        assert_eq!(app.key(KeyCode::Enter), Effect::Refresh("osm".into()));
        let mut tool = App { overlay: Some(Overlay::Pin), source: 1, ..app };
        assert_eq!(tool.key(KeyCode::Enter), Effect::None);
    }

    #[test]
    fn a_click_selects_a_row_and_a_second_click_opens_its_pin() {
        let mut app = app();
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut click = |app: &mut App, hit: Hit| {
            terminal.draw(|frame| app.draw(frame)).unwrap();
            let (area, _) = *app.hits.iter().find(|(_, drawn)| *drawn == hit).unwrap();
            app.click(area.x, area.y)
        };
        assert_eq!(click(&mut app, Hit::Row(1)), Effect::None);
        assert_eq!(view(&app), (Screen::Sources, None, 1, 0));
        assert_eq!(click(&mut app, Hit::Row(1)), Effect::None);
        assert_eq!(app.overlay, Some(Overlay::Pin));
        click(&mut app, Hit::Screen(Screen::Runs));
        assert_eq!((app.screen, app.overlay), (Screen::Runs, None));
    }
}
