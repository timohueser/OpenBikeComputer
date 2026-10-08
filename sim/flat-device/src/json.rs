//! The two answers that cross to JavaScript as documents rather than as bytes.
//!
//! Hand-written because there are two of them and both are flat: a serialization crate in this
//! graph would be a dependency the shipped bridges do not carry, for seven fields. The `u64`s cross
//! as strings, because the ids and lengths on the other side are `bigint`s and a JSON number is a
//! `double`.

use obc_storage::flat::EntryMeta;

use crate::TracedRequest;

/// The catalog as the device would list it, in catalog order.
pub fn catalog_json(entries: &[EntryMeta]) -> String {
    let rows: Vec<String> = entries
        .iter()
        .map(|entry| {
            format!(
                concat!(
                    r#"{{"objectId":"{}","revision":"{}","payloadLength":"{}","payloadCrc32":{},"#,
                    r#""kind":{},"flags":{},"displayName":{}}}"#
                ),
                entry.id.0,
                entry.revision.0,
                entry.payload_len,
                entry.payload_crc,
                entry.kind as u16,
                entry.flags.bits(),
                string(entry.name.as_str().unwrap_or_default()),
            )
        })
        .collect();
    format!("[{}]", rows.join(","))
}

/// Every traced control record, in the order the device was handed them.
pub fn trace_json(trace: &[TracedRequest]) -> String {
    let rows: Vec<String> =
        trace.iter().map(|one| format!(r#"{{"opcode":{},"requestId":{}}}"#, one.opcode, one.request_id)).collect();
    format!("[{}]", rows.join(","))
}

/// One JSON string literal. Display names are rider text, so the escape has to be real.
fn string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < ' ' => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_name_crosses_as_a_real_json_string() {
        assert_eq!(string(r#"a "quoted" \ name"#), r#""a \"quoted\" \\ name""#);
        assert_eq!(string("a\u{1}b"), "\"a\\u0001b\"");
    }
}
