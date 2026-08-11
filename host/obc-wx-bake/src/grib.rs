//! Strict GRIB2 field decoding against a pinned source contract.
//!
//! The per-source geometry/template bytes this module enforces were measured and frozen by WX1
//! (`docs/decisions/WX1-weather-source-contracts.md`). Everything is fail-closed: a grid
//! definition, template, unit, interval or value outside the pinned contract fails the cycle —
//! it is never resampled, clamped or guessed into plausible weather.

use std::io::{Cursor, Read};

use chrono::{TimeZone, Utc};
use grib::{def::grib2::DataRepresentationTemplate, Grib2SubmessageDecoder};

pub const MAX_COMPRESSED_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_GRID_POINTS: usize = 30_000_000;

/// The exact Section-3 bytes of the ICON-EU regular-lat-lon single-level grid: 1,377 x 657 at
/// 0.0625 degrees from (29.5 N, 336.5 E), scanning +i west-east and +j south-north.
pub const ICON_EU_GRID_DEFINITION_HEX: &str = "00000dcdf10000000006ffffffffffffffffffffffffffffff000005610000029100000000ffffffff01c22260140e9520300433bea003b9aca00000f4240000f42440";

/// The exact Section-3 bytes of the MRMS CONUS grid: 7,000 x 3,500 at 0.01 degrees from
/// (54.995 N, 230.005 E) to (20.005001 N, 299.994998 E), scanning +i west-east and -j
/// north-south (WX1's pinned capture).
pub const MRMS_CONUS_GRID_DEFINITION_HEX: &str = "000175d720000000000201006128ee01006152b0010060ff2700001b5800000dac00000001000f4240034728380db59908300131408911e18f76000027100000271000";

/// The exact Section-3 bytes of the HRRR CONUS Lambert-conformal grid: 1,799 x 1,059 at 3 km,
/// first point (21.138123 N, 237.280472 E), LaD/Latin1/Latin2 38.5, LoV 262.5, scanning +i
/// west-east and +j south-north (WX1's pinned capture).
pub const HRRR_CONUS_GRID_DEFINITION_HEX: &str = "00001d11f50000001e06000000000000000000000000000000000007070000042301428acb0e249cd808024b76a00fa56ea0002dc6c0002dc6c00040024b76a0024b76a00000000000000000";

/// The exact Section-3 bytes of the GFS 0.25-degree global grid: 1,440 x 721 from (90 N, 0 E) to
/// (90 S, 359.75 E), scanning +i west-east and -j north-south (WX1's pinned capture).
pub const GFS_GLOBAL_GRID_DEFINITION_HEX: &str = "00000fd7a00000000006000000000000000000000000000000000005a0000002d100000000ffffffff055d4a800000000030855d4a80157159700003d0900003d09000";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpectedGrib {
    pub discipline: u8,
    pub category: u8,
    pub parameter: u8,
    pub grid_template: u16,
    pub expected_points: usize,
    pub expected_grid_definition_hex: &'static str,
    pub product_template: u16,
    pub representation_templates: &'static [u16],
    /// Exact encoded values this source documents as "no data" (MRMS's -1 missing and -3 no
    /// coverage). They survive decode as-is; every *other* negative value fails the cycle. A
    /// source without documented sentinels declares an empty slice.
    pub missing_sentinels: &'static [f32],
    /// Submessage counts this source's contracted object may contain. A selector may deliberately
    /// accept a duplicated upstream record as one consecutive span; point-field sources contract
    /// exactly one message.
    pub allowed_messages: &'static [usize],
    /// When more than one message is present they must decode bit-identically — the WX1 rule
    /// that refuses to pick an undocumented first or second occurrence.
    pub require_identical_messages: bool,
}

impl ExpectedGrib {
    /// Is `value` one of this source's documented no-data sentinels? NaN is always no-data: a
    /// bitmap-masked or otherwise absent cell must never decode as dry weather.
    pub fn is_missing(&self, value: f32) -> bool {
        value.is_nan() || self.missing_sentinels.contains(&value)
    }
}

/// One decoded field plus the byte-derived temporal identity a caller cross-checks its selected
/// lead against. Index/file names are never trusted as temporal identity.
#[derive(Clone, Debug)]
pub struct DecodedField {
    pub values: Vec<f32>,
    pub packing_increment: f32,
    pub reference_unix_seconds: i64,
    pub valid_start_unix_seconds: i64,
    pub valid_end_unix_seconds: i64,
}

pub fn decode_bzip2_field(compressed: &[u8], expected: &ExpectedGrib) -> Result<DecodedField, String> {
    if compressed.is_empty() || compressed.len() as u64 > MAX_COMPRESSED_BYTES {
        return Err("compressed GRIB size is outside the WX1 limits".into());
    }
    let mut decoder = bzip2_rs::DecoderReader::new(compressed).take(MAX_DECOMPRESSED_BYTES + 1);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).map_err(|error| format!("bzip2: {error}"))?;
    decode_field(&bytes, expected)
}

