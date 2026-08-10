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
        // All four edges: a bbox that agreed on one corner and not the others would still pick the
        // wrong product for a corridor, and containment is the whole selection rule.
        assert_eq!(client.bounds.south_udeg, baker.bbox_udeg.south_udeg, "{}", baker.id);
        assert_eq!(client.bounds.west_udeg, baker.bbox_udeg.west_udeg, "{}", baker.id);
        assert_eq!(client.bounds.north_udeg, baker.bbox_udeg.north_udeg, "{}", baker.id);
        assert_eq!(client.bounds.east_udeg, baker.bbox_udeg.east_udeg, "{}", baker.id);
        assert_eq!(client.cell_lat_udeg, baker.cell.lat_udeg);
        assert_eq!(client.cell_lon_udeg, baker.cell.lon_udeg);
        assert_eq!(client.nominal_m, baker.cell.nominal_m);
        assert_eq!(client.reference_time, baked::parse_rfc3339(&baker.reference_time).unwrap());
        assert_eq!(client.generated_at, baked::parse_rfc3339(&baker.generated_at).unwrap());
        assert_eq!(client.staleness_deadline, baked::parse_rfc3339(&baker.staleness_deadline).unwrap());
        assert_eq!(client.attribution.text, baker.attribution.text);
        assert_eq!(client.attribution.url, baker.attribution.url);
        assert_eq!(client.frames.len(), baker.frames.len());
        for (frame, baked_frame) in client.frames.iter().zip(&baker.frames) {
            // The key and the CRC are what a Range read is planned and verified against; the
            // valid_at is what freshness is judged on; the geometry is what the fetched header must
            // agree with before a cell is trusted. A drift in any one of them is a silent outage.
            assert_eq!(frame.key, baked_frame.key);
            assert_eq!(frame.offset_min, baked_frame.offset_min);
            assert_eq!(frame.bytes, baked_frame.bytes);
            assert_eq!(format!("0x{:08X}", frame.object_crc32), baked_frame.object_crc32);
            assert_eq!(frame.valid_at, baked::parse_rfc3339(&baked_frame.valid_at).unwrap());
            assert_eq!(
                frame.source_class,
                match baked_frame.source_class {
                    baked::SourceClass::Observation => manifest::SourceClass::Observation,
                    baked::SourceClass::Forecast => manifest::SourceClass::Forecast,
                }
            );
            let (geometry, baked_geometry) = (&frame.geometry, &baked_frame.geometry);
            assert_eq!(geometry.south_udeg, baked_geometry.south_udeg);
            assert_eq!(geometry.west_udeg, baked_geometry.west_udeg);
            assert_eq!(geometry.cell_lat_udeg, baked_geometry.cell_lat_udeg);
            assert_eq!(geometry.cell_lon_udeg, baked_geometry.cell_lon_udeg);
            assert_eq!(geometry.width, baked_geometry.width);
            assert_eq!(geometry.height, baked_geometry.height);
            assert_eq!(geometry.cell_size_m, baked_geometry.cell_size_m);
            assert_eq!(geometry.tile_edge, baked_geometry.tile_edge);
            assert_eq!(geometry.entries_per_page, baked_geometry.entries_per_page);
        }
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
        undirected: true,
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

/// The corridor the bundle tests describe: the crops' own region, generously boxed.
fn test_corridor() -> manifest::Bbox {
    manifest::Bbox { south_udeg: 47_000_000, west_udeg: 7_000_000, north_udeg: 47_500_000, east_udeg: 7_500_000 }
}

fn hourly() -> met::Hourly {
    met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode")
}

#[test]
fn a_built_bundle_opens_through_the_production_reader() {
    let hourly = hourly();
    let crops = [crop(hourly.valid_from, 9_000, 32, 32, 4), crop(hourly.valid_from + 900, 9_000, 32, 32, 6)];
    let (bytes, report) =
        bundle::build(1, 0xABCD, hourly.valid_from, (47_100_000, 7_100_000), &test_corridor(), &crops, &hourly)
            .expect("build");
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
        bundle::build(1, 1, hourly.valid_from, (47_100_000, 7_100_000), &test_corridor(), &crops, &hourly)
            .expect("build");
    assert_eq!(report.frames, 1, "the coarse frame tiles its own window");
    assert_eq!(report.dropped_incompatible, 1, "the fine frame is dropped rather than resampled");
    assert!(obc_weather::WeatherReader::open(&obc_formats::io::SliceSource(&bytes)).is_ok());
}

