//! Shared implementation for durable-object filename policies.

#[inline(always)]
pub(crate) fn is_admitted(short_ext: &[u8], uploaded_ext: &[u8], long: Option<&str>, sideload_ext: &[u8]) -> bool {
    if long.is_some_and(|name| name.starts_with('.')) {
        return false;
    }
    short_ext == uploaded_ext
        || long.is_some_and(|name| {
            let bytes = name.as_bytes();
            bytes.len() >= sideload_ext.len()
                && bytes[bytes.len() - sideload_ext.len()..].eq_ignore_ascii_case(sideload_ext)
        })
}

#[inline(always)]
pub(crate) fn uploaded_id(short_base: &[u8], short_ext: &[u8], prefix: &[u8], uploaded_ext: &[u8]) -> Option<u16> {
    if short_ext != uploaded_ext {
        return None;
    }
    decimal_id(short_base.strip_prefix(prefix)?)
}

#[inline(always)]
fn decimal_id(digits: &[u8]) -> Option<u16> {
    if digits.is_empty() {
        return None;
    }
    let mut id = 0u32;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            return None;
        }
        id = id * 10 + u32::from(digit - b'0');
        if id > u32::from(u16::MAX) {
            return None;
        }
    }
    Some(id as u16)
}
