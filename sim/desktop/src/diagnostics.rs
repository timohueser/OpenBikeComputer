//! Optional JSONL observations of one headless session. Each record is written immediately.
use std::{cell::RefCell, fs::OpenOptions, io::Write, rc::Rc, time::Instant};

use obc_host_core::trace::{FeederCall, FeederKind, ObjectKind, TraceSink};
use serde_json::{json, Value};

#[derive(Clone, Default)]
pub(crate) struct Diagnostics(Option<Rc<RefCell<Output>>>);

struct Output {
    writer: Box<dyn Write>,
    error: Option<String>,
    sequence: u64,
    pass_started: Instant,
}

impl Diagnostics {
    pub fn open(path: Option<&str>) -> Result<Self, String> {
        let Some(path) = path else { return Ok(Self::default()) };
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("cannot create diagnostics {path}: {error}"))?;
        Ok(Self(Some(Rc::new(RefCell::new(Output {
            writer: Box::new(file),
            error: None,
            sequence: 0,
            pass_started: Instant::now(),
        })))))
    }

    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    pub fn record(&self, event: &str, data: Value) {
        let Some(output) = &self.0 else { return };
        let mut output = output.borrow_mut();
        if output.error.is_some() {
            return;
        }
        let record = json!({"seq": output.sequence, "event": event, "data": data});
        output.sequence += 1;
        if let Err(error) = writeln!(output.writer, "{record}") {
            output.error = Some(format!("cannot write diagnostics: {error}"));
        }
    }

    pub fn check(&self) -> Result<(), String> {
        self.0.as_ref().and_then(|output| output.borrow().error.clone()).map_or(Ok(()), Err)
    }

    fn elapsed_us(&self) -> u128 {
        self.0.as_ref().map_or(0, |output| output.borrow().pass_started.elapsed().as_micros())
    }
}

impl TraceSink for Diagnostics {
    fn pass_input(
        &mut self,
        now: obc_app::device_core::PassClock,
        gestures: &[obc_app::Gesture],
        outcomes: &obc_app::device_core::OutcomeSlots,
        facts: &obc_app::device_core::ExternalFacts,
        screen: &str,
    ) {
        self.record(
            "pass_input",
            json!({
                "ui_ms": now.ui.0, "ride_ms": now.ride.0, "screen": screen,
                "gestures": format!("{gestures:?}"), "outcomes": format!("{outcomes:?}"),
                "facts": format!("{facts:?}"),
            }),
        );
        if let Some(output) = &self.0 {
            output.borrow_mut().pass_started = Instant::now();
        }
    }

    fn pass_output(&mut self, plan: &obc_app::device_core::PassPlan, screen: &str) {
        self.record(
            "pass_output",
            json!({
                "screen": screen, "host_since_pass_us": self.elapsed_us(), "plan": format!("{plan:?}"),
            }),
        );
    }

    fn executed(&mut self, outcomes: &obc_app::device_core::OutcomeSlots, screen: &str) {
        self.record(
            "executed",
            json!({
                "screen": screen, "host_since_pass_us": self.elapsed_us(), "outcomes": format!("{outcomes:?}"),
            }),
        );
    }

    fn feeder(&mut self, call: FeederCall) {
        self.record("feeder", json!({"call": format!("{call:?}")}));
    }

    fn feeder_object(
        &mut self,
        feeder: FeederKind,
        scope: &'static str,
        kind: ObjectKind,
        id: obc_app::CatalogObjectId,
        len: usize,
    ) {
        self.record(
            "feeder",
            json!({"kind": format!("{feeder:?}"), "scope": scope,
            "object_kind": format!("{kind:?}"), "object": format!("{id:?}"), "len": len}),
        );
    }

    fn feeder_revision(&mut self, feeder: FeederKind, scope: &'static str, revision: u16, len: usize) {
        self.record("feeder", json!({"kind": format!("{feeder:?}"), "scope": scope, "revision": revision, "len": len}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_output_stays_failed() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("full"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let trace = Diagnostics(Some(Rc::new(RefCell::new(Output {
            writer: Box::new(Broken),
            error: None,
            sequence: 0,
            pass_started: Instant::now(),
        }))));
        trace.record("session", json!({}));
        trace.record("finished", json!({}));
        assert!(trace.check().unwrap_err().contains("full"));
        assert_eq!(trace.0.unwrap().borrow().sequence, 1);
    }
}
