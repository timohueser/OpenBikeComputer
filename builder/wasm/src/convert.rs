//! The target-independent conversion core: byte buffers in, byte buffers out, and a typed failure
//! with a message a rider can act on. Nothing here knows about the browser, so the error mapping
//! and the two guards below are tested natively.

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_route::{BikeType, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS};

/// Largest point count an `.obcr` can store, and so the ceiling [`gpx_to_obcr`] converts up to:
/// every chunk full, with consecutive chunks sharing their seam vertex.
///
/// It counts stored vertices: what survives decimation, plus the synthetic vertices the
/// `int16`-delta densify guard inserts. A GPX with far more track points converts fine if its shape
/// decimates below the ceiling.
pub const MAX_STORED_POINTS: usize = MAX_ROUTE_CHUNKS * (MAX_POINTS_PER_CHUNK - 1) + 1;

/// A conversion failure: a stable machine-readable [`ErrorCode`] plus prose the UI can show
/// verbatim. A caller that special-cases a cause branches on `code`; everyone else shows
/// `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertFailure {
    pub code: ErrorCode,
    pub message: String,
}

impl ConvertFailure {
    fn new(code: ErrorCode, message: impl Into<String>) -> ConvertFailure {
        ConvertFailure { code, message: message.into() }
    }
}

impl core::fmt::Display for ConvertFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ConvertFailure {}

/// Why a conversion failed. The string form ([`ErrorCode::as_str`]) is the wire contract with the
/// browser wrapper: it lands on the thrown JS `Error` as `.code`, so renaming one breaks the
/// frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The dropped file is zero bytes.
    EmptyFile,
    /// The route file does not open like an XML document.
    NotGpx,
    /// Valid-looking GPX with no `<trkpt>` carrying both `lat` and `lon`, which the device cannot
    /// ride.
    GpxNoTrackPoints,
    /// The decimated route still exceeds [`MAX_STORED_POINTS`].
    GpxTooManyPoints,
    /// The bytes are not a finished ride-v6 object.
    NotRide,
    /// The finished ride carries no recorded points.
    RideNoPoints,
    /// A read ran past the end of the input: the file is truncated or the upload was cut short.
    InputTruncated,
    /// A defect in this bridge or in the shared converter.
    Internal,
}

impl ErrorCode {
    /// The stable kebab-case identifier the browser wrapper re-exports as its `ConvertErrorCode`
    /// union. Keep these in sync with `builder/web/src/lib/convert/bridge.ts`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::EmptyFile => "empty-file",
            ErrorCode::NotGpx => "not-gpx",
            ErrorCode::GpxNoTrackPoints => "gpx-no-track-points",
            ErrorCode::GpxTooManyPoints => "gpx-too-many-points",
            ErrorCode::NotRide => "not-ride",
            ErrorCode::RideNoPoints => "ride-no-points",
            ErrorCode::InputTruncated => "input-truncated",
            ErrorCode::Internal => "internal",
        }
    }
}

/// Convert a GPX file's bytes into a `.obcr` route named `name` and typed `bike`.
///
/// Byte-for-byte the same output as the shared unattributed converter on the same input: this function
/// contributes no geometry, only the buffer adapter and the guards below.
///
/// The two pre-checks, empty and not-XML, are the two failures a dropped file produces, and the
/// converter cannot tell them apart. Without them both surface as the generic "no track points".
pub fn gpx_to_obcr(gpx: &[u8], name: &str, bike: BikeType) -> Result<Vec<u8>, ConvertFailure> {
    if gpx.is_empty() {
        return Err(ConvertFailure::new(
            ErrorCode::EmptyFile,
            "This file is empty (0 bytes). Pick the .gpx your route planner exported — a \
             zero-byte file usually means the export or the download failed part-way.",
        ));
    }
    if !opens_like_xml(gpx) {
        return Err(ConvertFailure::new(
            ErrorCode::NotGpx,
            "This does not look like a GPX file — it does not begin with an XML declaration or \
             tag. GPX is the only route format this page can convert; a Garmin .fit or .tcx \
             export has to be converted to GPX first.",
        ));
    }

    // About 16 KB of the emitter's bounded chunk index lives on this frame, which is small against
    // wasm's 1 MiB stack. Nothing here is boxed, because nothing here is big: the scanners' buffers
    // are 4 KB each and sequential, never co-resident.
    let mut sink = VecSink(Vec::new());
    obc_route::gpx_to_obcr_attributed(&SliceSource(gpx), name, bike, &mut sink, None, |_, _| Ok(0))
        .map_err(describe_gpx_error)?;
    Ok(sink.0)
}

