//! Authored article bundles, independent of the production encoder.
pub type Article<'a> = ([u8; 2], &'a [&'a str], &'a [&'a str]);
pub fn bundle(default: [u8; 2], variants: &[Article<'_>]) -> Vec<u8> {
    let mut bytes = vec![0; 4 + variants.len() * 20];
    bytes[..2].copy_from_slice(&default);
    bytes[2..4].copy_from_slice(&(variants.len() as u16).to_le_bytes());
    for (i, (language, text, credits)) in variants.iter().enumerate() {
        let at = 4 + i * 20;
        bytes[at..at + 2].copy_from_slice(language);
        bytes[at + 2] = text.len() as u8;
        for (offset, fields) in [(at + 4, *text), (at + 12, *credits)] {
            let start = bytes.len();
            bytes.extend_from_slice(&(fields.len() as u16).to_le_bytes());
            let mut position = 2 + (fields.len() + 1) * 4;
            bytes.extend_from_slice(&(position as u32).to_le_bytes());
            for field in fields {
                position += field.len();
                bytes.extend_from_slice(&(position as u32).to_le_bytes());
            }
            for field in fields {
                bytes.extend_from_slice(field.as_bytes());
            }
            let len = bytes.len() - start;
            bytes[offset..offset + 4].copy_from_slice(&(start as u32).to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&(len as u32).to_le_bytes());
        }
    }
    bytes
}