/// The MRMS container: one gzipped GRIB2 message. Decompression is capped before the reader ever
/// sees the body, so a zip bomb cannot outgrow the WX1 limits.
pub fn decode_gzip_field(compressed: &[u8], expected: &ExpectedGrib) -> Result<DecodedField, String> {
    if compressed.is_empty() || compressed.len() as u64 > MAX_COMPRESSED_BYTES {
        return Err("compressed GRIB size is outside the WX1 limits".into());
    }
    let mut decoder = flate2::read::GzDecoder::new(compressed).take(MAX_DECOMPRESSED_BYTES + 1);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).map_err(|error| format!("gzip: {error}"))?;
    decode_field(&bytes, expected)
}

/// Decode one contracted field.
///
/// The third-party GRIB decoder is fed unvalidated upstream bytes, and its complex-packing
/// spatial-differencing path is reachable with arithmetic that panics on mutated input (the WX6
/// fuzz suite finds it in seconds). A panic there must not be the difference between a failed
/// cycle and a crashed service, so it is caught and reported as what it is: a corrupt upstream.
pub fn decode_field(bytes: &[u8], expected: &ExpectedGrib) -> Result<DecodedField, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode_field_inner(bytes, expected))) {
        Ok(result) => result,
        Err(_) => Err("GRIB decoding panicked on malformed upstream bytes".into()),
    }
}

fn decode_field_inner(bytes: &[u8], expected: &ExpectedGrib) -> Result<DecodedField, String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err("decompressed GRIB size is outside the WX1 limits".into());
    }
    // Borrow the caller's bytes rather than copying them: the MRMS body is ~100 MB decompressed,
    // and a `to_vec()` here would double the transient on the largest decode the baker runs.
    let context = grib::from_reader(Cursor::new(bytes)).map_err(|error| format!("GRIB parse: {error}"))?;
    let mut decoded: Option<DecodedField> = None;
    let mut messages = 0usize;
    for (_, submessage) in context.iter() {
        messages += 1;
        if messages > *expected.allowed_messages.iter().max().unwrap_or(&1) {
            return Err("GRIB contains more messages than the source contract allows".into());
        }
        let discipline = submessage.indicator().discipline;
        let category = submessage.prod_def().parameter_category();
        let parameter = submessage.prod_def().parameter_number();
        if discipline != expected.discipline
            || category != Some(expected.category)
            || parameter != Some(expected.parameter)
        {
            return Err(format!("unexpected GRIB parameter d={discipline} c={category:?} p={parameter:?}"));
        }
        if submessage.grid_def().grid_tmpl_num() != expected.grid_template {
            return Err(format!(
                "grid template {} is outside the source contract",
                submessage.grid_def().grid_tmpl_num()
            ));
        }
        if encode_hex(submessage.grid_def().iter().copied()) != expected.expected_grid_definition_hex {
            return Err("GRIB grid definition bytes are outside the exact source contract".into());
        }
        let product_template = submessage.prod_def().prod_tmpl_num();
        if product_template != expected.product_template {
            return Err(format!("product template {product_template} is outside the source contract"));
        }
        let representation_template = submessage.repr_def().repr_tmpl_num();
        if !expected.representation_templates.contains(&representation_template) {
            return Err(format!("representation template {representation_template} is outside the source contract"));
        }
        let temporal = submessage.temporal_info();
        let reference_unix_seconds = temporal.ref_time.ok_or("GRIB reference time is invalid")?.timestamp();
        let forecast_time = temporal.forecast_time_target.ok_or("GRIB forecast time is invalid")?.timestamp();
        let (valid_start_unix_seconds, valid_end_unix_seconds) = if product_template == 8 {
            pdt48_interval(submessage.prod_def(), forecast_time, reference_unix_seconds)?
        } else if product_template == 0 {
            (forecast_time, forecast_time)
        } else {
            return Err("GRIB product template has no audited valid-interval parser".into());
        };
        let section5 = submessage.section5().map_err(|error| format!("GRIB section 5: {error}"))?;
        let simple = match &section5.payload.template {
            DataRepresentationTemplate::_5_2(template) => &template.simple,
            DataRepresentationTemplate::_5_3(template) => &template.simple,
            DataRepresentationTemplate::_5_41(template) => &template.simple,
            DataRepresentationTemplate::_5_42(template) => &template.simple,
            _ => return Err("selected GRIB representation has no audited packing increment".into()),
        };
        let packing_increment = 2_f32.powi(i32::from(simple.exp)) * 10_f32.powi(-i32::from(simple.dec));
        if !packing_increment.is_normal() || packing_increment <= 0.0 {
            return Err("GRIB packing increment is invalid".into());
        }
        let points = usize::try_from(submessage.grid_def().num_points()).map_err(|_| "GRIB point count overflows")?;
        if points != expected.expected_points || points > MAX_GRID_POINTS {
            return Err(format!("GRIB has {points} points, expected {}", expected.expected_points));
        }
        let values: Vec<f32> = Grib2SubmessageDecoder::from(submessage)
            .map_err(|error| format!("GRIB decoder: {error}"))?
            .dispatch()
            .map_err(|error| format!("GRIB unpack: {error}"))?
            .collect();
        if values.len() != points {
            return Err(format!("decoded {} values for a {points}-point grid", values.len()));
        }
        // Value sanity: a documented sentinel stays a sentinel, and everything else must be a
        // finite non-negative rate/accumulation. A source that documents no missing
        // representation may not smuggle one in as NaN.
        for value in &values {
            if expected.missing_sentinels.contains(value) {
                continue;
            }
            if value.is_nan() {
                if expected.missing_sentinels.is_empty() {
                    return Err("GRIB contains NaN in a source that documents no missing value".into());
                }
                continue;
            }
            if !value.is_finite() || *value < 0.0 {
                return Err(format!("GRIB contains an invalid precipitation value {value}"));
            }
        }
        match &decoded {
            None => {
                decoded = Some(DecodedField {
                    values,
                    packing_increment,
                    reference_unix_seconds,
                    valid_start_unix_seconds,
                    valid_end_unix_seconds,
                });
            }
            Some(first) => {
                // A repeated record is only acceptable when the source contract says so *and*
                // the repetition is genuinely indistinguishable after decode; never pick an
                // undocumented first or second occurrence.
                if !expected.require_identical_messages {
                    return Err("GRIB repeats a field the source contract does not duplicate".into());
                }
                if first.reference_unix_seconds != reference_unix_seconds
                    || first.valid_start_unix_seconds != valid_start_unix_seconds
                    || first.valid_end_unix_seconds != valid_end_unix_seconds
                    || first.packing_increment != packing_increment
                    || first.values.len() != values.len()
                    || first.values.iter().zip(&values).any(|(a, b)| a.to_bits() != b.to_bits())
                {
                    return Err("duplicate GRIB fields are not identical after decode".into());
                }
            }
        }
    }
    if !expected.allowed_messages.contains(&messages) {
        return Err(format!("GRIB contains {messages} messages, outside the source contract"));
    }
    decoded.ok_or_else(|| "GRIB contains no submessages".into())
}

