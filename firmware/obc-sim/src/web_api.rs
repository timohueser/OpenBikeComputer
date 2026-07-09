//! JS ⇄ sim bridge for the landing page's guided feature demos (wasm only).
//!
//! A `thread_local!` command queue the eframe app ([`SimGui::update`](crate::gui)) drains once per
//! frame, plus a published current-screen name the page polls. This makes the page's demo engine
//! **closed-loop**: it pushes gestures/playback commands and advances a step only when the sim
//! actually reached the next screen — so the real, variable-duration route planner and the ambient
//! playback never desync from a script of fixed sleeps. The bridge holds no handle to the
//! eframe-owned `SimGui` (the standard eframe/JS interop shape); the app reaches in via the two
//! `pub(crate)` helpers each frame.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::collections::VecDeque;

// Reach wasm-bindgen through eframe's re-export (the crate has no direct `wasm-bindgen` dep). The
// bare `use eframe::wasm_bindgen;` binds the crate name so the `#[wasm_bindgen]` attribute macro's
// generated `wasm_bindgen::…` paths resolve; the prelude glob brings the attribute macro itself.
use eframe::wasm_bindgen;
use eframe::wasm_bindgen::prelude::*;
use obc_app::Gesture;

/// One queued command from the page, drained in `SimGui::update`. Gestures are injected through the
/// app's deterministic [`apply_gesture`](obc_app::App::apply_gesture) seam; the playback + mode
/// commands drive the sim's GPX replay / tour baseline.
pub(crate) enum TourCmd {
    Gesture(Gesture),
    Play,
    Pause,
    Seek(f64),
    Enter,
    Exit,
    Ambient,
}

#[derive(Default)]
struct Bridge {
    queue: VecDeque<TourCmd>,
    screen: String,
}

thread_local! {
    static BRIDGE: RefCell<Bridge> = RefCell::new(Bridge::default());
}

/// Push a command from the page's demo engine. Vocabulary (exact strings): `press`, `back`, `hold`,
/// `backhold`, `turn:<n>` (signed detents), `play`, `pause`, `seek:<secs>`, `enter` (enter demo mode
/// + reset to the staged baseline), `exit` (hand control back where the demo left it), `ambient`
/// (reset to the clean live ride with the visitor's controls enabled — the first carousel page).
/// Unknown or malformed input is ignored — the page can't crash the sim with a typo.
#[wasm_bindgen]
pub fn obc_tour_cmd(cmd: &str) {
    let parsed = match cmd {
        "press" => Some(TourCmd::Gesture(Gesture::Press)),
        "back" => Some(TourCmd::Gesture(Gesture::Back)),
        "hold" => Some(TourCmd::Gesture(Gesture::Hold)),
        "backhold" => Some(TourCmd::Gesture(Gesture::BackHold)),
        "play" => Some(TourCmd::Play),
        "pause" => Some(TourCmd::Pause),
        "enter" => Some(TourCmd::Enter),
        "exit" => Some(TourCmd::Exit),
        "ambient" => Some(TourCmd::Ambient),
        other => {
            if let Some(n) = other.strip_prefix("turn:") {
                n.trim().parse::<i32>().ok().map(|n| TourCmd::Gesture(Gesture::Turn(n)))
            } else if let Some(t) = other.strip_prefix("seek:") {
                t.trim().parse::<f64>().ok().map(TourCmd::Seek)
            } else {
                None
            }
        }
    };
    if let Some(c) = parsed {
        BRIDGE.with(|b| b.borrow_mut().queue.push_back(c));
    }
}

/// The current (input-receiving) screen's variant name, e.g. `"Map"`, `"Menu"`, `"PoiMenu"`,
/// `"PoiList"`, `"PoiDetail"`, `"NavConfirm"`, `"NavPlanning"`, `"RouteOverview"`, `"NavFail"`,
/// `"Statistics"`, `"Climb"`. The page polls this to advance a demo step only once the sim reached
/// the target screen — no fixed sleeps, and it waits out the real planner (screen becomes
/// `RouteOverview` / `NavFail` when it finishes).
#[wasm_bindgen]
pub fn obc_tour_state() -> String {
    BRIDGE.with(|b| b.borrow().screen.clone())
}

/// Drain the queued commands (called once per frame by the eframe app).
pub(crate) fn drain_cmds() -> Vec<TourCmd> {
    BRIDGE.with(|b| b.borrow_mut().queue.drain(..).collect())
}

/// Publish the current screen name for the page to poll (called once per frame after the app ticks).
pub(crate) fn publish_screen(name: &str) {
    BRIDGE.with(|b| {
        let mut br = b.borrow_mut();
        if br.screen != name {
            br.screen.clear();
            br.screen.push_str(name);
        }
    });
}
