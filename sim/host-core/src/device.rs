//! Owned device state and frame sequencing for RGBA host shells.

use crate::flat_map::FlatMap;
use crate::{
    frame, photo, ActiveRouteSession, FlatRideStore, FlatRouteStore, HostLoop, HostPlatform, RgbaFrame, TrackRepository,
};
use embedded_graphics::geometry::OriginDimensions;
use obc_app::device_core::{PassClock, PassPlan, PlatformSupport};
use obc_app::{App, Dirty, Gesture};
use obc_ports::Sensors;
use obc_route::ElevationSource;

pub struct DeviceHost<T> {
    pub map: FlatMap,
    pub app: Box<App>,
    pub routes: FlatRouteStore,
    pub rides: FlatRideStore,
    pub tracks: T,
    pub host: HostLoop,
    pub session: ActiveRouteSession,
    pub elevation: Box<dyn ElevationSource>,
    pub ready: bool,
    scratch: Box<obc_render::RenderScratch>,
    frame: RgbaFrame,
    photo: photo::Preparer,
}

impl<T: TrackRepository> DeviceHost<T> {
    pub fn new(
        app: Box<App>,
        map: FlatMap,
        routes: FlatRouteStore,
        rides: FlatRideStore,
        tracks: T,
        elevation: Box<dyn ElevationSource>,
        (width, height): (u32, u32),
    ) -> Self {
        Self {
            map,
            app,
            routes,
            rides,
            tracks,
            elevation,
            host: HostLoop::new(),
            session: ActiveRouteSession::new(),
            ready: false,
            scratch: Box::new(obc_render::RenderScratch::new()),
            frame: RgbaFrame::new(width, height),
            photo: photo::Preparer::default(),
        }
    }

    /// `after_pass` observes cues and input cancellation before effects run.
    pub fn step(
        &mut self,
        clock: PassClock,
        gestures: &[Gesture],
        mut sensors: Sensors<'_>,
        support: PlatformSupport,
        platform: &mut dyn HostPlatform,
        after_pass: impl FnOnce(&mut App, &PassPlan),
    ) -> PassPlan {
        self.session.sync(&self.app, &mut self.routes);
        let mut plan = {
            let route = frame::active_route(&self.session, &self.routes);
            // Sensor ports borrow only for this pass, like the temporary route reader.
            let sensors = Sensors {
                loc: &mut *sensors.loc,
                altimeter: sensors.altimeter.as_mut().map(|s| &mut **s as &mut dyn obc_ports::AltimeterSource),
                temperature: sensors.temperature.as_mut().map(|s| &mut **s as &mut dyn obc_ports::TemperatureSource),
                clock: sensors.clock.as_mut().map(|s| &mut **s as &mut dyn obc_ports::ClockSource),
                compass: sensors.compass.as_mut().map(|s| &mut **s as &mut dyn obc_ports::CompassSource),
                fuel: sensors.fuel.as_mut().map(|s| &mut **s as &mut dyn obc_ports::FuelGauge),
                hr: sensors.hr.as_mut().map(|s| &mut **s as &mut dyn obc_ports::HeartRateSource),
                power: sensors.power.as_mut().map(|s| &mut **s as &mut dyn obc_ports::PowerSource),
                cadence: sensors.cadence.as_mut().map(|s| &mut **s as &mut dyn obc_ports::CadenceSource),
            };
            self.host.pass(&mut self.app, clock, gestures, sensors, route.as_ref(), support)
        };
        after_pass(&mut self.app, &plan);
        self.host.execute(
            &mut self.app,
            &mut plan,
            &mut self.session,
            &mut self.routes,
            &mut self.rides,
            &mut self.tracks,
            &mut (),
            &self.map,
            &mut *self.elevation,
            platform,
        );
        plan
    }

    pub fn render_if_dirty(&mut self, dirty: Dirty, panorama: Option<&obc_app::peak_view::Panorama>) -> bool {
        if !dirty.map && !dirty.overlay && self.ready && !self.app.photo_pending() {
            return false;
        }
        // Effects can replace the active route; render the committed geometry.
        self.session.sync(&self.app, &mut self.routes);
        let route = frame::active_route(&self.session, &self.routes);
        let reader = self.map.reader();
        let size = self.frame.size();
        frame::render(
            &mut self.app,
            &mut self.scratch,
            &mut self.frame,
            frame::Scene { reader: &reader, route: route.as_ref() },
            panorama,
            (size.width as f32, size.height as f32),
            frame::device_rgb888,
            &obc_render::NoopClock,
            Some(self.photo.interactive(dirty.map || !self.ready)),
        );
        self.app.set_resident_frame(true);
        self.ready = true;
        true
    }

    pub fn frame(&self) -> &[u8] {
        self.frame.as_rgba()
    }
}
