//! The target-independent conversion core: byte buffers in, byte buffers out, and a typed
//! failure with a message a rider can act on.
//!
//! Nothing here knows about the browser — that is the point. `cargo test` runs these functions
//! natively, so the error mapping and the two guards below are covered by the workspace suite
//! rather than only by whatever a browser happens to exercise.

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_formats::track::RECORD_LEN as TRACK_RECORD_LEN;
use obc_route::{MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS};

/// Largest point count an `.obcr` can *store*, and therefore the ceiling
/// [`gpx_to_obcr`] converts up to: every chunk full, consecutive chunks sharing their seam
/// vertex, so `MAX_ROUTE_CHUNKS` chunks hold `256 + 255 × 255` vertices.
///
/// This counts **stored** vertices — what survives decimation, plus the synthetic vertices the
/// `int16`-delta densify guard inserts. A GPX with far more `<trkpt>`s than this converts fine
/// as long as its shape decimates below the ceiling, so the message this constant feeds says
/// "after decimation" rather than promising anything about the input.
pub const MAX_STORED_POINTS: usize = MAX_ROUTE_CHUNKS * (MAX_POINTS_PER_CHUNK - 1) + 1;

/// A conversion failure: a stable machine-readable [`ErrorCode`] plus prose the UI can show
/// verbatim. The two travel together deliberately — a caller that wants to special-case one
/// cause branches on `code`, and everyone else just displays `message`.
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

/// Why a conversion failed. The string form ([`ErrorCode::as_str`]) is the **wire contract** with
/// the browser wrapper — it lands on the thrown JS `Error` as `.code`, so renaming one is a
/// breaking change for the frontend, not a refactor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// The dropped file is zero bytes.
    EmptyFile,
    /// The route file does not open like an XML document (a FIT/TCX export, a ZIP, a stray image).
    NotGpx,
    /// Valid-looking GPX with no `<trkpt>` carrying both `lat` and `lon` — waypoints or a `<rte>`
    /// route only, which the device cannot ride.
    GpxNoTrackPoints,
    /// The decimated route still exceeds [`MAX_STORED_POINTS`].
    GpxTooManyPoints,
    /// The ride log opens like text, so it is almost certainly a GPX handed to the wrong direction.
    NotTrackLog,
    /// The ride log is shorter than one record — nothing was ever written.
    TrackNoPoints,
    /// A read ran past the end of the input: the file is truncated or the upload was cut short.
    InputTruncated,
    /// A defect in this bridge or in the shared converter. The message says so, and says to report it.
    Internal,
}

impl ErrorCode {
    /// The stable kebab-case identifier the browser wrapper re-exports as its `ConvertErrorCode`
    /// union. Keep these in sync with `packer/web_builder/frontend/src/lib/convert/bridge.ts`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::EmptyFile => "empty-file",
            ErrorCode::NotGpx => "not-gpx",
            ErrorCode::GpxNoTrackPoints => "gpx-no-track-points",
            ErrorCode::GpxTooManyPoints => "gpx-too-many-points",
            ErrorCode::NotTrackLog => "not-track-log",
            ErrorCode::TrackNoPoints => "track-no-points",
            ErrorCode::InputTruncated => "input-truncated",
            ErrorCode::Internal => "internal",
        }
    }
}

/// Convert a GPX file's bytes into a `.obcr` route named `name`.
///
/// Byte-for-byte the same output as the native path (`obc_route::gpx_to_obcr`) on the same input
/// — this function contributes no geometry, only the buffer adapter and the guards below. The
/// `protocol-vectors/route-*.obcr` fixtures pin that equality from both sides.
///
/// The two pre-checks (empty, not-XML) exist because they are the two failures a *dropped file*
/// actually produces, and the converter cannot tell them apart: both would otherwise surface as
/// the generic "no track points". They are the only place the browser path's behaviour differs
/// from the native one, and only for inputs the native path would reject anyway.
pub fn gpx_to_obcr(gpx: &[u8], name: &str) -> Result<Vec<u8>, ConvertFailure> {
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

    // ~16 KB of the emitter's bounded chunk index lives on this frame (256 × `ChunkMeta`), which
    // is a rounding error against wasm's 1 MiB stack — unlike obc-web-demo's ≈277 KB `MapCache`,
    // which had to be heap-boxed to avoid overflowing it (#661). Nothing here is boxed because
    // nothing here is big; the scanners' buffers are 4 KB each and sequential, never co-resident.
    let mut sink = VecSink(Vec::new());
    obc_route::gpx_to_obcr(&SliceSource(gpx), name, &mut sink).map_err(describe_gpx_error)?;
    Ok(sink.0)
}

