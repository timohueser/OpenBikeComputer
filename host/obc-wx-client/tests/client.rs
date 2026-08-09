//! The client suite. Nothing here opens a socket: every byte comes from `specs/vectors/` or from
//! the two captured documents in `tests/fixtures/`. `--weather live` is the only network path in
//! the project, and it is behind a flag.

use obc_wx_client::corridor::{self, Crop};
use obc_wx_client::http::{FailureControls, FaultyHttp, FixtureHttp};
use obc_wx_client::manifest::{self, SourceClass};
use obc_wx_client::met;
use obc_wx_client::select::{self, Corridor, NoRainMap};
use obc_wx_client::{bundle, WeatherClient};

const ORIGIN: &str = "https://wx.test";
const MANIFEST_URL: &str = "https://wx.test/wx/v1/manifest.json";
const MET_ENDPOINT: &str = "https://met.test/complete";

fn vector(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../specs/vectors/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn rfc3339(unix: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0).unwrap().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Build a manifest entry **from the vector's own header**, so a fixture can never describe an
/// object it does not match — the same discipline the Swift suite uses.
fn frame_json(key: &str, bytes: &[u8]) -> (String, obc_formats::obcg::Header) {
    let header = obc_formats::obcg::decode_header(&bytes[..128].try_into().unwrap()).expect("vector header");
    let json = format!(
        r#"{{"offset_min":0,"valid_at":"{}","source_class":"{}","key":"{key}","bytes":{},
            "object_crc32":"0x{:08X}",
            "geometry":{{"south_udeg":{},"west_udeg":{},"cell_lat_udeg":{},"cell_lon_udeg":{},
                         "width":{},"height":{},"cell_size_m":{},"tile_edge":{},"entries_per_page":{}}}}}"#,
        rfc3339(header.valid_at),
        if header.flags & obc_formats::obcg::FLAG_OBSERVED != 0 { "observation" } else { "forecast" },
        header.total_len,
        header.object_crc32,
        header.south_lat_udeg,
        header.west_lon_udeg,
        header.cell_lat_udeg,
        header.cell_lon_udeg,
        header.width,
        header.height,
        header.cell_size_m,
        header.tile_edge,
        header.entries_per_page,
    );
    (json, header)
}

fn product_json(id: &str, tier: u8, header: &obc_formats::obcg::Header, deadline: i64, frames: &str) -> String {
    format!(
        r#"{{"id":"{id}","tier":{tier},
            "bbox_udeg":{{"south_udeg":{},"west_udeg":{},"north_udeg":{},"east_udeg":{}}},
            "cell":{{"lat_udeg":{},"lon_udeg":{},"nominal_m":{}}},
            "reference_time":"{}","generated_at":"{}","staleness_deadline":"{}",
            "attribution":{{"text":"test","url":"https://example.invalid"}},
            "frames":[{frames}]}}"#,
        header.south_lat_udeg,
        header.west_lon_udeg,
        header.north_lat_udeg(),
        header.east_lon_udeg(),
        header.cell_lat_udeg,
        header.cell_lon_udeg,
        header.cell_size_m,
        rfc3339(header.reference_time),
        rfc3339(header.reference_time),
        rfc3339(deadline),
    )
}