/// PDT 4.8's explicit accumulation interval. The generic forecast-time field is only the
/// interval start; the template's own end timestamp, range unit/length, range count and
/// increment semantics are parsed and cross-checked (WX1: f001/f001, a renamed lead, and an
/// interrupted interval all fail here).
fn pdt48_interval(
    definition: &grib::ProdDefinition,
    forecast_time: i64,
    reference_unix_seconds: i64,
) -> Result<(i64, i64), String> {
    let payload: Vec<_> = definition.iter().copied().collect();
    if definition.num_coordinates() != 0 || payload.len() != 53 {
        return Err("PDT 4.8 payload length/coordinates changed".into());
    }
    let end = Utc
        .with_ymd_and_hms(
            i32::from(u16::from_be_bytes([payload[29], payload[30]])),
            u32::from(payload[31]),
            u32::from(payload[32]),
            u32::from(payload[33]),
            u32::from(payload[34]),
            u32::from(payload[35]),
        )
        .single()
        .ok_or("PDT 4.8 interval end is invalid")?
        .timestamp();
    let range_bytes: [u8; 4] = payload[44..48].try_into().expect("length checked");
    let tail_bytes: [u8; 4] = payload[49..53].try_into().expect("length checked");
    let head_bytes: [u8; 4] = payload[37..41].try_into().expect("length checked");
    if payload[36] != 1
        || u32::from_be_bytes(head_bytes) != 0
        || payload[41] != 1
        || payload[42] != 2
        || payload[48] != u8::MAX
        || u32::from_be_bytes(tail_bytes) != 0
    {
        return Err("PDT 4.8 is not one uninterrupted accumulation interval".into());
    }
    let range_seconds = duration_seconds(payload[43], u32::from_be_bytes(range_bytes))?;
    if end < forecast_time
        || end.checked_sub(forecast_time) != Some(range_seconds)
        || forecast_time < reference_unix_seconds
    {
        return Err("PDT 4.8 interval start/end/duration disagree".into());
    }
    Ok((forecast_time, end))
}

fn duration_seconds(unit: u8, value: u32) -> Result<i64, String> {
    let multiplier: i64 = match unit {
        0 => 60,
        1 => 3_600,
        2 => 86_400,
        10 => 10_800,
        11 => 21_600,
        12 => 43_200,
        13 => 1,
        _ => return Err(format!("unsupported GRIB time unit {unit}")),
    };
    i64::from(value).checked_mul(multiplier).ok_or_else(|| "GRIB duration overflows".into())
}

fn encode_hex(bytes: impl IntoIterator<Item = u8>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::new();
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}
