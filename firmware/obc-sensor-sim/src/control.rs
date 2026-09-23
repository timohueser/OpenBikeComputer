/// Crank revolution data and offset compensation; force-based measurement context.
pub const POWER_FEATURES: u32 = (1 << 3) | (1 << 9);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Response {
    pub bytes: [u8; 5],
    pub len: usize,
    pub calibrating: bool,
}

#[derive(Default)]
pub struct Control {
    busy: bool,
}

impl Control {
    /// Errors are ATT codes; accepted procedures complete through an indication.
    pub fn start(&mut self, data: &[u8], subscribed: bool, stopped: bool) -> Result<Response, u8> {
        if !subscribed {
            return Err(0xfd);
        }
        if self.busy {
            return Err(0xfe);
        }
        let Some(&opcode) = data.first() else { return Err(0x0d) };
        let status = match (opcode, data.len(), stopped) {
            (0x0c, 1, true) => 1,
            (0x0c, 1, false) => 4,
            (0x0c, _, _) => 3,
            _ => 2,
        };
        self.busy = true;
        Ok(Response {
            bytes: [0x20, opcode, status, 0, 0],
            len: if status == 1 { 5 } else { 3 },
            calibrating: opcode == 0x0c && data.len() == 1,
        })
    }

    pub fn confirmed(&mut self) {
        self.busy = false;
    }
}
