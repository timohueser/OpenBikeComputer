#![no_std]

pub mod control;
pub mod input;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    Power,
    Cadence,
    HeartRate,
}

impl Sensor {
    pub fn next(self) -> Self {
        match self {
            Self::Power => Self::Cadence,
            Self::Cadence => Self::HeartRate,
            Self::HeartRate => Self::Power,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Power => "OBC Mock Power",
            Self::Cadence => "OBC Mock Cadence",
            Self::HeartRate => "OBC Mock HR",
        }
    }

    pub fn service(self) -> u16 {
        match self {
            Self::Power => 0x1818,
            Self::Cadence => 0x1816,
            Self::HeartRate => 0x180d,
        }
    }

    pub fn measurement(self) -> u16 {
        match self {
            Self::Power => 0x2a63,
            Self::Cadence => 0x2a5b,
            Self::HeartRate => 0x2a37,
        }
    }

    pub fn limit(self) -> u16 {
        match self {
            Self::Power => 2000,
            Self::Cadence => 250,
            Self::HeartRate => 240,
        }
    }

    pub fn address(self, mut factory: [u8; 6]) -> [u8; 6] {
        factory[0] ^= self as u8;
        factory[5] |= 0xc0;
        factory
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Steady,
    Ramp,
    Intervals,
}

pub struct Simulator {
    pub sensor: Sensor,
    pub scenario: Scenario,
    pub stopped: bool,
    pub online: bool,
    levels: [u16; 3],
    scenario_start: u64,
    last_ms: u64,
    crank_phase: u64,
    revolutions: u16,
    event_time: u16,
}

impl Default for Simulator {
    fn default() -> Self {
        Self {
            sensor: Sensor::Power,
            scenario: Scenario::Steady,
            stopped: false,
            online: true,
            levels: [200, 90, 120],
            scenario_start: 0,
            last_ms: 0,
            crank_phase: 0,
            revolutions: 0,
            event_time: 0,
        }
    }
}

impl Simulator {
    pub fn base(&self) -> u16 {
        self.levels[self.sensor as usize]
    }

    pub fn adjust(&mut self, up: bool) {
        let value = &mut self.levels[self.sensor as usize];
        *value = if up { value.saturating_add(1).min(self.sensor.limit()) } else { value.saturating_sub(1) };
    }

    pub fn next_sensor(&mut self, now_ms: u64) {
        self.sensor = self.sensor.next();
        self.scenario = Scenario::Steady;
        self.scenario_start = now_ms;
        self.stopped = false;
        self.crank_phase = 0;
        self.revolutions = 0;
        self.event_time = 0;
        self.last_ms = now_ms;
    }

    pub fn next_scenario(&mut self, now_ms: u64) {
        self.scenario = match self.scenario {
            Scenario::Steady => Scenario::Ramp,
            Scenario::Ramp => Scenario::Intervals,
            Scenario::Intervals => Scenario::Steady,
        };
        self.scenario_start = now_ms;
    }

    pub fn value(&self, now_ms: u64) -> u16 {
        if self.stopped {
            return 0;
        }
        let elapsed = now_ms.saturating_sub(self.scenario_start);
        let percent = match self.scenario {
            Scenario::Steady => 100,
            Scenario::Ramp => {
                let phase = (elapsed % 40_000) as u32;
                50 + if phase < 20_000 { phase } else { 40_000 - phase } / 400
            }
            Scenario::Intervals => {
                if elapsed % 20_000 < 10_000 {
                    50
                } else {
                    100
                }
            }
        };
        (u32::from(self.base()) * percent / 100) as u16
    }

    pub fn rpm(&self, now_ms: u64) -> u16 {
        match self.sensor {
            Sensor::Cadence => self.value(now_ms),
            Sensor::Power if self.value(now_ms) > 0 => 90,
            _ => 0,
        }
    }

    /// Integrate crank motion independently of notifications and connections.
    pub fn tick(&mut self, now_ms: u64) {
        let elapsed = now_ms.saturating_sub(self.last_ms);
        self.last_ms = now_ms;
        let rpm = u64::from(self.rpm(now_ms));
        if rpm == 0 {
            return;
        }
        self.crank_phase += rpm * elapsed;
        let turns = self.crank_phase / 60_000;
        self.crank_phase %= 60_000;
        if turns > 0 {
            self.revolutions = self.revolutions.wrapping_add(turns as u16);
            // The event is the last completed revolution, not this notification's send time.
            self.event_time = ((now_ms * 1024 - self.crank_phase * 1024 / rpm) / 1000) as u16;
        }
    }

    pub fn measurement(&self, now_ms: u64) -> ([u8; 8], usize) {
        let mut bytes = [0; 8];
        let value = self.value(now_ms);
        let crank_offset = match self.sensor {
            Sensor::Power => {
                bytes[0] = 0x20;
                bytes[2..4].copy_from_slice(&value.to_le_bytes());
                4
            }
            Sensor::Cadence => {
                bytes[0] = 0x02;
                1
            }
            Sensor::HeartRate => {
                // Contact supported; stopped output simulates a removed strap.
                bytes[0] = if self.stopped { 0x04 } else { 0x06 };
                bytes[1] = value as u8;
                return (bytes, 2);
            }
        };
        bytes[crank_offset..crank_offset + 2].copy_from_slice(&self.revolutions.to_le_bytes());
        bytes[crank_offset + 2..crank_offset + 4].copy_from_slice(&self.event_time.to_le_bytes());
        (bytes, crank_offset + 4)
    }
}
