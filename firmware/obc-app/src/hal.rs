//! Compatibility façade for the semantic host/hardware ports.
//!
//! Nominal sample, event, and capability definitions live in [`obc_ports`]. Existing consumers may
//! continue importing them through `obc_app::hal` while FAR-04 (#797) moves adapter dependencies to
//! the foundation crate directly.

pub use obc_ports::{
    AltimeterSource, Button, ButtonEvent, CadenceSource, ClockSource, CompassSource, Fix, FuelGauge, GpsTime,
    HeartRateSource, InputClock, InputEvent, InputSource, LocationSource, PowerSource, RideClock, Sensors,
    SettingsStore, TemperatureSource, TrackError, TrackSink,
};
