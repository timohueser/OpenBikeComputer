//! The FIT container: the file header, definition and data messages in both byte orders,
//! compressed timestamps and the CRC.

use crate::Error;

const HEADER_SIZE: usize = 14;
const PROTOCOL_1_0: u8 = 0x10;
/// The profile that the message and field numbers of this crate come from: 21.221.
const PROFILE: u16 = 21_221;
const TIMESTAMP: u8 = 253;

/// The FIT CRC-16, over the whole file up to the CRC itself.
pub fn crc(bytes: &[u8]) -> u16 {
    const TABLE: [u16; 16] = [
        0x0000, 0xCC01, 0xD801, 0x1400, 0xF001, 0x3C00, 0x2800, 0xE401, 0xA001, 0x6C00, 0x7800, 0xB401, 0x5000, 0x9C01,
        0x8801, 0x4400,
    ];
    bytes.iter().fold(0, |mut crc, &byte| {
        for nibble in [byte & 0xF, byte >> 4] {
            let tmp = TABLE[usize::from(crc & 0xF)];
            crc = (crc >> 4) ^ tmp ^ TABLE[usize::from(nibble)];
        }
        crc
    })
}

struct Field {
    num: u8,
    size: usize,
    base: u8,
}

struct Definition {
    global: u16,
    big_endian: bool,
    fields: Vec<Field>,
    /// Developer fields are skipped, so only their byte count matters.
    size: usize,
}

/// One data message, with its time from field 253 or from a compressed timestamp header.
pub struct Message<'a> {
    pub global: u16,
    pub time: Option<u32>,
    definition: &'a Definition,
    data: &'a [u8],
}

