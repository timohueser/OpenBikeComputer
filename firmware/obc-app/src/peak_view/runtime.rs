//! Shared job lifecycle. Platforms own terrain storage and bounded work slices.
use crate::{screen::Screen, App};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Waiting,
    Building(u64),
    Ready,
    Unavailable,
}

pub struct Progress {
    pub complete: bool,
    pub revision: u64,
}

#[derive(Debug)]
pub struct Failed;

pub trait Platform {
    fn start(&mut self, app: &mut App, position: (i32, i32)) -> bool;
    fn step(&mut self, app: &mut App) -> Result<Progress, Failed>;
    fn cancel(&mut self);
}

#[derive(Default)]
pub struct Lifecycle {
    position: Option<(i32, i32)>,
    status: Status,
    active: bool,
    started_ms: u64,
    presented: bool,
    painted_ms: u64,
}
impl Lifecycle {
    pub fn busy(&self) -> bool {
        self.active && matches!(self.status, Status::Building(_))
    }

    /// Pause under drawers, text and credits; release storage for photos or other screens.
    pub fn reconcile(&mut self, app: &App) -> bool {
        self.active = matches!(app.top_screen(), Screen::PeakView(_));
        if !app.peak_view_retains_panorama() {
            *self = Self::default();
            return true;
        }
        if self.position.is_none() && !app.peak_view_needs_position() && app.state.user_fix.is_some() {
            // Opening the screen must schedule its first work slice without a spinner timer.
            self.status = Status::Building(0);
        }
        false
    }

    pub fn update(&mut self, app: &mut App, platform: &mut impl Platform, now_ms: u64) {
        if self.reconcile(app) {
            platform.cancel();
        }
        if !self.active {
            return;
        }
        if app.peak_view_needs_position() {
            return;
        }
        let Some(position) =
            app.peak_view_position().or_else(|| app.state.user_fix.map(|fix| (fix.lat, fix.lon))).or(self.position)
        else {
            self.status = Status::Waiting;
            app.set_peak_view_status(self.status);
            return;
        };
        if self.position.is_none_or(|old| !self.busy() && super::moved(old, position)) {
            platform.cancel();
            self.position = Some(position);
            self.started_ms = now_ms;
            self.painted_ms = now_ms;
            self.presented = false;
            app.state.peak_view_peak_count = 0;
            self.status = if platform.start(app, position) { Status::Building(0) } else { Status::Unavailable };
            app.set_peak_view_status(self.status);
        }
        if self.busy() {
            match platform.step(app) {
                Ok(progress) if progress.complete => self.status = Status::Ready,
                Ok(progress) if now_ms.saturating_sub(self.painted_ms) >= 500 => {
                    self.status = Status::Building(progress.revision);
                    self.painted_ms = now_ms;
                }
                Ok(_) => {}
                Err(Failed) => self.status = Status::Unavailable,
            }
        }
        app.set_peak_view_status(self.status);
    }

    /// Time from job start to its first presented frame. This is telemetry, never a display gate.
    pub fn note_presented(&mut self, app: &App, now_ms: u64) -> Option<u64> {
        if !self.presented
            && self.position.is_some()
            && matches!(app.top_screen(), Screen::PeakView(_))
            && matches!(self.status, Status::Building(_) | Status::Ready)
        {
            self.presented = true;
            Some(now_ms.saturating_sub(self.started_ms))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Job {
        starts: u8,
        complete: bool,
        fail: bool,
        revision: u64,
    }
    impl Platform for Job {
        fn start(&mut self, _: &mut App, _: (i32, i32)) -> bool {
            self.starts += 1;
            true
        }
        fn step(&mut self, _: &mut App) -> Result<Progress, Failed> {
            if self.fail {
                Err(Failed)
            } else {
                Ok(Progress { complete: self.complete, revision: self.revision })
            }
        }
        fn cancel(&mut self) {}
    }
    #[test]
    fn lifecycle_paces_progress_preserves_running_position_and_latches_failure() {
        let mut app = App::new(crate::AppState::new(0, 0, 1.0));
        app.state.peak_view_profile = Some(super::super::PeakViewProfile::at(0, 0, 0));
        app.show_peak_view();
        let mut lifecycle = Lifecycle::default();
        let mut job = Job::default();
        lifecycle.update(&mut app, &mut job, 0);
        assert_eq!(job.starts, 0);
        assert!(!lifecycle.busy(), "waiting for GPS does not request terrain work");
        let mut loc = crate::harness::support::OnceFix(Some(obc_ports::Fix::at(0, 0)));
        app.tick(obc_ports::RideClock(0), obc_ports::Sensors::new(&mut loc), None);
        lifecycle.reconcile(&app);
        assert!(lifecycle.busy(), "an available fix schedules the first slice before any spinner timer");
        assert_eq!(lifecycle.note_presented(&app, 5), None, "the pre-job hatch has no job timing yet");
        lifecycle.update(&mut app, &mut job, 10);
        assert_eq!(job.starts, 1);
        assert_eq!(lifecycle.status, Status::Building(0));
        assert_eq!(lifecycle.note_presented(&app, 20), Some(10));
        job.revision = 1;
        lifecycle.update(&mut app, &mut job, 509);
        assert_eq!(lifecycle.status, Status::Building(0));
        lifecycle.update(&mut app, &mut job, 510);
        assert_eq!(lifecycle.status, Status::Building(1));
        app.state.user_fix.as_mut().unwrap().lat = 1000;
        lifecycle.update(&mut app, &mut job, 600);
        assert_eq!(job.starts, 1);
        job.complete = true;
        lifecycle.update(&mut app, &mut job, 610);
        assert_eq!(lifecycle.status, Status::Ready);
        job.fail = true;
        lifecycle.update(&mut app, &mut job, 620);
        assert_eq!(job.starts, 2);
        assert_eq!(lifecycle.status, Status::Unavailable);
        lifecycle.update(&mut app, &mut job, 2000);
        assert_eq!(job.starts, 2);
        assert_eq!(lifecycle.note_presented(&app, 2010), None);
    }
}
