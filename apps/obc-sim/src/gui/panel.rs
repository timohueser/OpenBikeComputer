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

/// A short human label for a [`Retention`](obc_app::Retention) level — the retention combo's text.
fn retention_label(r: obc_app::Retention) -> &'static str {
    use obc_app::Retention;
    match r {
        Retention::Never => "Never",
        Retention::Day1 => "1 day",
        Retention::Week1 => "1 week",
        Retention::Week2 => "2 weeks",
        Retention::Month1 => "1 month",
        Retention::Month2 => "2 months",
    }
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

                        egui::CollapsingHeader::new("Sensors")
                            .default_open(false)
                            .show(ui, |ui| self.show_sensor_controls(ui));

                        separator_above(ui);

                        egui::CollapsingHeader::new("Auto-delete (retention)")
                            .default_open(false)
                            .show(ui, |ui| self.show_retention_controls(ui));

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
            // The store-changed edge (#450) is the manual rescan + id-carrying re-feed below — the
            // whole mechanism a `StoreChanged` event + drained `RescanStore` would drive on the
            // board. (No app-side `StoreChanged` is raised here: the counted cue would otherwise sit
            // pending for the GUI's next `HostLoop` frame to redundantly re-scan.)
            self.store.rescan();
            self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
            // The same edge covers the trips (epic #526): a dropped-in / removed `.obt` re-groups
            // the menu. Fed after the routes so the stage ids resolve against the fresh catalog.
            self.trip_store.rescan();
            self.app.set_trips(&self.trip_store.inputs());
            // The same edge covers the ride catalog (#454): a dropped-in `RD{id}.ORD` or an edited
            // `SYNCED.SET` shows up on the Rides screen without a relaunch.
            self.ride_store.rescan();
            self.app.set_rides(self.ride_store.catalog(), self.ride_store.ids());
        }
        ui.weak("re-scans the routes + trips + tracks folders like a BLE commit/delete");

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

        // Trip delete (epic #526, TR2): the protocol-level trip delete — removes the `.obt` and
        // leaves the member routes as top-level routes (non-cascading, spec §7.7). Stands in for the
        // on-device delete until the TR3 folder UI wires it to a hold gesture. Deleting drops the
        // folder, so its member routes fall back to the unfiled top level on the next re-group.
        let mut delete_trip: Option<u16> = None;
        let mut inject_trip: Option<u16> = None;
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
                    if ui.button("Delete trip (removes the .obt)").clicked() {
                        delete_trip = Some(trips[self.panel.trip_sel].id);
                    }
                });
                ui.weak("upload → the TRIP RECEIVED popup (replaces any route popup) · delete is non-cascading");
            }
        }
        // The trip-commit event, exactly the board's order: the trip catalog is already fed (the
        // scan above / the store-changed re-feed), then the event resolves the durable id — on the
        // device the trip object always lands after its member routes, so this popup replaces the
        // burst's last per-route popup (single most-recent-wins slot).
        if let Some(id) = inject_trip {
            self.app.apply_event(obc_app::HostEvent::TripUploaded { id, replaced: false });
        }
        if let Some(id) = delete_trip {
            // A trip delete doesn't move the *route* store, so no store-changed edge — the trip
            // re-feed is the whole mechanism (the deleted folder's routes fall back to unfiled).
            if self.trip_store.delete_by_id(id) {
                self.app.set_trips(&self.trip_store.inputs());
            }
        }
    }

    /// The **auto-delete / retention** controls (auto-expiry epic #638, S3) — the sim's face of the
    /// self-cleaning storage feature, so route/ride expiry is eyeball-testable without hardware:
    ///
    /// - **GPS time** toggles the trusted-clock feed ([`SimClock`](super::SimClock)); off is the
    ///   fresh-device untrusted state where nothing auto-deletes.
    /// - **+1 day** fast-forwards the fed clock 24 h and forces the next sweep, so a 1-day-retention
    ///   route or an aged synced ride disappears in seconds.
    /// - **Set route retention** stands in for the phone's `setRouteRetention` (until S4 gives it a
    ///   wire): pick a route + level and the sweep will delete it once it's been unused that long.
    /// - **Mark ride synced** is the `ackRides` stand-in: it stamps a ride's `synced_at` so the
    ///   sweep can auto-delete it per the device `ride_retention` setting.
    fn show_retention_controls(&mut self, ui: &mut egui::Ui) {
        use obc_app::Retention;

        ui.checkbox(&mut self.panel.gps_time, "GPS time (trusted clock)");
        ui.weak("feeds host UTC as a GPS fix — off = untrusted, nothing auto-deletes");

        ui.horizontal(|ui| {
            if ui.button("+1 day").clicked() {
                self.panel.clock_offset_secs = self.panel.clock_offset_secs.saturating_add(86_400);
                // Force the next tick's sweep regardless of the (fast-forwarded) wall-clock hour, so
                // an expiry is visible immediately instead of on the next hour boundary.
                self.app.force_retention_sweep();
            }
            if ui.button("reset clock").clicked() {
                self.panel.clock_offset_secs = 0;
            }
            ui.weak(format!("+{} d", self.panel.clock_offset_secs / 86_400));
        });

        ui.separator();

        // Set a route's retention level (the setRouteRetention stand-in until S4).
        let mut apply: Option<(u16, Retention)> = None;
        {
            let routes = self.app.routes();
            if routes.is_empty() {
                ui.weak("no routes — add .obcr files to the routes folder");
            } else {
                self.panel.retention_route_sel = self.panel.retention_route_sel.min(routes.len() - 1);
                let ids = self.app.route_ids();
                let sel_id = ids[self.panel.retention_route_sel];
                egui::ComboBox::from_id_salt("retention-route")
                    .selected_text(routes[self.panel.retention_route_sel].name.as_str())
                    .show_ui(ui, |ui| {
                        for (i, r) in routes.iter().enumerate() {
                            ui.selectable_value(&mut self.panel.retention_route_sel, i, r.name.as_str());
                        }
                    });
                egui::ComboBox::from_id_salt("retention-level")
                    .selected_text(retention_label(self.panel.retention_level))
                    .show_ui(ui, |ui| {
                        for lvl in [
                            Retention::Never,
                            Retention::Day1,
                            Retention::Week1,
                            Retention::Week2,
                            Retention::Month1,
                            Retention::Month2,
                        ] {
                            ui.selectable_value(&mut self.panel.retention_level, lvl, retention_label(lvl));
                        }
                    });
                if ui.button("Set route retention").clicked() {
                    apply = Some((sel_id, self.panel.retention_level));
                }
                let meta = self.store.retention_of(sel_id);
                ui.weak(format!(
                    "route id {sel_id}: {} · last_used {}",
                    retention_label(meta.retention),
                    if meta.last_used_utc == 0 { "unset".to_string() } else { meta.last_used_utc.to_string() }
                ));
            }
        }
        if let Some((id, level)) = apply {
            self.store.set_retention(id, level);
        }

        ui.separator();

        // Mark a ride synced (the ackRides stand-in) so the sweep can auto-delete it.
        let mut mark: Option<u16> = None;
        {
            let rides = self.app.rides();
            if rides.is_empty() {
                ui.weak("no rides — record one (Ride ▶ … ▶ Finish) to test ride expiry");
            } else {
                self.panel.synced_ride_sel = self.panel.synced_ride_sel.min(rides.len() - 1);
                let ids = self.app.ride_ids();
                let sel_id = ids[self.panel.synced_ride_sel];
                egui::ComboBox::from_id_salt("synced-ride")
                    .selected_text(rides[self.panel.synced_ride_sel].name.as_str())
                    .show_ui(ui, |ui| {
                        for (i, r) in rides.iter().enumerate() {
                            let tag = if r.synced { " (synced)" } else { "" };
                            ui.selectable_value(
                                &mut self.panel.synced_ride_sel,
                                i,
                                format!("{}{tag}", r.name.as_str()),
                            );
                        }
                    });
                if ui.button("Mark ride synced (ackRides)").clicked() {
                    mark = Some(sel_id);
                }
                ui.weak("stamps synced_at = now; the sweep deletes it per the device ride_retention");
            }
        }
        if let Some(id) = mark {
            let utc = self.app.wall_unix_now();
            self.ride_store.mark_synced(id, utc);
            self.ride_store.rescan();
            self.app.set_rides(self.ride_store.catalog(), self.ride_store.ids());
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
        // The store-changed edge is the rescan + id-carrying re-feed (see the "Store changed"
        // button); then the `RouteUploaded` event carries the durable id — the rescan-then-resolve
        // ordering the board's ride loop and the app both rely on.
        self.store.rescan();
        self.app.set_routes_with_ids(self.store.catalog(), self.store.ids());
        if replace {
            self.store.sync_active(None); // force the geometry reopen off the fresh bytes
        }
        let elevation = self.store.elevation_sparkline(id);
        self.app.apply_event(obc_app::HostEvent::RouteUploaded { id, replaced: replace, elevation });
    }

    /// The synthetic BLE-sensor controls (epic #707 SE8): the sim's face of the HR / power /
    /// cadence seam. One **effort follows speed** switch synthesizes all three from the replayed GPX
    /// speed (plus light noise) — the no-slider-babysitting path for a plausible recorded ride; with
    /// it off, each quantity has an enable toggle + a fixed-value slider. The values feed
    /// [`SimSensors`](crate::sim_sensors::SimSensors) each tick at the ~1 Hz fresh-mailbox cadence, so
    /// toggling one off mid-ride makes its tile go stale → `--` (the app's 5 s gate) and the log drop
    /// it. Injected values share the app's `Sensors` wiring with the future BLE central (SE6).
    fn show_sensor_controls(&mut self, ui: &mut egui::Ui) {
        let cfg = &mut self.sim_sensors.cfg;
        ui.checkbox(&mut cfg.effort_follows_speed, "Effort follows speed");
        ui.weak("synthesize HR/power/cadence from the replayed GPX speed (with light noise)");

        // With the synth on, the per-quantity toggles/sliders are ignored, so grey them out.
        ui.add_enabled_ui(!cfg.effort_follows_speed, |ui| {
            ui.add_space(4.0);
            // One row per quantity: an enable toggle + a fixed-value slider (bpm / W / rpm). These
            // rows are indented inside the "Sensors" collapsing header and each slider carries a
            // value box, so the panel-wide `slider_width` set at the top overflows them off the
            // right edge (clipping the value). Size the rail to the space actually left — a label
            // column + the value box — and lay the rows out in a `Grid` so the checkbox column is a
            // uniform width and the three sliders line up.
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

    /// The **map-referenced altimeter** readout (elevation epic #1068, EL8) — the simulator half of
    /// the device's `altfuse:` RTT line, and the inspection surface #529 was waiting on.
    ///
    /// Raw vs. fused is the whole story: with `--baro-drift` injected the raw row walks away from
    /// the terrain while the fused row stays on it, and `Offset` is the number doing the work.
    /// `Reference P` is the sea-level-reduced pressure — the trend a storm heuristic would read,
    /// with the ride's own climbing already subtracted out. Nothing here is drawn on the device.
    fn show_altimeter(&self, ui: &mut egui::Ui) {
        let a = self.app.activity.altitude();
        let baro = self.app.activity.baro_elevation_m();
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

            ui.label("Reference P");
            let p = baro.and_then(|b| a.reference_pressure_hpa(b));
            ui.label(p.map_or_else(|| "—".to_string(), |hpa| format!("{hpa:.2} hPa")));
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

            // Chunk-cache hit rate + source overhead. The map renders in two collect passes (A:
            // select candidates, B: re-decode winners) over the visible chunks, so a healthy hit
            // rate keeps reads near one per visible chunk.
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
