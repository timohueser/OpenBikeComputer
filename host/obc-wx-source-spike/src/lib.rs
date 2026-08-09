//! WX1 host-side source contract probes.
//!
//! This crate is deliberately a disposable validation seam, not the WX5/WX6
//! production baker. It gives captured provider bytes a Rust-only path through
//! archive decompression, GRIB2/HDF5 decoding, and schema checks so provider
//! formats cannot leak into the companion app.

use std::{
    error::Error,
    fmt,
    fs::File,
    io::{BufReader, Cursor, Read},
    path::Path,
};

use flate2::read::GzDecoder;
use grib::{def::grib2::DataRepresentationTemplate, Grib2SubmessageDecoder};
use hdf5_pure::{AttrValue, File as Hdf5File};
use serde::Deserialize;

pub const MAX_COMPRESSED_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_GRID_POINTS: usize = 30_000_000;
pub const DWD_RV_PROJDEF: &str = "+proj=stere +lat_ts=60 +lat_0=90 +lon_0=10 +x_0=543196.83521776402 +y_0=3622588.8619310022 +units=m +a=6378137 +b=6356752.3142451802 +no_defs";

#[derive(Debug)]
pub struct ContractError(String);

impl ContractError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ContractError {}

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteRange {
    pub start: u64,
    pub end_inclusive: u64,
}

impl ByteRange {
    pub fn len(self) -> u64 {
        if self.is_empty() {
            0
        } else {
            self.end_inclusive - self.start + 1
        }
    }

    pub fn is_empty(self) -> bool {
        self.start > self.end_inclusive
    }
}

/// Resolve one unique wgrib2-style `.idx` record to an HTTP byte range.
///
/// `object_len` is mandatory because the final record has no following offset.
/// Ambiguous matches fail closed; callers must use the exact parameter/level/
/// interval tuple from the decision record.
pub fn idx_range(index: &str, needle: &str, object_len: u64) -> Result<ByteRange> {
    idx_span(index, needle, object_len, 1)
}

/// Resolve a deliberately duplicated, consecutive `.idx` selection as one
/// range. GFS currently advertises two indistinguishable APCP records; fetching
/// the span and proving the decoded fields identical is safer than selecting an
/// undocumented occurrence.
pub fn idx_span(index: &str, needle: &str, object_len: u64, expected_matches: usize) -> Result<ByteRange> {
    if needle.is_empty() || object_len == 0 || expected_matches == 0 {
        return Err(ContractError::new("idx selector, object length, and expected match count must be nonzero").into());
    }
    let mut entries = Vec::new();
    for (line_number, line) in index.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.splitn(3, ':');
        let _record = fields.next();
        let offset = fields
            .next()
            .ok_or_else(|| ContractError::new(format!("idx line {} has no offset", line_number + 1)))?
            .parse::<u64>()?;
        entries.push((offset, line));
    }
    if entries.is_empty() {
        return Err(ContractError::new("idx is empty").into());
    }
    for pair in entries.windows(2) {
        if pair[0].0 >= pair[1].0 {
            return Err(ContractError::new("idx offsets are not strictly increasing").into());
        }
    }
    if entries.last().expect("not empty").0 >= object_len {
        return Err(ContractError::new("idx offset lies beyond the object").into());
    }

    let matches: Vec<_> = entries.iter().enumerate().filter(|(_, (_, line))| line.contains(needle)).collect();
    if matches.len() != expected_matches {
        return Err(ContractError::new(format!(
            "idx selector expected {expected_matches} records; {needle:?} matched {}",
            matches.len()
        ))
        .into());
    }
    if matches.windows(2).any(|pair| pair[1].0 != pair[0].0 + 1) {
        return Err(ContractError::new("matched idx records are not consecutive").into());
    }
    let (first_position, (start, _)) = matches[0];
    let last_position = first_position + expected_matches - 1;
    let next = entries.get(last_position + 1).map_or(object_len, |entry| entry.0);
    Ok(ByteRange { start: *start, end_inclusive: next - 1 })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpectedGrib {
    pub discipline: u8,
    pub category: u8,
    pub parameter: u8,
    pub grid_template: u16,
    pub expected_points: Option<usize>,
    pub product_template: Option<u16>,
    pub representation_templates: &'static [u16],
    pub expected_messages: usize,
    pub require_identical_messages: bool,
    pub missing_sentinels: &'static [f32],
}