fn manifest_json(now: i64, products: &[String]) -> String {
    format!(r#"{{"version":1,"generated_at":"{}","products":[{}]}}"#, rfc3339(now), products.join(","))
}

// ── manifest ───────────────────────────────────────────────────────────────────────────────

/// The production document the service actually publishes must parse with nothing skipped. This
/// is the whole point of a captured fixture: a synthetic one only proves the test author's model.
#[test]
fn the_production_manifest_parses_with_nothing_skipped() {
    let parsed = manifest::parse(&fixture("manifest-production.json")).expect("production manifest");
    assert_eq!(parsed.skipped_products, 0, "a live manifest must not lose products to the client's own strictness");
    let ids: Vec<&str> = parsed.products.iter().map(|product| product.id.as_str()).collect();
    assert!(ids.contains(&"dwd-rv") && ids.contains(&"icon-eu") && ids.contains(&"us") && ids.contains(&"gfs"));
    let dwd = parsed.products.iter().find(|product| product.id == "dwd-rv").unwrap();
    assert_eq!(dwd.tier, 1);
    assert!(dwd.bounds.is_well_formed());
    assert!(!dwd.attribution.text.is_empty(), "attribution is manifest data, not UI guesswork");
}

/// A document the *baker* wrote, read by the client's independent model. The two are separate
/// implementations of one JSON Schema; this is what stops them drifting apart silently.
#[test]
fn a_baker_written_manifest_round_trips_through_the_client_model() {
    use obc_wx_bake::manifest as baked;
    let source: baked::Manifest = serde_json::from_slice(&fixture("manifest-production.json")).expect("baker parse");
    let text = baked::to_json(&source);
    let parsed = manifest::parse(text.as_bytes()).expect("client parse");
    assert_eq!(parsed.skipped_products, 0);
    assert_eq!(parsed.products.len(), source.products.len());
    for (client, baker) in parsed.products.iter().zip(&source.products) {
        assert_eq!(client.id, baker.id);
        assert_eq!(client.tier, baker.tier);
        assert_eq!(client.frames.len(), baker.frames.len());
        assert_eq!(client.bounds.south_udeg, baker.bbox_udeg.south_udeg);
        assert_eq!(client.staleness_deadline, baked::parse_rfc3339(&baker.staleness_deadline).unwrap());
    }
}

/// Additive fields are the baker's normal way of shipping. A client that refused them would turn
/// every future deploy into an outage, so tolerance here is a requirement, not laxity.
#[test]
fn an_unknown_field_is_tolerated_and_an_unknown_version_is_not() {
    let mut document: serde_json::Value = serde_json::from_slice(&fixture("manifest-production.json")).expect("json");
    document["future_field"] = serde_json::json!("whatever the baker adds next");
    document["products"][0]["another_new_key"] = serde_json::json!(7);
    assert!(manifest::parse(document.to_string().as_bytes()).is_ok());

    document["version"] = serde_json::json!(2);
    assert_eq!(
        manifest::parse(document.to_string().as_bytes()),
        Err(manifest::ManifestError::UnsupportedVersion(2)),
        "an unknown document version is a hard stop; guessing at a format is not an option"
    );
}

/// One broken adapter must cost only its own product. The rest of the service keeps working.
#[test]
fn a_malformed_product_is_skipped_and_counted_rather_than_failing_the_document() {
    let mut document: serde_json::Value = serde_json::from_slice(&fixture("manifest-production.json")).expect("json");
    let good = document["products"].as_array().unwrap().len();
    document["products"][1]["tier"] = serde_json::json!("radar"); // type-wrong
    document["products"][2]["frames"][0]["object_crc32"] = serde_json::json!("not-hex");
    let parsed = manifest::parse(document.to_string().as_bytes()).expect("still a manifest");
    assert_eq!(parsed.skipped_products, 2);
    assert_eq!(parsed.products.len(), good - 2);
}

// ── selection ──────────────────────────────────────────────────────────────────────────────

fn three_tier_manifest(now: i64) -> (manifest::Manifest, Corridor) {
    let (frame, header) = frame_json("wx/v1/fixtures/multipage", &vector("grid-multipage.obcg"));
    let radar = product_json("radar", 1, &header, now + 600, &frame);
    let model = product_json("model", 2, &header, now + 600, &frame);
    let floor = product_json("floor", 3, &header, now + 600, &frame);
    let parsed = manifest::parse(manifest_json(now, &[floor, radar, model]).as_bytes()).expect("manifest");
    let mid_lat = ((header.south_lat_udeg as i64 + header.north_lat_udeg()) / 2) as i32;
    let mid_lon = ((header.west_lon_udeg as i64 + header.east_lon_udeg()) / 2) as i32;
    // A corridor two cells wide, safely inside the product window.
    let corridor = Corridor {
        bounds: manifest::Bbox {
            south_udeg: i64::from(mid_lat) - i64::from(header.cell_lat_udeg),
            north_udeg: i64::from(mid_lat) + i64::from(header.cell_lat_udeg),
            west_udeg: i64::from(mid_lon) - i64::from(header.cell_lon_udeg),
            east_udeg: i64::from(mid_lon) + i64::from(header.cell_lon_udeg),
        },
        lat_udeg: mid_lat,
        lon_udeg: mid_lon,
    };
    (parsed, corridor)
}

#[test]
fn the_highest_tier_covering_the_corridor_wins() {
    let now = 1_800_000_000;
    let (manifest, corridor) = three_tier_manifest(now);
    let (chosen, report) = select::select(&manifest, &corridor, now);
    assert_eq!(chosen.expect("a product").id, "radar");
    assert!(report.expired.is_empty());
}

/// Expiry is visible, never silent: the expired tier cannot be chosen, cannot shadow the tier
/// below it, and is named in the report so a panel can say *why* the rider dropped a tier.
#[test]
fn an_expired_tier_is_skipped_and_reported_rather_than_used() {
    let now = 1_800_000_000;
    let (frame, header) = frame_json("wx/v1/fixtures/multipage", &vector("grid-multipage.obcg"));
    let stale_radar = product_json("radar", 1, &header, now - 1, &frame);
    let fresh_model = product_json("model", 2, &header, now + 600, &frame);
    let manifest = manifest::parse(manifest_json(now, &[stale_radar, fresh_model]).as_bytes()).expect("manifest");
    let (_, corridor) = three_tier_manifest(now);
    let (chosen, report) = select::select(&manifest, &corridor, now);
    assert_eq!(chosen.expect("a product").id, "model");
    assert_eq!(report.expired, vec!["radar".to_string()]);
}

#[test]
fn every_covering_product_expired_is_an_explicit_reason_not_a_dry_map() {
    let now = 1_800_000_000;
    let (frame, header) = frame_json("wx/v1/fixtures/multipage", &vector("grid-multipage.obcg"));
    let manifest =
        manifest::parse(manifest_json(now, &[product_json("radar", 1, &header, now - 60, &frame)]).as_bytes())
            .expect("manifest");
    let (_, corridor) = three_tier_manifest(now);
    let (chosen, report) = select::select(&manifest, &corridor, now);
    assert_eq!(chosen.unwrap_err(), NoRainMap::AllCoveringProductsExpired { latest_deadline: now - 60 });
    assert_eq!(report.expired, vec!["radar".to_string()]);
}

/// A corridor half outside a product is not answerable by it. Overlap is not coverage.
#[test]
fn partial_overlap_is_not_coverage() {
    let now = 1_800_000_000;
    let (manifest, corridor) = three_tier_manifest(now);
    let product = &manifest.products[0];
    let outside = Corridor {
        bounds: manifest::Bbox {
            south_udeg: product.bounds.south_udeg - 1,
            north_udeg: product.bounds.north_udeg,
            west_udeg: product.bounds.west_udeg,
            east_udeg: product.bounds.east_udeg,
        },
        ..corridor
    };
    let (chosen, _) = select::select(&manifest, &corridor, now);
    assert!(chosen.is_ok());
    let (chosen, _) = select::select(&manifest, &outside, now);
    assert_eq!(chosen.unwrap_err(), NoRainMap::CorridorNotCovered);
}

/// Freshness is not answerability: a product inside its deadline whose frames all sit outside the
/// usable window must fall through, not shadow the tier below with nothing.
#[test]
fn a_fresh_product_with_no_usable_frames_falls_through() {
    let now = 1_800_000_000;
    let (manifest, corridor) = three_tier_manifest(now);
    // The vectors' frames are stamped far from `now + 2 h`, so at their own instant they are
    // usable and a day later they are not — same manifest, different answer.
    let (chosen, _) = select::select(&manifest, &corridor, now);
    assert!(chosen.is_ok());
    let (chosen, _) = select::select(&manifest, &corridor, now + 30 * 3600);
    assert!(matches!(chosen.unwrap_err(), NoRainMap::AllCoveringProductsExpired { .. } | NoRainMap::NoFramesInWindow));
}

// ── corridor extraction ────────────────────────────────────────────────────────────────────

fn multipage_setup() -> (FixtureHttp, manifest::Frame, manifest::Bbox) {
    let bytes = vector("grid-multipage.obcg");
    let (frame_json_text, header) = frame_json("wx/v1/fixtures/multipage", &bytes);
    let product = product_json("radar", 1, &header, header.valid_at + 600, &frame_json_text);
    let parsed = manifest::parse(manifest_json(header.valid_at, &[product]).as_bytes()).expect("manifest");
    let frame = parsed.products[0].frames[0].clone();
    // The pinned corridor: cells (20,20)…(39,39) of the 40 × 40 grid — the same window the Swift
    // request-accounting test and `host/obc-vectors` use.
    let bounds = manifest::Bbox {
        south_udeg: i64::from(header.south_lat_udeg) + 20 * i64::from(header.cell_lat_udeg),
        north_udeg: i64::from(header.south_lat_udeg) + 40 * i64::from(header.cell_lat_udeg) - 1,
        west_udeg: i64::from(header.west_lon_udeg) + 20 * i64::from(header.cell_lon_udeg),
        east_udeg: i64::from(header.west_lon_udeg) + 40 * i64::from(header.cell_lon_udeg) - 1,
    };
    let http = FixtureHttp::new().with_object(corridor::join(ORIGIN, &frame.key), bytes);
    (http, frame, bounds)
}

/// The frozen §7 read pattern: the header, the directory pages arithmetic says cover the
/// corridor, and only the non-dry tiles those pages name. Nothing else — a corridor consumer that
/// quietly downloaded the object would still pass a cell-value test, so the *ledger* is the test.
#[test]
fn corridor_extraction_reads_only_the_header_covering_pages_and_needed_tiles() {
    let (mut http, frame, bounds) = multipage_setup();
    let crop = corridor::crop_frame(&mut http, ORIGIN, &frame, &bounds).expect("crop");
    assert_eq!(crop.width, 20);
    assert_eq!(crop.height, 20);

    let header_reads = http.ledger.iter().filter(|(_, range)| *range == Some((0, 127))).count();
    assert_eq!(header_reads, 1, "exactly one header read");
    let object = 406u64; // grid-multipage.obcg
    for (_, range) in &http.ledger {
        let (_start, end) = range.expect("every corridor read is a Range read");
        assert!(end < object, "a corridor read must never run past the object");
    }
    assert!(
        http.fetched_bytes() < object,
        "corridor extraction moved {} of {object} bytes — it must never need the whole frame",
        http.fetched_bytes()
    );
}

/// The manifest is a plan; the header is the truth. A manifest that re-stamped a frame to look
/// current is exactly the attack this catches, before a single cell is trusted.
#[test]
fn a_manifest_that_disagrees_with_the_header_refuses_the_frame() {
    for mutate in [
        (|frame: &mut manifest::Frame| frame.valid_at += 60) as fn(&mut manifest::Frame),
        |frame: &mut manifest::Frame| frame.object_crc32 ^= 1,
        |frame: &mut manifest::Frame| frame.bytes += 1,
        |frame: &mut manifest::Frame| frame.geometry.width += 1,
    ] {
        let (mut http, mut frame, bounds) = multipage_setup();
        mutate(&mut frame);
        assert!(
            corridor::crop_frame(&mut http, ORIGIN, &frame, &bounds).is_err(),
            "a frame whose header contradicts its manifest entry must be refused"
        );
    }
}

/// A flipped bit anywhere in a fetched page or tile is caught by the production CRCs — the
/// simulator's corrupt-tile control is a transport fault, not a second validation path.
#[test]
fn a_corrupted_fetch_is_caught_by_the_production_crcs() {
    let (http, frame, bounds) = multipage_setup();
    let reads = {
        let (mut http, frame, bounds) = multipage_setup();
        let _ = corridor::crop_frame(&mut http, ORIGIN, &frame, &bounds);
        http.ledger.len()
    };
    for index in 0..reads as u32 {
        let mut faulty = FaultyHttp::new(
            http.clone(),
            FailureControls { corrupt_request: Some(index), ..FailureControls::default() },
        );
        assert!(
            corridor::crop_frame(&mut faulty, ORIGIN, &frame, &bounds).is_err(),
            "corrupting request {index} must be caught, not decoded into weather"
        );
    }
}

#[test]
fn a_truncated_fetch_is_refused() {
    let (http, frame, bounds) = multipage_setup();
    let mut faulty = FaultyHttp::new(http, FailureControls { truncate_request: Some(0), ..FailureControls::default() });
    assert!(corridor::crop_frame(&mut faulty, ORIGIN, &frame, &bounds).is_err());
}

/// The dry sentinel means dry; a no-data tile is *encoded*. A corridor over an all-no-data frame
/// must come back as no-data cells, never as a dry map.
#[test]
fn a_no_data_tile_decodes_to_no_data_and_never_to_dry() {
    let bytes = vector("grid-nodata-tile.obcg");
    let (frame_text, header) = frame_json("wx/v1/fixtures/nodata", &bytes);
    let product = product_json("radar", 1, &header, header.valid_at + 600, &frame_text);
    let parsed = manifest::parse(manifest_json(header.valid_at, &[product]).as_bytes()).expect("manifest");
    let frame = parsed.products[0].frames[0].clone();
    let bounds = parsed.products[0].bounds;
    let mut http = FixtureHttp::new().with_object(corridor::join(ORIGIN, &frame.key), bytes);
    let bounds = manifest::Bbox { north_udeg: bounds.north_udeg - 1, east_udeg: bounds.east_udeg - 1, ..bounds };
    let crop = corridor::crop_frame(&mut http, ORIGIN, &frame, &bounds).expect("crop");
    assert!(crop.cells.iter().all(|&cell| cell == obc_formats::precip4::INTENSITY_NODATA));
    assert!(crop.partial, "unavailable cells must mark the frame partial");
}

// ── MET ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_captured_met_document_decodes_to_24_consecutive_hours() {
    let hourly = met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode");
    for (index, record) in hourly.records.iter().enumerate() {
        assert_eq!(record.valid_time_offset_s, index as u32 * 3600);
        assert_ne!(record.temperature_deci_c, obc_formats::obcw::TEMP_UNAVAILABLE);
        assert_ne!(record.condition, obc_formats::obcw::CONDITION_UNAVAILABLE);
        assert!(record.wind_from_deg < 360);
    }
}