/// Convert a recorded `.obct` ride log's bytes into a GPX 1.1 document named `name`.
///
/// Byte-for-byte the same output as the native path (`obc_route::track_to_gpx`) — again, only the
/// buffer adapter is new. `protocol-vectors/track-log.obct` + `track-export.gpx` pin the pair.
pub fn track_to_gpx(log: &[u8], name: &str) -> Result<String, ConvertFailure> {
    if log.is_empty() {
        return Err(ConvertFailure::new(
            ErrorCode::EmptyFile,
            "This file is empty (0 bytes). Pick the ride log the device wrote — a zero-byte file \
             usually means the copy off the card failed part-way.",
        ));
    }
    if announces_itself_as_xml(log) {
        return Err(ConvertFailure::new(
            ErrorCode::NotTrackLog,
            "This is an XML file, not a recorded ride log. Ride logs are the device's own binary \
             .obct format — if this is already a GPX it needs no conversion.",
        ));
    }
    // The converter itself tolerates a short tail (a power-loss mid-write leaves a partial record
    // and the log stays valid to the 20-byte boundary), so a log below one whole record simply
    // yields an empty — valid, useless — GPX. Refuse it here instead: "your recording is empty"
    // is the true answer, and a browser download of a point-free GPX helps nobody.
    if log.len() < TRACK_RECORD_LEN {
        return Err(ConvertFailure::new(
            ErrorCode::TrackNoPoints,
            format!(
                "This ride log holds no track points: it is {} bytes, short of the {}-byte record \
                 the device writes per GPS fix. The recording ended before the first fix landed.",
                log.len(),
                TRACK_RECORD_LEN
            ),
        ));
    }

    let mut sink = VecSink(Vec::new());
    obc_route::track_to_gpx(&SliceSource(log), name, &mut sink).map_err(describe_track_error)?;
    // Every byte the exporter writes is ASCII except `name`, which arrived as a `&str`, so this
    // cannot fail. Mapped rather than unwrapped all the same: a panic in wasm is an opaque trap.
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

/// Could `bytes` be XML at all — does the document start with `<`?
///
/// The **lenient** test, and only the GPX direction uses it. A false accept costs nothing: a
/// binary file that happens to start with `<` falls through to the scanner and comes back as
/// [`ErrorCode::GpxNoTrackPoints`], which is still a true statement about it. A false *reject*
/// would be the expensive mistake — refusing a real route — so the bar stays at one byte.
fn opens_like_xml(bytes: &[u8]) -> bool {
    document_start(bytes).first() == Some(&b'<')
}

/// Does `bytes` *say* it is XML — does the document start with `<?xml` or `<gpx`?
///
/// The **strict** test, for the ride-log direction, where the mistake runs the other way. A
/// recorded `.obct` is a headerless record array whose first byte is a longitude's low byte, so
/// roughly one real ride log in 256 opens with `0x3C` — `<`. The lenient test above would refuse
/// those outright, with a message insisting they are XML. Requiring an actual XML or GPX opening
/// tag makes that collision need five specific bytes in a row, which no coordinate produces,
/// while still catching the case this guard exists for: a GPX dropped on the wrong target.
fn announces_itself_as_xml(bytes: &[u8]) -> bool {
    let start = document_start(bytes);
    start.starts_with(b"<?xml") || start.starts_with(b"<gpx")
}

/// Map a GPX→OBCR failure onto the browser vocabulary.
///
/// The match is exhaustive **on purpose**: [`Error`] is shared across the whole byte seam, so a
/// new variant must break this build and get a deliberate message rather than silently collapse
/// into someone else's text. That is also why the unreachable-from-here variants get honest
/// "this is a bug" prose instead of being folded into a plausible-sounding lie.
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
        // Neither is reachable from a GPX conversion (nothing on this path validates a format
        // tag), so say so rather than inventing a plausible cause the file does not have.
        Error::BadMagic | Error::BadVersion => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: the route converter reported a file-format mismatch, which it does \
             not produce when reading GPX. This is a bug — please report it with the file.",
        ),
    }
}

