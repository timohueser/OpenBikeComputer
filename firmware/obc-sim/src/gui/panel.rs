//! The "Controls" window — a second egui immediate viewport driving the simulated device:
//! the manual GPS fix (position / zoom / camera / orientation), GPX replay, and the
//! render-stats readout. A second `impl SimGui` block, so it mutates the same fields.

use eframe::egui;
use obc_app::CameraMode;

use super::housing::Colorway;
use super::units::{format_clock, format_distance, mpp_to_zoom, zoom_to_mpp, MAX_ZOOM, MIN_ZOOM, MPP_MAX, MPP_MIN};
use super::SimGui;
use crate::calib;

/// The red used for inline error / warning labels across the panel.
const ERROR_RED: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);

/// A 6 pt gap then a separator — the divider above most control-panel sections.
fn separator_above(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.separator();
}

/// A labelled `0–100%` progress bar (a render-stats buffer-utilization row).
#[allow(dead_code)]
fn util_bar(ui: &mut egui::Ui, label: &str, frac: f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::ProgressBar::new(frac).text(format!("{:.0}%", frac * 100.0)));
    });
}

// TEMP debug (scratch-budget investigation): the two render paths, colored the same in the legend
// and the stacked bars below.
const KIND_LINE: egui::Color32 = egui::Color32::from_rgb(80, 150, 235); // lines = blue
const KIND_POLY: egui::Color32 = egui::Color32::from_rgb(227, 165, 43); // polygons = amber

/// TEMP debug: a stacked buffer-utilization bar splitting the fill into the line vs polygon
/// contribution. `line`/`poly` are this frame's counts of the resource; `cap` its scratch capacity.
/// The blue segment is lines and the amber segment polygons, laid end to end, so the total fill is
/// `(line + poly) / cap` — how close this frame is to the scratch limit, and which path is eating it.
fn kind_bar(ui: &mut egui::Ui, label: &str, line: usize, poly: usize, cap: usize) {
    ui.horizontal(|ui| {
        ui.label(label);
        let cap_f = cap.max(1) as f32;
        let counts = format!("{line}L {poly}P · {:.0}%", 100.0 * (line + poly) as f32 / cap_f);
        // Reserve room for the trailing counts label; the bar takes the rest.
        let bar_w = (ui.available_width() - 118.0).max(60.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_w, 15.0), egui::Sense::hover());
        let painter = ui.painter();
        let rounding = 3.0;
        painter.rect_filled(rect, rounding, ui.visuals().extreme_bg_color);
        let line_frac = (line as f32 / cap_f).clamp(0.0, 1.0);
        let poly_frac = (poly as f32 / cap_f).clamp(0.0, 1.0 - line_frac);
        let w = rect.width();
        if line_frac > 0.0 {
            let seg = egui::Rect::from_min_size(rect.left_top(), egui::vec2(w * line_frac, rect.height()));
            painter.rect_filled(seg, rounding, KIND_LINE);
        }
        if poly_frac > 0.0 {
            let seg = egui::Rect::from_min_size(
                egui::pos2(rect.left() + w * line_frac, rect.top()),
                egui::vec2(w * poly_frac, rect.height()),
            );
            painter.rect_filled(seg, rounding, KIND_POLY);
        }
        ui.label(counts);
    });
}

