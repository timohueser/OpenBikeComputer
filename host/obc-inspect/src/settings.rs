//! The persisted settings blob, decoded by `obc-app`.
//!
//! The blob carries no magic: byte 0 is its layout version. The file name selects this printer.

use obc_app::settings;

use crate::report::Report;

pub fn report(blob: &[u8]) -> Result<Report, String> {
    let stored = *blob.first().ok_or("the settings blob is empty")?;
    let decoded = settings::decode(blob).ok_or_else(|| {
        format!(
            "the settings blob does not decode: version {stored} against the supported {}..={}, \
             a short blob, or a failed CRC",
            settings::MIN_SUPPORTED,
            settings::VERSION
        )
    })?;

    let mut out = Report::new();
    out.put("version", stored).put("bytes", blob.len());
    let mut fields = Report::new();
    decoded.for_each_field(|name, value| {
        fields.put(name, format!("{value:?}"));
    });
    out.group("fields", fields);
    Ok(out)
}
