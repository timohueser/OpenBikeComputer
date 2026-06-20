//! The simulator's **device housing** — the stylized body drawn around the screen
//! so the sim resembles the physical bikepacking computer instead of a bare
//! framebuffer floating in the window.
//!
//! This is pure host chrome: drawn with the egui painter *around* the screen
//! texture, never through the device framebuffer and never through the 64-color
//! device quantization (so its colors are independent of the map palette). Nothing
//! here touches `obc-render` / `obc-app`.
//!
//! **This is a placeholder industrial design, meant to be retuned.** Everything that
//! defines the look lives in one place: the geometry in [`HousingStyle`] (named,
//! commented constants in *screen-pixel units*, so the whole device scales with the
//! display scale) and the colors in [`Colorway`] / [`HousingPalette`]. Adjust those
//! — the drawing code derives everything from them and shouldn't need to change for
//! a reskin.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Rounding, Stroke, Vec2};

/// Charcoal behind the device, matching the reference render's backdrop.
pub fn background() -> Color32 {
    hex("#1e1e20")
}

/// Backdrop padding (screen-pixel units) left around the device when sizing the
/// window, so it floats in a little charcoal instead of touching the edges.
pub const WINDOW_MARGIN: f32 = 18.0;

/// The four body colors the device ships in. Selectable live in the control panel
/// (and via `--colorway`); the front wordmark always reads `OBM` regardless.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Colorway {
    Coral,
    Mint,
    Mustard,
    Slate,
}

impl Colorway {
    /// All four, in the reference render's order — drives the dropdown.
    pub const ALL: [Colorway; 4] =
        [Colorway::Coral, Colorway::Mint, Colorway::Mustard, Colorway::Slate];

    pub fn label(self) -> &'static str {
        match self {
            Colorway::Coral => "coral",
            Colorway::Mint => "mint",
            Colorway::Mustard => "mustard",
            Colorway::Slate => "slate",
        }
    }

    /// Parse a `--colorway` value (case-insensitive); `None` if unrecognized.
    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.label().eq_ignore_ascii_case(s.trim()))
    }

    /// The body color; the rest of the palette is derived from it + shared dark tones.
    fn body(self) -> Color32 {
        match self {
            Colorway::Coral => hex("#cb6750"),
            Colorway::Mint => hex("#62be9c"),
            Colorway::Mustard => hex("#e0b348"),
            Colorway::Slate => hex("#6c7891"),
        }
    }

    pub fn palette(self) -> HousingPalette {
        let body = self.body();
        HousingPalette {
            body,
            body_edge: darken(body, 0.72),
            wordmark: darken(body, 0.5),
            // Shared dark tones across all colorways (the bezel + side controls).
            bezel: hex("#141518"),
            button: hex("#36393f"),
            button_pressed: hex("#26282d"),
            knurl: hex("#50545c"),
        }
    }
}

/// Resolved colors for one colorway (see [`Colorway::palette`]).
pub struct HousingPalette {
    pub body: Color32,
    pub body_edge: Color32,
    pub wordmark: Color32,
    pub bezel: Color32,
    pub button: Color32,
    pub button_pressed: Color32,
    pub knurl: Color32,
}

/// Live state of the on-device controls, so the housing animates with the user's
/// input. Sourced from the emulated encoder/Back. (The long-press confirm feedback
/// lives in the control panel's knob ring, not here.)
#[derive(Clone, Copy, Default)]
pub struct ControlVisual {
    /// Encoder rotation (radians) — scrolls the scroll-wheel's knurling.
    pub knob_angle: f32,
    pub encoder_down: bool,
    pub back_down: bool,
}