/// Freiburg supplies neither optional field. They must read *unavailable*, because a zero here
/// would be a forecast of "no chance of rain" that MET never made.
#[test]
fn absent_optional_fields_are_unavailable_and_never_zero() {
    let hourly = met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode");
    assert!(hourly
        .records
        .iter()
        .all(|record| record.precipitation_probability_pct == obc_formats::obcw::PROBABILITY_UNAVAILABLE));
    assert!(hourly.records.iter().all(|record| record.wind_gust_deci_ms == obc_formats::obcw::WIND_SPEED_UNAVAILABLE));
}

/// Present-but-wrong is malformed. Silently downgrading a bad value to "unavailable" would hide a
/// broken provider behind a plausible screen.
#[test]
fn a_present_but_invalid_optional_field_is_malformed() {
    let mut document: serde_json::Value = serde_json::from_slice(&fixture("met-freiburg-24h.json")).unwrap();
    document["properties"]["timeseries"][3]["data"]["next_1_hours"]["details"]["probability_of_precipitation"] =
        serde_json::json!(140.0);
    assert!(met::decode(document.to_string().as_bytes(), 0).is_err());

    let mut document: serde_json::Value = serde_json::from_slice(&fixture("met-freiburg-24h.json")).unwrap();
    document["properties"]["meta"]["units"]["air_temperature"] = serde_json::json!("fahrenheit");
    assert!(met::decode(document.to_string().as_bytes(), 0).is_err(), "a unit change is not something to convert");
}