impl SimGui {
    /// Draw the "Controls" window. Re-declared every frame; the widgets edit the panel
    /// mirrors / `AppState`, then the mirrors are pushed into the
    /// [`SimLocationSource`](crate::sim_location::SimLocationSource) for the next `App::tick`.
    pub(super) fn show_control_panel(&mut self, ctx: &egui::Context) {
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("controls"),
            egui::ViewportBuilder::default().with_title("Controls").with_inner_size([360.0, 770.0]),
            |ctx, _class| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Device color");
                            egui::ComboBox::from_id_salt("colorway").selected_text(self.colorway.label()).show_ui(
                                ui,
                                |ui| {
                                    for c in Colorway::ALL {
                                        ui.selectable_value(&mut self.colorway, c, c.label());
                                    }
                                },
                            );
                        });

                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // Let sliders span the panel width, leaving room for the value box.
                        ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(140.0);

                        // A loaded GPX track owns the fix (as the device's GPS would), so the manual
                        // position/heading inputs go read-only. Camera/zoom/orientation stay live.
                        let replaying = self.gpx.is_some();

                        // Position — the GPS fix, edited in degrees (stored as µdeg).
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

                        separator_above(ui);

                        // Compass — the magnetometer heading, always live but only effective on a
                        // heading-up map with no GPS course (so it rotates the map during a replay pause,
                        // no-op otherwise).
                        ui.label("Compass (heading when stopped)");
                        ui.add(egui::Slider::new(&mut self.panel.compass_deg, 0.0..=360.0).suffix("°").step_by(1.0));

                        separator_above(ui);

                        // Zoom — meters-per-pixel on a log scale. Only write back when dragged, so it
                        // never fights the mouse scroll (which can range past the slider's bounds).
                        ui.label("Zoom");
                        let mut mpp = zoom_to_mpp(self.app.state.zoom);
                        let resp =
                            ui.add(egui::Slider::new(&mut mpp, MPP_MIN..=MPP_MAX).logarithmic(true).custom_formatter(
                                |n, _| {
                                    let v = if n < 1.0 {
                                        format!("{n:.3}")
                                    } else if n < 100.0 {
                                        format!("{n:.1}")
                                    } else {
                                        format!("{n:.0}")
                                    };
                                    format!("{v} m/px")
                                },
                            ));
                        if resp.changed() {
                            self.app.state.zoom = mpp_to_zoom(mpp).clamp(MIN_ZOOM, MAX_ZOOM);
                        }
                        let span = zoom_to_mpp(self.app.state.zoom) * self.dev_w as f32;
                        ui.label(format!("{} across screen", format_distance(span)));

                        separator_above(ui);

                        // Camera mode and Orientation — paired on one row. Orientation (north-up vs
                        // heading-up) is independent of the camera mode.
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
                        // Entering Follow: snap the fix onto the camera center so the view doesn't jump
                        // (Free moved the camera away from the fix).
                        if prev_mode == CameraMode::Free && self.app.state.mode == CameraMode::Follow {
                            self.panel.lat_deg = self.app.state.cam_lat as f64 / 1e6;
                            self.panel.lon_deg = self.app.state.cam_lon as f64 / 1e6;
                        }

                        separator_above(ui);

                        // GPX replay — play a recorded track back as a simulated GPS sensor. The player
                        // is the active `LocationSource` while a track is loaded.
                        ui.label("GPX replay");
                        if ui.button("Load GPX…").clicked() {
                            if let Some(path) = rfd::FileDialog::new().add_filter("GPX track", &["gpx"]).pick_file() {
                                self.load_gpx(&path);
                            }
                        }
                        self.show_gpx_controls(ui);

                        separator_above(ui);

                        egui::CollapsingHeader::new("Bluetooth")
                            .default_open(false)
                            .show(ui, |ui| self.show_ble_controls(ui));

                        separator_above(ui);

                        self.show_display_controls(ui);

                        separator_above(ui);

                        egui::CollapsingHeader::new("Render Stats")
                            .default_open(true)
                            .show(ui, |ui| self.show_render_stats(ui));
                    });

                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.quit = true;
                    }
                });
            },
        );

        // Push the mirrors into the location + compass sources (the app reads them next tick).
        self.loc.set_position((self.panel.lat_deg * 1e6).round() as i32, (self.panel.lon_deg * 1e6).round() as i32);
        self.loc.set_course(self.panel.heading_deg);
        self.compass.set(self.panel.compass_deg);
    }

    /// Display size — the 1:1 "actual size" toggle (needs a calibration) plus a (re)calibrate
    /// button. The calibration screen lives in [`super::SimGui`].
    fn show_display_controls(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Display size").strong());
        let calibrated = self.points_per_mm.is_some();
        ui.horizontal(|ui| {
            let resp = ui.add_enabled(calibrated, egui::Checkbox::new(&mut self.physical, "Actual size (1:1)"));
            if resp.changed() {
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
                    calib::PANEL_W_MM,
                    calib::PANEL_H_MM
                ));
            }
            None => {
                ui.weak("not calibrated — click Calibrate… to set 1:1 size");
            }
        }
        if let Some(e) = &self.calib_error {
            ui.colored_label(ERROR_RED, e);
        }
    }

    /// The Bluetooth injection controls — the sim's face of the host→app BLE seam (epic #447). P1
    /// exposes the connected toggle (the connected indicator's driver); P2 adds the passkey injection
    /// (the passkey card); P3 adds the store-changed injection (the live-catalog rescan's driver);
    /// P8 adds the paired flag (the Bluetooth screen's "Paired" row — its Forget hold clears it,
    /// like the board's RRAM clear; the radio-off state isn't injected here — it's the device's own
    /// `Settings::ble_enabled`, flipped on the Bluetooth screen); P4 will add the full inject-upload
    /// UI — all editing the same [`obc_app::BleStatus`] mirror pushed into the app each frame, so no
    /// restructuring is needed then.
    fn show_ble_controls(&mut self, ui: &mut egui::Ui) {
        use obc_app::BleLink;
        let mut connected = self.panel.ble.link == BleLink::Connected;
        if ui.checkbox(&mut connected, "Phone connected").changed() {
            self.panel.ble.link = if connected { BleLink::Connected } else { BleLink::Advertising };
        }
        ui.checkbox(&mut self.panel.ble.paired, "Paired (bond stored)");
        ui.weak("drives the indicator + the Bluetooth screen's status/Paired rows");

        // Passkey injection (P2): a "Pairing" toggle mirrors the BLE side's `PassKeyDisplay` →
        // `passkey: Some` (opens the card) / cleared → `None` (closes it), with a numeric field to
        // set the 6-digit code the card renders. `set_ble_status` reconciles the host-pushed card
        // each frame from `self.panel.ble.passkey`, exactly as the board's ride loop does.
        let mut pairing = self.panel.ble.passkey.is_some();
        if ui.checkbox(&mut pairing, "Pairing (show passkey card)").changed() {
            self.panel.ble.passkey = pairing.then_some(123_456);
        }
        if let Some(passkey) = self.panel.ble.passkey.as_mut() {
            ui.add(
                egui::Slider::new(passkey, 0..=999_999)
                    .text("passkey")
                    .custom_formatter(|n, _| format!("{:06}", n as u32)),
            );
        }

        // The store-changed edge (#450), exactly the device's sequence: the store notifies, the
        // host rescans and re-feeds the id-carrying catalog, the app remaps held indices by id.
        // Drop/remove an `.obcr` in the routes folder, then click — a mid-session upload/delete
        // without a radio (P4 adds the full inject-upload popup flow on top of this edge).
        if ui.button("Store changed (rescan routes + rides)").clicked() {
            self.app.notify_store_changed();
            let _ = self.app.take_store_changed();
            self.store.rescan();
            self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
            // The same edge covers the ride catalog (#454): a dropped-in `RD{id}.ORD` or an edited
            // `SYNCED.SET` shows up on the Rides screen without a relaunch.
            self.ride_store.rescan();
            self.app.set_rides(self.ride_store.catalog(), self.ride_store.ids());
        }
        ui.weak("re-scans the routes + tracks folders like a BLE commit/delete");

        // Upload injection (P4): the route-upload popups' driver. Pick a catalog route, then
        // inject it as a fresh upload (a new file — copy of the pick) or a replace-by-id (the
        // pick's bytes rewritten in place). Each button runs the exact device sequence via
        // [`SimGui::inject_upload`]; replace the *actively navigated* route to see the
        // forced-adoption info card.
        let mut inject: Option<bool> = None; // Some(replace?)
        {
            let routes = self.app.routes();
            if routes.is_empty() {
                ui.weak("no routes to inject — add .obcr files to the routes folder");
            } else {
                self.panel.upload_sel = self.panel.upload_sel.min(routes.len() - 1);
                let ids = self.app.route_ids();
                egui::ComboBox::from_label("upload")
                    .selected_text(routes[self.panel.upload_sel].name.as_str())
                    .show_ui(ui, |ui| {
                        for (i, r) in routes.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.panel.upload_sel,
                                i,
                                format!("{} (id {})", r.name.as_str(), ids[i]),
                            );
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("Inject upload (new)").clicked() {
                        inject = Some(false);
                    }
                    if ui.button("Inject upload (replace)").clicked() {
                        inject = Some(true);
                    }
                });
                ui.weak("rescan + upload event → idle / mid-ride / active-replace popup");
            }
        }
        if let Some(replace) = inject {
            self.inject_upload(self.panel.upload_sel, replace);
        }
    }

    /// Drive the exact device route-upload sequence from the control panel (epic #447, P4):
    /// mutate the routes folder (a fresh copy for "new"; an in-place bytes rewrite for
    /// "replace-by-id"), then the store-changed edge → rescan + identity remap, **then** the
    /// upload event carrying the durable id — the same ordering the board's ride loop sees (the
    /// rescan first, so the id resolves in the fresh catalog). A replace also drops the cached
    /// active-route bytes so the next frame reopens them, mirroring the board's
    /// close-and-reopen of the geometry handle.
    fn inject_upload(&mut self, sel: usize, replace: bool) {
        let id = if replace { self.store.touch_route(sel) } else { self.store.duplicate_route(sel) };
        let Some(id) = id else { return };
        self.app.notify_store_changed();
        let _ = self.app.take_store_changed();
        self.store.rescan();
        self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
        if replace {
            self.store.sync_active(None); // force the geometry reopen off the fresh bytes
        }
        self.app.notify_route_uploaded(id, replace);
    }

    /// The loaded-track controls: play/pause (auto-follows), a seek scrubber, and a 1×–10× speed
    /// slider. Split out so the "eject" mutation of `self.gpx` doesn't tangle with the active
    /// `&mut` borrow of the player.
    fn show_gpx_controls(&mut self, ui: &mut egui::Ui) {
        let Some(player) = self.gpx.as_mut() else {
            if let Some(err) = &self.gpx_error {
                ui.colored_label(ERROR_RED, err);
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
                // Play follows the moving fix; the user can still switch back to Free mid-playback.
                if player.is_playing() {
                    self.app.state.mode = CameraMode::Follow;
                }
            }
            if ui.button("⏏ Eject").clicked() {
                eject = true;
            }
        });

        if dur > 0.0 {
            let mut t = player.time();
            let resp = ui.add(egui::Slider::new(&mut t, 0.0..=dur).show_value(false).text("seek"));
            if resp.changed() {
                player.seek(t);
            }
            ui.label(format!("{} / {}", format_clock(player.time()), format_clock(dur)));

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

    /// The collapsing render-stats readout: last frame's [`RenderStats`](obc_render::RenderStats)
    /// counts plus the scratch-buffer utilization bars.
    fn show_render_stats(&self, ui: &mut egui::Ui) {
        let s = &self.last_stats;

        egui::Grid::new("render_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("LOD");
            ui.label(format!("{}", s.lod));
            ui.end_row();

            ui.label("Chunks");
            ui.label(format!("{}", s.chunks_visited));
            ui.end_row();

            // Chunk-cache hit rate + source overhead. The map renders through 4 priority passes
            // over the same chunks, so a healthy hit rate keeps reads near one per visible chunk.
            let cache_reqs = s.map_chunk_hits + s.map_chunk_misses;
            ui.label("Map SD");
            if cache_reqs == 0 {
                ui.label("—");
            } else {
                let hit_pct = 100.0 * s.map_chunk_hits as f32 / cache_reqs as f32;
                ui.label(format!("{:.0}% hit · {} rd · {} B", hit_pct, s.map_sd_reads, s.map_bytes_read));
            }
            ui.end_row();

            ui.label("Features");
            ui.label(format!("{} / {} drawn", s.features_drawn, s.features_tried));
            ui.end_row();

            ui.label("Dropped");
            let drop_color = if s.features_dropped > 0 { ERROR_RED } else { ui.visuals().text_color() };
            ui.colored_label(drop_color, format!("{}", s.features_dropped));
            ui.end_row();

            ui.label("Points");
            ui.label(format!("{} / {} drawn", s.points_drawn, s.points_tried));
            ui.end_row();

            // Active route overlay: points decoded vs. actually stroked. The gap (as you zoom out)
            // is the per-segment view clip + subpixel fold doing its job.
            ui.label("Route");
            ui.label(format!("{} / {} drawn · {} chunks", s.route_points_drawn, s.route_points, s.route_chunks));
            ui.end_row();

            // Host-measured frame draw time. 0 = not yet measured.
            ui.label("Render");
            if s.render_us == 0 {
                ui.label("—");
            } else {
                ui.label(format!("{:.2} ms", s.render_us as f64 / 1000.0));
            }
            ui.end_row();

            // Render-on-demand signal: which planes the firmware *would* have re-rendered. The sim
            // always redraws, so this is informational — `map` fires on gestures / camera-moving
            // fixes and stays quiet when idle.
            let d = self.last_dirty;
            ui.label("Dirty");
            let on = egui::Color32::from_rgb(227, 165, 43); // amber, like the device accent
            let off = ui.visuals().weak_text_color();
            ui.horizontal(|ui| {
                ui.colored_label(if d.map { on } else { off }, "map");
                ui.colored_label(if d.overlay { on } else { off }, "overlay");
            });
            ui.end_row();

            // Self-diffing present: rows actually *pushed* this frame vs. the full height, decided
            // by the per-row hash diff — idle → 0 (free), a Home minute tick → a few clock rows, a
            // map pan → ~all. An exact full-frame diff oracle backs each number (a miss panics).
            let p = self.present.stats;
            ui.label("Present");
            if p.total_rows == 0 {
                ui.label("—");
            } else {
                let pct = 100.0 * p.pushed_rows as f32 / p.total_rows as f32;
                ui.label(format!("{} / {} rows · {} spans ({:.0}%)", p.pushed_rows, p.total_rows, p.spans, pct));
            }
            ui.end_row();
        });

        ui.add_space(4.0);
        // TEMP debug (scratch-budget investigation): scratch utilization split by render path, so
        // the line vs polygon contribution to each buffer is visible at saturating zoom levels.
        ui.horizontal(|ui| {
            ui.label("Scratch by kind");
            ui.colored_label(KIND_LINE, "■ lines");
            ui.colored_label(KIND_POLY, "■ polygons");
        });
        kind_bar(ui, "Spans", s.line_spans, s.poly_spans, obc_render::MAX_SPANS);
        kind_bar(ui, "Points", s.line_points, s.poly_points, obc_render::MAX_FRAME_POINTS);
        kind_bar(ui, "Rings", s.line_rings, s.poly_rings, obc_render::MAX_FRAME_RINGS);
    }
}
