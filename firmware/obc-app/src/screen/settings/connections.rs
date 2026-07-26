//! The Connections settings menu — the device's two radios in one drawer: **Phone** (the BLE
//! pairing screen, [`BluetoothScreen`]) and **Sensors** (the HR / power / cadence scan,
//! [`SensorsScreen`]). A thin nav list whose rows open those existing pages unchanged — no controls
//! of its own, so it's pure navigation like the top-level Settings list.

use obc_render::Surface;

use crate::input::Gesture;
use crate::screen::{list, BluetoothScreen, Ctx, Render, Screen, SensorsScreen, Transition};
use crate::Msg;

/// The two rows: Phone (BLE pairing) then Sensors (BLE sensors).
const N_ITEMS: usize = 2;

/// The Connections menu. State is the highlighted row.
#[derive(Debug, Default)]
pub struct ConnectionsScreen {
    selected: usize,
}

impl ConnectionsScreen {
    pub fn new() -> Self {
        ConnectionsScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, N_ITEMS),
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::Bluetooth(BluetoothScreen::new())),
                _ => Transition::Push(Screen::Sensors(SensorsScreen::new())),
            },
            Gesture::Back => Transition::Pop, // climb back to the Settings list
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let items: [&str; N_ITEMS] = [rx.t(Msg::ConnectionsPhone), rx.t(Msg::ConnectionsSensors)];
        list::nav_list(cv, rx.w, rx.h, rx.t(Msg::ConnectionsTitle), &items, self.selected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut ConnectionsScreen, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut s = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: &mut s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Phone opens the Bluetooth screen; Sensors opens the Sensors screen; Back climbs out.
    #[test]
    fn rows_open_their_pages() {
        let mut scr = ConnectionsScreen::new();
        assert!(matches!(run(&mut scr, Gesture::Press), Transition::Push(Screen::Bluetooth(_))));
        run(&mut scr, Gesture::Step(1)); // → Sensors
        assert_eq!(scr.selected, 1);
        assert!(matches!(run(&mut scr, Gesture::Press), Transition::Push(Screen::Sensors(_))));
        assert!(matches!(run(&mut scr, Gesture::Back), Transition::Pop));
    }
}