/// The frozen WX1 table, including the order that makes thunder beat every precipitation family
/// and an unknown code become a truthful gap rather than a guess.
#[test]
fn the_symbol_table_is_the_frozen_wx1_mapping() {
    use obc_formats::obcw::*;
    let cases = [
        ("clearsky_day", CONDITION_CLEAR),
        ("fair_polartwilight", CONDITION_MOSTLY_CLEAR),
        ("partlycloudy_night", CONDITION_PARTLY_CLOUDY),
        ("cloudy", CONDITION_OVERCAST),
        ("fog", CONDITION_FOG),
        ("lightrain", CONDITION_DRIZZLE),
        ("heavyrain", CONDITION_RAIN),
        ("lightrainshowers_day", CONDITION_SHOWERS),
        ("sleetshowers_night", CONDITION_SLEET),
        ("heavysnow", CONDITION_SNOW),
        ("rainshowersandthunder_day", CONDITION_THUNDERSTORM),
        ("heavysleetandthunder", CONDITION_THUNDERSTORM),
        ("something_new_met_invented", CONDITION_UNAVAILABLE),
    ];
    for (symbol, expected) in cases {
        assert_eq!(met::condition_for(symbol), Some(expected), "{symbol}");
    }
    assert_eq!(met::condition_for("  "), None, "an empty code is a broken document, not a gap");
}

