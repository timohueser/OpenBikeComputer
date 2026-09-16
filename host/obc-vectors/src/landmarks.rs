//! An authored OBCM §9 section, independent of the map encoder.

pub fn section() -> Vec<u8> {
    let mut bytes = vec![0; 100];
    bytes[..4].copy_from_slice(&1u32.to_le_bytes());
    bytes[4..6].copy_from_slice(&84u16.to_le_bytes());
    bytes[6..8].copy_from_slice(&1u16.to_le_bytes());
    bytes[8..12].copy_from_slice(&100u32.to_le_bytes());
    bytes[16..24].copy_from_slice(&123u64.to_le_bytes());
    bytes[24..28].copy_from_slice(&8_000_000i32.to_le_bytes());
    bytes[28..32].copy_from_slice(&47_000_000i32.to_le_bytes());
    bytes[32] = 1;
    bytes[36..38].copy_from_slice(&0xffffu16.to_le_bytes());
    for (at, payload) in [(68, b"Burg".to_vec()), (76, article())] {
        let offset = bytes.len() as u32;
        bytes[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
    }
    let length = bytes.len() as u32;
    bytes[12..16].copy_from_slice(&length.to_le_bytes());
    bytes
}

fn bundle(fields: &[&str]) -> Vec<u8> {
    let mut bytes = (fields.len() as u16).to_le_bytes().to_vec();
    let mut offset = 2 + (fields.len() + 1) * 4;
    for field in fields {
        bytes.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += field.len();
    }
    bytes.extend_from_slice(&(offset as u32).to_le_bytes());
    for field in fields {
        bytes.extend_from_slice(field.as_bytes());
    }
    bytes
}

fn article() -> Vec<u8> {
    let mut bytes = vec![0; 24];
    bytes[..2].copy_from_slice(b"de");
    bytes[2] = 1;
    bytes[4..6].copy_from_slice(b"de");
    bytes[6] = 1;
    for (at, fields) in [
        (8, vec!["Eine Burg."]),
        (16, vec!["https://example.org/article", "1", "CC BY-SA 4.0", "Autoren", "Quelle: Autoren."]),
    ] {
        let payload = bundle(&fields);
        let offset = bytes.len() as u32;
        bytes[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend(payload);
    }
    bytes
}