/// The companion to the drop above, and the reason the baker now treats nesting as a contract it
/// verifies before publishing: when the coarse lattice **is** an integer multiple of the fine one,
/// both frames survive, each at its own resolution and with nothing resampled.
///
/// This is the shipped US product's shape — a 1 km MRMS observation under a 3 km HRRR window. For
/// as long as HRRR published 27,000 x 34,000 microdegree cells over MRMS's 10,000, neither stride
/// divided, and every CONUS rider silently lost the only radar frame in the timeline.
#[test]
fn a_nesting_observation_frame_survives_at_its_own_resolution() {
    let hourly = hourly();
    let crops = [
        crop(hourly.valid_from, 10_000, 30, 30, 5),       // 1 km observation
        crop(hourly.valid_from + 900, 30_000, 10, 10, 3), // 3 km model, exactly 3x — sets the window
    ];
    let (bytes, report) =
        bundle::build(1, 1, hourly.valid_from, (47_100_000, 7_100_000), &test_corridor(), &crops, &hourly)
            .expect("build");
    assert_eq!(report.frames, 2, "both frames tile the window");
    assert_eq!(report.dropped_incompatible, 0, "nothing is refused");
    let source = obc_formats::io::SliceSource(&bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("the device must be able to open it");
    let observation = reader.frame(0).expect("observation frame");
    let model = reader.frame(1).expect("model frame");
    assert_eq!(
        (observation.width, observation.height),
        (30, 30),
        "the observation keeps its own 1 km lattice — it is not coarsened onto the model's"
    );
    assert_eq!(
        (model.width, model.height),
        (10, 10),
        "and the model keeps its own — the shared window is an extent, not a resolution"
    );
}

/// No rain product at all still produces a bundle: the hourly half stands on its own, and the
/// screens get the explicit hourly-only state rather than a guess.
#[test]
fn an_hourly_only_bundle_is_still_a_valid_bundle() {
    let hourly = hourly();
    let bytes = bundle::hourly_only(1, 1, hourly.valid_from, (48_060_000, 7_900_000), &test_corridor(), &hourly)
        .expect("build");
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
        bundle::build(1, 1, hourly.valid_from, (47_100_000, 7_100_000), &test_corridor(), &crops, &hourly)
            .expect("build");
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
        undirected: true,
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

// ── the manifest's revalidation window ─────────────────────────────────────────────────────

/// OBCG §10 in full: reuse for 60 s without asking, then **revalidate with the stored ETag** — and
/// a `304` restarts the window rather than leaving the client asking again a second later. A
/// revalidation that did not re-anchor `fetched_at` would turn one request per minute into one per
/// poll for every rider on the service.
#[test]
fn the_manifest_revalidates_with_its_etag_and_a_304_restarts_the_window() {
    let now = 1_800_000_000;
    let (http, mut client, _) = wired_client(now);
    let mut http = http.with_headers(MANIFEST_URL, Some("\"bake-42\""), None, None);

    client.manifest(&mut http, now).expect("first fetch");
    let after_first = http.ledger.len();
    client.manifest(&mut http, now + 30).expect("inside the window");
    assert_eq!(http.ledger.len(), after_first, "no request at all inside the 60 s window");

    // Past the window: one conditional request, answered 304 because the ETag still matches.
    client.manifest(&mut http, now + 61).expect("revalidated");
    assert_eq!(http.ledger.len(), after_first + 1);
    let (_, _) = http.ledger.last().expect("a request was made");

    // The 304 re-anchored the window: the next 60 s are free again.
    client.manifest(&mut http, now + 100).expect("inside the restarted window");
    assert_eq!(http.ledger.len(), after_first + 1, "a 304 must restart the freshness window");
    client.manifest(&mut http, now + 122).expect("revalidated again");
    assert_eq!(http.ledger.len(), after_first + 2);
}

// ── the privacy contract ───────────────────────────────────────────────────────────────────

/// **The epic's headline invariant, as an assertion.** Every request to the OBC service is a
/// key-addressed read of an immutable object: no query string, no coordinate, nothing derived from
/// one. The corridor decides *which* objects — and that is all the service ever learns. MET is the
/// single third party that receives a position, and it receives it rounded to four decimals.
#[test]
fn the_service_never_receives_a_coordinate() {
    let now = 1_800_000_000;
    let (mut http, mut client, corridor) = wired_client(now);
    client.fetch(&mut http, &corridor, now, 7).expect("fetch");

    let mut met_seen = 0;
    for (url, _) in &http.ledger {
        if url.starts_with(MET_ENDPOINT) {
            met_seen += 1;
            continue;
        }
        assert!(url.starts_with(ORIGIN), "an OBC request must address the service origin: {url}");
        assert!(!url.contains('?'), "an OBC request must carry no query string at all: {url}");
        for forbidden in ["lat", "lon", "coord", "="] {
            assert!(!url.contains(forbidden), "{url} contains {forbidden:?} — the service must learn no position");
        }
        // Not even the digits: a corridor edge smuggled into a key would defeat the whole design.
        for udeg in [corridor.lat_udeg, corridor.lon_udeg, corridor.bounds.south_udeg as i32] {
            let digits = udeg.abs().to_string();
            assert!(!url.contains(&digits), "{url} contains the coordinate {digits}");
        }
    }
    assert_eq!(met_seen, 1, "exactly one request carries the rider's position, and it goes to MET");
    let met_url = http.ledger.iter().find(|(url, _)| url.starts_with(MET_ENDPOINT)).map(|(url, _)| url.clone());
    assert_eq!(met_url.as_deref(), Some(format!("{MET_ENDPOINT}?lat=0.0000&lon=0.0000").as_str()));
}

// ── partial content ────────────────────────────────────────────────────────────────────────

/// A server that ignores `Range` and streams the whole object is answering lawfully; the client
/// slices it itself rather than reading the head of a file as if it were the middle. The proof is
/// that the crop is **identical** to the one an honest 206 origin produces.
#[test]
fn a_200_to_a_range_request_is_sliced_and_produces_the_same_crop() {
    let (mut honest, frame, bounds) = multipage_setup();
    let expected = corridor::crop_frame(&mut honest, ORIGIN, &frame, &bounds).expect("crop over 206");

    let (whole_object, frame, bounds) = multipage_setup();
    let mut whole_object = whole_object.ignoring_ranges();
    let sliced = corridor::crop_frame(&mut whole_object, ORIGIN, &frame, &bounds).expect("crop over 200");
    assert_eq!(sliced, expected, "the same bytes must decode to the same crop, whatever status carried them");
}

/// An origin that lies about partial content: `206` with more bytes than were asked for, or a
/// `Content-Range` naming other bytes. Both are refusals — slicing an over-long "partial" answer
/// would silently accept a server contradicting itself, and every CRC below would then blame the
/// producer for it.
#[test]
fn a_206_that_does_not_match_the_request_is_refused() {
    use obc_wx_client::http::{Http, HttpError, Request, Response};

    struct Lying {
        object: Vec<u8>,
        honest_content_range: bool,
    }
    impl Http for Lying {
        fn perform(&mut self, request: &Request, _cap: u64) -> Result<Response, HttpError> {
            let (start, end) = request.range.expect("a corridor read is always a Range read");
            Ok(Response {
                status: 206,
                body: self.object.clone(), // the whole object, under a partial-content status
                content_range: Some(if self.honest_content_range {
                    format!("bytes 0-{}/{}", self.object.len() - 1, self.object.len())
                } else {
                    format!("bytes {start}-{end}/{}", self.object.len())
                }),
                ..Response::empty()
            })
        }
    }

    let bytes = vector("grid-multipage.obcg");
    let (_, frame, bounds) = multipage_setup();
    for honest_content_range in [true, false] {
        let mut http = Lying { object: bytes.clone(), honest_content_range };
        let error = corridor::crop_frame(&mut http, ORIGIN, &frame, &bounds).expect_err("must be refused");
        assert!(
            matches!(error, corridor::CropError::Http(HttpError::RangeNotHonoured(_))),
            "an over-long 206 is a range that was not honoured, not something to slice: {error:?}"
        );
    }
}

/// 2xx is not a licence. Only `200` and `206` describe the bytes a corridor read asked for; a
/// `204`/`203`/`205` answer is refused rather than decoded (the phone refuses them too).
#[test]
fn only_200_and_206_are_acted_on() {
    use obc_wx_client::http::{Http, HttpError, Request, Response};

    struct Status(u16);
    impl Http for Status {
        fn perform(&mut self, _request: &Request, _cap: u64) -> Result<Response, HttpError> {
            Ok(Response { status: self.0, ..Response::empty() })
        }
    }
    let (_, frame, bounds) = multipage_setup();
    for status in [201u16, 202, 203, 204, 205, 226] {
        let mut http = Status(status);
        let error = corridor::crop_frame(&mut http, ORIGIN, &frame, &bounds).expect_err("must be refused");
        assert!(
            matches!(error, corridor::CropError::Http(HttpError::Status { code, .. }) if code == status),
            "status {status} must be refused as a status, not decoded"
        );
    }
}

// ── the frame cache ────────────────────────────────────────────────────────────────────────

/// Frame objects are immutable by the publishing contract, so a corridor already cropped out of
/// one is knowledge, not a guess. At a 30-minute cadence most of a product's timeline carries over
/// between fetches; re-reading it would be bytes spent to learn what the client already knows.
#[test]
fn an_immutable_frame_is_cropped_once_and_then_served_from_the_cache() {
    let now = 1_800_000_000;
    let (mut http, mut client, corridor) = wired_client(now);
    let first = client.fetch(&mut http, &corridor, now, 1).expect("first fetch");
    assert_eq!(first.diagnostics.cached_frames, 0, "the first fetch has nothing to reuse");
    let frame_url = corridor::join(ORIGIN, "wx/v1/fixtures/multipage");
    let frame_reads = |http: &FixtureHttp| http.ledger.iter().filter(|(url, _)| *url == frame_url).count();
    let after_first = frame_reads(&http);
    assert!(after_first > 1, "the first fetch really did read header, pages and tiles");

    let second = client.fetch(&mut http, &corridor, now + 1, 2).expect("second fetch");
    assert_eq!(frame_reads(&http), after_first, "an immutable frame must not be fetched twice");
    assert_eq!(second.diagnostics.cached_frames, 1);
    assert_eq!(second.diagnostics.service_requests, 0, "nothing at all was needed from the service");
}

/// The window is half the key: a wider corridor asks a different question and must be a miss, not
/// a smaller crop stretched to fit.
#[test]
fn a_wider_corridor_misses_the_cache() {
    let now = 1_800_000_000;
    let (mut http, mut client, corridor) = wired_client(now);
    client.fetch(&mut http, &corridor, now, 1).expect("first fetch");
    let wider = Corridor {
        bounds: manifest::Bbox {
            south_udeg: corridor.bounds.south_udeg - 50_000,
            north_udeg: corridor.bounds.north_udeg + 50_000,
            west_udeg: corridor.bounds.west_udeg - 50_000,
            east_udeg: corridor.bounds.east_udeg + 50_000,
        },
        ..corridor.clone()
    };
    let second = client.fetch(&mut http, &wider, now + 1, 2).expect("second fetch");
    assert_eq!(second.diagnostics.cached_frames, 0, "a different window is a miss");
    assert!(second.diagnostics.service_requests > 0);
}

// ── failure controls ───────────────────────────────────────────────────────────────────────

/// `--weather-fail-from`: the service starts answering an error mid-job. The cached manifest and
/// the cached hourly forecast both stand — neither is an outage — but the rain half is dropped
/// with a stated reason rather than half-drawn.
#[test]
fn a_service_that_starts_failing_keeps_the_caches_and_states_why_there_is_no_rain() {
    let now = 1_800_000_000;
    let (http, mut client, corridor) = wired_client(now);
    let mut warm = http.clone();
    client.fetch(&mut warm, &corridor, now, 1).expect("warm fetch");

    // A corridor one cell wider misses the frame cache, so the failing reads are actually
    // attempted; the rider's position is unchanged, so MET's cache still answers.
    let shifted = Corridor {
        bounds: manifest::Bbox { north_udeg: corridor.bounds.north_udeg + 20_000, ..corridor.bounds },
        ..corridor.clone()
    };
    let mut failing = FaultyHttp::new(http, FailureControls { fail_from: Some((0, 503)), ..Default::default() });
    let bundle = client.fetch(&mut failing, &shifted, now + 61, 2).expect("still a bundle");
    assert!(bundle.diagnostics.service_requests > 0, "the failing reads were genuinely attempted");
    assert!(bundle.diagnostics.no_rain_map.is_some(), "the reason must be stated");
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.header().frame_count, 0, "a failing service produces no frames, never invented ones");
    assert_ne!(reader.header().valid_from, 0, "…while the cached hourly forecast still ships");
}

// ── the corridor projection ────────────────────────────────────────────────────────────────

fn span_km(corridor: &Corridor) -> (f64, f64) {
    let lat = (corridor.bounds.north_udeg - corridor.bounds.south_udeg) as f64 / 1e6 * 111.32;
    let cos = (f64::from(corridor.lat_udeg) / 1e6).to_radians().cos();
    let lon = (corridor.bounds.east_udeg - corridor.bounds.west_udeg) as f64 / 1e6 * 111.32 * cos;
    (lon, lat)
}

/// No bearing and no speed vouched for: a disc of the floor radius. A rider who might go any
/// direction gets a disc, never a fabricated heading.
#[test]
fn a_fix_with_nothing_vouched_for_is_the_undirected_floor_disc() {
    let corridor =
        Corridor::projected(&select::Fix { lat_udeg: 48_060_000, lon_udeg: 7_900_000, ..Default::default() });
    assert!(corridor.undirected);
    let (width, height) = span_km(&corridor);
    assert!((height - 20.0).abs() < 0.5, "10 km each way: {height}");
    assert!((width - 20.0).abs() < 0.5, "10 km each way: {width}");
}

/// The reach ceiling is the documented 120 km, not 125: the lateral margin is the radius of each
/// sampled disc, never something added to the reach.
#[test]
fn an_implausible_speed_clamps_the_reach_at_the_documented_ceiling() {
    assert_eq!(Corridor::reach_m(Some(1_000.0)), Some(select::MAX_REACH_M));
    assert_eq!(Corridor::reach_m(Some(0.0)), Some(select::MIN_REACH_M));
    assert_eq!(Corridor::reach_m(None), None, "an absent speed is absent, not a floor in disguise");
    assert_eq!(Corridor::reach_m(Some(f64::NAN)), None, "a NaN speed is not a speed");
    // Undirected (no bearing): the disc is the reach itself, so 120 km each way and not 125.
    let corridor =
        Corridor::projected(&select::Fix { lat_udeg: 0, lon_udeg: 0, speed_ms: Some(1_000.0), ..Default::default() });
    let (_, height) = span_km(&corridor);
    assert!((height - 240.0).abs() < 1.0, "2 x 120 km: {height}");
}

/// A bearing the device vouches for reaches *ahead*: the corridor is long along the course and
/// stays margin-wide across it. This is the shape that makes containment pick a different tier
/// than a same-radius disc would.
#[test]
fn a_vouched_for_bearing_and_speed_project_a_directed_corridor() {
    let east = Corridor::projected(&select::Fix {
        lat_udeg: 48_060_000,
        lon_udeg: 7_900_000,
        bearing_deg: Some(90.0),
        speed_ms: Some(10.0), // 20 km in two hours → the 10 km floor is not what is being measured
        ..Default::default()
    });
    assert!(!east.undirected);
    let (width, height) = span_km(&east);
    assert!(width > height, "the corridor must be longer along the course than across it: {width} x {height}");
    assert!(east.bounds.east_udeg - i64::from(east.lon_udeg) > i64::from(east.lon_udeg) - east.bounds.west_udeg);
}

/// A NaN course is not a course. Taking the directed branch with one produces no forward reach at
/// all and collapses the corridor *below* the undirected floor — so it must fall through to the
/// reach-sized disc, exactly like an absent bearing.
#[test]
fn a_non_finite_bearing_falls_through_to_the_disc() {
    let broken = Corridor::projected(&select::Fix {
        lat_udeg: 48_060_000,
        lon_udeg: 7_900_000,
        bearing_deg: Some(f64::NAN),
        speed_ms: Some(10.0),
        ..Default::default()
    });
    let honest = Corridor::projected(&select::Fix {
        lat_udeg: 48_060_000,
        lon_udeg: 7_900_000,
        speed_ms: Some(10.0),
        ..Default::default()
    });
    assert!(broken.undirected);
    assert_eq!(broken.bounds, honest.bounds, "an unusable heading must read as no heading");
}

/// The route the rider is on beats any projection of it — but only the stretch inside the reach: a
/// 300 km route must not become a 300 km corridor.
#[test]
fn the_route_ahead_widens_the_corridor_only_as_far_as_the_reach() {
    let base = select::Fix {
        lat_udeg: 48_000_000,
        lon_udeg: 7_000_000,
        speed_ms: Some(5.0), // 36 km reach
        ..Default::default()
    };
    // A route running due north, a point every ~11 km, far past the reach.
    let route: Vec<(i32, i32)> = (1..=30).map(|step| (48_000_000 + step * 100_000, 7_000_000)).collect();
    let with_route = Corridor::projected(&select::Fix { route_ahead: route, ..base.clone() });
    let without = Corridor::projected(&base);
    assert!(with_route.bounds.north_udeg > without.bounds.north_udeg, "the route must widen the corridor");
    let reach_north = i64::from(base.lat_udeg) + (36_000.0 / 111_320.0 * 1e6) as i64 + 50_000;
    assert!(
        with_route.bounds.north_udeg < reach_north + (select::LATERAL_MARGIN_M / 111_320.0 * 1e6) as i64,
        "…and must stop at the reach, not follow the whole route"
    );
}

// ── the common window ──────────────────────────────────────────────────────────────────────

/// Two lattices of equal cell area: the window is stated over the **latest** of them, the phone's
/// tie-break. Picking the earliest instead silently anchors the bundle on the oldest of the equal
/// frames and drops a different set than the phone drops for the same manifest.
#[test]
fn equal_lattices_break_the_tie_on_the_latest_frame() {
    let hourly = hourly();
    let early = crop(hourly.valid_from, 9_000, 16, 16, 2);
    // Same stride, half a cell off — the two can never tile each other, so whichever wins the tie
    // is the one that survives, and the loser is dropped rather than resampled.
    let late = Crop {
        valid_at: hourly.valid_from + 900,
        south_udeg: early.south_udeg + 4_500,
        west_udeg: early.west_udeg + 4_500,
        ..crop(hourly.valid_from + 900, 9_000, 16, 16, 6)
    };
    let (bytes, report) = bundle::build(
        1,
        1,
        hourly.valid_from,
        (47_100_000, 7_100_000),
        &test_corridor(),
        &[early, late.clone()],
        &hourly,
    )
    .expect("build");
    assert_eq!(report.frames, 1);
    assert_eq!(report.dropped_incompatible, 1);
    let source = obc_formats::io::SliceSource(&bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.frame(0).expect("frame").valid_at, late.valid_at, "the later of two equal lattices wins");
}

/// An hourly-only bundle states the **corridor** it answers, not an invented degree around the
/// rider: the screens then say *hourly only here* over the region the question was about.
#[test]
fn an_hourly_only_bundle_declares_the_corridor_it_answered() {
    let hourly = hourly();
    let corridor =
        manifest::Bbox { south_udeg: 47_950_000, west_udeg: 7_850_000, north_udeg: 48_170_000, east_udeg: 7_950_000 };
    let bytes =
        bundle::hourly_only(1, 1, hourly.valid_from, (48_060_000, 7_900_000), &corridor, &hourly).expect("build");
    let source = obc_formats::io::SliceSource(&bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    let header = reader.header();
    assert_eq!(i64::from(header.south_lat_udeg), corridor.south_udeg);
    assert_eq!(i64::from(header.west_lon_udeg), corridor.west_udeg);
    assert_eq!(i64::from(header.north_lat_udeg), corridor.north_udeg);
    assert_eq!(i64::from(header.east_lon_udeg), corridor.east_udeg);
    assert_eq!(header.frame_count, 0);
}
