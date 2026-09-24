//! The settings pages: one screen type over nine row tables. A page is a list of rows in the
//! shared row grammar. A door opens a page, a value opens the drawer editor as a sheet over the
//! page, a switch flips in place, an act does something (a destructive one on a hold), and an info
//! row is read-only.
//!
//! Editing is live: a commit writes into the shared [`Settings`](crate::Settings), and
//! [`App::apply_gesture`](crate::App::apply_gesture) flags the host to persist the change.

use core::fmt::Write as _;

use obc_render::Surface;

use crate::ble::{BleLink, BondStatus};
use crate::input::Gesture;
use crate::screen::context_drawer::{ContextDrawerScreen, ContextFacts, ContextToggle, ContextValue};
use crate::screen::vocab::chrome::LIST_TOP;
use crate::screen::vocab::list::{list_frame, scrollbar};
use crate::screen::vocab::rows::{self, Line2, RowIcon, ROW_GAP};
use crate::screen::{Ctx, Render, Screen, Transition};
use crate::sensors::SensorPhase;
use crate::{t, AppState, Msg};

/// A page the hub or another page opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Door {
    Ride,
    Display,
    Sound,
    Connections,
    Power,
    System,
    StatFields,
    Sensors,
    DateTime,
    Language,
    Firmware,
    About,
    /// The factory reset's confirm page. A door lettered as the destructive act it leads to.
    Reset,
}

/// Something a row does when pressed, or, for a destructive act, when held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    /// Drop the phone bond. Guarded, and shown only while there is one to drop.
    ForgetPhone,
    /// Scan the card for an update. Inert while a ride records, because the install reboots.
    InstallUpdate,
}

/// A read-only line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Info {
    PhoneStatus,
    GpsFix,
    LocalTime,
    FwVersion,
    Map,
    CardFree,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Door(Door),
    Value(ContextValue),
    Toggle(ContextToggle),
    Act(Act),
    Info(Info),
}

pub(crate) struct Row {
    pub label: Msg,
    pub item: Item,
}

pub(crate) struct Menu {
    pub title: Msg,
    pub rows: &'static [Row],
}

const fn door(label: Msg, d: Door) -> Row {
    Row { label, item: Item::Door(d) }
}
const fn value(label: Msg, v: ContextValue) -> Row {
    Row { label, item: Item::Value(v) }
}
const fn toggle(label: Msg, t: ContextToggle) -> Row {
    Row { label, item: Item::Toggle(t) }
}
const fn act(label: Msg, a: Act) -> Row {
    Row { label, item: Item::Act(a) }
}
const fn info(label: Msg, i: Info) -> Row {
    Row { label, item: Item::Info(i) }
}

pub(crate) static HUB: Menu = Menu {
    title: Msg::SettingsTitle,
    rows: &[
        door(Msg::SettingsRide, Door::Ride),
        door(Msg::SettingsDisplay, Door::Display),
        door(Msg::SettingsSound, Door::Sound),
        door(Msg::SettingsConnections, Door::Connections),
        door(Msg::SettingsPower, Door::Power),
        door(Msg::SettingsSystem, Door::System),
    ],
};

pub(crate) static RIDE: Menu = Menu {
    title: Msg::RideTitle,
    rows: &[
        door(Msg::RideFields, Door::StatFields),
        value(Msg::RidePages, ContextValue::StatCycle),
        value(Msg::RideClimb, ContextValue::ClimbMode),
        value(Msg::RideWaypoints, ContextValue::WaypointMode),
        value(Msg::RideBikeType, ContextValue::BikeProfile),
        value(Msg::RideMaxHr, ContextValue::MaxHr),
        value(Msg::RideFtp, ContextValue::Ftp),
    ],
};

pub(crate) static DISPLAY: Menu = Menu {
    title: Msg::DisplayTitle,
    rows: &[
        value(Msg::DisplayBrightness, ContextValue::Brightness),
        value(Msg::DisplayTheme, ContextValue::Theme),
        value(Msg::DisplayIdle, ContextValue::IdleReturn),
    ],
};

pub(crate) static SOUND: Menu = Menu {
    title: Msg::SoundTitle,
    rows: &[value(Msg::SoundLevel, ContextValue::Sound), toggle(Msg::SoundKeyTones, ContextToggle::KeyTones)],
};

