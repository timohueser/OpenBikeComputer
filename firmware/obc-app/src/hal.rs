//! Compatibility façade for the semantic host/hardware ports.
//!
//! Nominal sample, event, and capability definitions live in [`obc_ports`]. Existing downstream
//! consumers may continue importing the established `obc_app::hal` paths, while implementations
//! and the app itself bind directly to the foundation crate.

pub use obc_ports::{
    AltimeterSource, Button, ButtonEvent, CadenceSource, ClockSource, CompassSource, Fix, FuelGauge, GpsTime,
    HeartRateSource, InputClock, InputEvent, InputSource, LocationSource, PowerSource, RideClock, Sensors,
    SettingsStore, TemperatureSource, TrackError, TrackSink,
};