#[derive(Clone, Debug, PartialEq)]
pub struct GribSummary {
    pub messages: usize,
    pub points: usize,
    pub missing: usize,
    pub dry: usize,
    pub positive: usize,
    pub minimum: f32,
    pub maximum: f32,
    pub grid_template: u16,
    pub product_template: u16,
    pub representation_template: u16,
    pub packing_increment: f32,
    pub reference_unix_seconds: i64,
    /// Forecast-time field exposed by the GRIB template. For interval products
    /// (PDT 4.8) this is the interval start, not the interval end.
    pub forecast_time_unix_seconds: i64,
}

pub fn validate_grib_file(path: &Path, expected: ExpectedGrib) -> Result<GribSummary> {
    let metadata = path.metadata()?;
    if metadata.len() == 0 || metadata.len() > MAX_DECOMPRESSED_BYTES {
        return Err(ContractError::new("GRIB input size is outside spike limits").into());
    }
    let file = BufReader::new(File::open(path)?);
    validate_grib_reader(file, expected)
}

pub fn validate_gzip_grib_file(path: &Path, expected: ExpectedGrib) -> Result<GribSummary> {
    reject_large_compressed(path)?;
    let decoder = GzDecoder::new(BufReader::new(File::open(path)?));
    validate_grib_reader(decoder.take(MAX_DECOMPRESSED_BYTES + 1), expected)
}

pub fn validate_bzip2_grib_file(path: &Path, expected: ExpectedGrib) -> Result<GribSummary> {
    Ok(validate_bzip2_grib_file_with_values(path, expected)?.0)
}

fn validate_bzip2_grib_file_with_values(path: &Path, expected: ExpectedGrib) -> Result<(GribSummary, Vec<f32>)> {
    reject_large_compressed(path)?;
    let decoder = bzip2_rs::DecoderReader::new(BufReader::new(File::open(path)?));
    validate_grib_reader_with_values(decoder.take(MAX_DECOMPRESSED_BYTES + 1), expected)
}

fn reject_large_compressed(path: &Path) -> Result<()> {
    let len = path.metadata()?.len();
    if len == 0 || len > MAX_COMPRESSED_BYTES {
        return Err(ContractError::new("compressed input size is outside spike limits").into());
    }
    Ok(())
}

fn validate_grib_reader(mut reader: impl Read, expected: ExpectedGrib) -> Result<GribSummary> {
    Ok(validate_grib_reader_inner(&mut reader, expected, false)?.0)
}

fn validate_grib_reader_with_values(mut reader: impl Read, expected: ExpectedGrib) -> Result<(GribSummary, Vec<f32>)> {
    let (summary, values) = validate_grib_reader_inner(&mut reader, expected, true)?;
    Ok((summary, values.expect("value retention was requested for a non-empty GRIB")))
}

