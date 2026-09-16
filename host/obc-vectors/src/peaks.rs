//! Authored OBCM §10 bytes: three summits, two article identities and multilingual text.
fn fields(values: &[&str]) -> Vec<u8> {
    let mut b = (values.len() as u16).to_le_bytes().to_vec();
    let mut offset = 2 + (values.len() + 1) * 4;
    for v in values {
        b.extend((offset as u32).to_le_bytes());
        offset += v.len();
    }
    b.extend((offset as u32).to_le_bytes());
    for v in values {
        b.extend(v.as_bytes());
    }
    b
}
fn article(variants: &[(&str, &str)]) -> Vec<u8> {
    let mut b = vec![0; 4 + variants.len() * 20];
    b[..2].copy_from_slice(variants[0].0.as_bytes());
    b[2] = variants.len() as u8;
    for (i, (language, text)) in variants.iter().enumerate() {
        let at = 4 + i * 20;
        b[at..at + 2].copy_from_slice(language.as_bytes());
        b[at + 2] = 1;
        for (slot, data) in [
            fields(&[text]),
            fields(&["https://example.org/revision", "1", "CC BY-SA 4.0", "Authors", "Source: Authors"]),
        ]
        .into_iter()
        .enumerate()
        {
            let pos = at + 4 + slot * 8;
            let off = b.len() as u32;
            b[pos..pos + 4].copy_from_slice(&off.to_le_bytes());
            b[pos + 4..pos + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
            b.extend(data);
        }
    }
    b
}
pub fn section() -> Vec<u8> {
    let payload = 24 + 3 * 44 + 2 * 64;
    let mut b = vec![0; payload];
    b[0] = 1;
    b[2] = 64;
    b[4] = 3;
    b[8] = 2;
    b[12..16].copy_from_slice(&(payload as u32).to_le_bytes());
    for (i, (node, id, index)) in [(101u64, 1u8, 0u32), (102, 1, 0), (103, 2, 1)].into_iter().enumerate() {
        let at = 24 + i * 44;
        b[at..at + 8].copy_from_slice(&((1u64 << 62) | node).to_le_bytes());
        b[at + 8..at + 40].fill(id);
        b[at + 40..at + 44].copy_from_slice(&index.to_le_bytes());
    }
    for (i, (name, text)) in [
        ("Shared massif", article(&[("en", "A shared mountain."), ("de", "Ein gemeinsamer Berg.")])),
        ("Remote summit", article(&[("fr", "Une montagne.")])),
    ]
    .into_iter()
    .enumerate()
    {
        let at = 24 + 3 * 44 + i * 64;
        let id = (i + 1) as u8;
        b[at..at + 32].fill(id);
        for (slot, data) in [name.as_bytes().to_vec(), text].into_iter().enumerate() {
            let off = b.len() as u32;
            let len = 33 + data.len() as u32;
            let pos = at + 32 + slot * 8;
            b[pos..pos + 4].copy_from_slice(&off.to_le_bytes());
            b[pos + 4..pos + 8].copy_from_slice(&len.to_le_bytes());
            b.extend([id; 32]);
            b.push(slot as u8);
            b.extend(data);
        }
    }
    let len = b.len() as u32;
    b[16..20].copy_from_slice(&len.to_le_bytes());
    b
}
