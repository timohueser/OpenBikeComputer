//! The board's [`Sounder`]: a piezo between the two PWM21 channels, played by [`buzzer_task`].

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_nrf::peripherals::{P1_06, P1_07, PWM21};
use embassy_nrf::pwm::{DutyCycle, SimpleConfig, SimplePwm};
use embassy_nrf::Peri;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use obc_ports::{Note, Sounder, Volume};

/// The next pattern. When two arrive before the task runs, the `Signal` keeps only the newer one.
static PATTERN: Signal<CriticalSectionRawMutex, (&'static [Note], Volume)> = Signal::new();

/// A duty value that holds the line low for the whole period.
const LOW: DutyCycle = DutyCycle::inverted(0);

/// The ride loop's handle to the buzzer task. `play` only signals, so it never blocks the loop.
pub(crate) struct Buzzer(());

impl Buzzer {
    /// Arm PWM21 on the provisional piezo pins and spawn the task that owns it.
    ///
    /// The PWM stays disabled between notes. A disabled PWM hands the pads back to their GPIO
    /// level, which `SimpleConfig`'s idle level sets low, so a rest and the idle state are both
    /// pins low with no DC across the piezo.
    pub(crate) fn new(
        spawner: Spawner,
        pwm: Peri<'static, PWM21>,
        a: Peri<'static, P1_06>,
        b: Peri<'static, P1_07>,
    ) -> Self {
        // The default 1 MHz PWM clock resolves a 3 kHz note to 0.1 % and reaches down to 31 Hz.
        let pwm = SimplePwm::new_2ch(pwm, a, b, &SimpleConfig::default());
        pwm.disable();
        spawner.spawn(defmt::unwrap!(buzzer_task(pwm)));
        Buzzer(())
    }
}

impl Sounder for Buzzer {
    /// The PWM really drives two pins. On a DK with no piezo wired to them, nothing is heard.
    fn available(&self) -> bool {
        true
    }

    fn play(&mut self, notes: &'static [Note], volume: Volume) {
        PATTERN.signal((notes, volume));
    }
}

/// Play each signalled pattern to its end, or until a newer one replaces it.
///
/// It runs on the thread-mode executor, because starting a note ends in `SimplePwm`'s busy-wait on
/// `SEQEND`: one period of the note, so under 1 ms for any note above 1 kHz.
#[embassy_executor::task]
async fn buzzer_task(mut pwm: SimplePwm<'static>) -> ! {
    loop {
        let mut next = PATTERN.wait().await;
        while let Either::Second(newer) = select(play(&mut pwm, next.0, next.1), PATTERN.wait()).await {
            next = newer;
        }
    }
}

/// Every step starts from a stopped PWM, so a pattern cut off mid-note leaves nothing behind and a
/// new count-top never meets a counter that already runs above it.
async fn play(pwm: &mut SimplePwm<'static>, notes: &'static [Note], volume: Volume) {
    for note in notes {
        pwm.disable();
        if note.hz != 0 {
            tone(pwm, note.hz, volume);
        }
        Timer::after_millis(u64::from(note.ms)).await;
    }
    pwm.disable();
}

/// A 50 % square wave at `hz` on channel 0. Loud drives channel 1 in opposite phase, which doubles
/// the swing across the piezo; Quiet holds channel 1 low.
///
/// A normal duty value is high from the compare point to the top, an inverted one is high below it.
fn tone(pwm: &mut SimplePwm<'static>, hz: u16, volume: Volume) {
    pwm.set_period(u32::from(hz));
    let half = pwm.max_duty() / 2;
    let b = match volume {
        Volume::Loud => DutyCycle::normal(half),
        Volume::Quiet => LOW,
    };
    pwm.enable();
    pwm.set_all_duties([DutyCycle::inverted(half), b, LOW, LOW]);
}