fn validate_grib_reader_inner(
    mut reader: impl Read,
    expected: ExpectedGrib,
    retain_first_values: bool,
) -> Result<(GribSummary, Option<Vec<f32>>)> {
    if expected.expected_messages == 0 {
        return Err(ContractError::new("expected GRIB message count must be nonzero").into());
    }
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err(ContractError::new("decompressed GRIB exceeds spike limit").into());
    }
    let context = grib::from_reader(Cursor::new(bytes))?;
    let mut summary: Option<GribSummary> = None;
    let mut first_values: Option<Vec<u32>> = None;
    let mut decoded_first = None;
    for (_, submessage) in context.iter() {
        let discipline = submessage.indicator().discipline;
        let category = submessage.prod_def().parameter_category();
        let parameter = submessage.prod_def().parameter_number();
        if discipline != expected.discipline
            || category != Some(expected.category)
            || parameter != Some(expected.parameter)
        {
            return Err(ContractError::new(format!(
                "unexpected GRIB parameter d={discipline} c={category:?} p={parameter:?}"
            ))
            .into());
        }
        let grid_template = submessage.grid_def().grid_tmpl_num();
        let product_template = submessage.prod_def().prod_tmpl_num();
        let representation_template = submessage.repr_def().repr_tmpl_num();
        let temporal = submessage.temporal_info();
        let reference_unix_seconds =
            temporal.ref_time.ok_or_else(|| ContractError::new("GRIB reference time is invalid"))?.timestamp();
        let forecast_time_unix_seconds = temporal
            .forecast_time_target
            .ok_or_else(|| ContractError::new("GRIB forecast time is invalid"))?
            .timestamp();
        let section5 = submessage.section5()?;
        let simple = match &section5.payload.template {
            DataRepresentationTemplate::_5_2(template) => &template.simple,
            DataRepresentationTemplate::_5_3(template) => &template.simple,
            DataRepresentationTemplate::_5_41(template) => &template.simple,
            DataRepresentationTemplate::_5_42(template) => &template.simple,
            _ => {
                return Err(ContractError::new("selected GRIB representation has no audited packing increment").into());
            }
        };
        let packing_increment = 2_f32.powi(i32::from(simple.exp)) * 10_f32.powi(-i32::from(simple.dec));
        if !packing_increment.is_normal() || packing_increment <= 0.0 {
            return Err(ContractError::new("GRIB packing increment is invalid").into());
        }
        if grid_template != expected.grid_template {
            return Err(ContractError::new(format!(
                "grid template {grid_template} != expected {}",
                expected.grid_template
            ))
            .into());
        }
        if expected.product_template.is_some_and(|value| value != product_template) {
            return Err(ContractError::new(format!(
                "product template {product_template} is outside the source contract"
            ))
            .into());
        }
        if !expected.representation_templates.contains(&representation_template) {
            return Err(ContractError::new(format!(
                "representation template {representation_template} is outside the source contract"
            ))
            .into());
        }

        let points = usize::try_from(submessage.grid_def().num_points())?;
        if points == 0 || points > MAX_GRID_POINTS {
            return Err(ContractError::new("GRIB point count is outside spike limits").into());
        }
        if let Some(expected_points) = expected.expected_points {
            if expected_points != points {
                return Err(ContractError::new(format!("GRIB has {points} points, expected {expected_points}")).into());
            }
        }
        let values: Vec<_> = Grib2SubmessageDecoder::from(submessage)?.dispatch()?.collect();
        if values.len() != points {
            return Err(ContractError::new(format!("decoded {} values for a {points}-point grid", values.len())).into());
        }
        if retain_first_values && decoded_first.is_none() {
            decoded_first = Some(values.clone());
        }
        if expected.require_identical_messages {
            let value_bits: Vec<_> = values.iter().map(|value| value.to_bits()).collect();
            match &first_values {
                None => first_values = Some(value_bits),
                Some(first) if first == &value_bits => {}
                Some(_) => {
                    return Err(ContractError::new("duplicate GRIB fields are not byte-identical after decode").into());
                }
            }
        }
        let mut minimum = f32::INFINITY;
        let mut maximum = f32::NEG_INFINITY;
        let mut missing = 0;
        let mut dry = 0;
        let mut positive = 0;
        for value in values {
            if value.is_nan() || expected.missing_sentinels.contains(&value) {
                missing += 1;
            } else if !value.is_finite() {
                return Err(ContractError::new("GRIB contains a non-finite value").into());
            } else if value < 0.0 {
                return Err(
                    ContractError::new(format!("GRIB contains undocumented negative precipitation {value}")).into()
                );
            } else {
                minimum = minimum.min(value);
                maximum = maximum.max(value);
                if value == 0.0 {
                    dry += 1;
                } else {
                    positive += 1;
                }
            }
        }
        if minimum == f32::INFINITY {
            return Err(ContractError::new("GRIB contains no finite values").into());
        }

        match &mut summary {
            None => {
                summary = Some(GribSummary {
                    messages: 1,
                    points,
                    missing,
                    dry,
                    positive,
                    minimum,
                    maximum,
                    grid_template,
                    product_template,
                    representation_template,
                    packing_increment,
                    reference_unix_seconds,
                    forecast_time_unix_seconds,
                });
            }
            Some(existing) => {
                if existing.points != points
                    || existing.grid_template != grid_template
                    || existing.product_template != product_template
                    || existing.representation_template != representation_template
                    || existing.packing_increment != packing_increment
                    || existing.reference_unix_seconds != reference_unix_seconds
                    || existing.forecast_time_unix_seconds != forecast_time_unix_seconds
                {
                    return Err(ContractError::new("GRIB submessages disagree on geometry/templates").into());
                }
                existing.messages += 1;
                existing.missing += missing;
                existing.dry += dry;
                existing.positive += positive;
                existing.minimum = existing.minimum.min(minimum);
                existing.maximum = existing.maximum.max(maximum);
            }
        }
    }
    let summary = summary.ok_or_else(|| ContractError::new("GRIB contains no submessages"))?;
    if summary.messages != expected.expected_messages {
        return Err(ContractError::new(format!(
            "decoded {} GRIB messages, expected {}",
            summary.messages, expected.expected_messages
        ))
        .into());
    }
    Ok((summary, decoded_first))
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeaccumulationSummary {
    pub points: usize,
    pub dry: usize,
    pub positive: usize,
    pub packing_roundoff_cells: usize,
    pub maximum_negative_roundoff: f32,
    pub packing_roundoff_limit_mm: f32,
    pub maximum_delta: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct CumulativeField<'a> {
    pub run_reference_unix_seconds: i64,
    pub forecast_hour: u16,
    pub values_mm: &'a [f32],
}

