//! The simulator's device housing: the stylized body drawn around the screen.
//!
//! Host chrome only, drawn with the egui painter around the screen texture, never through the
//! device framebuffer or the 64-colour quantization, so its colours are independent of the map
//! palette.
//!
//! The look lives in the geometry ([`HousingStyle`], in screen-pixel units so it scales with the
//! display scale) and the colours ([`Colorway`] and [`HousingPalette`]). The drawing code derives
//! everything from them, so a reskin only touches those.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Rounding, Stroke, Vec2};

/// Charcoal behind the device, matching the reference render's backdrop.
pub fn background() -> Color32 {
    hex("#1e1e20")
}

/// Backdrop padding in screen-pixel units, left around the device when sizing the window, so it
/// floats in a little charcoal instead of touching the edges.
pub const WINDOW_MARGIN: f32 = 18.0;

/// The body colors the device ships in, selectable in the control panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Colorway {
    Petrol,
    Forest,
    Wine,
    Aubergine,
    Stealth,
}

impl Colorway {
    /// Drives the dropdown, in the colorway sheet's order. Forest is the default.
    pub const ALL: [Colorway; 5] =
        [Colorway::Petrol, Colorway::Forest, Colorway::Wine, Colorway::Aubergine, Colorway::Stealth];

    pub fn label(self) -> &'static str {
        match self {
            Colorway::Petrol => "petrol",
            Colorway::Forest => "forest",
            Colorway::Wine => "wine",
            Colorway::Aubergine => "aubergine",
            Colorway::Stealth => "stealth",
        }
    }

    /// The upper-shell colour. The rest of the palette derives from it and the shared dark
    /// tones.
    fn body(self) -> Color32 {
        match self {
            Colorway::Petrol => hex("#29465c"),
            Colorway::Forest => hex("#3d736e"),
            Colorway::Wine => hex("#693744"),
            Colorway::Aubergine => hex("#4d3c77"),
            Colorway::Stealth => hex("#1f252c"),
        }
    }

    /// The accent base the upper shell sits on: the lighter half of the two-tone body.
    fn accent(self) -> Color32 {
        match self {
            Colorway::Petrol => hex("#3ad3e2"),
            Colorway::Forest => hex("#8be3bc"),
            Colorway::Wine => hex("#ff8b77"),
            Colorway::Aubergine => hex("#d9d0f5"),
            Colorway::Stealth => hex("#465059"),
        }
    }

    pub fn palette(self) -> HousingPalette {
        let body = self.body();
        HousingPalette {
            body,
            body_edge: darken(body, 0.72),
            accent: self.accent(),
            wordmark: darken(body, 0.62),
            // Shared dark tones across all colorways: the bezel and the four rubber buttons.
            bezel: hex("#141518"),
            button: hex("#36393f"),
            button_pressed: hex("#26282d"),
            button_texture: hex("#22242a"),
        }
    }
}

/// Resolved colors for one colorway (see [`Colorway::palette`]).
pub struct HousingPalette {
    pub body: Color32,
    pub body_edge: Color32,
    /// The lighter base shell the body sits on, which shows as the two-tone lip.
    pub accent: Color32,
    pub wordmark: Color32,
    pub bezel: Color32,
    pub button: Color32,
    pub button_pressed: Color32,
    /// The fine grid moulded into each rubber button pad.
    pub button_texture: Color32,
}

/// Live state of the on-device controls, so the housing animates with the user's input.
#[derive(Clone, Copy, Default)]
pub struct ControlVisual {
    pub up_down: bool,
    pub down_down: bool,
    pub select_down: bool,
    pub back_down: bool,
}

/// Which of the four buttons a rect belongs to, which picks the flank a press sinks toward.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flank {
    Left,
    Right,
}

/// All housing geometry, in screen-pixel units, so one display `scale` scales the whole device
/// while the screen stays an exact multiple.
pub struct HousingStyle {
    /// Coloured body padding around the screen. The bottom is roomier, because the device wears a
    /// tall chin under the bezel.
    pub pad_x: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
    /// Outer body corner radius.
    pub body_radius: f32,
    /// How far the lighter accent base peeks out past the upper shell: an even lip on all four
    /// sides, so the two-tone edge reads as a concentric rim and not a dropped shadow.
    pub accent_lip: f32,
    /// Black bezel thickness around the screen, and its corner radius.
    pub bezel_gap: f32,
    pub bezel_radius: f32,
    /// The four buttons: how far a pad sits inside the body, how far it protrudes past the body
    /// edge, its corner radius, and how far a pressed pad sinks in.
    pub btn_inset: f32,
    pub btn_protrude: f32,
    pub btn_radius: f32,
    pub btn_press: f32,
    /// Button pad height and the gap between the two pads of a flank. The pair is centred on the
    /// body's vertical midpoint, and both flanks share it, so the two upper pads sit level.
    pub btn_h: f32,
    pub btn_gap: f32,
    /// Spacing of the grid moulded into each rubber pad.
    pub btn_grid: f32,
    /// `OBC` wordmark font size.
    pub wordmark_size: f32,
}