pub(crate) static CONNECTIONS: Menu = Menu {
    title: Msg::ConnectionsTitle,
    // The sensors above the phone: they are the rows a rider comes back to.
    rows: &[
        toggle(Msg::BluetoothRadio, ContextToggle::BleEnabled),
        door(Msg::ConnectionsSensors, Door::Sensors),
        info(Msg::ConnectionsPhone, Info::PhoneStatus),
        act(Msg::BluetoothForget, Act::ForgetPhone),
    ],
};

pub(crate) static POWER: Menu = Menu {
    title: Msg::PowerTitle,
    rows: &[value(Msg::PowerGpsFix, ContextValue::FixInterval), toggle(Msg::PowerPowerSave, ContextToggle::PowerSaver)],
};

pub(crate) static SYSTEM: Menu = Menu {
    title: Msg::SystemTitle,
    rows: &[
        value(Msg::SystemUnits, ContextValue::Units),
        door(Msg::SystemDatetime, Door::DateTime),
        door(Msg::SystemLanguage, Door::Language),
        door(Msg::SystemUpdate, Door::Firmware),
        door(Msg::SystemAbout, Door::About),
        door(Msg::ResetFactory, Door::Reset),
    ],
};

pub(crate) static DATETIME: Menu = Menu {
    title: Msg::DatetimeTitle,
    rows: &[
        info(Msg::DatetimeGpsFix, Info::GpsFix),
        info(Msg::DatetimeLocalTime, Info::LocalTime),
        value(Msg::DatetimeOffset, ContextValue::UtcOffset),
    ],
};

pub(crate) static FIRMWARE: Menu = Menu {
    title: Msg::FirmwareTitle,
    rows: &[
        act(Msg::FirmwareInstallUpdate, Act::InstallUpdate),
        info(Msg::FirmwareVersion, Info::FwVersion),
        info(Msg::FirmwareMap, Info::Map),
        info(Msg::FirmwareCardFree, Info::CardFree),
    ],
};

impl Item {
    /// Whether the row is on the page at all. The phone's Forget row is drawn only while there is a
    /// bond to drop, and the Sound door only on a platform that can make a sound.
    fn shown(self, state: &AppState) -> bool {
        match self {
            Item::Act(Act::ForgetPhone) => state.bond_status.can_forget(state.device.ble_paired),
            Item::Door(Door::Sound) => state.sound_available,
            _ => true,
        }
    }

    /// Whether the cursor can land on the row.
    fn selectable(self) -> bool {
        !matches!(self, Item::Info(_))
    }

    /// Whether the row acts right now. An inert row draws recessed.
    fn live(self, f: &ContextFacts) -> bool {
        match self {
            Item::Act(Act::InstallUpdate) => !f.recording,
            _ => true,
        }
    }

    fn two_lines(self) -> bool {
        match self {
            Item::Door(d) => matches!(d, Door::Sensors | Door::DateTime | Door::Language | Door::Firmware),
            Item::Value(_) | Item::Info(_) => true,
            Item::Act(Act::InstallUpdate) => true,
            Item::Toggle(_) | Item::Act(Act::ForgetPhone) => false,
        }
    }

    fn height(self) -> i32 {
        rows::row_height(self.two_lines())
    }
}

/// One settings page: its table and the cursor. The cursor indexes the table, and is resolved onto
/// a selectable row that is shown before it is read, so a row that disappears under it (the Forget
/// row after a completed forget) moves it to a neighbour.
pub struct SettingsPage {
    menu: &'static Menu,
    selected: usize,
}

impl core::fmt::Debug for SettingsPage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SettingsPage({}, row {})", t(self.menu.title, crate::settings::Language::En), self.selected)
    }
}

impl SettingsPage {
    pub(crate) fn new(menu: &'static Menu) -> Self {
        let selected = menu.rows.iter().position(|r| r.item.selectable()).unwrap_or(0);
        SettingsPage { menu, selected }
    }

    pub fn hub() -> Self {
        Self::new(&HUB)
    }

    /// The cursor, on a row that is shown and selectable. Walks forward, then wraps, so a hidden
    /// row under the cursor yields to the next one.
    fn resolved(&self, state: &AppState) -> usize {
        let rows = self.menu.rows;
        let ok = |i: usize| rows[i].item.shown(state) && rows[i].item.selectable();
        (0..rows.len()).map(|k| (self.selected + k) % rows.len()).find(|&i| ok(i)).unwrap_or(self.selected)
    }