#[derive(Clone, Debug, PartialEq)]
pub struct CumulativeStepSummary {
    pub points: usize,
    pub dry: usize,
    pub positive: usize,
    pub maximum_mm: f32,
}

/// Validate one hourly step from cumulative GFS APCP fields.
///
/// The first hour of a run is differenced from zero. Every later hour must be
/// differenced from the immediately preceding field of the same run. This
/// makes a run transition explicit and prevents subtracting an old run from a
/// new one or counting a cumulative field twice.
pub fn validate_gfs_cumulative_step(
    earlier: Option<CumulativeField<'_>>,
    later: CumulativeField<'_>,
) -> Result<CumulativeStepSummary> {
    if later.forecast_hour == 0 || later.values_mm.is_empty() || later.values_mm.len() > MAX_GRID_POINTS {
        return Err(ContractError::new("GFS cumulative field has an invalid lead or point count").into());
    }
    let earlier_values = match earlier {
        None if later.forecast_hour == 1 => None,
        None => {
            return Err(ContractError::new("only GFS forecast hour 1 may use the zero baseline").into());
        }
        Some(field)
            if field.run_reference_unix_seconds == later.run_reference_unix_seconds
                && field.forecast_hour.checked_add(1) == Some(later.forecast_hour)
                && field.values_mm.len() == later.values_mm.len() =>
        {
            Some(field.values_mm)
        }
        Some(_) => {
            return Err(
                ContractError::new("GFS cumulative fields cross a run, skip an hour, or disagree on geometry").into()
            );
        }
    };

    let mut dry = 0;
    let mut positive = 0;
    let mut maximum_mm = 0.0f32;
    for (index, later_value) in later.values_mm.iter().copied().enumerate() {
        let earlier_value = earlier_values.map_or(0.0, |values| values[index]);
        if !earlier_value.is_finite() || !later_value.is_finite() || earlier_value < 0.0 || later_value < earlier_value
        {
            return Err(ContractError::new("GFS cumulative precipitation is invalid or decreased").into());
        }
        let delta = later_value - earlier_value;
        if delta == 0.0 {
            dry += 1;
        } else {
            positive += 1;
            maximum_mm = maximum_mm.max(delta);
        }
    }
    Ok(CumulativeStepSummary { points: later.values_mm.len(), dry, positive, maximum_mm })
}

fn gfs_apcp_contract(expected_messages: usize) -> Result<ExpectedGrib> {
    if !matches!(expected_messages, 1 | 2) {
        return Err(ContractError::new("GFS APCP must contain one cumulative field or two exact duplicates").into());
    }
    Ok(ExpectedGrib {
        discipline: 0,
        category: 1,
        parameter: 8,
        grid_template: 0,
        expected_points: Some(1_038_240),
        product_template: Some(8),
        representation_templates: &[2, 3],
        expected_messages,
        require_identical_messages: expected_messages == 2,
        missing_sentinels: &[],
    })
}

fn validate_gfs_file_with_values(path: &Path, expected_messages: usize) -> Result<(GribSummary, Vec<f32>)> {
    let metadata = path.metadata()?;
    if metadata.len() == 0 || metadata.len() > MAX_DECOMPRESSED_BYTES {
        return Err(ContractError::new("GFS GRIB input size is outside spike limits").into());
    }
    validate_grib_reader_with_values(BufReader::new(File::open(path)?), gfs_apcp_contract(expected_messages)?)
}

pub fn validate_gfs_apcp_file(path: &Path, expected_messages: usize) -> Result<GribSummary> {
    Ok(validate_gfs_file_with_values(path, expected_messages)?.0)
}

