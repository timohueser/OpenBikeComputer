//! The "Controls" window — the second OS window (an egui immediate viewport) that
//! drives the simulated device: the emulated encoder + Back, the manual GPS fix
//! (position / heading / zoom / camera / orientation), GPX replay, and the render-
//! stats readout. Split out of [`super`]'s host loop so the panel UI lives apart
//! from the framebuffer/texture plumbing; it is a second `impl SimGui` block, so it
//! reads and mutates the same fields directly.

use eframe::egui;
use obc_app::{Button, CameraMode, InputClock};

use crate::calib;
use super::housing::Colorway;
use super::units::{
    format_clock, format_distance, mpp_to_zoom, zoom_to_mpp, MAX_ZOOM, MIN_ZOOM, MPP_MAX, MPP_MIN,
};
use super::SimGui;

// Control-panel knob colors (host chrome — picked for the egui panel, not the
// device screen, so they need not pass through the 64-color quantization).
const KNOB_FILL: egui::Color32 = egui::Color32::from_rgb(70, 60, 48);
const KNOB_EDGE: egui::Color32 = egui::Color32::from_rgb(150, 120, 80);
const NOTCH: egui::Color32 = egui::Color32::from_rgb(234, 223, 192);
const AMBER: egui::Color32 = egui::Color32::from_rgb(227, 165, 43);

/// Points along a clockwise arc from 12 o'clock, sweeping `progress` (0–1) of a
/// full turn at radius `r` — the encoder hold-progress ring around the knob.
fn arc_points(center: egui::Pos2, r: f32, progress: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let sweep = progress.clamp(0.0, 1.0) * TAU;
    let n = ((sweep / 0.25).ceil() as usize).max(2);
    (0..=n)
        .map(|i| {
            let a = -FRAC_PI_2 + sweep * (i as f32 / n as f32);
            center + egui::Vec2::angled(a) * r
        })
        .collect()
}

