/// Why a control-plane descriptor failed to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    Truncated,
    UnknownOp(u8),
    UnknownStatus(u8),
    /// A fixed-width field decoded correctly but names a value outside the command's contract.
    Bounds,
}

/// The result of a `command` write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CommandStatus {
    Ok = 0,
    UnknownCommand = 1,
    NotFound = 2,
    Busy = 3,
    Error = 4,
}

impl CommandStatus {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            0 => Self::Ok,
            1 => Self::UnknownCommand,
            2 => Self::NotFound,
            3 => Self::Busy,
            4 => Self::Error,
            other => return Err(DescriptorError::UnknownStatus(other)),
        })
    }
}

/// The result notified after a `command` write (`msg = 3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandResult {
    /// Echoes the command byte.
    pub command: u8,
    pub status: CommandStatus,
    /// Command-specific; 0 unless documented.
    pub detail: u8,
}

impl CommandResult {
    pub fn new(command: u8, status: CommandStatus) -> Self {
        Self { command, status, detail: 0 }
    }

    /// `detail` carries a command's own extra answer byte.
    pub fn with_detail(command: u8, status: CommandStatus, detail: u8) -> Self {
        Self { command, status, detail }
    }
}

/// `installFw`: the `cmd` byte only. Asks the device to install the staged update package.
pub const CMD_INSTALL_FW: u8 = 3;
/// `forgetBond`: the `cmd` byte only. The device answers `commandResult(ok)`, then clears its side
/// of the bond, drops the link, and advertises for open pairing again. The gated `command`
/// characteristic needs the encrypted link, so only the bonded phone can send it.
pub const CMD_FORGET_BOND: u8 = 4;
pub const CMD_SET_CLOCK: u8 = 5;
pub const SET_CLOCK_MIN_UTC: u32 = 1_577_836_800;
/// The magnitude bound on a `setClock` UTC offset: 14 h, the real-world offset span. A write
/// outside it is rejected.
pub const SET_CLOCK_MAX_OFFSET_MIN: i16 = 14 * 60;

/// Map the device state at the BLE edge to the `installFw` `commandResult.status`. The command
/// never installs on its own: a physical confirm on the device is always necessary, and whether a
/// package is staged is that flow's own answer rather than a second one given here.
pub const fn install_fw_reply(busy: bool) -> CommandStatus {
    if busy {
        CommandStatus::Busy
    } else {
        CommandStatus::Ok
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetClock {
    /// The phone's current UTC time, unix seconds.
    pub utc: u32,
    /// The phone's current local UTC offset in minutes (`+02:00` → `120`), DST already folded in.
    pub offset_min: i16,
}

impl SetClock {
    /// The wire length: `cmd u8 · utc u32 · offset_min i16`.
    pub const ENCODED_LEN: usize = 7;

    /// Decode a full `command` write, starting at the command byte. The write must be exactly 7
    /// bytes: `setClock` has no variable tail, so trailing bytes are malformed.
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        let bytes: [u8; Self::ENCODED_LEN] = data.try_into().map_err(|_| DescriptorError::Truncated)?;
        if bytes[0] != CMD_SET_CLOCK {
            return Err(DescriptorError::UnknownOp(bytes[0]));
        }
        let utc = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let offset_min = i16::from_le_bytes([bytes[5], bytes[6]]);
        // `unsigned_abs`, not `abs`: the field can decode to `i16::MIN`, whose `abs` overflows and
        // panics in a debug build. `unsigned_abs` gives 32768, over the bound, so the decode rejects.
        if utc < SET_CLOCK_MIN_UTC || offset_min.unsigned_abs() > SET_CLOCK_MAX_OFFSET_MIN as u16 {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self { utc, offset_min })
    }

    /// Returns the written length, or `None` for a too-small buffer. The firmware only decodes;
    /// this is here for the shared-vector tests and the app-side codec.
    pub fn encode(utc: u32, offset_min: i16, out: &mut [u8]) -> Option<usize> {
        if out.len() < Self::ENCODED_LEN {
            return None;
        }
        out[0] = CMD_SET_CLOCK;
        out[1..5].copy_from_slice(&utc.to_le_bytes());
        out[5..7].copy_from_slice(&offset_min.to_le_bytes());
        Some(Self::ENCODED_LEN)
    }
}
/// One `status` characteristic notification: a `u8` discriminator and a fixed body. The app ignores
/// unknown discriminators and never fails the link over one. This is the only device-to-app control
/// channel, so every message shares one subscription and one ordering domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMessage {
    /// `msg = 3`, 4 bytes.
    CommandResult(CommandResult),
}

impl StatusMessage {
    /// A notify buffer of this size fits any message.
    pub const MAX_ENCODED_LEN: usize = 4;

    /// Encode into a fixed buffer; the returned length is the slice to notify (`&buf[..len]`).
    pub fn encode(&self) -> ([u8; Self::MAX_ENCODED_LEN], usize) {
        let mut b = [0u8; Self::MAX_ENCODED_LEN];
        let len = match self {
            Self::CommandResult(c) => {
                b[0] = 3;
                b[1] = c.command;
                b[2] = c.status.as_u8();
                b[3] = c.detail;
                4
            }
        };
        (b, len)
    }

    /// `Ok(None)` for an unknown discriminator; `Err` only for a known one with a malformed body.
    pub fn decode(data: &[u8]) -> Result<Option<Self>, DescriptorError> {
        let Some(&msg) = data.first() else {
            return Err(DescriptorError::Truncated);
        };
        Ok(Some(match msg {
            3 => {
                if data.len() < 4 {
                    return Err(DescriptorError::Truncated);
                }
                Self::CommandResult(CommandResult {
                    command: data[1],
                    status: CommandStatus::from_u8(data[2])?,
                    detail: data[3],
                })
            }
            _ => return Ok(None),
        }))
    }
}

/// The Config object, the one object small enough to cross GATT whole-blob instead of the CoC. A
/// rename is a Config write with a changed `name`. Append-only: readers ignore unknown trailing
/// bytes and an absent trailing field means the device default. `name` borrows the wire buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config<'a> {
    /// UTF-8, at most [`Config::MAX_NAME`] bytes.
    pub name: &'a [u8],
    /// `0 = metric · 1 = imperial`.
    pub units: u8,
}

impl<'a> Config<'a> {
    /// Matches the OBCR route-name cap.
    pub const MAX_NAME: usize = 48;
    pub const MAX_ENCODED: usize = 128;
    /// The smallest well-formed blob: `name_len` (2) + empty name + `units` (1).
    pub const MIN_ENCODED: usize = 3;

    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let len = 2 + self.name.len() + 1;
        if self.name.len() > Self::MAX_NAME || len > Self::MAX_ENCODED || out.len() < len {
            return None;
        }
        out[0..2].copy_from_slice(&(self.name.len() as u16).to_le_bytes());
        out[2..2 + self.name.len()].copy_from_slice(self.name);
        out[2 + self.name.len()] = self.units;
        Some(len)
    }

    pub fn decode(data: &'a [u8]) -> Option<Self> {
        if data.len() < Self::MIN_ENCODED || data.len() > Self::MAX_ENCODED {
            return None;
        }
        let name_len = u16::from_le_bytes([data[0], data[1]]) as usize;
        if name_len > Self::MAX_NAME || 2 + name_len + 1 > data.len() {
            return None;
        }
        Some(Self { name: &data[2..2 + name_len], units: data[2 + name_len] })
    }
}