/// Decode two index-selected GFS cumulative files and prove a valid hourly
/// step. Run identity is derived from the GRIB reference time, while the caller
/// supplies only the lead hours selected by the audited `.idx` contract.
pub fn validate_gfs_cumulative_files(
    earlier: Option<(&Path, usize, u16)>,
    later: (&Path, usize, u16),
) -> Result<CumulativeStepSummary> {
    let (later_path, later_messages, later_hour) = later;
    let (later_summary, later_values) = validate_gfs_file_with_values(later_path, later_messages)?;
    let earlier_field = match earlier {
        None => None,
        Some((path, messages, hour)) => {
            let (summary, values) = validate_gfs_file_with_values(path, messages)?;
            if summary.points != later_summary.points
                || summary.grid_template != later_summary.grid_template
                || summary.product_template != later_summary.product_template
                || summary.reference_unix_seconds != later_summary.reference_unix_seconds
            {
                return Err(ContractError::new("GFS cumulative files disagree on run/geometry/templates").into());
            }
            Some((hour, values))
        }
    };
    let earlier_field = earlier_field.as_ref().map(|(hour, values)| CumulativeField {
        run_reference_unix_seconds: later_summary.reference_unix_seconds,
        forecast_hour: *hour,
        values_mm: values,
    });
    validate_gfs_cumulative_step(
        earlier_field,
        CumulativeField {
            run_reference_unix_seconds: later_summary.reference_unix_seconds,
            forecast_hour: later_hour,
            values_mm: &later_values,
        },
    )
}