/// Convert a finished ride-v6 object's bytes into a GPX 1.1 document named `name`.
///
/// Byte-for-byte the same output as `obc_route::track_to_gpx`; only the buffer adapter is new.
pub fn track_to_gpx(ride: &[u8], name: &str) -> Result<String, ConvertFailure> {
    if ride.is_empty() {
        return Err(ConvertFailure::new(
            ErrorCode::EmptyFile,
            "This file is empty (0 bytes). Download the finished ride again; a zero-byte file \
             means the copy failed before any of the object arrived.",
        ));
    }
    let source = SliceSource(ride);
    let info = obc_route::RideInfo::read(&source).map_err(describe_track_error)?;
    if info.point_count == 0 {
        return Err(ConvertFailure::new(
            ErrorCode::RideNoPoints,
            "This finished ride holds no track points. It was probably stopped before the first \
             GPS fix landed, so there is nothing to export as GPX.",
        ));
    }

    let mut sink = VecSink(Vec::new());
    obc_route::track_to_gpx(&source, name, &mut sink).map_err(describe_track_error)?;
    // Every byte the exporter writes is ASCII except `name`, which arrived as a `&str`, so this
    // cannot fail. Mapped rather than unwrapped, because a panic in wasm is an opaque trap.
    String::from_utf8(sink.0).map_err(|_| {
        ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: the exported GPX was not valid UTF-8. This is a bug in the \
             conversion bridge — please report it.",
        )
    })
}

/// The bytes after an optional UTF-8 BOM and any leading whitespace.
fn document_start(bytes: &[u8]) -> &[u8] {
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let lead = body.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(body.len());
    &body[lead..]
}

/// Could `bytes` be XML at all: does the document start with `<`?
///
/// A false accept costs nothing, because the scanner then answers
/// [`ErrorCode::GpxNoTrackPoints`], which is still true. A false reject would refuse a real route,
/// so the bar stays at one byte.
fn opens_like_xml(bytes: &[u8]) -> bool {
    document_start(bytes).first() == Some(&b'<')
}

/// Map a GPX-to-OBCR failure onto the browser vocabulary.
///
/// The match is exhaustive on purpose: [`Error`] is shared across the whole byte seam, so a new
/// variant must break this build and get a deliberate message. The variants unreachable from here
/// say they are a defect rather than inventing a plausible cause.
fn describe_gpx_error(e: Error) -> ConvertFailure {
    match e {
        Error::Empty => ConvertFailure::new(
            ErrorCode::GpxNoTrackPoints,
            "This GPX has no track points: no <trkpt> element carries both a lat and a lon. \
             Planners write those only when you export a *track* — a file holding just <wpt> \
             waypoints, or a <rte> route, has no line to ride. Re-export it as a track.",
        ),
        Error::TooLarge => ConvertFailure::new(
            ErrorCode::GpxTooManyPoints,
            format!(
                "This route is too long for the device route format: even after decimation it \
                 needs more than {MAX_STORED_POINTS} stored points ({MAX_ROUTE_CHUNKS} chunks × \
                 {MAX_POINTS_PER_CHUNK}). Split it into day stages, or lower the point density in \
                 your planner, and convert again."
            ),
        ),
        Error::BadOffset => ConvertFailure::new(
            ErrorCode::InputTruncated,
            "Reading this GPX ran past the end of the file. It is truncated — the export or the \
             upload was cut short. Re-export it and try again.",
        ),
        Error::Io => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: writing the .obcr into memory failed. This is a bug in the \
             conversion bridge — please report it.",
        ),
        // Nothing on this path validates a format tag, so neither is reachable from a GPX
        // conversion.
        Error::BadMagic | Error::BadVersion => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: the route converter reported a file-format mismatch, which it does \
             not produce when reading GPX. This is a bug — please report it with the file.",
        ),
    }
}

/// Map a track-to-GPX failure onto the browser vocabulary. Split from [`describe_gpx_error`]
/// because the same [`Error`] means a different thing in each direction: `Empty` is a GPX with no
/// track one way and an empty recording the other.
fn describe_track_error(e: Error) -> ConvertFailure {
    match e {
        Error::Empty => ConvertFailure::new(
            ErrorCode::RideNoPoints,
            "This finished ride holds no track points, so there is nothing to export as GPX.",
        ),
        Error::BadOffset | Error::BadMagic | Error::BadVersion => ConvertFailure::new(
            ErrorCode::NotRide,
            "These bytes are not one complete ride-v6 object. Download the finished ride again; \
             unfinished sample logs and older ride formats are not accepted.",
        ),
        Error::Io => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: writing the exported GPX into memory failed. This is a bug in the \
             conversion bridge — please report it.",
        ),
        Error::TooLarge => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: the ride exporter reported a capacity failure it should not \
             produce. This is a bug — please report it with the file.",
        ),
    }
}

