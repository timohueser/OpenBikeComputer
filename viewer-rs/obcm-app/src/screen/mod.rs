//! The screen system — the modular core of the on-device UI.
//!
//! This is the architecture the brief calls for: `no_std`, zero-alloc, **no
//! retained widget tree**. Screens are an [`Screen`] enum dispatched by `match`
//! (static dispatch, no `dyn`), each variant a small module with typed state.
//! Navigation is a *return value* — a screen's [`handle`](Screen::handle) returns
//! a [`Transition`] that [`apply`] runs against a [`heapless::Vec`] stack, so
//! overlays Push and `back`/Resume Pop back to their caller automatically and the
//! `back` that pops the top is the guaranteed escape.
//!
//! **Adding a screen is a local edit** (the modularity test): add a module with a
//! state struct + `handle`/`draw`, add one [`Screen`] variant + its three match
//! arms, and `Push` it from wherever it's reached. No central dispatch table, no
//! trait objects, no allocation.
//!
//! The shared context is split by role: [`Ctx`] is the *logic* half handed to
//! `handle` (the mutable camera/mode + clock), [`Render`] is the *draw* half (the
//! read-only state plus the `Reader`, the reusable `MapRenderer`, and the
//! in-flight hold-progress for the confirm ring).

use embedded_graphics::draw_target::DrawTarget;
use obcm_reader::Reader;
use obcm_render::{MapRenderer, RenderStats};

use crate::activity::Activity;
use crate::app::AppState;
use crate::input::Gesture;

mod home;
mod map;
mod menu;
mod ride_control;

pub use home::HomeScreen;
pub use map::MapScreen;
pub use menu::MenuScreen;
pub use ride_control::RideControl;

/// Maximum overlay depth (Home → Map → Ride control / Menu → …). Sized with
/// headroom; the real flow never nests more than a few deep. Growing it costs a
/// few enum-sized slots of static RAM, nothing per frame.
pub const MAX_DEPTH: usize = 8;

/// The screen stack: the bottom is the always-present root (Home), the top is the
/// screen currently receiving input.
pub type Stack = heapless::Vec<Screen, MAX_DEPTH>;

/// What a screen's [`handle`](Screen::handle) asks the navigation stack to do
/// next; [`apply`] runs it. This small closed set covers the whole spec's flow.
pub enum Transition {
    /// Stay on this screen (the gesture was handled in place, or is unbound).
    None,
    /// Open `screen` as the new top — a forward navigation or an overlay.
    Push(Screen),
    /// Return to the screen that opened this one — the `back` / Resume escape.
    Pop,
    /// Swap this screen for `screen` without growing the stack — sibling moves
    /// (Map ↔ Elevation) and "consume this screen" steps (Route menu → Map).
    Replace(Screen),
    /// Clear every overlay back to the Home root — Finish / Discard / power-down.
    Home,
}

/// Apply a [`Transition`] to the stack. The root is never popped, so `back`
/// always has a defined target and the stack can never empty.
pub fn apply(stack: &mut Stack, t: Transition) {
    match t {
        Transition::None => {}
        Transition::Push(s) => {
            let _ = stack.push(s); // MAX_DEPTH has headroom; an overflow just no-ops
        }
        Transition::Pop => {
            if stack.len() > 1 {
                stack.pop();
            }
        }
        Transition::Replace(s) => {
            if let Some(top) = stack.last_mut() {
                *top = s;
            }
        }
        Transition::Home => stack.truncate(1),
    }
}

/// Logic context handed to [`Screen::handle`]: the mutable app state a screen
/// adjusts (camera + mode) plus the millis clock. The render half is [`Render`].
pub struct Ctx<'a> {
    /// The camera / orientation / last-fix state (a screen may zoom, pan, …).
    pub state: &'a mut AppState,
    /// The ride mode + (later) tracking accumulators.
    pub activity: &'a mut Activity,
    /// Current millis clock.
    pub now_ms: u32,
}

/// Render context handed to [`Screen::draw`]: the read-only state plus the map
/// `Reader`, the reusable `MapRenderer`, and the in-flight encoder hold-progress
/// (0.0–1.0) the guarded-action confirm ring fills with.
pub struct Render<'a, 'd> {
    pub reader: &'a Reader<'d>,
    pub renderer: &'a mut MapRenderer,
    pub state: &'a AppState,
    pub activity: &'a Activity,
    pub w: f32,
    pub h: f32,
    pub now_ms: u32,
    pub hold_progress: f32,
}

/// The on-device screens. Each variant owns its typed state and forwards the
/// contract to that screen's inherent `handle`/`draw`. Adding a screen = one
/// variant + three arms here + its module.
pub enum Screen {
    Home(HomeScreen),
    Map(MapScreen),
    RideControl(RideControl),
    Menu(MenuScreen),
}

impl Screen {
    /// Handle one gesture, returning the navigation [`Transition`] it triggers.
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self {
            Screen::Home(s) => s.handle(g, cx),
            Screen::Map(s) => s.handle(g, cx),
            Screen::RideControl(s) => s.handle(g, cx),
            Screen::Menu(s) => s.handle(g, cx),
        }
    }

    /// Draw the screen. Returns the map [`RenderStats`] for the Map screen, and
    /// default stats for the others (so the host's stats panel keeps working).
    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        match self {
            Screen::Home(s) => s.draw(target, rx, color_fn),
            Screen::Map(s) => s.draw(target, rx, color_fn),
            Screen::RideControl(s) => s.draw(target, rx, color_fn),
            Screen::Menu(s) => s.draw(target, rx, color_fn),
        }
    }

    /// Whether this screen draws *over* the one below (the stack composites it on
    /// top) rather than replacing the view. Only Ride control is an overlay — it
    /// pauses on top of the still-visible map.
    pub fn is_overlay(&self) -> bool {
        matches!(self, Screen::RideControl(_))
    }
}

/// The "explorer's field map" palette in RGB565 (the format/style color space),
/// so screen text and chrome quantize through the host `color_fn` exactly like
/// map styles.
///
/// **Tuned to the 64-color (RGB222) gamut.** The panel only has 4 levels per
/// channel (0/85/170/255), so each value below is chosen for the *quantized*
/// result, with the device-64 color noted. (Earlier, true-color-picked values
/// clipped: parchment → white, the tan accents → pink.) The trailing comments are
/// the device-64 RGB each value lands on — keep them in sync if you retune.
pub mod palette {
    /// Pack 8-bit RGB into RGB565.
    pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
        (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
    }

    pub const PARCHMENT: u16 = rgb565(234, 223, 165); // → (255,255,170) warm cream
    pub const PARCHMENT_SHADE: u16 = rgb565(180, 170, 105); // → (170,170,85) tan
    pub const HUD: u16 = rgb565(46, 37, 26); // → (0,0,0) near-black frame
    pub const WOOD: u16 = rgb565(150, 100, 40); // → (170,85,0) wood brown
    /// Lighter wood for inset borders / frame lines.
    pub const WOOD_LIGHT: u16 = rgb565(180, 168, 100); // → (170,170,85) tan
    pub const INK: u16 = rgb565(44, 33, 20); // → (0,0,0) text black
    /// Muted ink for secondary / sub-label text.
    pub const SUBTEXT: u16 = rgb565(110, 90, 58); // → (85,85,0) olive
    /// Hairline rule between list rows.
    pub const RULE: u16 = rgb565(180, 170, 100); // → (170,170,85) tan
    pub const AMBER: u16 = rgb565(227, 165, 43); // → (255,170,0) accent
    pub const WARNING: u16 = rgb565(192, 73, 46); // → (255,85,0) warning
}