/// Map a track→GPX failure onto the browser vocabulary. Exhaustive for the same reason as
/// [`describe_gpx_error`], and split from it because the same [`Error`] means a different thing
/// in each direction — `Empty` is "your GPX has no track" one way and "your recording is empty"
/// the other.
fn describe_track_error(e: Error) -> ConvertFailure {
    match e {
        Error::Empty => ConvertFailure::new(
            ErrorCode::TrackNoPoints,
            "This ride log holds no usable records. The recording ended before the first GPS fix \
             was written.",
        ),
        Error::BadOffset => ConvertFailure::new(
            ErrorCode::InputTruncated,
            "Reading this ride log ran past the end of the file. The copy off the card was cut \
             short — copy it again.",
        ),
        Error::Io => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: writing the exported GPX into memory failed. This is a bug in the \
             conversion bridge — please report it.",
        ),
        // The exporter has no capacity-bounded output and reads no format tag, so none of these
        // is reachable from here.
        Error::TooLarge | Error::BadMagic | Error::BadVersion => ConvertFailure::new(
            ErrorCode::Internal,
            "Internal error: the track exporter reported a format or capacity failure it does not \
             produce. This is a bug — please report it with the file.",
        ),
    }
}

/// A growable in-memory [`ByteSink`]. The OBCR writer streams the body and then patches the
/// header back at offset 0, so `patch_at` has to be real — and bounds-checked, because a slice
/// panic inside wasm is an opaque `unreachable` trap with no message.
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

    /// The 9-point track from `obc-vectors`' route source, inline so this crate's tests stand on
    /// their own; the byte-identity proof against the real fixture is the frontend's job (it is
    /// the one place a wasm build and the checked-in native output can be compared).
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

    /// The bridge adds nothing to the bytes: its output equals the shared converter's, driven
    /// through the very same sink shape the rest of the tree uses.
    #[test]
    fn gpx_conversion_matches_the_shared_converter_byte_for_byte() {
        let mine = gpx_to_obcr(TINY_GPX.as_bytes(), "Tiny").unwrap();
        let mut reference = VecSink(Vec::new());
        obc_route::gpx_to_obcr(&SliceSource(TINY_GPX.as_bytes()), "Tiny", &mut reference).unwrap();
        assert_eq!(mine, reference.0);
        assert_eq!(&mine[0..4], b"OBCR");
    }

    #[test]
    fn track_conversion_matches_the_shared_converter_byte_for_byte() {
        let log = [
            obc_formats::track::encode_record(&obc_route::TrackPoint {
                lon: 7_842_000,
                lat: 47_995_000,
                ele: 300,
                t_ms: 0,
                segment_start: true,
                hr: Some(142),
                cadence: None,
                power: None,
            }),
            obc_formats::track::encode_record(&obc_route::TrackPoint {
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

        let mine = track_to_gpx(&log, "a < b").unwrap();
        let mut reference = VecSink(Vec::new());
        obc_route::track_to_gpx(&SliceSource(&log), "a < b", &mut reference).unwrap();
        assert_eq!(mine.as_bytes(), reference.0.as_slice());
        assert!(mine.contains("<name>a &lt; b</name>"), "name escaping survives: {mine}");
    }

    /// Every guard the UI shows a message for, and the codes it branches on. The distinction that
    /// matters: an empty file, a non-GPX file and a GPX without a track are three different
    /// answers, not one "invalid file".
    #[test]
    fn each_rejected_input_gets_its_own_code() {
        assert_eq!(code_of(gpx_to_obcr(b"", "x")), ErrorCode::EmptyFile);
        assert_eq!(code_of(gpx_to_obcr(&[0x00, 0x01, 0x02, 0x03], "x")), ErrorCode::NotGpx);
        // A FIT file's header starts with a length byte then ".FIT" — binary, so it is caught by
        // the sniff rather than mis-reported as a GPX with no track.
        assert_eq!(code_of(gpx_to_obcr(b"\x0e\x10\x2e\x46\x49\x54", "x")), ErrorCode::NotGpx);
        // Waypoints but no track: well-formed GPX, nothing to ride.
        let wpt_only = r#"<?xml version="1.0"?><gpx><wpt lat="48.0" lon="7.8"><name>Home</name></wpt></gpx>"#;
        assert_eq!(code_of(gpx_to_obcr(wpt_only.as_bytes(), "x")), ErrorCode::GpxNoTrackPoints);

        assert_eq!(code_of(track_to_gpx(b"", "x")), ErrorCode::EmptyFile);
        assert_eq!(code_of(track_to_gpx(b"<?xml version=\"1.0\"?><gpx></gpx>", "x")), ErrorCode::NotTrackLog);
        assert_eq!(code_of(track_to_gpx(&[0xAB; TRACK_RECORD_LEN - 1], "x")), ErrorCode::TrackNoPoints);
    }

    /// A leading BOM and leading whitespace are both normal in exported GPX; neither may be
    /// mistaken for a binary file.
    #[test]
    fn the_xml_sniff_tolerates_a_bom_and_leading_whitespace() {
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"\n  ");
        bom.extend_from_slice(TINY_GPX.as_bytes());
        assert!(gpx_to_obcr(&bom, "Tiny").is_ok());
    }

    /// The ride-log guard must not fire on a *real* log whose first byte happens to be `<`.
    ///
    /// A `.obct` has no header: byte 0 is the first longitude's low byte, so about one log in 256
    /// starts with `0x3C`. A one-byte "looks like XML" test would refuse those, insisting a
    /// perfectly good recording is an XML file — which is why that direction demands an actual
    /// `<?xml` / `<gpx` opening instead.
    #[test]
    fn a_ride_log_starting_with_an_angle_bracket_still_converts() {
        // lon ≡ 0x3C (mod 256): 7_842_000 - 0xD0 + 0x3C = 7_841_852, whose LE encoding starts 0x3C.
        let lon: i32 = 7_841_852;
        assert_eq!(lon.to_le_bytes()[0], b'<', "the fixture only tests what it claims to");
        let log = obc_formats::track::encode_record(&obc_route::TrackPoint {
            lon,
            lat: 47_995_000,
            ele: 300,
            t_ms: 0,
            segment_start: true,
            hr: None,
            cadence: None,
            power: None,
        });
        let gpx = track_to_gpx(&log, "Angle").expect("a real ride log, not XML");
        assert!(gpx.contains("lon=\"7.841852\""), "converted the point: {gpx}");

        // The guard still catches what it is for: a GPX dropped on the ride-log target.
        assert_eq!(code_of(track_to_gpx(TINY_GPX.as_bytes(), "x")), ErrorCode::NotTrackLog);
        assert_eq!(code_of(track_to_gpx(b"  <gpx version=\"1.1\"></gpx>", "x")), ErrorCode::NotTrackLog);
    }

    /// The message is the product here, so pin that each one actually says what is wrong and what
    /// to do — no test can catch "Invalid file" creeping back in, but this catches an empty or
    /// bare one.
    #[test]
    fn messages_name_the_cause_and_the_fix() {
        let no_track = gpx_to_obcr(b"<gpx></gpx>", "x").unwrap_err();
        assert!(no_track.message.contains("<trkpt>"), "names the missing element: {no_track}");
        assert!(no_track.message.contains("track"), "says what to re-export: {no_track}");

        let not_gpx = gpx_to_obcr(&[0xFF; 8], "x").unwrap_err();
        assert!(not_gpx.message.contains(".fit"), "points at the likely real format: {not_gpx}");

        let short = track_to_gpx(&[0xAB; 4], "x").unwrap_err();
        assert!(short.message.contains('4'), "quotes the actual size: {short}");
        assert!(short.message.contains("20-byte"), "explains the record width: {short}");
    }

    /// A route past the storage ceiling reports *that*, with the number in it. Built as a
    /// zig-zag whose every vertex is ~2 m off the chord, so the decimator (ε = 1 m) keeps them
    /// all and the emitter runs out of chunks.
    #[test]
    fn a_route_over_the_storage_ceiling_says_so() {
        let mut gpx = String::from("<?xml version=\"1.0\"?><gpx><trk><trkseg>");
        for i in 0..=MAX_STORED_POINTS {
            // ~1.1 m per step east, alternating ~2.2 m north/south — well inside the int16 delta
            // guard, well outside the 1 m decimation tolerance.
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

        let e = gpx_to_obcr(gpx.as_bytes(), "Too long").unwrap_err();
        assert_eq!(e.code, ErrorCode::GpxTooManyPoints);
        assert!(e.message.contains(&MAX_STORED_POINTS.to_string()), "quotes the ceiling: {e}");
    }

    /// The storage ceiling is derived from the format's own caps, not typed in twice.
    #[test]
    fn the_storage_ceiling_follows_the_format_caps() {
        assert_eq!(MAX_STORED_POINTS, 65_281);
        assert_eq!(MAX_STORED_POINTS, MAX_POINTS_PER_CHUNK + (MAX_ROUTE_CHUNKS - 1) * (MAX_POINTS_PER_CHUNK - 1));
    }

    /// A patch outside what was written is a bug, and must surface as an error rather than a
    /// slice panic (which wasm reports as an unreachable trap with no message at all).
    #[test]
    fn the_sink_refuses_an_out_of_range_patch() {
        let mut sink = VecSink(vec![0u8; 4]);
        assert_eq!(sink.patch_at(2, &[1, 2, 3]), Err(Error::BadOffset));
        assert_eq!(sink.patch_at(u32::MAX, &[1]), Err(Error::BadOffset));
        assert_eq!(sink.patch_at(1, &[9, 9]), Ok(()));
        assert_eq!(sink.0, [0, 9, 9, 0]);
    }
}
