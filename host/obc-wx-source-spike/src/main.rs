use std::{env, error::Error, path::Path};

use obc_wx_source_spike::{
    idx_range, idx_span, validate_bzip2_grib_file, validate_dwd_rv_hdf5, validate_dwd_rv_tar, validate_gfs_apcp_file,
    validate_gfs_cumulative_files, validate_grib_file, validate_gzip_grib_file, validate_icon_eu_deaccumulation,
    validate_met_fixture, validate_met_response, ExpectedGrib,
};

const COMPLEX_PACKING: &[u16] = &[2, 3];
const PNG_PACKING: &[u16] = &[41];
const CCSDS_PACKING: &[u16] = &[42];
const NO_SENTINELS: &[f32] = &[];
const MRMS_SENTINELS: &[f32] = &[-1.0, -3.0];

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [command, path] if command == "dwd-rv" => {
            println!("{:#?}", validate_dwd_rv_hdf5(Path::new(path))?);
        }
        [command, path] if command == "dwd-rv-tar" => {
            println!("{:#?}", validate_dwd_rv_tar(Path::new(path))?);
        }
        [command, path] if command == "met" => {
            println!("{:#?}", validate_met_fixture(Path::new(path))?);
        }
        [command, path] if command == "met-response" => {
            println!("{:#?}", validate_met_response(Path::new(path))?);
        }
        [command, path] if command == "mrms" => {
            println!(
                "{:#?}",
                validate_gzip_grib_file(
                    Path::new(path),
                    ExpectedGrib {
                        // MRMS uses NOAA local table 209 for PrecipRate.
                        discipline: 209,
                        category: 6,
                        parameter: 1,
                        grid_template: 0,
                        expected_points: Some(24_500_000),
                        product_template: Some(0),
                        representation_templates: PNG_PACKING,
                        expected_messages: 1,
                        require_identical_messages: false,
                        missing_sentinels: MRMS_SENTINELS,
                    }
                )?
            );
        }
        [command, path] if command == "icon-eu" => {
            println!(
                "{:#?}",
                validate_bzip2_grib_file(
                    Path::new(path),
                    ExpectedGrib {
                        discipline: 0,
                        category: 1,
                        parameter: 52,
                        grid_template: 0,
                        expected_points: Some(904_689),
                        product_template: Some(8),
                        representation_templates: CCSDS_PACKING,
                        expected_messages: 1,
                        require_identical_messages: false,
                        missing_sentinels: NO_SENTINELS,
                    }
                )?
            );
        }
        [command, earlier, later] if command == "icon-eu-delta" => {
            println!(
                "{:#?}",
                validate_icon_eu_deaccumulation(
                    Path::new(earlier),
                    Path::new(later),
                    ExpectedGrib {
                        discipline: 0,
                        category: 1,
                        parameter: 52,
                        grid_template: 0,
                        expected_points: Some(904_689),
                        product_template: Some(8),
                        representation_templates: CCSDS_PACKING,
                        expected_messages: 1,
                        require_identical_messages: false,
                        missing_sentinels: NO_SENTINELS,
                    },
                )?
            );
        }
        [command, path] if command == "hrrr-prate" => {
            println!(
                "{:#?}",
                validate_grib_file(
                    Path::new(path),
                    ExpectedGrib {
                        discipline: 0,
                        category: 1,
                        parameter: 7,
                        grid_template: 30,
                        expected_points: Some(1_905_141),
                        product_template: Some(0),
                        representation_templates: COMPLEX_PACKING,
                        expected_messages: 1,
                        require_identical_messages: false,
                        missing_sentinels: NO_SENTINELS,
                    }
                )?
            );
        }
        [command, path] if command == "gfs-apcp" => {
            println!("{:#?}", validate_gfs_apcp_file(Path::new(path), 2)?);
        }
        [command, path, expected_messages] if command == "gfs-apcp" => {
            println!("{:#?}", validate_gfs_apcp_file(Path::new(path), expected_messages.parse()?)?);
        }
        [command, path, expected_messages] if command == "gfs-apcp-first" => {
            println!("{:#?}", validate_gfs_cumulative_files(None, (Path::new(path), expected_messages.parse()?, 1),)?);
        }
        [command, earlier, earlier_messages, earlier_hour, later, later_messages, later_hour]
            if command == "gfs-apcp-step" =>
        {
            println!(
                "{:#?}",
                validate_gfs_cumulative_files(
                    Some((Path::new(earlier), earlier_messages.parse()?, earlier_hour.parse()?,)),
                    (Path::new(later), later_messages.parse()?, later_hour.parse()?,),
                )?
            );
        }
        [command, index_path, selector, object_len] if command == "idx-range" => {
            let index = std::fs::read_to_string(index_path)?;
            let range = idx_range(&index, selector, object_len.parse()?)?;
            println!("{}-{} {}", range.start, range.end_inclusive, range.len());
        }
        [command, index_path, selector, object_len, expected_matches] if command == "idx-span" => {
            let index = std::fs::read_to_string(index_path)?;
            let range = idx_span(&index, selector, object_len.parse()?, expected_matches.parse()?)?;
            println!("{}-{} {}", range.start, range.end_inclusive, range.len());
        }
        _ => {
            return Err("usage: obc-wx-source-spike <dwd-rv|dwd-rv-tar|met|met-response|mrms|icon-eu|hrrr-prate> FILE\n       obc-wx-source-spike gfs-apcp FILE [EXPECTED_MESSAGES]\n       obc-wx-source-spike gfs-apcp-first FILE EXPECTED_MESSAGES\n       obc-wx-source-spike gfs-apcp-step EARLIER EARLIER_MESSAGES EARLIER_HOUR LATER LATER_MESSAGES LATER_HOUR\n       obc-wx-source-spike icon-eu-delta EARLIER LATER\n       obc-wx-source-spike idx-range INDEX SELECTOR OBJECT_LENGTH\n       obc-wx-source-spike idx-span INDEX SELECTOR OBJECT_LENGTH EXPECTED_MATCHES".into());
        }
    }
    Ok(())
}