/// `Expires` is absolute: inside it MET is not contacted at all. That rule is MET's terms, and
/// the cache *is* the throttle.
#[test]
fn met_is_not_contacted_again_inside_its_expires_window() {
    // Derived from the capture's own first hour, so a re-capture cannot rot this test.
    let now = hourly().valid_from;
    let http_date = |unix: i64| {
        chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string()
    };
    let url = format!("{MET_ENDPOINT}?lat=48.0600&lon=7.9000");
    let mut http = FixtureHttp::new().with_object(url.clone(), fixture("met-freiburg-24h.json")).with_headers(
        url,
        None,
        Some(&http_date(now)),
        Some(&http_date(now + 1_800)),
    );
    let mut client = met::MetClient::new().with_endpoint(MET_ENDPOINT);
    client.hourly(&mut http, 48_060_000, 7_900_000, now).expect("first fetch");
    let after_first = http.ledger.len();
    client.hourly(&mut http, 48_060_000, 7_900_000, now + 60).expect("second call");
    assert_eq!(http.ledger.len(), after_first, "a second call inside Expires must issue no request at all");
    // Past `Expires` the client revalidates rather than staying silent forever.
    client.hourly(&mut http, 48_060_000, 7_900_000, now + 1_801).expect("revalidation");
    assert_eq!(http.ledger.len(), after_first + 1);
}

