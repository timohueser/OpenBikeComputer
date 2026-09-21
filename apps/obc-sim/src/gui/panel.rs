//! The Controls window: a second egui immediate viewport driving the simulated device, with the
//! manual GPS fix, GPX replay and the render-stats readout. It is a second `impl SimGui` block, so
//! it mutates the same fields.

use eframe::egui;
use obc_app::CameraMode;
use obc_host_core::TripCatalog;

use super::housing::Colorway;
use super::units::{format_clock, format_distance, mpp_to_zoom, zoom_to_mpp, MAX_ZOOM, MIN_ZOOM, MPP_MAX, MPP_MIN};
use super::SimGui;
use crate::calib;

/// The red used for inline error / warning labels across the panel.
const ERROR_RED: egui::Color32 = egui::Color32::from_rgb(220, 80, 80);

/// A gap then a separator: the divider above most control-panel sections.
fn separator_above(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.separator();
}
// The two render paths, coloured the same in the legend and the stacked bars below.
const KIND_LINE: egui::Color32 = egui::Color32::from_rgb(80, 150, 235); // lines = blue
const KIND_POLY: egui::Color32 = egui::Color32::from_rgb(227, 165, 43); // polygons = amber

/// A stacked buffer-utilization bar splitting the fill into the line and polygon contributions.
/// `line` and `poly` are this frame's counts of the resource and `cap` is its scratch capacity, so
/// the total fill says how close this frame is to the scratch limit and which path is eating it.
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
    /// Draw the Controls window. It is re-declared every frame: the widgets edit the panel
    /// mirrors, and the mirrors are then pushed into the
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

                        // A loaded GPX track owns the fix, as the device's GPS would, so the
                        // manual position and heading inputs go read-only.
                        let replaying = self.gpx.is_some();

                        // The GPS fix, edited in degrees and stored as microdegrees.
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

                        // The magnetometer heading, which applies while the GPS has no course. It
                        // rotates a heading-up map during a replay pause and always drives a
                        // stopped Peak View.
                        ui.label("Compass (heading when stopped)");
                        ui.add(egui::Slider::new(&mut self.panel.compass_deg, 0.0..=360.0).suffix("°").step_by(1.0));

                        separator_above(ui);

                        // Meters per pixel on a log scale. Written back only when dragged, so it
                        // never fights the mouse scroll, which can range past the slider's bounds.
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

                        // Camera mode and orientation, paired on one row. Orientation is
                        // independent of the camera mode.
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
                        // Entering Follow snaps the fix onto the camera center, so the view does
                        // not jump: Free moved the camera away from the fix.
                        if prev_mode == CameraMode::Free && self.app.state.mode == CameraMode::Follow {
                            self.panel.lat_deg = self.app.state.cam_lat as f64 / 1e6;
                            self.panel.lon_deg = self.app.state.cam_lon as f64 / 1e6;
                        }

                        separator_above(ui);

                        // GPX replay plays a recorded track back as a simulated GPS sensor. The
                        // player is the active `LocationSource` while a track is loaded.
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

                        egui::CollapsingHeader::new("Sensors")
                            .default_open(false)
                            .show(ui, |ui| self.show_sensor_controls(ui));

                        separator_above(ui);

                        separator_above(ui);

                        self.show_display_controls(ui);

                        separator_above(ui);

                        egui::CollapsingHeader::new("Render Stats")
                            .default_open(true)
                            .show(ui, |ui| self.show_render_stats(ui));

                        separator_above(ui);

                        egui::CollapsingHeader::new("Altimeter")
                            .default_open(false)
                            .show(ui, |ui| self.show_altimeter(ui));
                    });

                    if ctx.input(|i| i.viewport().close_requested()) {
                        self.quit = true;
                    }
                });
            },
        );

        // Push the mirrors into the location and compass sources, which the app reads next tick.
        self.loc.set_position((self.panel.lat_deg * 1e6).round() as i32, (self.panel.lon_deg * 1e6).round() as i32);
        let moving = self.loc.current().and_then(|fix| fix.speed_mps).is_some_and(|speed| speed > 0.5);
        self.loc.set_course(moving.then_some(self.panel.heading_deg));
        self.compass.set(self.panel.compass_deg);
    }

    /// Display size: the 1:1 actual-size toggle, which needs a calibration, plus a calibrate
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

    /// The Bluetooth injection controls: the sim's face of the BLE seam. The connected toggle
    /// drives the connected indicator, the passkey injection drives the passkey card, the
    /// store-changed injection drives the live-catalog rescan, and the paired flag drives the
    /// Bluetooth screen's paired row. The radio-off state is not injected here, because it is the
    /// device's own setting. Every control edits the same [`obc_app::BleStatus`] mirror pushed into
    /// the app each frame.
    fn show_ble_controls(&mut self, ui: &mut egui::Ui) {
        use obc_app::BleLink;
        let mut connected = self.panel.ble.link == BleLink::Connected;
        if ui.checkbox(&mut connected, "Phone connected").changed() {
            self.panel.ble.link = if connected { BleLink::Connected } else { BleLink::Advertising };
        }
        ui.checkbox(&mut self.panel.ble.paired, "Paired (bond stored)");
        ui.weak("drives the indicator + the Bluetooth screen's status/Paired rows");

        // Passkey injection: a Pairing toggle mirrors the BLE side's passkey display, so setting
        // it opens the card and clearing it closes the card, with a numeric field for the code.
        // `set_ble_status` reconciles the host-pushed card each frame, as the board's ride loop
        // does.
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

        // Upload injection drives the route-upload popups. Pick a catalog route, then inject it
        // as a fresh upload, which copies the pick, or as a replace by id, which rewrites the
        // pick's bytes in place. Replacing the actively navigated route shows the forced-adoption
        // info card.
        let mut inject: Option<bool> = None; // Some(replace?)
        {
            let routes = self.app.routes();
            if routes.is_empty() {
                ui.weak("no routes to inject — import a GPX or start with route fixtures");
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
                ui.weak("rescan + upload fact → idle / mid-ride / active-replace popup");
            }
        }
        if let Some(replace) = inject {
            self.inject_upload(self.panel.upload_sel, replace);
        }

        // The protocol-level trip delete: it removes the trip object and leaves the member routes
        // as top-level routes, so they fall back to the unfiled top level on the next re-group.
        let mut delete_trip: Option<obc_app::CatalogObjectId> = None;
        let mut inject_trip: Option<obc_app::CatalogObjectId> = None;
        {
            let trips = self.app.trips();
            if !trips.is_empty() {
                ui.separator();
                self.panel.trip_sel = self.panel.trip_sel.min(trips.len() - 1);
                egui::ComboBox::from_label("trip").selected_text(trips[self.panel.trip_sel].name.as_str()).show_ui(
                    ui,
                    |ui| {
                        for (i, t) in trips.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.panel.trip_sel,
                                i,
                                format!("{} ({} stages, id {})", t.name.as_str(), t.stage_indices.len(), t.id),
                            );
                        }
                    },
                );
                ui.horizontal(|ui| {
                    if ui.button("Inject trip upload").clicked() {
                        inject_trip = Some(trips[self.panel.trip_sel].id);
                    }
                    if ui.button("Delete trip object").clicked() {
                        delete_trip = Some(trips[self.panel.trip_sel].id);
                    }
                });
                ui.weak("upload → the TRIP RECEIVED popup (replaces any route popup) · delete is non-cascading");
            }
        }
        // The trip-commit fact, in the board's order: the trip catalog is already fed, and the
        // fact then resolves the durable id. On the device the trip object always lands after its
        // member routes, so this popup replaces the burst's last per-route one.
        if let Some(id) = inject_trip {
            self.host.facts().note_trip_upload(obc_app::device_core::TripUpload { id, replaced: false });
        }
        if let Some(id) = delete_trip {
            match self.trip_store.delete_by_id(id) {
                Ok(_) => self.note_card_commit(),
                Err(error) => eprintln!("trip delete: {error:?}"),
            }
        }
    }
    /// Commit a fixture copy or exact replacement, then report its real object identity.
    fn inject_upload(&mut self, sel: usize, replace: bool) {
        let id = match crate::routes::import_copy(&mut self.store, sel, replace) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("route upload: {error}");
                return;
            }
        };
        self.note_card_commit();
        let elevation = crate::routes::elevation_sparkline(&self.store, id);
        self.host.facts().note_route_upload(obc_app::device_core::RouteUpload { id, replaced: replace, elevation });
    }

    /// The synthetic BLE-sensor controls: the sim's face of the heart rate, power and cadence
    /// seam. The effort-follows-speed switch synthesizes all three from the replayed speed, with
    /// light noise; with it off, each quantity has an enable toggle and a fixed-value slider. The
    /// values feed [`SimSensors`](crate::sim_sensors::SimSensors) each tick at the fresh-mailbox
    /// cadence, so toggling one off mid-ride makes its tile go stale and the log drop it.
    fn show_sensor_controls(&mut self, ui: &mut egui::Ui) {
        let cfg = &mut self.sim_sensors.cfg;
        ui.checkbox(&mut cfg.effort_follows_speed, "Effort follows speed");
        ui.weak("synthesize HR/power/cadence from the replayed GPX speed (with light noise)");

        // With the synth on, the per-quantity toggles and sliders are ignored, so grey them out.
        ui.add_enabled_ui(!cfg.effort_follows_speed, |ui| {
            ui.add_space(4.0);
            // One row per quantity: an enable toggle and a fixed-value slider. The rows are
            // indented inside the collapsing header and each slider carries a value box, so the
            // panel-wide `slider_width` would overflow them off the right edge. The rail is sized
            // to the space left, and a `Grid` keeps the checkbox column a uniform width.
            ui.spacing_mut().slider_width = (ui.available_width() - 150.0).max(80.0);
            egui::Grid::new("sim_sensor_sliders").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.checkbox(&mut cfg.hr_enabled, "HR");
                ui.add(egui::Slider::new(&mut cfg.hr_bpm, 40..=220).suffix(" bpm"));
                ui.end_row();
                ui.checkbox(&mut cfg.power_enabled, "Power");
                ui.add(egui::Slider::new(&mut cfg.power_w, 0..=1000).suffix(" W"));
                ui.end_row();
                ui.checkbox(&mut cfg.cadence_enabled, "Cadence");
                ui.add(egui::Slider::new(&mut cfg.cadence_rpm, 0..=130).suffix(" rpm"));
                ui.end_row();
            });
        });
    }

    /// The loaded-track controls: play and pause, a seek scrubber, and a speed slider. Split out so
    /// the eject mutation of `self.gpx` does not tangle with the active borrow of the player.
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
                // Play follows the moving fix; the user can still switch back to Free.
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

    /// The map-referenced altimeter readout: the simulator half of the device's `altfuse:` RTT
    /// line.
    ///
    /// Raw against fused is the whole story: when replay conditions move the raw row away from the
    /// terrain, the fused row stays on it, and the offset is the number doing the work.
    fn show_altimeter(&self, ui: &mut egui::Ui) {
        let a = self.app.recorder.altitude();
        let baro = self.app.recorder.baro_elevation_m();
        egui::Grid::new("altimeter_stats").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label("Raw baro");
            ui.label(baro.map_or_else(|| "—".to_string(), |m| format!("{m:.1} m")));
            ui.end_row();

            ui.label("Map ref");
            ui.label(a.map_reference_m().map_or_else(|| "—".to_string(), |m| format!("{m:.0} m")));
            ui.end_row();

            ui.label("Offset");
            ui.label(a.offset_m().map_or_else(|| "—".to_string(), |m| format!("{m:+.1} m")));
            ui.end_row();

            ui.label("Fused");
            let fused = baro.and_then(|b| a.fused_m(b));
            match fused {
                Some(m) => ui.label(format!("{m:.1} m")),
                None if a.offset_m().is_some() => {
                    ui.colored_label(ui.visuals().weak_text_color(), format!("settling {}/20", a.accepted()))
                }
                None => ui.label("—"),
            };
            ui.end_row();

            ui.label("Samples");
            ui.label(format!("{} ok · {} gated · {} re-seed", a.accepted(), a.gated(), a.reseeds()));
            ui.end_row();
        });
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

            // Chunk-cache hit rate and source overhead. The map renders in two collect passes
            // over the visible chunks, so a healthy hit rate keeps reads near one per chunk.
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

            // Active route overlay: points decoded against points stroked. The gap at far zoom is
            // the per-segment view clip and the subpixel fold.
            ui.label("Route");
            ui.label(format!("{} / {} drawn · {} chunks", s.route_points_drawn, s.route_points, s.route_chunks));
            ui.end_row();

            // Host-measured frame draw time. Zero means not yet measured.
            ui.label("Render");
            if s.render_us == 0 {
                ui.label("—");
            } else {
                ui.label(format!("{:.2} ms", s.render_us as f64 / 1000.0));
            }
            ui.end_row();

            // Render-on-demand signal: which planes the firmware would have re-rendered. The sim
            // always redraws, so this is a readout. The map plane fires on gestures and
            // camera-moving fixes and stays quiet when idle.
            let d = self.last_dirty;
            ui.label("Dirty");
            let on = egui::Color32::from_rgb(227, 165, 43); // amber, like the device accent
            let off = ui.visuals().weak_text_color();
            ui.horizontal(|ui| {
                ui.colored_label(if d.map { on } else { off }, "map");
                ui.colored_label(if d.overlay { on } else { off }, "overlay");
            });
            ui.end_row();

            // The pass's sleep decision: the timer a firmware host would arm before parking. The
            // sim repaints continuously so its Controls window stays live, so this is shown and not
            // obeyed. `now` means decided work is still in flight, and `event` means nothing is
            // time-animating.
            ui.label("Next wake");
            match self.last_wake_ms {
                Some(0) => ui.colored_label(on, "now"),
                Some(ms) => ui.label(format!("{ms} ms")),
                None => ui.colored_label(off, "event"),
            };
            ui.end_row();

            // Self-diffing present: rows pushed this frame against the full height, decided by the
            // per-row hash diff. Idle pushes none, a minute tick pushes a few clock rows, and a map
            // pan pushes nearly all. An exact full-frame diff oracle backs each number.
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
        // Scratch utilization split by render path, so each buffer's line and polygon
        // contributions are visible at saturating zoom levels.
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

impl SimGui {}