/// All housing geometry, in **screen-pixel units** (relative to the 240×320 device
/// screen), so a single display `scale` scales the whole device while the screen
/// stays an exact multiple. Tweak these to reshape the housing — the draw code
/// derives every rect from them.
pub struct HousingStyle {
    /// Colored body padding around the screen (left/right, above, below). The bottom
    /// is roomier to seat the `OBM` wordmark.
    pub pad_x: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
    /// Outer body corner radius.
    pub body_radius: f32,
    /// Black bezel thickness around the screen, and its corner radius.
    pub bezel_gap: f32,
    pub bezel_radius: f32,
    /// Side controls: how far a pill sits inside the body, how far it protrudes past
    /// the body edge, the pill corner radius, and how far a pressed pill sinks in.
    pub btn_inset: f32,
    pub btn_protrude: f32,
    pub btn_radius: f32,
    pub btn_press: f32,
    /// Encoder + Back pill heights and their vertical centers (fraction of body height).
    pub enc_h: f32,
    pub enc_cy: f32,
    pub back_h: f32,
    pub back_cy: f32,
    /// Scroll-wheel knurl ridge spacing and how fast it scrolls per radian of turn.
    pub knurl_spacing: f32,
    pub knurl_gain: f32,
    /// `OBM` wordmark font size.
    pub wordmark_size: f32,
}

impl Default for HousingStyle {
    fn default() -> Self {
        HousingStyle {
            pad_x: 30.0,
            pad_top: 30.0,
            pad_bottom: 74.0,
            body_radius: 40.0,
            bezel_gap: 16.0,
            bezel_radius: 26.0,
            btn_inset: 6.0,
            btn_protrude: 13.0,
            btn_radius: 8.0,
            btn_press: 3.0,
            enc_h: 78.0,
            enc_cy: 0.34,
            back_h: 46.0,
            back_cy: 0.58,
            knurl_spacing: 7.0,
            knurl_gain: 18.0,
            wordmark_size: 24.0,
        }
    }
}

/// The resolved on-screen rects for a placed device. The caller hit-tests `encoder`
/// / `back` for the clickable controls and blits the framebuffer into `screen`.
pub struct Layout {
    pub body: Rect,
    pub bezel: Rect,
    pub screen: Rect,
    pub encoder: Rect,
    pub back: Rect,
    pub wordmark_center: Pos2,
    pub wordmark_size: f32,
    /// Points per device pixel (carried so [`draw`] needn't re-take it).
    pub scale: f32,
}

impl HousingStyle {
    /// Full device footprint (incl. the protruding side buttons) in screen-pixel
    /// units, given the screen's pixel size.
    pub fn device_size_px(&self, screen: Vec2) -> Vec2 {
        let body_w = screen.x + 2.0 * self.pad_x;
        let body_h = screen.y + self.pad_top + self.pad_bottom;
        Vec2::new(body_w + self.btn_protrude, body_h)
    }

    /// Window footprint: the device plus the backdrop [`WINDOW_MARGIN`] on every side,
    /// so the device floats in a little charcoal.
    pub fn window_size_px(&self, screen: Vec2) -> Vec2 {
        self.device_size_px(screen) + Vec2::splat(2.0 * WINDOW_MARGIN)
    }

    /// Screen corner radius (points) — follows the bezel's *inner* radius so the
    /// rounded display corners track the black insert. The caller rounds the screen
    /// texture by this (its corners then reveal the bezel behind).
    pub fn screen_radius_pts(&self, scale: f32) -> f32 {
        (self.bezel_radius - self.bezel_gap).max(0.0) * scale
    }

    /// Resolve every rect for the device centered in `available`, at `scale` points
    /// per screen-pixel.
    pub fn layout(&self, available: Rect, scale: f32, screen: Vec2) -> Layout {
        let s = scale;
        // Center the device, snapped to whole points so an integer scale stays crisp.
        let origin = (available.center() - self.device_size_px(screen) * s / 2.0).round();
        let body_w = (screen.x + 2.0 * self.pad_x) * s;
        let body_h = (screen.y + self.pad_top + self.pad_bottom) * s;
        let body = Rect::from_min_size(origin, Vec2::new(body_w, body_h));

        let screen_rect = Rect::from_min_size(
            origin + Vec2::new(self.pad_x * s, self.pad_top * s),
            Vec2::new(screen.x * s, screen.y * s),
        );
        let bezel = screen_rect.expand(self.bezel_gap * s);

        // Side pills, hung on the body's right edge.
        let bx0 = body.right() - self.btn_inset * s;
        let bx1 = body.right() + self.btn_protrude * s;
        let pill = |cy_frac: f32, h: f32| {
            let cy = body.top() + cy_frac * body_h;
            Rect::from_min_max(Pos2::new(bx0, cy - h * s / 2.0), Pos2::new(bx1, cy + h * s / 2.0))
        };
        let encoder = pill(self.enc_cy, self.enc_h);
        let back = pill(self.back_cy, self.back_h);

        let wordmark_center =
            Pos2::new(body.center().x, screen_rect.bottom() + self.pad_bottom * s / 2.0);

        Layout {
            body,
            bezel,
            screen: screen_rect,
            encoder,
            back,
            wordmark_center,
            wordmark_size: self.wordmark_size * s,
            scale: s,
        }
    }
}

