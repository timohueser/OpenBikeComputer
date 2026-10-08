//! Radio permission and sensor discovery intent, independent of controller execution.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorActivity {
    Park,
    Discover,
    Connect,
}

#[derive(Clone, Copy)]
pub struct RadioPolicy {
    enabled: bool,
    usb_inhibited: bool,
    discovery: bool,
}

impl RadioPolicy {
    pub const fn new(enabled: bool, usb_inhibited: bool) -> Self {
        Self { enabled, usb_inhibited, discovery: false }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_usb_inhibited(&mut self, inhibited: bool) {
        self.usb_inhibited = inhibited;
    }

    pub fn set_discovery(&mut self, requested: bool) {
        self.discovery = requested;
    }

    pub fn enabled(&self) -> bool {
        self.enabled && !self.usb_inhibited
    }

    /// Discovery repeats while requested. Permission withdrawal suspends that intent;
    /// closing the scan list removes it, including while the radio is inhibited.
    pub fn sensor_activity(&self) -> SensorActivity {
        if !self.enabled() {
            SensorActivity::Park
        } else if self.discovery {
            SensorActivity::Discover
        } else {
            SensorActivity::Connect
        }
    }
}
