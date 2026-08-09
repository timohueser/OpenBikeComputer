use std::path::{Path, PathBuf};

use obc_wx_source_spike::{
    idx_range, idx_span, validate_bzip2_grib_file, validate_dwd_rv_hdf5, validate_gfs_apcp_file,
    validate_gfs_cumulative_files, validate_gfs_cumulative_step, validate_grib_file, validate_gzip_grib_file,
    validate_icon_eu_deaccumulation, validate_met_fixture, CumulativeField, ExpectedGrib, DWD_RV_PROJDEF,
    GFS_GLOBAL_GRID_DEFINITION_HEX, HRRR_CONUS_GRID_DEFINITION_HEX, ICON_EU_GRID_DEFINITION_HEX,
    MRMS_GRID_DEFINITION_HEX,
};

const NONE: &[f32] = &[];

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn expected(
    discipline: u8,
    category: u8,
    parameter: u8,
    grid_template: u16,
    product_template: Option<u16>,
    representation_templates: &'static [u16],
) -> ExpectedGrib {
    ExpectedGrib {
        discipline,
        category,
        parameter,
        grid_template,
        expected_points: Some(match (discipline, category, parameter) {
            (209, 6, 1) => 24_500_000,
            (0, 1, 52) => 904_689,
            (0, 1, 7) => 1_905_141,
            (0, 1, 8) => 1_038_240,
            _ => panic!("test helper has no pinned point count for this source"),
        }),
        expected_grid_definition_hex: match (discipline, category, parameter) {
            (209, 6, 1) => MRMS_GRID_DEFINITION_HEX,
            (0, 1, 52) => ICON_EU_GRID_DEFINITION_HEX,
            (0, 1, 7) => HRRR_CONUS_GRID_DEFINITION_HEX,
            (0, 1, 8) => GFS_GLOBAL_GRID_DEFINITION_HEX,
            _ => panic!("test helper has no pinned grid definition for this source"),
        },
        product_template,
        representation_templates,
        expected_messages: 1,
        require_identical_messages: false,
        missing_sentinels: NONE,
    }
}

#[test]
fn dwd_rv_raw_hdf5_pins_projection_scale_missing_and_convective_rain() {
    let summary = validate_dwd_rv_hdf5(&fixture("dwd-rv-20260809-1130-f000.h5")).unwrap();
    assert_eq!((summary.width, summary.height), (1_100, 1_200));
    assert_eq!(summary.projection, DWD_RV_PROJDEF);
    assert_eq!(summary.nodata, 4_294_967_295);
    assert_eq!(summary.undetect, 0);
    assert_eq!(summary.positive_cells, 28_048);
    assert_eq!(summary.missing_cells, 621_815);
    assert!((summary.maximum_mm_5min - 4.192_999_713_956_145).abs() < 1e-12);
    assert_eq!(summary.reference_unix_seconds, 1_786_275_000);
    assert_eq!(summary.valid_start_unix_seconds, 1_786_274_700);
    assert_eq!(summary.valid_end_unix_seconds, 1_786_275_000);
}

#[test]
fn mrms_contract_distinguishes_dry_missing_and_rain() {
    let mut contract = expected(209, 6, 1, 0, Some(0), &[41]);
    contract.missing_sentinels = &[-1.0, -3.0];
    let summary = validate_gzip_grib_file(&fixture("mrms-conus-20260808-020000.grib2.gz"), contract).unwrap();
    assert_eq!(summary.points, 24_500_000);
    assert_eq!(summary.missing, 8_357_311);
    assert_eq!(summary.dry, 15_816_149);
    assert_eq!(summary.positive, 326_540);
    assert_eq!(summary.maximum, 185.3);
    assert_eq!(summary.reference_unix_seconds, 1_786_154_400);
    assert_eq!(summary.valid_start_unix_seconds, 1_786_154_400);
    assert_eq!(summary.valid_end_unix_seconds, 1_786_154_400);
}

#[test]
fn icon_eu_contract_decodes_ccsds_and_bounds_deaccumulation_roundoff() {
    let contract = expected(0, 1, 52, 0, Some(8), &[42]);
    let first = fixture("icon-eu-20260809T06-f001.grib2.bz2");
    let second = fixture("icon-eu-20260809T06-f002.grib2.bz2");
    let field = validate_bzip2_grib_file(&second, contract).unwrap();
    assert_eq!(field.points, 904_689);
    assert_eq!(field.representation_template, 42);
    assert_eq!(field.dry, 620_164);
    assert_eq!(field.positive, 284_525);
    assert_eq!(field.reference_unix_seconds, 1_786_255_200);
    assert_eq!(field.valid_start_unix_seconds, 1_786_255_200);
    assert_eq!(field.valid_end_unix_seconds, 1_786_262_400);

    let delta = validate_icon_eu_deaccumulation(&first, &second, contract).unwrap();
    assert_eq!(delta.points, 904_689);
    assert_eq!(delta.packing_roundoff_cells, 9_005);
    assert_eq!(delta.maximum_negative_roundoff, 1.0 / 4096.0);
    assert_eq!(delta.packing_roundoff_limit_mm, 3.0 / 8192.0);
    assert!(delta.maximum_delta > 12.14 && delta.maximum_delta < 12.15);
    assert!(validate_icon_eu_deaccumulation(&first, &first, contract).is_err());
}