impl Message<'_> {
    fn field(&self, num: u8) -> Option<(&Field, &[u8])> {
        let mut at = 0;
        for field in &self.definition.fields {
            if field.num == num {
                return Some((field, &self.data[at..at + field.size]));
            }
            at += field.size;
        }
        None
    }

    /// The first value of an integer field, or `None` when it is absent or holds the invalid value of its type.
    pub fn int(&self, num: u8) -> Option<i64> {
        let (field, bytes) = self.field(num)?;
        let (size, signed, invalid): (usize, bool, u64) = match field.base {
            0 | 2 | 13 => (1, false, 0xFF),
            1 => (1, true, 0x7F),
            3 => (2, true, 0x7FFF),
            4 => (2, false, 0xFFFF),
            5 => (4, true, 0x7FFF_FFFF),
            6 => (4, false, 0xFFFF_FFFF),
            10 => (1, false, 0),
            11 => (2, false, 0),
            12 => (4, false, 0),
            14 => (8, true, 0x7FFF_FFFF_FFFF_FFFF),
            15 => (8, false, u64::MAX),
            16 => (8, false, 0),
            _ => return None,
        };
        let bytes = bytes.get(..size)?;
        let raw = if self.definition.big_endian {
            bytes.iter().fold(0u64, |raw, &b| raw << 8 | u64::from(b))
        } else {
            bytes.iter().rev().fold(0u64, |raw, &b| raw << 8 | u64::from(b))
        };
        if raw == invalid {
            return None;
        }
        let shift = 64 - 8 * size as u32;
        Some(if signed { ((raw << shift) as i64) >> shift } else { raw as i64 })
    }

    /// A string field up to its first zero byte.
    pub fn text(&self, num: u8) -> Option<String> {
        let (field, bytes) = self.field(num)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        (field.base == 7 && end > 0).then(|| String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

/// The data part of a file: after the header, up to the size that the header gives or the end of
/// the bytes. A device that loses power can leave the size at 0.
fn data(bytes: &[u8]) -> Result<&[u8], Error> {
    let size = usize::from(*bytes.first().ok_or(Error::NotFit)?);
    if !matches!(size, 12 | 14) || bytes.len() < size || &bytes[8..12] != b".FIT" {
        return Err(Error::NotFit);
    }
    // A header CRC of 0 means that the writer did not set it.
    let header_crc = if size == 14 { u16::from_le_bytes([bytes[12], bytes[13]]) } else { 0 };
    if header_crc != 0 && header_crc != crc(&bytes[..12]) {
        return Err(Error::NotFit);
    }
    let data_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let end = if data_size == 0 { bytes.len() } else { bytes.len().min(size.saturating_add(data_size)) };
    Ok(&bytes[size..end])
}

fn definition(data: &[u8], developer: bool) -> Option<(Definition, usize)> {
    let big_endian = *data.get(1)? == 1;
    let global = [*data.get(2)?, *data.get(3)?];
    let global = if big_endian { u16::from_be_bytes(global) } else { u16::from_le_bytes(global) };
    let count = usize::from(*data.get(4)?);
    let mut at = 5;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let field = data.get(at..at + 3)?;
        fields.push(Field { num: field[0], size: usize::from(field[1]), base: field[2] & 0x1F });
        at += 3;
    }
    let mut size: usize = fields.iter().map(|f| f.size).sum();
    if developer {
        let count = usize::from(*data.get(at)?);
        at += 1;
        for _ in 0..count {
            size += usize::from(*data.get(at + 1)?);
            at += 3;
        }
        data.get(..at)?;
    }
    Some((Definition { global, big_endian, fields, size }, at))
}

/// Calls `visit` with each data message in file order. The CRC at the end is not checked: the
/// messages stop at the first byte that cannot be read, so a cut file gives what comes before the cut.
pub fn decode(bytes: &[u8], mut visit: impl FnMut(&Message)) -> Result<(), Error> {
    let data = data(bytes)?;
    let mut definitions: [Option<Definition>; 16] = Default::default();
    let mut last_time: Option<u32> = None;
    let mut at = 0;
    while let Some(&header) = data.get(at) {
        at += 1;
        let (local, compressed) = if header & 0x80 != 0 {
            (usize::from(header >> 5 & 0x3), Some(u32::from(header & 0x1F)))
        } else if header & 0x40 != 0 {
            let Some((definition, size)) = definition(&data[at..], header & 0x20 != 0) else { return Ok(()) };
            definitions[usize::from(header & 0x0F)] = Some(definition);
            at += size;
            continue;
        } else {
            (usize::from(header & 0x0F), None)
        };
        let Some(definition) = &definitions[local] else { return Ok(()) };
        let Some(data) = data.get(at..at + definition.size) else { return Ok(()) };
        at += definition.size;
        let mut message = Message { global: definition.global, time: None, definition, data };
        message.time = match compressed {
            // A time past the end of u32 is dropped.
            Some(offset) => last_time.and_then(|last| {
                let time = (last & !0x1F) + offset;
                if offset < last & 0x1F {
                    time.checked_add(0x20)
                } else {
                    Some(time)
                }
            }),
            None => message.int(TIMESTAMP).map(|t| t as u32),
        };
        last_time = message.time.or(last_time);
        visit(&message);
    }
    Ok(())
}

/// One field value for [`Writer::message`]. A writer leaves out a field that it has no value for.
#[derive(Clone, Copy)]
pub enum Value<'a> {
    Enum(u8),
    U8(u8),
    I8(i8),
    U16(u16),
    U32(u32),
    U32z(u32),
    I32(i32),
    Text(&'a str),
}

impl Value<'_> {
    fn base(&self) -> u8 {
        match self {
            Value::Enum(_) => 0x00,
            Value::I8(_) => 0x01,
            Value::U8(_) => 0x02,
            Value::U16(_) => 0x84,
            Value::I32(_) => 0x85,
            Value::U32(_) => 0x86,
            Value::Text(_) => 0x07,
            Value::U32z(_) => 0x8C,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match *self {
            Value::Enum(v) | Value::U8(v) => vec![v],
            Value::I8(v) => v.to_le_bytes().to_vec(),
            Value::U16(v) => v.to_le_bytes().to_vec(),
            Value::U32(v) | Value::U32z(v) => v.to_le_bytes().to_vec(),
            Value::I32(v) => v.to_le_bytes().to_vec(),
            Value::Text(text) => {
                let mut end = text.len().min(254);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                let mut bytes = text.as_bytes()[..end].to_vec();
                bytes.push(0);
                bytes
            }
        }
    }
}

/// Writes a little-endian FIT file. Each global message gets its own local type, and the writer
/// writes a new definition only when the fields of a message change.
pub struct Writer {
    out: Vec<u8>,
    definitions: Vec<Vec<u8>>,
}

impl Default for Writer {
    fn default() -> Self {
        Self { out: vec![0; HEADER_SIZE], definitions: Vec::new() }
    }
}

impl Writer {
    pub fn message(&mut self, global: u16, fields: &[(u8, Value)]) {
        let values: Vec<_> = fields.iter().map(|(_, value)| value.bytes()).collect();
        let mut definition = vec![0, 0];
        definition.extend(global.to_le_bytes());
        definition.push(fields.len() as u8);
        for ((num, value), bytes) in fields.iter().zip(&values) {
            definition.extend([*num, bytes.len() as u8, value.base()]);
        }
        let local = match self.definitions.iter().position(|d| d[2..4] == definition[2..4]) {
            Some(local) => local,
            None => {
                self.definitions.push(Vec::new());
                self.definitions.len() - 1
            }
        };
        assert!(local < 16, "a FIT file has 16 local message types");
        if self.definitions[local] != definition {
            self.out.push(0x40 | local as u8);
            self.out.extend(&definition);
            self.definitions[local] = definition;
        }
        self.out.push(local as u8);
        values.iter().for_each(|bytes| self.out.extend(bytes));
    }

    pub fn finish(mut self) -> Vec<u8> {
        let data_size = (self.out.len() - HEADER_SIZE) as u32;
        let mut header = vec![HEADER_SIZE as u8, PROTOCOL_1_0];
        header.extend(PROFILE.to_le_bytes());
        header.extend(data_size.to_le_bytes());
        header.extend(b".FIT");
        header.extend(crc(&header).to_le_bytes());
        self.out[..HEADER_SIZE].copy_from_slice(&header);
        let crc = crc(&self.out);
        self.out.extend(crc.to_le_bytes());
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file of hand-written messages, with the header size and data size filled in.
    fn file(data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![12, PROTOCOL_1_0, 0, 0];
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(b".FIT");
        bytes.extend(data);
        bytes
    }

    #[test]
    fn compressed_timestamps_count_on_from_the_last_full_time() {
        // Local 0: record with timestamp (uint32) and heart rate. Local 1: record with heart rate only.
        let mut data = vec![0x40, 0, 0, 20, 0, 2, 253, 4, 0x86, 3, 1, 2];
        data.extend([0x41, 0, 0, 20, 0, 1, 3, 1, 2]);
        data.extend([0x00, 0x3E, 0, 0, 0, 100]); // time 62: its low five bits are 30
        data.extend([0x80 | 1 << 5 | 31, 101]); // offset 31 >= 30: time 63
        data.extend([0x80 | 1 << 5 | 2, 102]); // offset 2 < 31: the counter wraps, time 66
        let mut seen = Vec::new();
        decode(&file(&data), |m| seen.push((m.time, m.int(3)))).unwrap();
        assert_eq!(seen, [(Some(62), Some(100)), (Some(63), Some(101)), (Some(66), Some(102))]);
    }

    #[test]
    fn a_compressed_timestamp_past_the_end_of_u32_is_dropped() {
        let mut data = vec![0x40, 0, 0, 20, 0, 1, 253, 4, 0x86];
        data.extend([0x41, 0, 0, 20, 0, 1, 3, 1, 2]);
        data.extend([0x00, 0xFE, 0xFF, 0xFF, 0xFF]);
        data.extend([0x80 | 1 << 5 | 1, 100]); // offset 1 < 30: the counter wraps past u32
        let mut seen = Vec::new();
        decode(&file(&data), |m| seen.push(m.time)).unwrap();
        assert_eq!(seen, [Some(0xFFFF_FFFE), None]);
    }

    #[test]
    fn reads_big_endian_messages_and_skips_developer_fields() {
        // Header 0x60: a definition with developer fields. Global 20 in big-endian order, one
        // sint32 field and one developer field of 3 bytes.
        let mut data = vec![0x60, 0, 1, 0, 20, 1, 0, 4, 0x85, 1, 0, 3, 0];
        data.extend([0x00, 0xFF, 0xFF, 0xFF, 0xFE, 9, 9, 9]);
        data.extend([0x00, 0x7F, 0xFF, 0xFF, 0xFF, 9, 9, 9]);
        let mut seen = Vec::new();
        decode(&file(&data), |m| seen.push((m.global, m.int(0)))).unwrap();
        assert_eq!(seen, [(20, Some(-2)), (20, None)]);
    }

    #[test]
    fn a_wrong_header_crc_is_not_fit_and_a_header_crc_of_zero_is_accepted() {
        let mut writer = Writer::default();
        writer.message(20, &[(3, Value::U8(90))]);
        let mut bytes = writer.finish();
        assert!(decode(&bytes, |_| {}).is_ok());
        bytes[12..14].copy_from_slice(&[0, 0]);
        assert!(decode(&bytes, |_| {}).is_ok());
        bytes[12] = 1;
        assert_eq!(decode(&bytes, |_| {}), Err(Error::NotFit));
        assert_eq!(decode(b"<?xml", |_| {}), Err(Error::NotFit));
    }

    #[test]
    fn the_crc_is_crc_16_arc() {
        // The FIT CRC is the reflected CRC-16 with polynomial 0x8005 and start 0 (CRC-16/ARC).
        assert_eq!(crc(b"123456789"), 0xBB3D);
        let mut writer = Writer::default();
        writer.message(0, &[(0, Value::Enum(4)), (8, Value::Text("Grimsel"))]);
        assert_eq!(crc(&writer.finish()), 0, "the CRC over a written file and its own CRC is 0");
    }
}