    /// Move the cursor `n` selectable shown rows, wrapping at both ends.
    fn step(&mut self, n: i32, state: &AppState) {
        let rows = self.menu.rows;
        let len = rows.len() as i32;
        let dir = n.signum();
        let mut i = self.resolved(state) as i32;
        for _ in 0..n.unsigned_abs() {
            // Step at least one row, then on to the next selectable shown row, at most one lap.
            for _ in 0..len {
                i = (i + dir).rem_euclid(len);
                let item = rows[i as usize].item;
                if item.selectable() && item.shown(state) {
                    break;
                }
            }
        }
        self.selected = i as usize;
    }

    /// True while the cursor is on a guarded act, so its hold fill draws.
    pub(crate) fn selection_is_guarded(&self, state: &AppState) -> bool {
        matches!(self.menu.rows[self.resolved(state)].item, Item::Act(Act::ForgetPhone))
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let i = self.resolved(cx.state);
        let row = &self.menu.rows[i];
        let live = row.item.live(&cx.context_facts());
        match g {
            Gesture::Step(n) => {
                self.step(n, cx.state);
                Transition::None
            }
            Gesture::Press => match row.item {
                Item::Door(d) => Transition::Push(d.open(cx)),
                Item::Value(v) => Transition::Push(Screen::ContextDrawer(ContextDrawerScreen::editor(
                    v,
                    row.label,
                    &cx.context_facts(),
                ))),
                Item::Toggle(t) => {
                    t.flip(cx);
                    Transition::None
                }
                Item::Act(Act::InstallUpdate) if live => {
                    cx.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
                    Transition::Push(Screen::DfuCheck(super::super::DfuCheckScreen::new()))
                }
                Item::Act(_) | Item::Info(_) => Transition::None,
            },
            // The completed hold is the confirmation. The host does the removal.
            Gesture::Hold if row.item == Item::Act(Act::ForgetPhone) => {
                cx.state.ble_forget_requested = true;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        let facts = rx.context_facts();
        let selected = self.resolved(rx.state);
        let shown: heapless::Vec<usize, 8> =
            (0..self.menu.rows.len()).filter(|&i| self.menu.rows[i].item.shown(rx.state)).collect();
        let heights: heapless::Vec<i32, 8> = shown.iter().map(|&i| self.menu.rows[i].item.height()).collect();
        let avail = h - LIST_TOP - 6;
        let sel_pos = shown.iter().position(|&i| i == selected).unwrap_or(0);
        let (first, end) = rows::window_by_height(&heights, sel_pos, avail);
        list_frame(cv, w, h, rx.t(self.menu.title), sel_pos + 1, shown.len(), end - first);

        let mut y = LIST_TOP;
        for k in first..end {
            let row = &self.menu.rows[shown[k]];
            let area = rows::row_rect(y, w, heights[k]);
            let is_selected = shown[k] == selected;
            let live = row.item.live(&facts);
            let label = rx.t(row.label);
            let mut buf = heapless::String::<24>::new();
            match row.item {
                Item::Door(Door::Reset) => rows::danger_door_row(cv, area, label, is_selected),
                Item::Door(d) => rows::nav_row(cv, area, label, d.hint(rx, &mut buf), is_selected, live, true),
                Item::Value(v) => {
                    let text = v.choice_label(v.committed(&facts), rx, &mut buf);
                    rows::nav_row(cv, area, label, Some(Line2::text(text)), is_selected, live, true);
                }
                Item::Toggle(tg) => rows::switch_row(cv, area, label, tg.read(&facts), is_selected),
                Item::Act(a) => {
                    let hint = (a == Act::InstallUpdate && !live).then(|| rx.t(Msg::FirmwareRecording));
                    let danger = a == Act::ForgetPhone;
                    rows::action_row(cv, area, label, hint, is_selected, live, danger, rx.hold_progress);
                }
                Item::Info(i) => rows::info_row(cv, area, label, Line2::text(i.text(rx, &mut buf))),
            }
            y += heights[k] + ROW_GAP;
        }
        scrollbar(cv, w - 8, LIST_TOP, avail, shown.len(), first, end - first);
    }
}

impl Door {
    fn open(self, cx: &mut Ctx) -> Screen {
        match self {
            Door::Ride => Screen::Ride(SettingsPage::new(&RIDE)),
            Door::Display => Screen::Display(SettingsPage::new(&DISPLAY)),
            Door::Sound => Screen::Sound(SettingsPage::new(&SOUND)),
            Door::Connections => Screen::Connections(SettingsPage::new(&CONNECTIONS)),
            Door::Power => Screen::Power(SettingsPage::new(&POWER)),
            Door::System => Screen::System(SettingsPage::new(&SYSTEM)),
            Door::StatFields => Screen::StatFields(super::StatFieldsScreen::new()),
            Door::Sensors => Screen::Sensors(super::SensorsScreen::new()),
            Door::DateTime => Screen::DateTime(SettingsPage::new(&DATETIME)),
            Door::Language => Screen::Language(super::LanguageScreen::new(cx.settings.language)),
            Door::Firmware => {
                // The Firmware page shows the free space, so start the card scan on entry.
                cx.storage.admit_intent(crate::device_core::storage_info::StorageInfoIntent::RefreshRequested);
                Screen::Firmware(SettingsPage::new(&FIRMWARE))
            }
            Door::About => Screen::About(super::AboutScreen::new()),
            Door::Reset => Screen::Reset(super::ResetScreen::new()),
        }
    }

    /// The line under a door's label: what is behind it, when that is worth a glance.
    fn hint<'a>(self, rx: &'a Render, buf: &'a mut heapless::String<24>) -> Option<Line2<'a>> {
        let lang = rx.settings.language;
        match self {
            Door::Language => Some(Line2 { icon: Some(RowIcon::Flag(lang)), text: lang.name() }),
            Door::Firmware => Some(Line2::text(if rx.fw_version.is_empty() { "--" } else { rx.fw_version })),
            Door::DateTime => {
                // Day, month, time: 13 cells at most, which clears the chevron column in every
                // language.
                let local = rx.settings.local_clock();
                let _ = write!(
                    buf,
                    "{} {} {:02}:{:02}",
                    local.day,
                    crate::settings::month_name(local, lang),
                    local.hour,
                    local.minute
                );
                Some(Line2::text(buf.as_str()))
            }
            Door::Sensors => {
                if !rx.settings.saved_sensors.iter().any(|s| s.present) {
                    return Some(Line2::text(rx.t(Msg::SensorsNotSet)));
                }
                let connected = rx.sensor_status.iter().filter(|s| s.phase == SensorPhase::Connected).count();
                let _ = write!(buf, "{connected} {}", rx.t(Msg::ConnectionsConnected));
                Some(Line2::text(buf.as_str()))
            }
            _ => None,
        }
    }
}

impl Info {
    fn text<'a>(self, rx: &'a Render, buf: &'a mut heapless::String<24>) -> &'a str {
        let s = rx.settings;
        let lang = s.language;
        match self {
            Info::PhoneStatus => {
                let device = rx.state.device;
                match rx.state.bond_status {
                    BondStatus::Pending => rx.t(Msg::BluetoothRemoving),
                    BondStatus::Failed(_) => rx.t(Msg::BluetoothRemoveFailed),
                    BondStatus::RestartRequired => rx.t(Msg::BluetoothRestart),
                    // The switch decides `Off`, so the line flips with it and does not wait for
                    // the radio to wind down.
                    _ if !s.ble_enabled || device.ble_link == BleLink::Off => rx.t(Msg::BluetoothOff),
                    _ if !device.ble_paired => rx.t(Msg::ConnectionsNotPaired),
                    _ if device.ble_link == BleLink::Connected => rx.t(Msg::BluetoothConnected),
                    _ => rx.t(Msg::BluetoothAdvertising),
                }
            }
            Info::GpsFix => {
                if rx.state.user_fix.is_some() {
                    let _ = write!(buf, "{}{:02}:{:02}", t(Msg::DatetimeUtc, lang), s.clock.hour, s.clock.minute);
                    buf.as_str()
                } else {
                    rx.t(Msg::DatetimeSearching)
                }
            }
            Info::LocalTime => {
                // The offset can cross midnight, so take the date and the time from `local_clock`.
                // The ISO date, because a spelled month plus the year runs past the row.
                let local = s.local_clock();
                let _ = write!(
                    buf,
                    "{}-{:02}-{:02} {:02}:{:02}",
                    local.year, local.month, local.day, local.hour, local.minute
                );
                buf.as_str()
            }
            Info::FwVersion => {
                if rx.fw_version.is_empty() {
                    "--"
                } else {
                    rx.fw_version
                }
            }
            Info::Map => {
                if rx.map_name.is_empty() {
                    "--"
                } else {
                    let _ = write!(buf, "{} \u{00b7} v{}", rx.map_name, rx.map_obcm_version);
                    buf.as_str()
                }
            }
            Info::CardFree => match rx.card_free_bytes {
                Some(bytes) => {
                    let mut short = heapless::String::<16>::new();
                    super::super::vocab::fmt::write_bytes_short(&mut short, bytes);
                    let _ = buf.push_str(&short);
                    buf.as_str()
                }
                None => "--",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, DfuAction};
    use crate::screen::test_ctx;
    use crate::settings::{ClimbMode, Language, Units};
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut SettingsPage, st: &mut AppState, s: &mut Settings, g: Gesture) -> Transition {
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    fn world() -> (AppState, Settings) {
        (AppState::new(0, 0, 1.0), Settings::default())
    }

    #[test]
    fn every_hub_door_opens_its_page_and_the_tables_fit_a_page() {
        let (mut st, mut s) = world();
        let mut hub = SettingsPage::hub();
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::Ride(_))));
        run(&mut hub, &mut st, &mut s, Gesture::Step(1));
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::Display(_))));
        run(&mut hub, &mut st, &mut s, Gesture::Step(1));
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::Connections(_))));
        run(&mut hub, &mut st, &mut s, Gesture::Step(1));
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::Power(_))));
        run(&mut hub, &mut st, &mut s, Gesture::Step(1));
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::System(_))));
        run(&mut hub, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(hub.selected, 0, "the cursor wraps");
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Back), Transition::Pop));

        st.sound_available = true;
        run(&mut hub, &mut st, &mut s, Gesture::Step(2));
        assert!(matches!(run(&mut hub, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::Sound(_))));

        for menu in [&HUB, &RIDE, &DISPLAY, &SOUND, &CONNECTIONS, &POWER, &SYSTEM, &DATETIME, &FIRMWARE] {
            assert!(menu.rows.len() <= 8, "a page's rows fit the draw's window vectors");
            assert!(menu.rows.iter().any(|r| r.item.selectable()), "a page has a row to land on");
        }
    }

    #[test]
    fn a_value_row_opens_the_editor_sheet_and_a_switch_flips_in_place() {
        let (mut st, mut s) = world();
        let mut ride = SettingsPage::new(&RIDE);
        run(&mut ride, &mut st, &mut s, Gesture::Step(2));
        let t = run(&mut ride, &mut st, &mut s, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::ContextDrawer(_))), "the Climb row opens its editor as a sheet");
        assert_eq!(s.climb_mode, ClimbMode::Auto, "opening commits nothing");

        let mut power = SettingsPage::new(&POWER);
        run(&mut power, &mut st, &mut s, Gesture::Step(1));
        assert!(matches!(run(&mut power, &mut st, &mut s, Gesture::Press), Transition::None));
        assert!(s.power_saver, "press flips the switch");

        let mut system = SettingsPage::new(&SYSTEM);
        assert!(matches!(
            run(&mut system, &mut st, &mut s, Gesture::Press),
            Transition::Push(Screen::ContextDrawer(_))
        ));
        assert_eq!(s.units, Units::Metric);
        run(&mut system, &mut st, &mut s, Gesture::Step(2));
        s.language = Language::Fr;
        assert!(matches!(run(&mut system, &mut st, &mut s, Gesture::Press), Transition::Push(Screen::Language(_))));
    }

    #[test]
    fn the_cursor_skips_info_rows_and_the_forget_row_comes_and_goes() {
        let (mut st, mut s) = world();
        let mut conn = SettingsPage::new(&CONNECTIONS);
        assert_eq!(conn.selected, 0, "starts on the Bluetooth switch");
        run(&mut conn, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(conn.selected, 1, "→ Sensors");
        run(&mut conn, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(conn.selected, 0, "unpaired: the info row and the hidden Forget row are skipped, so it wraps");
        run(&mut conn, &mut st, &mut s, Gesture::Hold);
        assert!(!st.ble_forget_requested, "unpaired: a hold does nothing");

        st.device.ble_paired = true;
        run(&mut conn, &mut st, &mut s, Gesture::Step(2));
        assert_eq!(conn.selected, 3, "paired: the Forget row is on the page, past the info row");
        assert!(conn.selection_is_guarded(&st));
        run(&mut conn, &mut st, &mut s, Gesture::Press);
        assert!(!st.ble_forget_requested, "a plain press never forgets");
        run(&mut conn, &mut st, &mut s, Gesture::Hold);
        assert!(st.ble_forget_requested, "the completed hold records the forget request");

        st.device.ble_paired = false;
        assert_eq!(conn.resolved(&st), 0, "the row under the cursor vanished: it moves on");
        assert!(!conn.selection_is_guarded(&st));

        let mut dt = SettingsPage::new(&DATETIME);
        assert_eq!(dt.selected, 2, "Date & time parks on its one editable row");
        run(&mut dt, &mut st, &mut s, Gesture::Step(-3));
        assert_eq!(dt.selected, 2, "…and stays there");
    }

    /// Every page label clears its right-hand column on the 240 px panel in every language, in
    /// Body or in the Label cut the row falls back to, and none needs a second line for itself.
    #[test]
    fn every_page_label_fits_its_column_in_every_language() {
        use obc_render::text::{text_width, Font};
        let area_w = 240 - 2 * rows::ROW_X;
        for lang in Language::ALL {
            for menu in [&HUB, &RIDE, &DISPLAY, &SOUND, &CONNECTIONS, &POWER, &SYSTEM, &DATETIME, &FIRMWARE] {
                for row in menu.rows {
                    let label = t(row.label, lang);
                    assert!(
                        !label.contains('\n'),
                        "{lang:?}: page label {label:?} breaks lines; a page row states a value there"
                    );
                    let room = match row.item {
                        Item::Toggle(_) => area_w - 10 - 58,
                        Item::Door(_) | Item::Value(_) => area_w - 10 - 28,
                        Item::Act(_) | Item::Info(_) => area_w - 10,
                    };
                    let lw = text_width(label, Font::Label) as i32;
                    assert!(lw <= room, "{lang:?}: page label {label:?} ({lw} px) overruns {room} px even in Label");
                }
            }
            // The fixed second lines that are copy rather than data: the fix status on an info
            // row, and the empty sensors hint under a door.
            for (msg, room) in [(Msg::DatetimeSearching, area_w - 10), (Msg::SensorsNotSet, area_w - 10 - 28)] {
                let lw = text_width(t(msg, lang), Font::Label) as i32;
                assert!(lw <= room, "{lang:?}: {:?} overruns its row", t(msg, lang));
            }
        }
    }

    fn drained_dfu(dfu: &mut crate::dfu::DfuState) -> Option<DfuAction> {
        dfu.next_effect().map(|effect| match effect {
            crate::dfu::DfuEffect::Scan { .. } => DfuAction::Scan,
            crate::dfu::DfuEffect::ArmInstall { .. } => DfuAction::Install,
        })
    }

    #[test]
    fn install_update_posts_a_scan_unless_a_ride_records() {
        let (mut st, mut s) = world();
        let mut act = Activity::new(Mode::Idle);
        let mut rec = crate::RecorderMachine::new();
        let mut dfu = crate::dfu::DfuState::new();
        let mut fw = SettingsPage::new(&FIRMWARE);
        let t = {
            let mut cx = Ctx { dfu: &mut dfu, recorder: &mut rec, ..test_ctx(&mut st, &mut act, &mut s) };
            fw.handle(Gesture::Press, &mut cx)
        };
        assert!(matches!(t, Transition::Push(Screen::DfuCheck(_))), "opens the scan wait");
        assert_eq!(drained_dfu(&mut dfu), Some(DfuAction::Scan), "and posts a Scan request");

        rec.test_open();
        let t = {
            let mut cx = Ctx { dfu: &mut dfu, recorder: &mut rec, ..test_ctx(&mut st, &mut act, &mut s) };
            fw.handle(Gesture::Press, &mut cx)
        };
        assert!(matches!(t, Transition::None), "disabled while recording");
        assert_eq!(drained_dfu(&mut dfu), None, "and nothing is posted");
    }

    #[test]
    fn opening_firmware_arms_the_free_space_measurement() {
        let (mut st, mut s) = world();
        let mut act = Activity::new(Mode::Idle);
        let mut storage = crate::device_core::storage_info::StorageInfo::new();
        let mut system = SettingsPage::new(&SYSTEM);
        let t = {
            let mut cx = Ctx { storage: &mut storage, ..test_ctx(&mut st, &mut act, &mut s) };
            system.handle(Gesture::Step(3), &mut cx);
            system.handle(Gesture::Press, &mut cx)
        };
        assert!(matches!(t, Transition::Push(Screen::Firmware(_))));
        assert!(storage.next_effect().is_some(), "opening Firmware arms the free-space measurement");
    }
}