#[test]
fn hrrr_idx_range_and_complex_spatial_packing_are_pinned() {
    let index = std::fs::read_to_string(fixture("hrrr-conus-20260808T00-f002.idx")).unwrap();
    let range = idx_range(&index, ":PRATE:surface:120 min fcst:", 186_047_054).unwrap();
    assert_eq!((range.start, range.end_inclusive, range.len()), (165_672_006, 165_714_866, 42_861));

    let summary = validate_grib_file(
        &fixture("hrrr-conus-20260808T00-prate-f002-t120.grib2"),
        expected(0, 1, 7, 30, Some(0), &[3]),
    )
    .unwrap();
    assert_eq!(summary.points, 1_905_141);
    assert_eq!(summary.representation_template, 3);
    assert_eq!(summary.dry, 1_879_080);
    assert_eq!(summary.positive, 26_061);
    assert_eq!(summary.reference_unix_seconds, 1_786_147_200);
    assert_eq!(summary.valid_start_unix_seconds, 1_786_154_400);
    assert_eq!(summary.valid_end_unix_seconds, 1_786_154_400);
}

#[test]
fn gfs_idx_duplicate_span_must_decode_to_identical_global_fields() {
    let index = std::fs::read_to_string(fixture("gfs-global-20260809T06-f003.idx")).unwrap();
    let range = idx_span(&index, ":APCP:surface:0-3 hour acc fcst:", 539_185_590, 2).unwrap();
    assert_eq!((range.start, range.end_inclusive, range.len()), (427_163_736, 427_804_201, 640_466));

    let summary = validate_gfs_apcp_file(&fixture("gfs-global-20260809T06-apcp-f003.grib2"), 2).unwrap();
    assert_eq!(summary.messages, 2);
    assert_eq!(summary.representation_template, 3);
    assert_eq!(summary.points, 1_038_240);
    assert_eq!(summary.dry, 1_144_522);
    assert_eq!(summary.positive, 931_958);
    assert_eq!(summary.reference_unix_seconds, 1_786_255_200);
    assert_eq!(summary.valid_start_unix_seconds, 1_786_255_200);
    assert_eq!(summary.valid_end_unix_seconds, 1_786_266_000);
    let path = fixture("gfs-global-20260809T06-apcp-f003.grib2");
    assert!(validate_gfs_cumulative_files(None, (&path, 2, 1)).is_err());
}

#[test]
fn gfs_deaccumulation_never_crosses_a_run_boundary() {
    let first = [0.0, 2.0, 1.0];
    let second = [1.0, 2.0, 4.0];
    let first_field =
        CumulativeField { run_reference_unix_seconds: 1_786_257_600, forecast_hour: 1, values_mm: &first };
    let first_step = validate_gfs_cumulative_step(None, first_field).unwrap();
    assert_eq!((first_step.dry, first_step.positive, first_step.maximum_mm), (1, 2, 2.0));

    let second_step = validate_gfs_cumulative_step(
        Some(first_field),
        CumulativeField { run_reference_unix_seconds: 1_786_257_600, forecast_hour: 2, values_mm: &second },
    )
    .unwrap();
    assert_eq!((second_step.dry, second_step.positive, second_step.maximum_mm), (1, 2, 3.0));

    let new_run = CumulativeField { run_reference_unix_seconds: 1_786_279_200, forecast_hour: 1, values_mm: &first };
    assert!(validate_gfs_cumulative_step(Some(first_field), new_run).is_err());
    assert!(validate_gfs_cumulative_step(None, new_run).is_ok());
}

#[test]
fn met_contract_keeps_non_nordic_optional_fields_unavailable() {
    let oslo = validate_met_fixture(&fixture("met-locationforecast-oslo-24h.json")).unwrap();
    assert_eq!(oslo.hours, 24);
    assert_eq!(oslo.gust_hours, 24);
    assert_eq!(oslo.precipitation_probability_hours, 24);

    let manila = validate_met_fixture(&fixture("met-locationforecast-manila-24h.json")).unwrap();
    assert_eq!(manila.hours, 24);
    assert_eq!(manila.gust_hours, 0);
    assert_eq!(manila.precipitation_probability_hours, 0);
}