impl Default for HousingStyle {
    fn default() -> Self {
        // Proportioned off the reference render: the bezel reaches to within about 6 % of each
        // side, and the chin is about 22 % of the height.
        HousingStyle {
            pad_x: 34.0,
            pad_top: 32.0,
            pad_bottom: 118.0,
            body_radius: 42.0,
            accent_lip: 6.0,
            bezel_gap: 16.0,
            bezel_radius: 26.0,
            btn_inset: 6.0,
            btn_protrude: 13.0,
            btn_radius: 7.0,
            btn_press: 3.0,
            btn_h: 66.0,
            btn_gap: 22.0,
            btn_grid: 7.0,
            wordmark_size: 26.0,
        }
    }
}

/// The resolved on-screen rects for a placed device. The caller hit-tests the four button rects and
/// blits the framebuffer into `screen`.
pub struct Layout {
    pub body: Rect,
    /// The accent base peeking out behind `body`.
    pub base: Rect,
    pub bezel: Rect,
    pub screen: Rect,
    pub up: Rect,
    pub down: Rect,
    pub select: Rect,
    pub back: Rect,
    pub wordmark_center: Pos2,
    pub wordmark_size: f32,
    /// Points per device pixel, carried so [`draw`] does not re-take it.
    pub scale: f32,
}

impl HousingStyle {
    /// Full device footprint, including the protruding side buttons on both flanks, in
    /// screen-pixel units.
    pub fn device_size_px(&self, screen: Vec2) -> Vec2 {
        let body_w = screen.x + 2.0 * self.pad_x;
        let body_h = screen.y + self.pad_top + self.pad_bottom;
        // The pads hang off both flanks, and the accent lip rings the whole body.
        Vec2::new(body_w + 2.0 * (self.btn_protrude + self.accent_lip), body_h + 2.0 * self.accent_lip)
    }

    /// Window footprint: the device plus [`WINDOW_MARGIN`] on every side.
    pub fn window_size_px(&self, screen: Vec2) -> Vec2 {
        self.device_size_px(screen) + Vec2::splat(2.0 * WINDOW_MARGIN)
    }

    /// Screen corner radius in points. It follows the bezel's inner radius, so the rounded display
    /// corners track the black insert and reveal the bezel behind.
    pub fn screen_radius_pts(&self, scale: f32) -> f32 {
        (self.bezel_radius - self.bezel_gap).max(0.0) * scale
    }

    /// Resolve every rect for the device centred in `available`, at `scale` points per
    /// screen-pixel.
    pub fn layout(&self, available: Rect, scale: f32, screen: Vec2) -> Layout {
        let s = scale;
        // Centre the device, snapped to whole points so an integer scale stays crisp.
        let origin = (available.center() - self.device_size_px(screen) * s / 2.0).round();
        let body_w = (screen.x + 2.0 * self.pad_x) * s;
        let body_h = (screen.y + self.pad_top + self.pad_bottom) * s;
        // The body is inset by the button protrusion and the lip, so both have room.
        let inset = (self.btn_protrude + self.accent_lip) * s;
        let body = Rect::from_min_size(origin + Vec2::new(inset, self.accent_lip * s), Vec2::new(body_w, body_h));
        // The accent base is the same slab grown evenly on all four sides, so the two-tone edge is
        // symmetric rather than reading as a dropped shadow.
        let base = body.expand(self.accent_lip * s);

        let screen_rect = Rect::from_min_size(
            body.min + Vec2::new(self.pad_x * s, self.pad_top * s),
            Vec2::new(screen.x * s, screen.y * s),
        );
        let bezel = screen_rect.expand(self.bezel_gap * s);

        // Button pads, hung on both body edges. The pair straddles the body's vertical midpoint,
        // so the two pads sit `btn_gap` apart around the centre line.
        let half_pitch = (self.btn_h + self.btn_gap) / 2.0 * s;
        let pad = |flank: Flank, upper: bool| {
            let cy = body.center().y + if upper { -half_pitch } else { half_pitch };
            let (x0, x1) = match flank {
                Flank::Left => (body.left() - self.btn_protrude * s, body.left() + self.btn_inset * s),
                Flank::Right => (body.right() - self.btn_inset * s, body.right() + self.btn_protrude * s),
            };
            Rect::from_min_max(Pos2::new(x0, cy - self.btn_h * s / 2.0), Pos2::new(x1, cy + self.btn_h * s / 2.0))
        };

        let wordmark_center = Pos2::new(body.center().x, screen_rect.bottom() + self.pad_bottom * s / 2.0);

        Layout {
            body,
            base,
            bezel,
            screen: screen_rect,
            up: pad(Flank::Left, true),
            down: pad(Flank::Left, false),
            select: pad(Flank::Right, true),
            back: pad(Flank::Right, false),
            wordmark_center,
            wordmark_size: self.wordmark_size * s,
            scale: s,
        }
    }
}