/// A growable in-memory [`ByteSink`]. The OBCR writer streams the body and then patches the header
/// back at offset 0, so `patch_at` must be real and bounds-checked: a slice panic inside wasm is an
/// opaque trap with no message.
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(buf);
        Ok(())
    }

    fn patch_at(&mut self, offset: u32, buf: &[u8]) -> Result<(), Error> {
        let start = offset as usize;
        let end = start.checked_add(buf.len()).ok_or(Error::BadOffset)?;
        self.0.get_mut(start..end).ok_or(Error::BadOffset)?.copy_from_slice(buf);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished_ride(mut samples: Vec<u8>, name: &str) -> Vec<u8> {
        let points = (samples.len() / obc_formats::track::RECORD_LEN) as u32;
        samples.extend_from_slice(&obc_formats::ride::encode_footer(&obc_formats::ride::Footer::new(
            name,
            1_700_000_000,
            0,
            0,
            0,
            0,
            points,
            None,
            None,
            None,
            None,
            None,
        )));
        samples
    }

    /// The 9-point track from `obc-vectors`' route source, inline so this crate's tests stand on
    /// their own. The byte-identity proof against the real fixture is the frontend's job.
    const TINY_GPX: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test">
  <trk><name>t</name><trkseg>
    <trkpt lat="48.0000" lon="7.8200"><ele>236.0</ele></trkpt>
    <trkpt lat="48.0005" lon="7.8230"><ele>241.0</ele></trkpt>
    <trkpt lat="48.0002" lon="7.8265"><ele>249.5</ele></trkpt>
  </trkseg></trk>
</gpx>
"#;

    fn code_of(r: Result<impl core::fmt::Debug, ConvertFailure>) -> ErrorCode {
        r.expect_err("expected a failure").code
    }

    /// The bridge adds nothing to the bytes: its output equals the shared converter's, and the
    /// picked bike type lands in the header.
    #[test]
    fn gpx_conversion_matches_the_shared_converter_byte_for_byte() {
        for bike in BikeType::ALL {
            let mine = gpx_to_obcr(TINY_GPX.as_bytes(), "Tiny", bike).unwrap();
            let mut reference = VecSink(Vec::new());
            obc_route::gpx_to_obcr_attributed(
                &SliceSource(TINY_GPX.as_bytes()),
                "Tiny",
                bike,
                &mut reference,
                None,
                |_, _| Ok(0),
            )
            .unwrap();
            assert_eq!(mine, reference.0);
            assert_eq!(&mine[0..4], b"OBCR");
            assert_eq!(mine[obc_formats::obcr::BIKE_TYPE_OFF], bike as u8);
        }
    }

    #[test]
    fn track_conversion_matches_the_shared_converter_byte_for_byte() {
        let log = [
            obc_formats::track::encode_record(&obc_ports::TrackPoint {
                lon: 7_842_000,
                lat: 47_995_000,
                ele: 300,
                t_ms: 0,
                segment_start: true,
                hr: Some(142),
                cadence: None,
                power: None,
            }),
            obc_formats::track::encode_record(&obc_ports::TrackPoint {
                lon: -7_843_500,
                lat: -47_996_000,
                ele: 305,
                t_ms: 1_000,
                segment_start: false,
                hr: None,
                cadence: None,
                power: None,
            }),
        ]
        .concat();

        let ride = finished_ride(log, "a < b");
        let mine = track_to_gpx(&ride, "a < b").unwrap();
        let mut reference = VecSink(Vec::new());
        obc_route::track_to_gpx(&SliceSource(&ride), "a < b", &mut reference).unwrap();
        assert_eq!(mine.as_bytes(), reference.0.as_slice());
        assert!(mine.contains("<name>a &lt; b</name>"), "name escaping survives: {mine}");
    }

    /// An empty file, a non-GPX file and a GPX without a track are three different answers, not one
    /// invalid-file message.
    #[test]
    fn each_rejected_input_gets_its_own_code() {
        assert_eq!(code_of(gpx_to_obcr(b"", "x", BikeType::Road)), ErrorCode::EmptyFile);
        assert_eq!(code_of(gpx_to_obcr(&[0x00, 0x01, 0x02, 0x03], "x", BikeType::Road)), ErrorCode::NotGpx);
        // A FIT file's header starts with a length byte and then ".FIT", so the sniff catches it
        // rather than reporting a GPX with no track.
        assert_eq!(code_of(gpx_to_obcr(b"\x0e\x10\x2e\x46\x49\x54", "x", BikeType::Road)), ErrorCode::NotGpx);
        // Waypoints but no track: well-formed GPX, nothing to ride.
        let wpt_only = r#"<?xml version="1.0"?><gpx><wpt lat="48.0" lon="7.8"><name>Home</name></wpt></gpx>"#;
        assert_eq!(code_of(gpx_to_obcr(wpt_only.as_bytes(), "x", BikeType::Road)), ErrorCode::GpxNoTrackPoints);

        assert_eq!(code_of(track_to_gpx(b"", "x")), ErrorCode::EmptyFile);
        assert_eq!(code_of(track_to_gpx(b"<?xml version=\"1.0\"?><gpx></gpx>", "x")), ErrorCode::NotRide);
        assert_eq!(code_of(track_to_gpx(&[0xAB; 19], "x")), ErrorCode::NotRide);
    }

    /// A leading BOM and leading whitespace are both normal in exported GPX, and neither may be
    /// mistaken for a binary file.
    #[test]
    fn the_xml_sniff_tolerates_a_bom_and_leading_whitespace() {
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"\n  ");
        bom.extend_from_slice(TINY_GPX.as_bytes());
        assert!(gpx_to_obcr(&bom, "Tiny", BikeType::Road).is_ok());
    }

    #[test]
    fn a_headerless_sample_stream_is_not_a_ride() {
        let samples = obc_formats::track::encode_record(&obc_ports::TrackPoint {
            lon: 7_841_852,
            lat: 47_995_000,
            ele: 300,
            t_ms: 0,
            segment_start: true,
            hr: None,
            cadence: None,
            power: None,
        });
        assert_eq!(code_of(track_to_gpx(&samples, "retired")), ErrorCode::NotRide);
    }

    /// The message is the product here, so each one must say what is wrong and what to do.
    #[test]
    fn messages_name_the_cause_and_the_fix() {
        let no_track = gpx_to_obcr(b"<gpx></gpx>", "x", BikeType::Road).unwrap_err();
        assert!(no_track.message.contains("<trkpt>"), "names the missing element: {no_track}");
        assert!(no_track.message.contains("track"), "says what to re-export: {no_track}");

        let not_gpx = gpx_to_obcr(&[0xFF; 8], "x", BikeType::Road).unwrap_err();
        assert!(not_gpx.message.contains(".fit"), "points at the likely real format: {not_gpx}");

        let short = track_to_gpx(&[0xAB; 4], "x").unwrap_err();
        assert!(short.message.contains("ride-v6"), "names the required format: {short}");
    }

    /// A route past the storage ceiling reports that, with the number in it. The zig-zag keeps
    /// every vertex about 2 m off the chord, so the decimator keeps them all.
    #[test]
    fn a_route_over_the_storage_ceiling_says_so() {
        let mut gpx = String::from("<?xml version=\"1.0\"?><gpx><trk><trkseg>");
        for i in 0..=MAX_STORED_POINTS {
            // About 1.1 m per step east, alternating about 2.2 m north and south: inside the
            // int16 delta guard, outside the 1 m decimation tolerance.
            let lon = 7_800_000 + i as i32 * 15;
            let lat = 48_000_000 + if i % 2 == 0 { 0 } else { 20 };
            gpx.push_str(&format!(
                "<trkpt lat=\"{}.{:06}\" lon=\"{}.{:06}\"/>",
                lat / 1_000_000,
                lat % 1_000_000,
                lon / 1_000_000,
                lon % 1_000_000
            ));
        }
        gpx.push_str("</trkseg></trk></gpx>");

        let e = gpx_to_obcr(gpx.as_bytes(), "Too long", BikeType::Road).unwrap_err();
        assert_eq!(e.code, ErrorCode::GpxTooManyPoints);
        assert!(e.message.contains(&MAX_STORED_POINTS.to_string()), "quotes the ceiling: {e}");
    }

    /// The storage ceiling is derived from the format's own caps, not typed in twice.
    #[test]
    fn the_storage_ceiling_follows_the_format_caps() {
        assert_eq!(MAX_STORED_POINTS, 65_281);
        assert_eq!(MAX_STORED_POINTS, MAX_POINTS_PER_CHUNK + (MAX_ROUTE_CHUNKS - 1) * (MAX_POINTS_PER_CHUNK - 1));
    }

    /// A patch outside what was written must surface as an error and not a slice panic, which wasm
    /// reports as a trap with no message.
    #[test]
    fn the_sink_refuses_an_out_of_range_patch() {
        let mut sink = VecSink(vec![0u8; 4]);
        assert_eq!(sink.patch_at(2, &[1, 2, 3]), Err(Error::BadOffset));
        assert_eq!(sink.patch_at(u32::MAX, &[1]), Err(Error::BadOffset));
        assert_eq!(sink.patch_at(1, &[9, 9]), Ok(()));
        assert_eq!(sink.0, [0, 9, 9, 0]);
    }
}