/// Validate ICON-EU's cumulative `TOT_PREC` rule on two consecutive lead
/// hours. Independently packed cumulative fields may differ by at most half
/// the sum of their packing increments. A larger negative delta is an input-
/// contract failure; it is never clamped into plausible dry weather.
pub fn validate_icon_eu_deaccumulation(
    earlier: &Path,
    later: &Path,
    expected: ExpectedGrib,
) -> Result<DeaccumulationSummary> {
    let (earlier_summary, earlier_values) = validate_bzip2_grib_file_with_values(earlier, expected)?;
    let (later_summary, later_values) = validate_bzip2_grib_file_with_values(later, expected)?;
    if earlier_summary.points != later_summary.points
        || earlier_summary.grid_template != later_summary.grid_template
        || earlier_summary.product_template != later_summary.product_template
        || earlier_summary.reference_unix_seconds != later_summary.reference_unix_seconds
        || earlier_values.len() != later_values.len()
    {
        return Err(ContractError::new("ICON-EU cumulative fields disagree on run/geometry/templates").into());
    }
    let mut dry = 0;
    let mut positive = 0;
    let mut packing_roundoff_cells = 0;
    let mut maximum_negative_roundoff = 0.0f32;
    let mut maximum_delta = 0.0f32;
    let packing_roundoff_limit_mm = (earlier_summary.packing_increment + later_summary.packing_increment) / 2.0;
    for (earlier, later) in earlier_values.into_iter().zip(later_values) {
        if !earlier.is_finite() || !later.is_finite() || earlier < 0.0 || later < 0.0 {
            return Err(ContractError::new("ICON-EU cumulative field contains an invalid value").into());
        }
        let delta = later - earlier;
        if delta < -packing_roundoff_limit_mm {
            return Err(
                ContractError::new(format!("ICON-EU cumulative precipitation decreased by {} mm", -delta)).into()
            );
        }
        if delta <= 0.0 {
            dry += 1;
            if delta < 0.0 {
                packing_roundoff_cells += 1;
                maximum_negative_roundoff = maximum_negative_roundoff.max(-delta);
            }
        } else {
            positive += 1;
            maximum_delta = maximum_delta.max(delta);
        }
    }
    Ok(DeaccumulationSummary {
        points: earlier_summary.points,
        dry,
        positive,
        packing_roundoff_cells,
        maximum_negative_roundoff,
        packing_roundoff_limit_mm,
        maximum_delta,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct DwdRvSummary {
    pub width: u64,
    pub height: u64,
    pub gain: f64,
    pub offset: f64,
    pub nodata: u64,
    pub undetect: u64,
    pub positive_cells: usize,
    pub missing_cells: usize,
    pub maximum_mm_5min: f64,
    pub projection: String,
}

pub fn validate_dwd_rv_hdf5(path: &Path) -> Result<DwdRvSummary> {
    let length = path.metadata()?.len();
    if length == 0 || length > MAX_DECOMPRESSED_BYTES {
        return Err(ContractError::new("DWD HDF5 input size is outside spike limits").into());
    }
    let bytes = std::fs::read(path)?;
    validate_dwd_rv_bytes(bytes)
}

fn validate_dwd_rv_bytes(bytes: Vec<u8>) -> Result<DwdRvSummary> {
    if bytes.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err(ContractError::new("DWD HDF5 exceeds spike limit").into());
    }
    let file = Hdf5File::from_bytes(bytes)?;
    let dataset = file.dataset("dataset1/data1/data")?;
    let shape = dataset.shape()?;
    if shape.as_slice() != [1_200, 1_100] {
        return Err(ContractError::new("DWD RV raster is no longer the 1200x1100 native grid").into());
    }
    if shape[0].checked_mul(shape[1]).is_none_or(|value| value > MAX_GRID_POINTS as u64) {
        return Err(ContractError::new("DWD RV raster exceeds spike point limit").into());
    }

    let data_attrs = file.group("dataset1/data1/what")?.attrs()?;
    let where_attrs = file.group("where")?.attrs()?;
    if attr_string(&data_attrs, "quantity")? != "ACRR" {
        return Err(ContractError::new("DWD quantity is not ACRR").into());
    }
    let xscale = attr_f64(&where_attrs, "xscale")?;
    let yscale = attr_f64(&where_attrs, "yscale")?;
    if xscale != 1_000.0 || yscale != 1_000.0 {
        return Err(ContractError::new("DWD RV native grid is no longer 1 km").into());
    }
    let projection = attr_string(&where_attrs, "projdef")?;
    if projection != DWD_RV_PROJDEF {
        return Err(ContractError::new("DWD RV stereographic projection changed").into());
    }

    let gain = attr_f64(&data_attrs, "gain")?;
    let offset = attr_f64(&data_attrs, "offset")?;
    let nodata = attr_u64(&data_attrs, "nodata")?;
    let undetect = attr_u64(&data_attrs, "undetect")?;
    if gain != 0.000_999_999_931_780_621_3
        || offset != -0.000_999_999_931_780_621_3
        || nodata != 4_294_967_295
        || undetect != 0
    {
        return Err(ContractError::new("DWD RV scale or missing-value contract changed").into());
    }
    let raw = dataset.read_u32()?;
    let mut positive_cells = 0;
    let mut missing_cells = 0;
    let mut maximum_mm_5min = 0.0f64;
    for encoded in raw {
        let encoded = u64::from(encoded);
        if encoded == nodata {
            missing_cells += 1;
            continue;
        }
        if encoded == undetect {
            continue;
        }
        let value = encoded as f64 * gain + offset;
        if !value.is_finite() || value < 0.0 {
            return Err(ContractError::new("DWD RV contains invalid scaled precipitation").into());
        }
        if value > 0.0 {
            positive_cells += 1;
            maximum_mm_5min = maximum_mm_5min.max(value);
        }
    }
    Ok(DwdRvSummary {
        width: shape[1],
        height: shape[0],
        gain,
        offset,
        nodata,
        undetect,
        positive_cells,
        missing_cells,
        maximum_mm_5min,
        projection,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct DwdRvTarSummary {
    pub frames: usize,
    pub archive_bytes: u64,
    pub maximum_mm_5min: f64,
}

pub fn validate_dwd_rv_tar(path: &Path) -> Result<DwdRvTarSummary> {
    reject_large_compressed(path)?;
    let archive_bytes = path.metadata()?.len();
    let mut archive = tar::Archive::new(BufReader::new(File::open(path)?));
    let mut frames = 0usize;
    let mut maximum_mm_5min = 0.0f64;
    let mut run_prefix = None;
    for entry in archive.entries()? {
        let entry = entry?;
        if entry.size() > MAX_DECOMPRESSED_BYTES {
            return Err(ContractError::new("DWD RV HDF5 member exceeds spike limit").into());
        }
        let path = entry.path()?;
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .ok_or_else(|| ContractError::new("DWD RV tar member has a non-UTF-8 or empty name"))?;
        if run_prefix.is_none() {
            let prefix = name
                .strip_suffix("000-hd5")
                .filter(|prefix| prefix.starts_with("composite_rv_") && prefix.len() == 27)
                .ok_or_else(|| ContractError::new("DWD RV tar does not start with the expected lead-000 name"))?;
            run_prefix = Some(prefix.to_owned());
        }
        let expected_name = format!("{}{:03}-hd5", run_prefix.as_deref().expect("set above"), frames * 5);
        if name != expected_name {
            return Err(
                ContractError::new(format!("DWD RV tar frame {frames} is {name}, expected {expected_name}")).into()
            );
        }
        let mut bytes = Vec::new();
        entry.take(MAX_DECOMPRESSED_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_DECOMPRESSED_BYTES {
            return Err(ContractError::new("DWD RV HDF5 member exceeds spike limit").into());
        }
        let summary = validate_dwd_rv_bytes(bytes)?;
        maximum_mm_5min = maximum_mm_5min.max(summary.maximum_mm_5min);
        frames += 1;
    }
    if frames != 25 {
        return Err(ContractError::new(format!(
            "DWD RV tar contains {frames} frames; expected leads 000..120 every 5 minutes"
        ))
        .into());
    }
    Ok(DwdRvTarSummary { frames, archive_bytes, maximum_mm_5min })
}

fn attr_string(attrs: &std::collections::HashMap<String, AttrValue>, name: &str) -> Result<String> {
    attrs
        .get(name)
        .and_then(AttrValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ContractError::new(format!("attribute {name} is not a scalar string")).into())
}

fn attr_f64(attrs: &std::collections::HashMap<String, AttrValue>, name: &str) -> Result<f64> {
    attrs
        .get(name)
        .and_then(AttrValue::as_f64)
        .ok_or_else(|| ContractError::new(format!("attribute {name} is not a scalar number")).into())
}

fn attr_u64(attrs: &std::collections::HashMap<String, AttrValue>, name: &str) -> Result<u64> {
    let value = attrs.get(name).ok_or_else(|| ContractError::new(format!("attribute {name} is absent")))?;
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    if let Some(value) = value.as_f64() {
        if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64 {
            return Ok(value as u64);
        }
    }
    Err(ContractError::new(format!("attribute {name} is not a scalar unsigned integer")).into())
}

#[derive(Debug, Deserialize)]
struct MetFixture {
    hours: Vec<MetHour>,
}

#[derive(Debug, Deserialize)]
struct MetHour {
    time: String,
    air_temperature_c: f64,
    precipitation_amount_mm: f64,
    probability_of_precipitation_percent: Option<f64>,
    symbol_code: String,
    wind_from_direction_degrees: f64,
    wind_speed_mps: f64,
    wind_gust_mps: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetSummary {
    pub hours: usize,
    pub precipitation_probability_hours: usize,
    pub gust_hours: usize,
}

pub fn validate_met_fixture(path: &Path) -> Result<MetSummary> {
    let fixture: MetFixture = serde_json::from_slice(&read_json_input(path)?)?;
    if fixture.hours.len() != 24 {
        return Err(ContractError::new("MET extract must contain exactly 24 hours").into());
    }
    let mut previous = None;
    for hour in &fixture.hours {
        if previous.is_some_and(|value: &str| value >= hour.time.as_str()) {
            return Err(ContractError::new("MET timestamps are not strictly increasing").into());
        }
        previous = Some(hour.time.as_str());
        if !hour.air_temperature_c.is_finite()
            || !hour.precipitation_amount_mm.is_finite()
            || hour.precipitation_amount_mm < 0.0
            || hour.symbol_code.is_empty()
            || !hour.wind_from_direction_degrees.is_finite()
            || !(0.0..360.0).contains(&hour.wind_from_direction_degrees)
            || !hour.wind_speed_mps.is_finite()
            || hour.wind_speed_mps < 0.0
            || hour
                .probability_of_precipitation_percent
                .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
            || hour.wind_gust_mps.is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            return Err(ContractError::new("MET extract contains an invalid canonical value").into());
        }
    }
    Ok(MetSummary {
        hours: fixture.hours.len(),
        precipitation_probability_hours: fixture
            .hours
            .iter()
            .filter(|hour| hour.probability_of_precipitation_percent.is_some())
            .count(),
        gust_hours: fixture.hours.iter().filter(|hour| hour.wind_gust_mps.is_some()).count(),
    })
}

/// Validate a live Locationforecast 2.0 `complete` response without retaining
/// provider JSON in any production model. The companion adapter remains WX4;
/// this only proves the upstream schema and its explicitly optional fields.
pub fn validate_met_response(path: &Path) -> Result<MetSummary> {
    let root: serde_json::Value = serde_json::from_slice(&read_json_input(path)?)?;
    let properties = object_field(&root, "properties")?;
    let meta = map_object_field(properties, "meta")?;
    let units = map_object_field(meta, "units")?;
    for (field, unit) in [
        ("air_temperature", "celsius"),
        ("precipitation_amount", "mm"),
        ("wind_from_direction", "degrees"),
        ("wind_speed", "m/s"),
    ] {
        if units.get(field).and_then(serde_json::Value::as_str) != Some(unit) {
            return Err(ContractError::new(format!("MET unit for {field} changed")).into());
        }
    }
    let records = properties
        .get("timeseries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ContractError::new("MET timeseries is absent"))?;
    if records.len() < 24 {
        return Err(ContractError::new("MET response has fewer than 24 hourly records").into());
    }
    let mut previous = None;
    let mut precipitation_probability_hours = 0;
    let mut gust_hours = 0;
    for record in records.iter().take(24) {
        let time = record
            .get("time")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ContractError::new("MET record time is absent"))?;
        if previous.is_some_and(|value: &str| value >= time) {
            return Err(ContractError::new("MET response timestamps are not increasing").into());
        }
        previous = Some(time);
        let data = object_field(record, "data")?;
        let instant = map_object_field(map_object_field(data, "instant")?, "details")?;
        require_json_number(instant, "air_temperature")?;
        let wind_direction = require_json_number(instant, "wind_from_direction")?;
        let wind_speed = require_json_number(instant, "wind_speed")?;
        if !(0.0..360.0).contains(&wind_direction) || wind_speed < 0.0 {
            return Err(ContractError::new("MET wind is outside the source contract").into());
        }
        if let Some(gust) = instant.get("wind_speed_of_gust").and_then(serde_json::Value::as_f64) {
            if !gust.is_finite() || gust < 0.0 {
                return Err(ContractError::new("MET gust is outside the source contract").into());
            }
            gust_hours += 1;
        }
        let next_hour = map_object_field(data, "next_1_hours")?;
        let details = map_object_field(next_hour, "details")?;
        if require_json_number(details, "precipitation_amount")? < 0.0 {
            return Err(ContractError::new("MET precipitation amount is negative").into());
        }
        if let Some(probability) = details.get("probability_of_precipitation").and_then(serde_json::Value::as_f64) {
            if !probability.is_finite() || !(0.0..=100.0).contains(&probability) {
                return Err(ContractError::new("MET precipitation probability is outside 0...100").into());
            }
            precipitation_probability_hours += 1;
        }
        let symbol = map_object_field(next_hour, "summary")?
            .get("symbol_code")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ContractError::new("MET next-hour symbol is absent"))?;
        if symbol.is_empty() {
            return Err(ContractError::new("MET next-hour symbol is empty").into());
        }
    }
    if gust_hours > 0 && units.get("wind_speed_of_gust").and_then(serde_json::Value::as_str) != Some("m/s") {
        return Err(ContractError::new("MET gust unit changed").into());
    }
    if precipitation_probability_hours > 0
        && units.get("probability_of_precipitation").and_then(serde_json::Value::as_str) != Some("%")
    {
        return Err(ContractError::new("MET precipitation probability unit changed").into());
    }
    Ok(MetSummary { hours: 24, precipitation_probability_hours, gust_hours })
}

fn map_object_field<'a>(
    value: &'a serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>> {
    value
        .get(name)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ContractError::new(format!("JSON object {name} is absent")).into())
}

fn object_field<'a>(
    value: &'a serde_json::Value,
    name: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>> {
    value
        .get(name)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| ContractError::new(format!("JSON object {name} is absent")).into())
}

fn require_json_number(object: &serde_json::Map<String, serde_json::Value>, name: &str) -> Result<f64> {
    let value = object
        .get(name)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| ContractError::new(format!("JSON number {name} is absent")))?;
    if !value.is_finite() {
        return Err(ContractError::new(format!("JSON number {name} is not finite")).into());
    }
    Ok(value)
}

fn read_json_input(path: &Path) -> Result<Vec<u8>> {
    let length = path.metadata()?.len();
    if length == 0 || length > MAX_COMPRESSED_BYTES {
        return Err(ContractError::new("JSON input size is outside spike limits").into());
    }
    Ok(std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idx_range_requires_a_unique_exact_contract() {
        let index = "1:0:d=x:TMP:surface:\n2:10:d=x:APCP:surface:0-1 hour acc fcst:\n3:42:d=x:UGRD:surface:\n";
        assert_eq!(
            idx_range(index, ":APCP:surface:0-1 hour acc fcst:", 100).unwrap(),
            ByteRange { start: 10, end_inclusive: 41 }
        );
        assert!(idx_range(index, ":surface:", 100).is_err());
        assert!(idx_range(index, ":PRATE:", 100).is_err());
    }

    #[test]
    fn idx_span_accepts_only_deliberate_consecutive_duplicates() {
        let index = "1:0:a\n2:10:APCP\n3:20:APCP\n4:30:b\n";
        assert_eq!(idx_span(index, "APCP", 40, 2).unwrap(), ByteRange { start: 10, end_inclusive: 29 });
        assert!(idx_span("1:0:APCP\n2:10:x\n3:20:APCP\n", "APCP", 30, 2).is_err());
        assert!(idx_span(index, "APCP", 40, 0).is_err());
    }

    #[test]
    fn idx_range_rejects_unsorted_or_out_of_bounds_offsets() {
        assert!(idx_range("1:10:a\n2:9:b\n", "a", 100).is_err());
        assert!(idx_range("1:100:a\n", "a", 100).is_err());
    }
}