impl SimGui {
    /// Draw the "Controls" window — a second OS window (egui immediate viewport)
    /// that drives the simulated GPS fix. Re-declared every frame; the widgets
    /// edit the panel mirrors / `AppState`, then we push the mirrors into the
    /// [`SimLocationSource`](crate::sim_location::SimLocationSource) so the next
    /// `App::tick` picks them up.
    pub(super) fn show_control_panel(&mut self, ctx: &egui::Context) {
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("controls"),
            egui::ViewportBuilder::default()
                .with_title("Controls")
                .with_inner_size([360.0, 770.0]),
            |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // The device's own controls (encoder + Back).
                    self.show_device_controls(ui);
                    ui.add_space(6.0);

                    // Housing body color — purely cosmetic chrome (the four colorways).
                    ui.horizontal(|ui| {
                        ui.label("Device color");
                        egui::ComboBox::from_id_salt("colorway")
                            .selected_text(self.colorway.label())
                            .show_ui(ui, |ui| {
                                for c in Colorway::ALL {
                                    ui.selectable_value(&mut self.colorway, c, c.label());
                                }
                            });
                    });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // Let sliders span the panel width instead of egui's narrow
                    // default — leaves room for the value box on the right.
                    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(140.0);

                    // While a GPX track is loaded it owns the fix (like the device's
                    // GPS would), so the manual position/heading inputs are shown
                    // read-only and just track the replay. Camera/zoom/orientation
                    // stay live.
                    let replaying = self.gpx.is_some();

                    // Position — the GPS fix, edited in degrees (stored as µdeg).
                    // Lat and Lon share one row to spend width, not height.
                    ui.add_enabled_ui(!replaying, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Lat");
                            ui.add(
                                egui::DragValue::new(&mut self.panel.lat_deg)
                                    .speed(1e-4)
                                    .range(-90.0..=90.0)
                                    .max_decimals(6)
                                    .suffix("°"),
                            );
                            ui.add_space(12.0);
                            ui.label("Lon");
                            ui.add(
                                egui::DragValue::new(&mut self.panel.lon_deg)
                                    .speed(1e-4)
                                    .range(-180.0..=180.0)
                                    .max_decimals(6)
                                    .suffix("°"),
                            );
                        });
                    });

                    ui.add_space(6.0);
                    ui.separator();

                    // Heading — rides on Fix.course (degrees CW from north).
                    ui.add_enabled_ui(!replaying, |ui| {
                        ui.label("Heading");
                        ui.add(
                            egui::Slider::new(&mut self.panel.heading_deg, 0.0..=360.0)
                                .suffix("°")
                                .step_by(1.0),
                        );
                    });

                    ui.add_space(6.0);
                    ui.separator();

                    // Zoom — operated in meters-per-pixel on a log scale. Only
                    // write back when the user drags it, so it never fights the
                    // mouse scroll (which can range past the slider's bounds).
                    ui.label("Zoom");
                    let mut mpp = zoom_to_mpp(self.app.state.zoom);
                    let resp = ui.add(
                        egui::Slider::new(&mut mpp, MPP_MIN..=MPP_MAX)
                            .logarithmic(true)
                            .custom_formatter(|n, _| {
                                let v = if n < 1.0 {
                                    format!("{n:.3}")
                                } else if n < 100.0 {
                                    format!("{n:.1}")
                                } else {
                                    format!("{n:.0}")
                                };
                                format!("{v} m/px")
                            }),
                    );
                    if resp.changed() {
                        self.app.state.zoom = mpp_to_zoom(mpp).clamp(MIN_ZOOM, MAX_ZOOM);
                    }
                    let span = zoom_to_mpp(self.app.state.zoom) * self.dev_w as f32;
                    ui.label(format!("{} across screen", format_distance(span)));

                    ui.add_space(6.0);
                    ui.separator();

                    // Camera mode and Orientation — paired on one row (each a label
                    // over its toggle) to spend width instead of height. Orientation
                    // (north-up vs heading-up, rotating the map so Heading points to the
                    // top) is independent of the camera mode.
                    let prev_mode = self.app.state.mode;
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label("Camera");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut self.app.state.mode, CameraMode::Follow, "Follow");
                                ui.selectable_value(&mut self.app.state.mode, CameraMode::Free, "Free");
                            });
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.label("Orientation");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut self.app.state.heading_up, false, "North-up");
                                ui.selectable_value(&mut self.app.state.heading_up, true, "Heading-up");
                            });
                        });
                    });
                    // Entering Follow: snap the fix onto the current camera center so the
                    // view doesn't jump (in Free the mouse moved the camera away from the
                    // fix) and the panel reads the followed point.
                    if prev_mode == CameraMode::Free && self.app.state.mode == CameraMode::Follow {
                        self.panel.lat_deg = self.app.state.cam_lat as f64 / 1e6;
                        self.panel.lon_deg = self.app.state.cam_lon as f64 / 1e6;
                    }

                    ui.add_space(6.0);
                    ui.separator();

                    // GPX replay — load a recorded track and play it back as a
                    // simulated GPS sensor (position + derived course/speed). The
                    // player is the active `LocationSource` while a track is loaded.
                    ui.label("GPX replay");
                    if ui.button("Load GPX…").clicked() {
                        if let Some(path) =
                            rfd::FileDialog::new().add_filter("GPX track", &["gpx"]).pick_file()
                        {
                            self.load_gpx(&path);
                        }
                    }
                    self.show_gpx_controls(ui);

                    ui.add_space(6.0);
                    ui.separator();

                    // Display size — the 1:1 "actual size" toggle + ruler calibration.
                    self.show_display_controls(ui);

                    ui.add_space(6.0);
                    ui.separator();

                    // Expanded by default — the render stats are worth keeping an eye on.
                    egui::CollapsingHeader::new("Render Stats")
                        .default_open(true)
                        .show(ui, |ui| self.show_render_stats(ui));

                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.quit = true;
                    }
                });
            },
        );

        // Push the mirrors into the location source (the app reads them next tick).
        self.loc.set_position(
            (self.panel.lat_deg * 1e6).round() as i32,
            (self.panel.lon_deg * 1e6).round() as i32,
        );
        self.loc.set_course(self.panel.heading_deg);
    }

    /// The device's own controls — a rotary **encoder** (turn + push) and a
    /// **Back** button — emulated to resemble the real interface. Turn the knob by
    /// scrolling over it or dragging around it; PUSH / BACK are press-and-hold
    /// (held past the threshold they become `Hold` / `Back-hold`); the keyboard
    /// mirrors all of it. Raw events feed [`App::handle_input`](obc_app::App::handle_input),
    /// which runs the shared recognizer and drives the screen stack — so the encoder
    /// actually zooms the map, pauses into Ride control, etc. Encoder-hold progress
    /// shows as the amber confirm ring drawn around the knob.
    fn show_device_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Device — encoder + Back").strong());
        ui.add_space(4.0);

        // Read back before the closure borrows `self` for the knob interaction.
        let knob_angle = self.input.knob_angle();
        let enc_progress = self.app.encoder_hold_progress();
        const SZ: f32 = 96.0;
        const BTN_W: f32 = 110.0;
        const BTN_H: f32 = 34.0;

        // Knob on the left, PUSH / BACK stacked to its right — spend the panel's
        // width rather than its height.
        let (push_resp, back_resp) = ui
            .horizontal(|ui| {
                // Knob: a round encoder with a pointer notch + hold-progress ring.
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(SZ, SZ), egui::Sense::drag());
                let painter = ui.painter_at(rect);
                let center = rect.center();
                let radius = SZ * 0.42;
                painter.circle_filled(center, radius, KNOB_FILL);
                painter.circle_stroke(center, radius, egui::Stroke::new(2.0, KNOB_EDGE));
                // Pointer notch shows the knob's rotation.
                let notch = center + egui::Vec2::angled(knob_angle) * (radius - 7.0);
                painter.line_segment([center, notch], egui::Stroke::new(3.0, NOTCH));
                painter.circle_filled(notch, 4.0, NOTCH);
                // Encoder hold-progress arc — the guarded-action confirm ring.
                if enc_progress > 0.0 {
                    painter.add(egui::Shape::line(
                        arc_points(center, radius + 6.0, enc_progress),
                        egui::Stroke::new(4.0, AMBER),
                    ));
                }
                if resp.dragged() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.input.drag_to((p - resp.rect.center()).angle());
                    }
                }
                if resp.drag_stopped() {
                    self.input.end_drag();
                }
                if resp.hovered() {
                    let dy = ui.input(|i| i.smooth_scroll_delta.y);
                    if dy != 0.0 {
                        self.input.scroll(dy);
                    }
                }

                ui.add_space(16.0);

                // PUSH (encoder) / BACK — press-and-hold, stacked and vertically
                // centered beside the knob. ---
                ui.vertical(|ui| {
                    ui.add_space((SZ - 2.0 * BTN_H - 8.0) / 2.0);
                    let p = ui.add_sized([BTN_W, BTN_H], egui::Button::new("PUSH"));
                    ui.add_space(8.0);
                    let b = ui.add_sized([BTN_W, BTN_H], egui::Button::new("BACK"));
                    (p, b)
                })
                .inner
            })
            .inner;

        // Keyboard mirror: ←/→ (or [ ] / , .) turn; Enter push; Backspace back.
        // Read globally at the top of `update` (see [`kbd_turn`](SimGui::kbd_turn)) so it
        // works regardless of which widget has focus; here we just merge it with the
        // on-screen PUSH/BACK buttons' pointer state.
        self.input.turn(self.kbd_turn);
        self.input.set_button(Button::Encoder, push_resp.is_pointer_button_down_on() || self.kbd_enc);
        self.input.set_button(Button::Back, back_resp.is_pointer_button_down_on() || self.kbd_back);

        // Run this frame's raw events through the shared recognizer + screen stack
        // (the exact path the firmware uses), firing long-press at its threshold.
        let now = self.input.now_ms();
        self.app.handle_input(InputClock(now), &mut self.input);

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("keys: ←/→ turn · Enter push · Backspace back  (hold for long-press)")
                .weak()
                .size(11.0),
        );
    }

    /// Display size — the 1:1 "actual size" toggle (needs a calibration) plus a button
    /// to (re)calibrate. The toggle and the calibration screen live in
    /// [`super::SimGui`] ([`show_calibration`](super::SimGui::show_calibration)).
    fn show_display_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Display size").strong());
        let calibrated = self.points_per_mm.is_some();
        ui.horizontal(|ui| {
            let resp = ui.add_enabled(calibrated, egui::Checkbox::new(&mut self.physical, "Actual size (1:1)"));
            if resp.changed() {
                // Resize the window either way: to 1:1 on, back to the --scale default off.
                self.physical_resize_pending = true;
            }
            if ui.add_enabled(self.calib.is_none(), egui::Button::new("Calibrate…")).clicked() {
                self.calib = Some(super::CalibState::default());
            }
        });
        match self.points_per_mm {
            Some(ppm) => {
                ui.weak(format!(
                    "calibrated {ppm:.2} pt/mm · panel {:.1} × {:.1} mm",
                    calib::PANEL_W_MM, calib::PANEL_H_MM
                ));
            }
            None => {
                ui.weak("not calibrated — click Calibrate… to set 1:1 size");
            }
        }
        if let Some(e) = &self.calib_error {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), e);
        }
    }

    /// The loaded-track controls: play/pause (auto-follows), a seek scrubber, and
    /// a 1×–10× speed slider. Shows the load error (or nothing) when no track is
    /// loaded. Split out of [`show_control_panel`](Self::show_control_panel) so the
    /// "eject" mutation of `self.gpx` doesn't tangle with the active `&mut` borrow
    /// of the player.
    fn show_gpx_controls(&mut self, ui: &mut egui::Ui) {
        let Some(player) = self.gpx.as_mut() else {
            if let Some(err) = &self.gpx_error {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
            }
            return;
        };

        if let Some(label) = &self.gpx_label {
            ui.label(label);
        }

        let dur = player.duration();
        let mut eject = false;

        ui.horizontal(|ui| {
            let play_label = if player.is_playing() { "⏸ Pause" } else { "▶ Play" };
            if ui.button(play_label).clicked() {
                player.toggle();
                // Pressing play tracks the moving fix; the user can still switch
                // back to Free to pan around mid-playback.
                if player.is_playing() {
                    self.app.state.mode = CameraMode::Follow;
                }
            }
            if ui.button("⏏ Eject").clicked() {
                eject = true;
            }
        });

        if dur > 0.0 {
            // Scrubber — seek anywhere in the track, playing or paused.
            let mut t = player.time();
            let resp =
                ui.add(egui::Slider::new(&mut t, 0.0..=dur).show_value(false).text("seek"));
            if resp.changed() {
                player.seek(t);
            }
            ui.label(format!("{} / {}", format_clock(player.time()), format_clock(dur)));

            // Playback speed — real time (1×) up to 10×.
            let mut speed = player.speed();
            if ui.add(egui::Slider::new(&mut speed, 1.0..=10.0).suffix("×")).changed() {
                player.set_speed(speed);
            }
        } else {
            ui.label("track has no duration to replay");
        }

        if eject {
            self.gpx = None;
            self.gpx_label = None;
        }
    }

    /// The collapsing render-stats readout: LOD / chunk / feature / point counts
    /// from the last frame's [`RenderStats`](obc_render::RenderStats), plus the
    /// span / point / ring scratch-buffer utilization bars.
    fn show_render_stats(&self, ui: &mut egui::Ui) {
        let s = &self.last_stats;

        egui::Grid::new("render_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("LOD");
            ui.label(format!("{}", s.lod));
            ui.end_row();

            ui.label("Chunks");
            ui.label(format!("{}", s.chunks_visited));
            ui.end_row();

            ui.label("Features");
            ui.label(format!("{} / {} drawn", s.features_drawn, s.features_tried));
            ui.end_row();

            ui.label("Dropped");
            let drop_color = if s.features_dropped > 0 {
                egui::Color32::from_rgb(220, 80, 80)
            } else {
                ui.visuals().text_color()
            };
            ui.colored_label(drop_color, format!("{}", s.features_dropped));
            ui.end_row();

            ui.label("Points");
            ui.label(format!("{} / {} drawn", s.points_drawn, s.points_tried));
            ui.end_row();

            // Active route overlay (no LOD): points decoded vs. actually stroked, and chunks. As
            // you zoom out `pts` climbs with the visible route, but `drawn` tracks what's on-screen
            // (per-segment view clip + subpixel fold) — the gap is the clip doing its job.
            ui.label("Route");
            ui.label(format!("{} / {} drawn · {} chunks", s.route_points_drawn, s.route_points, s.route_chunks));
            ui.end_row();

            // Host-measured frame draw time (render + route/overlays). 0 = not yet measured.
            ui.label("Render");
            if s.render_us == 0 {
                ui.label("—");
            } else {
                ui.label(format!("{:.2} ms", s.render_us as f64 / 1000.0));
            }
            ui.end_row();
        });

        ui.add_space(4.0);
        ui.label("Buffer utilization");

        // Span buffer bar
        let span_pct = s.span_utilization;
        ui.horizontal(|ui| {
            ui.label("Spans");
            let bar = egui::ProgressBar::new(span_pct).text(format!("{:.0}%", span_pct * 100.0));
            ui.add(bar);
        });

        // Points buffer bar
        let pt_pct = s.point_utilization;
        ui.horizontal(|ui| {
            ui.label("Points");
            let bar = egui::ProgressBar::new(pt_pct).text(format!("{:.0}%", pt_pct * 100.0));
            ui.add(bar);
        });

        // Rings buffer bar
        let ring_pct = s.ring_utilization;
        ui.horizontal(|ui| {
            ui.label("Rings");
            let bar = egui::ProgressBar::new(ring_pct).text(format!("{:.0}%", ring_pct * 100.0));
            ui.add(bar);
        });
    }
}