/// Four decimals is simultaneously the privacy contract (~11 m) and the refetch threshold: a
/// rider who has not moved that far produces the same key and therefore no request.
#[test]
fn the_met_url_rounds_the_coordinate_to_four_decimals() {
    let client = met::MetClient::new().with_endpoint(MET_ENDPOINT);
    assert_eq!(client.url(47_123_456, -7_987_654), format!("{MET_ENDPOINT}?lat=47.1235&lon=-7.9877"));
    assert_eq!(client.url(47_123_456, -7_987_654), client.url(47_123_460, -7_987_650));
}

// ── bundle ─────────────────────────────────────────────────────────────────────────────────

fn crop(valid_at: i64, cell: u32, width: u32, height: u32, value: u8) -> Crop {
    Crop {
        valid_at,
        source_class: SourceClass::Forecast,
        south_udeg: 47_000_000,
        west_udeg: 7_000_000,
        cell_lat_udeg: cell,
        cell_lon_udeg: cell,
        cell_size_m: 1_000,
        width,
        height,
        cells: vec![value; (width * height) as usize],
        partial: false,
    }
}

fn hourly() -> met::Hourly {
    met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode")
}

#[test]
fn a_built_bundle_opens_through_the_production_reader() {
    let hourly = hourly();
    let crops = [crop(hourly.valid_from, 9_000, 32, 32, 4), crop(hourly.valid_from + 900, 9_000, 32, 32, 6)];
    let (bytes, report) =
        bundle::build(1, 0xABCD, hourly.valid_from, (47_100_000, 7_100_000), &crops, &hourly).expect("build");
    assert_eq!(report.frames, 2);
    assert!(bytes.len() <= bundle::PRODUCER_CAP);
    let source = obc_formats::io::SliceSource(&bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("the device must be able to open it");
    assert_eq!(reader.header().generation, 1);
    assert_eq!(reader.header().request_id, 0xABCD);
}

/// A composed product's fine observation frame cannot tile a coarse model window. It is dropped
/// and counted — never stretched onto a lattice it does not belong to.
#[test]
fn a_frame_whose_lattice_cannot_tile_the_window_is_dropped_not_resampled() {
    let hourly = hourly();
    let crops = [
        crop(hourly.valid_from, 10_000, 30, 30, 5),       // 1 km observation
        crop(hourly.valid_from + 900, 27_000, 11, 11, 3), // 3 km model — the coarsest, so it sets the window
    ];
    let (bytes, report) =
        bundle::build(1, 1, hourly.valid_from, (47_100_000, 7_100_000), &crops, &hourly).expect("build");
    assert_eq!(report.frames, 1, "the coarse frame tiles its own window");
    assert_eq!(report.dropped_incompatible, 1, "the fine frame is dropped rather than resampled");
    assert!(obc_weather::WeatherReader::open(&obc_formats::io::SliceSource(&bytes)).is_ok());
}

/// No rain product at all still produces a bundle: the hourly half stands on its own, and the
/// screens get the explicit hourly-only state rather than a guess.
#[test]
fn an_hourly_only_bundle_is_still_a_valid_bundle() {
    let hourly = hourly();
    let bytes = bundle::hourly_only(1, 1, hourly.valid_from, (48_060_000, 7_900_000), &hourly).expect("build");
    let source = obc_formats::io::SliceSource(&bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.header().frame_count, 0);
}

/// The producer cap is not negotiable: an oversized corridor shrinks its window (keeping every
/// timestamp) before it will drop a frame (which would put a hole in the two-hour timeline).
#[test]
fn an_oversized_corridor_shrinks_its_window_before_dropping_a_frame() {
    let hourly = hourly();
    // Deliberately incompressible cells: a uniform grid would RLE4 down to nothing and never
    // reach the cap, so the test would pass without exercising the shrink at all.
    let crops: Vec<Crop> = (0..9)
        .map(|index| {
            let mut crop = crop(hourly.valid_from + index * 900, 9_000, 200, 200, 0);
            for (position, cell) in crop.cells.iter_mut().enumerate() {
                *cell = ((position as u32 * 7 + index as u32) % 13) as u8;
            }
            crop
        })
        .collect();
    let (bytes, report) =
        bundle::build(1, 1, hourly.valid_from, (47_100_000, 7_100_000), &crops, &hourly).expect("build");
    assert!(bytes.len() <= bundle::PRODUCER_CAP);
    assert!(report.shrinks > 0);
    assert_eq!(report.dropped_oversize, 0, "every timestamp survives; only the window gives ground");
    assert_eq!(report.frames, 9);
}

// ── the whole job ──────────────────────────────────────────────────────────────────────────

fn wired_client(now: i64) -> (FixtureHttp, WeatherClient, Corridor) {
    let bytes = vector("grid-multipage.obcg");
    let (frame_text, header) = frame_json("wx/v1/fixtures/multipage", &bytes);
    let product = product_json("radar", 1, &header, now + 600, &frame_text);
    let document = manifest_json(now, &[product]);
    let met_url = format!("{MET_ENDPOINT}?lat=0.0000&lon=0.0000");
    let mid_lat = ((header.south_lat_udeg as i64 + header.north_lat_udeg()) / 2) as i32;
    let mid_lon = ((header.west_lon_udeg as i64 + header.east_lon_udeg()) / 2) as i32;
    let corridor = Corridor {
        bounds: manifest::Bbox {
            south_udeg: i64::from(mid_lat) - i64::from(header.cell_lat_udeg),
            north_udeg: i64::from(mid_lat) + i64::from(header.cell_lat_udeg),
            west_udeg: i64::from(mid_lon) - i64::from(header.cell_lon_udeg),
            east_udeg: i64::from(mid_lon) + i64::from(header.cell_lon_udeg),
        },
        lat_udeg: 0,
        lon_udeg: 0,
    };
    let http = FixtureHttp::new()
        .with_object(MANIFEST_URL, document.into_bytes())
        .with_object(corridor::join(ORIGIN, "wx/v1/fixtures/multipage"), bytes)
        .with_object(met_url, fixture("met-freiburg-24h.json"));
    (http, WeatherClient::new(ORIGIN).with_met_endpoint(MET_ENDPOINT), corridor)
}

#[test]
fn a_whole_fetch_produces_a_device_readable_bundle_with_the_product_recorded() {
    let now = 1_800_000_000;
    let (mut http, mut client, corridor) = wired_client(now);
    let bundle = client.fetch(&mut http, &corridor, now, 42).expect("fetch");
    assert_eq!(bundle.diagnostics.product.as_ref().map(|(id, tier)| (id.as_str(), *tier)), Some(("radar", 1)));
    assert!(obc_weather::WeatherReader::open(&obc_formats::io::SliceSource(&bundle.bytes)).is_ok());
    assert!(bundle.diagnostics.service_requests > 0);
}

/// Offline is a truthful hourly-only bundle with a stated reason. Never a blank screen, never a
/// fabricated map, and never "dry".
#[test]
fn offline_degrades_to_a_stated_hourly_only_bundle() {
    let now = 1_800_000_000;
    let (http, mut client, corridor) = wired_client(now);
    // MET answers from its own cache; only the OBC service is down. Prime the cache first.
    let mut warm = http.clone();
    client.fetch(&mut warm, &corridor, now, 1).expect("warm fetch");
    let mut offline = FaultyHttp::new(http, FailureControls { offline: true, ..FailureControls::default() });
    let bundle = client.fetch(&mut offline, &corridor, now + 3600, 2).expect("still a bundle");
    assert!(bundle.diagnostics.no_rain_map.is_some(), "the reason must be stated, not inferred");
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.header().frame_count, 0, "no rain product means no rain frames, not empty ones");
}

/// The manifest caches for at most 60 s (OBCG §10) and revalidates with its ETag past that — one
/// request per minute at most, whatever the refresh cadence asks for.
#[test]
fn the_manifest_is_not_refetched_inside_its_sixty_second_window() {
    let now = 1_800_000_000;
    let (mut http, mut client, _) = wired_client(now);
    client.manifest(&mut http, now).expect("first");
    let after_first = http.ledger.len();
    client.manifest(&mut http, now + 30).expect("cached");
    assert_eq!(http.ledger.len(), after_first);
    client.manifest(&mut http, now + 61).expect("revalidated");
    assert_eq!(http.ledger.len(), after_first + 1);
}