/// Paint the housing into the precomputed [`Layout`]. The caller then blits the framebuffer into
/// `lo.screen`, over the bezel.
pub fn draw(
    painter: &egui::Painter,
    lo: &Layout,
    style: &HousingStyle,
    palette: &HousingPalette,
    ctrl: &ControlVisual,
) {
    let scale = lo.scale;

    // The lighter base shell first: the upper body covers all but its lip.
    painter.rect_filled(lo.base, Rounding::same((style.body_radius + style.accent_lip) * scale), palette.accent);

    // The body: a coloured rounded slab with a darker rim against the backdrop.
    let body_round = Rounding::same(style.body_radius * scale);
    painter.rect_filled(lo.body, body_round, palette.body);
    painter.rect_stroke(lo.body, body_round, Stroke::new((1.5 * scale).max(1.0), palette.body_edge));

    // The four buttons, drawn before the bezel because they do not overlap it: UP and DOWN on the
    // left flank, SELECT and BACK on the right.
    for (rect, flank, pressed) in [
        (lo.up, Flank::Left, ctrl.up_down),
        (lo.down, Flank::Left, ctrl.down_down),
        (lo.select, Flank::Right, ctrl.select_down),
        (lo.back, Flank::Right, ctrl.back_down),
    ] {
        draw_pad(painter, rect, flank, scale, style, palette, pressed);
    }

    // The bezel: the dark frame the corner-rounded screen texture is blitted over.
    painter.rect_filled(lo.bezel, Rounding::same(style.bezel_radius * scale), palette.bezel);

    painter.text(
        lo.wordmark_center,
        Align2::CENTER_CENTER,
        "OBC",
        FontId::new(lo.wordmark_size, egui::FontFamily::Proportional),
        palette.wordmark,
    );
}

/// One rubber button pad: a rounded slab with a fine moulded grid, which sinks toward its flank and
/// darkens when pressed.
fn draw_pad(
    painter: &egui::Painter,
    rect: Rect,
    flank: Flank,
    scale: f32,
    style: &HousingStyle,
    palette: &HousingPalette,
    pressed: bool,
) {
    // A press sinks the pad into the body, so the direction flips with the flank.
    let sink = if pressed { style.btn_press * scale } else { 0.0 };
    let dx = match flank {
        Flank::Left => sink,
        Flank::Right => -sink,
    };
    let r = rect.translate(Vec2::new(dx, 0.0));
    let fill = if pressed { palette.button_pressed } else { palette.button };
    painter.rect_filled(r, Rounding::same(style.btn_radius * scale), fill);

    // A fine cross-hatch inset from the rounded corners, like textured rubber.
    let spacing = style.btn_grid * scale;
    let inset = style.btn_radius * scale;
    let inner = Rect::from_min_max(
        Pos2::new(r.left() + inset * 0.5, r.top() + inset),
        Pos2::new(r.right() - inset * 0.5, r.bottom() - inset),
    );
    if spacing < 0.5 || inner.width() <= 0.0 || inner.height() <= 0.0 {
        return;
    }
    let stroke = Stroke::new((1.0 * scale).max(1.0), palette.button_texture);
    let mut y = inner.top();
    while y <= inner.bottom() {
        painter.line_segment([Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)], stroke);
        y += spacing;
    }
    let mut x = inner.left();
    while x <= inner.right() {
        painter.line_segment([Pos2::new(x, inner.top()), Pos2::new(x, inner.bottom())], stroke);
        x += spacing;
    }
}

/// A `#rrggbb` literal as a `Color32`. It panics on a malformed literal, which can only be a
/// constant above.
fn hex(s: &str) -> Color32 {
    Color32::from_hex(s).expect("valid #rrggbb literal")
}

/// Scale each channel toward black by `f`, where 0 is black and 1 is unchanged.
fn darken(c: Color32, f: f32) -> Color32 {
    let ch = |x: u8| (x as f32 * f) as u8;
    Color32::from_rgb(ch(c.r()), ch(c.g()), ch(c.b()))
}
