#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Short,
    Long,
    Step,
}

#[derive(Clone, Copy, Default)]
pub struct Button {
    raw: bool,
    down: bool,
    changed: u64,
    pressed: u64,
    next_repeat: u64,
    long_sent: bool,
}

impl Button {
    /// Debounce for 30 ms. Adjustment keys repeat every 60 ms after a 500 ms hold.
    pub fn update(&mut self, raw: bool, now: u64, repeat: bool) -> Option<Press> {
        if raw != self.raw {
            self.raw = raw;
            self.changed = now;
        }
        if self.down != raw && now.saturating_sub(self.changed) >= 30 {
            self.down = raw;
            if raw {
                self.pressed = now;
                self.next_repeat = now + 500;
                self.long_sent = false;
                return repeat.then_some(Press::Step);
            }
            return (!repeat && !self.long_sent).then_some(Press::Short);
        }
        if self.down && raw {
            if repeat && now >= self.next_repeat {
                self.next_repeat = now + 60;
                return Some(Press::Step);
            }
            if !repeat && !self.long_sent && now.saturating_sub(self.pressed) >= 1000 {
                self.long_sent = true;
                return Some(Press::Long);
            }
        }
        None
    }
}