/// Paint the housing into the precomputed [`Layout`] (from [`HousingStyle::layout`]).
/// The caller then blits the framebuffer into `lo.screen`, over the bezel.
pub fn draw(
    painter: &egui::Painter,
    lo: &Layout,
    style: &HousingStyle,
    palette: &HousingPalette,
    ctrl: &ControlVisual,
) {
    let scale = lo.scale;

    // Body — colored rounded slab with a subtle darker rim against the backdrop.
    let body_round = Rounding::same(style.body_radius * scale);
    painter.rect_filled(lo.body, body_round, palette.body);
    painter.rect_stroke(
        lo.body,
        body_round,
        Stroke::new((1.5 * scale).max(1.0), palette.body_edge),
    );

    // Side controls (drawn before the bezel; they don't overlap it). The encoder
    // carries knurling (`Some(angle)`); Back is a plain button (`None`).
    draw_pill(painter, lo.encoder, scale, style, palette, ctrl.encoder_down, Some(ctrl.knob_angle));
    draw_pill(painter, lo.back, scale, style, palette, ctrl.back_down, None);

    // Bezel — the dark frame the (corner-rounded) screen texture is blitted over.
    painter.rect_filled(lo.bezel, Rounding::same(style.bezel_radius * scale), palette.bezel);

    // Front wordmark — always "OBM".
    painter.text(
        lo.wordmark_center,
        Align2::CENTER_CENTER,
        "OBM",
        FontId::new(lo.wordmark_size, egui::FontFamily::Proportional),
        palette.wordmark,
    );
}

/// A side pill (encoder or Back): sinks in and darkens when pressed. `knurl` is the
/// encoder's rotation (radians) — `Some` draws knurl ridges that scroll with the turn
/// (a side thumb-wheel look); `None` is a plain button.
fn draw_pill(
    painter: &egui::Painter,
    rect: Rect,
    scale: f32,
    style: &HousingStyle,
    palette: &HousingPalette,
    pressed: bool,
    knurl: Option<f32>,
) {
    let r = if pressed { rect.translate(Vec2::new(-style.btn_press * scale, 0.0)) } else { rect };
    let fill = if pressed { palette.button_pressed } else { palette.button };
    painter.rect_filled(r, Rounding::same(style.btn_radius * scale), fill);

    let Some(knob_angle) = knurl else {
        return;
    };
    // Knurl: horizontal ridges that scroll with rotation.
    let spacing = style.knurl_spacing * scale;
    let inset = style.btn_radius * scale; // keep ridges clear of the rounded ends
    let (top, bot) = (r.top() + inset, r.bottom() - inset);
    if bot > top && spacing > 0.5 {
        let offset = (knob_angle * style.knurl_gain * scale).rem_euclid(spacing);
        let stroke = Stroke::new((1.0 * scale).max(1.0), palette.knurl);
        let (x0, x1) = (r.left() + 2.0 * scale, r.right() - 2.0 * scale);
        let mut y = top + offset - spacing; // start one above so the scroll wraps smoothly
        while y <= bot {
            if y >= top {
                painter.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], stroke);
            }
            y += spacing;
        }
    }
}

/// A `#rrggbb` literal → `Color32`, so the palette reads as hex codes VSCode's color
/// picker can edit in place. Panics on a malformed literal (they're all constants above).
fn hex(s: &str) -> Color32 {
    Color32::from_hex(s).expect("valid #rrggbb literal")
}

/// Scale each channel toward black by `f` (0 = black, 1 = unchanged).
fn darken(c: Color32, f: f32) -> Color32 {
    let ch = |x: u8| (x as f32 * f) as u8;
    Color32::from_rgb(ch(c.r()), ch(c.g()), ch(c.b()))
}
