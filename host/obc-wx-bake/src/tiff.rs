//! A minimal classic-TIFF reader for the tiled deflate float32 cloud-optimized GeoTIFFs EUMETNET
//! OPERA publishes (WXR6, #1245) — and for nothing else.
//!
//! OPERA ships every composite twice: as ODIM HDF5 and as a COG. The COG is the one the baker
//! ingests, because the subset of TIFF it uses is small enough to read here: classic (not Big)
//! TIFF, `Compression = 8` (one zlib stream per tile), `Predictor = 1`, square tiles,
//! `PlanarConfiguration = 1`, `SampleFormat = 3` / `BitsPerSample = 32` on every sample. That is a
//! few hundred lines against a GDAL or HDF5 dependency, and it keeps the provider-format surface
//! inside the bakery where every other upstream format already lives.
//!
//! Only the **full-resolution image** (IFD 0) is read. OPERA's COGs carry four reduced-resolution
//! overviews after it; they are deliberately ignored rather than followed, because an overview is
//! a resampled derivative and the baker resamples exactly once, from native cells.
//!
//! Everything is bounds-checked against the buffer and against explicit ceilings before an
//! allocation is made: this parser is fed 3 MB of somebody else's bytes every five minutes, and
//! `tests/fuzz_decode.rs` mutates them.

use std::io::Read;

/// Ceiling on the pixel count of one image. OPERA's largest raster is 16.72 M pixels, so this is
/// a 2x headroom rather than the 4x it was: the value buffer is allocated from the header, before
/// any tile is touched, so it is the one number that decides how much a twelve-byte lie costs
/// (here 128 MB, and the tile pre-pass below has to agree the tiles exist first).
pub const MAX_PIXELS: u64 = 32_000_000;
/// Cap on how much an out-of-line tag's declared count may reserve up front. A `GeoKeyDirectory`
/// typed BYTE with a 32 M count is a legal-looking header and a 128 MB `Vec<u32>`; the values are
/// still read, the allocation just grows as they arrive.
const MAX_RESERVED_VALUES: usize = 4_096;
/// Ceiling on one decompressed tile: 1,024 x 1,024 pixels x 4 samples x 4 bytes.
pub const MAX_TILE_BYTES: u64 = 16 * 1024 * 1024;
/// Ceiling on the compressed object itself.
pub const MAX_OBJECT_BYTES: u64 = 32 * 1024 * 1024;

// The tags this reader understands. Anything else in the IFD is ignored.
const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_PLANAR_CONFIGURATION: u16 = 284;
const TAG_PREDICTOR: u16 = 317;
const TAG_TILE_WIDTH: u16 = 322;
const TAG_TILE_LENGTH: u16 = 323;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_BYTE_COUNTS: u16 = 325;
const TAG_SAMPLE_FORMAT: u16 = 339;
const TAG_MODEL_PIXEL_SCALE: u16 = 33_550;
const TAG_MODEL_TIEPOINT: u16 = 33_922;
const TAG_GEO_KEY_DIRECTORY: u16 = 34_735;
const TAG_GEO_DOUBLE_PARAMS: u16 = 34_736;
const TAG_GDAL_METADATA: u16 = 42_112;
const TAG_GDAL_NODATA: u16 = 42_113;

const COMPRESSION_DEFLATE: u32 = 8;
/// Adobe's original (pre-registration) deflate code; GDAL still writes it in some versions.
const COMPRESSION_DEFLATE_ADOBE: u32 = 32_946;
const SAMPLE_FORMAT_IEEE_FLOAT: u32 = 3;

/// One decoded full-resolution image: band 0's samples plus the georeferencing and provenance
/// the adapter validates its pinned contract against.
#[derive(Debug)]
pub struct Cog {
    pub width: u32,
    pub height: u32,
    pub samples_per_pixel: u32,
    pub tile_width: u32,
    pub tile_length: u32,
    /// Model coordinates of the **upper-left corner** of pixel (0, 0), from `ModelTiepoint`.
    pub ul_x: f64,
    pub ul_y: f64,
    /// `ModelPixelScale`, metres per pixel in x and y.
    pub pixel_x: f64,
    pub pixel_y: f64,
    /// `GDAL_NODATA`, the sentinel that means *this cell is outside radar coverage*. It is a
    /// different fact from a `NaN` sample, which means *covered, nothing detected* — see
    /// [`crate::source::opera`].
    pub nodata: f64,
    /// `GeoDoubleParams`, the projection parameters the adapter pins its LAEA constants against.
    pub geo_double_params: Vec<f64>,
    /// The raw `GeoKeyDirectory`, so a caller can pin the projection *method* and its units
    /// rather than only its numbers — see [`Cog::geo_key`].
    pub geo_key_directory: Vec<u32>,
    /// `GDAL_METADATA`, the GDAL-flavoured XML carrying OPERA's ODIM attributes.
    pub metadata: String,
    /// Band 0, row-major with row 0 at the **north** edge (TIFF's own order).
    pub values: Vec<f32>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    little: bool,
}

impl<'a> Reader<'a> {
    fn u16_at(&self, offset: usize) -> Result<u16, String> {
        let raw: [u8; 2] = self
            .bytes
            .get(offset..offset + 2)
            .ok_or("TIFF: read past the end of the object")?
            .try_into()
            .expect("2 bytes");
        Ok(if self.little { u16::from_le_bytes(raw) } else { u16::from_be_bytes(raw) })
    }

    fn u32_at(&self, offset: usize) -> Result<u32, String> {
        let raw: [u8; 4] = self
            .bytes
            .get(offset..offset + 4)
            .ok_or("TIFF: read past the end of the object")?
            .try_into()
            .expect("4 bytes");
        Ok(if self.little { u32::from_le_bytes(raw) } else { u32::from_be_bytes(raw) })
    }

    fn f64_at(&self, offset: usize) -> Result<f64, String> {
        let raw: [u8; 8] = self
            .bytes
            .get(offset..offset + 8)
            .ok_or("TIFF: read past the end of the object")?
            .try_into()
            .expect("8 bytes");
        Ok(if self.little { f64::from_le_bytes(raw) } else { f64::from_be_bytes(raw) })
    }

    fn f32_at(&self, offset: usize) -> Result<f32, String> {
        let raw: [u8; 4] =
            self.bytes.get(offset..offset + 4).ok_or("TIFF: read past the end of a tile")?.try_into().expect("4 bytes");
        Ok(if self.little { f32::from_le_bytes(raw) } else { f32::from_be_bytes(raw) })
    }
}

/// One IFD entry, with its values already located in the buffer (inline entries point at the
/// entry's own value field, so the accessors do not care which case they are in).
#[derive(Clone, Copy)]
struct Entry {
    field_type: u16,
    count: u32,
    values_at: usize,
}

fn type_size(field_type: u16) -> Option<usize> {
    Some(match field_type {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return None,
    })
}

struct Ifd<'a> {
    reader: Reader<'a>,
    entries: Vec<(u16, Entry)>,
}

impl<'a> Ifd<'a> {
    fn get(&self, tag: u16) -> Option<Entry> {
        self.entries.iter().find(|(candidate, _)| *candidate == tag).map(|(_, entry)| *entry)
    }

    /// An unsigned integer tag as `u32`, whatever integer width it was written in.
    fn integers(&self, tag: u16) -> Result<Vec<u32>, String> {
        let Some(entry) = self.get(tag) else { return Ok(Vec::new()) };
        let size = type_size(entry.field_type).ok_or_else(|| format!("TIFF: tag {tag} has an unknown type"))?;
        let mut values = Vec::with_capacity((entry.count as usize).min(MAX_RESERVED_VALUES));
        for index in 0..entry.count as usize {
            let at = entry.values_at + index * size;
            values.push(match entry.field_type {
                1 | 6 | 7 => u32::from(*self.reader.bytes.get(at).ok_or("TIFF: tag value past the end")?),
                3 | 8 => u32::from(self.reader.u16_at(at)?),
                4 | 9 => self.reader.u32_at(at)?,
                other => return Err(format!("TIFF: tag {tag} is type {other}, not an integer")),
            });
        }
        Ok(values)
    }

    fn scalar(&self, tag: u16) -> Result<Option<u32>, String> {
        let values = self.integers(tag)?;
        match values.len() {
            0 => Ok(None),
            1 => Ok(Some(values[0])),
            other => Err(format!("TIFF: tag {tag} has {other} values, expected one")),
        }
    }

    fn doubles(&self, tag: u16) -> Result<Vec<f64>, String> {
        let Some(entry) = self.get(tag) else { return Ok(Vec::new()) };
        if entry.field_type != 12 {
            return Err(format!("TIFF: tag {tag} is type {}, not DOUBLE", entry.field_type));
        }
        (0..entry.count as usize).map(|index| self.reader.f64_at(entry.values_at + index * 8)).collect()
    }

    fn ascii(&self, tag: u16) -> Result<String, String> {
        let Some(entry) = self.get(tag) else { return Ok(String::new()) };
        if entry.field_type != 2 {
            return Err(format!("TIFF: tag {tag} is type {}, not ASCII", entry.field_type));
        }
        let end = entry.values_at + entry.count as usize;
        let raw = self.reader.bytes.get(entry.values_at..end).ok_or("TIFF: ASCII tag past the end")?;
        let raw = raw.split(|byte| *byte == 0).next().unwrap_or_default();
        String::from_utf8(raw.to_vec()).map_err(|_| format!("TIFF: tag {tag} is not UTF-8"))
    }
}

fn read_ifd0(bytes: &[u8]) -> Result<Ifd<'_>, String> {
    let little = match bytes.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err("TIFF: not a TIFF (bad byte-order mark)".into()),
    };
    let reader = Reader { bytes, little };
    match reader.u16_at(2)? {
        42 => {}
        43 => return Err("TIFF: BigTIFF is not supported".into()),
        other => return Err(format!("TIFF: magic {other} is not 42")),
    }
    let ifd_at = reader.u32_at(4)? as usize;
    let count = reader.u16_at(ifd_at)? as usize;
    // Twelve bytes an entry plus the next-IFD pointer must all be inside the object before any
    // entry is trusted.
    let ifd_end = ifd_at.checked_add(2 + count * 12 + 4).ok_or("TIFF: IFD length overflows")?;
    if ifd_end > bytes.len() {
        return Err("TIFF: IFD runs past the end of the object".into());
    }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let at = ifd_at + 2 + index * 12;
        let tag = reader.u16_at(at)?;
        let field_type = reader.u16_at(at + 2)?;
        let value_count = reader.u32_at(at + 4)?;
        let Some(size) = type_size(field_type) else {
            // An unknown type is only fatal if the reader wants the tag; skip it here.
            continue;
        };
        let total = u64::from(value_count) * size as u64;
        let values_at = if total <= 4 {
            at + 8
        } else {
            let offset = reader.u32_at(at + 8)? as usize;
            if offset.checked_add(total as usize).is_none_or(|end| end > bytes.len()) {
                return Err(format!("TIFF: tag {tag} points outside the object"));
            }
            offset
        };
        entries.push((tag, Entry { field_type, count: value_count, values_at }));
    }
    Ok(Ifd { reader, entries })
}

/// Decode band 0 of the full-resolution image, with the georeferencing and metadata tags the
/// caller's source contract is checked against.
pub fn decode_band0(bytes: &[u8]) -> Result<Cog, String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_OBJECT_BYTES {
        return Err("TIFF: object size is outside the accepted bounds".into());
    }
    let ifd = read_ifd0(bytes)?;

    let width = ifd.scalar(TAG_IMAGE_WIDTH)?.ok_or("TIFF: no ImageWidth")?;
    let height = ifd.scalar(TAG_IMAGE_LENGTH)?.ok_or("TIFF: no ImageLength")?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("TIFF: {width} x {height} is outside the accepted image bounds"));
    }
    let samples_per_pixel = ifd.scalar(TAG_SAMPLES_PER_PIXEL)?.unwrap_or(1);
    if samples_per_pixel == 0 || samples_per_pixel > 4 {
        return Err(format!("TIFF: {samples_per_pixel} samples per pixel is outside the accepted range"));
    }
    match ifd.scalar(TAG_COMPRESSION)?.unwrap_or(1) {
        COMPRESSION_DEFLATE | COMPRESSION_DEFLATE_ADOBE => {}
        other => return Err(format!("TIFF: compression {other} is not deflate")),
    }
    match ifd.scalar(TAG_PREDICTOR)?.unwrap_or(1) {
        1 => {}
        other => return Err(format!("TIFF: predictor {other} is not supported")),
    }
    match ifd.scalar(TAG_PLANAR_CONFIGURATION)?.unwrap_or(1) {
        1 => {}
        other => return Err(format!("TIFF: planar configuration {other} is not interleaved")),
    }
    let bits = ifd.integers(TAG_BITS_PER_SAMPLE)?;
    let formats = ifd.integers(TAG_SAMPLE_FORMAT)?;
    if bits.len() != samples_per_pixel as usize || bits.iter().any(|value| *value != 32) {
        return Err("TIFF: every sample must be 32 bits".into());
    }
    if formats.len() != samples_per_pixel as usize || formats.iter().any(|value| *value != SAMPLE_FORMAT_IEEE_FLOAT) {
        return Err("TIFF: every sample must be IEEE float".into());
    }

    let tile_width = ifd.scalar(TAG_TILE_WIDTH)?.ok_or("TIFF: the image is not tiled")?;
    let tile_length = ifd.scalar(TAG_TILE_LENGTH)?.ok_or("TIFF: the image is not tiled")?;
    if tile_width == 0 || tile_length == 0 || !tile_width.is_multiple_of(16) || !tile_length.is_multiple_of(16) {
        return Err(format!("TIFF: {tile_width} x {tile_length} is not a legal tile size"));
    }
    let tile_samples = u64::from(tile_width) * u64::from(tile_length) * u64::from(samples_per_pixel);
    let tile_bytes = tile_samples * 4;
    if tile_bytes > MAX_TILE_BYTES {
        return Err("TIFF: tile is larger than the accepted ceiling".into());
    }
    let tile_cols = width.div_ceil(tile_width);
    let tile_rows = height.div_ceil(tile_length);
    let tile_count = tile_cols as usize * tile_rows as usize;
    let offsets = ifd.integers(TAG_TILE_OFFSETS)?;
    let counts = ifd.integers(TAG_TILE_BYTE_COUNTS)?;
    if offsets.len() != tile_count || counts.len() != tile_count {
        return Err(format!("TIFF: {} tile offsets and {} counts for {tile_count} tiles", offsets.len(), counts.len()));
    }

    let scale = ifd.doubles(TAG_MODEL_PIXEL_SCALE)?;
    let tiepoint = ifd.doubles(TAG_MODEL_TIEPOINT)?;
    if scale.len() < 2 || tiepoint.len() < 6 {
        return Err("TIFF: no usable ModelPixelScale/ModelTiepoint".into());
    }
    // A tiepoint anywhere but raster (0,0) would mean the georeferencing anchor is not the
    // upper-left pixel corner, and every index this decoder hands back would be shifted.
    if tiepoint[0] != 0.0 || tiepoint[1] != 0.0 {
        return Err("TIFF: ModelTiepoint is not anchored at raster (0, 0)".into());
    }
    if !scale[0].is_finite() || !scale[1].is_finite() || scale[0] <= 0.0 || scale[1] <= 0.0 {
        return Err("TIFF: ModelPixelScale is not a positive finite size".into());
    }
    let nodata_text = ifd.ascii(TAG_GDAL_NODATA)?;
    let nodata: f64 =
        nodata_text.trim().parse().map_err(|_| format!("TIFF: GDAL_NODATA {nodata_text:?} is not a number"))?;
    if !nodata.is_finite() {
        return Err("TIFF: GDAL_NODATA must be a finite sentinel".into());
    }

    // Prove every tile is really there before reserving the image. A header can claim 32 M pixels
    // in twelve bytes; it cannot claim them without also pointing at `tile_count` payloads inside
    // an object that is itself capped, so this pre-pass is what stops a small hostile object from
    // costing a large allocation.
    for (index, (start, length)) in offsets.iter().zip(&counts).enumerate() {
        let (start, length) = (*start as usize, *length as usize);
        let end = start.checked_add(length).ok_or("TIFF: tile extent overflows")?;
        if length == 0 || end > bytes.len() {
            return Err(format!("TIFF: tile {index} is outside the object"));
        }
    }

    // One tile at a time into a reused scratch buffer; only band 0 is kept.
    let mut values = vec![f32::NAN; width as usize * height as usize];
    let mut scratch = Vec::with_capacity(tile_bytes as usize);
    for tile_row in 0..tile_rows {
        for tile_col in 0..tile_cols {
            let index = tile_row as usize * tile_cols as usize + tile_col as usize;
            let start = offsets[index] as usize;
            let length = counts[index] as usize;
            let end = start.checked_add(length).ok_or("TIFF: tile extent overflows")?;
            let payload = bytes.get(start..end).ok_or_else(|| format!("TIFF: tile {index} is outside the object"))?;
            scratch.clear();
            let mut decoder = flate2::read::ZlibDecoder::new(payload).take(tile_bytes + 1);
            decoder.read_to_end(&mut scratch).map_err(|error| format!("TIFF: tile {index} inflate: {error}"))?;
            if scratch.len() as u64 != tile_bytes {
                return Err(format!("TIFF: tile {index} inflated to {} bytes, expected {tile_bytes}", scratch.len()));
            }
            let tile = Reader { bytes: &scratch, little: ifd.reader.little };
            // Tiles are always stored full-size; the rows and columns past the image edge are
            // padding and are dropped here rather than written outside the image.
            let rows = tile_length.min(height - tile_row * tile_length);
            let cols = tile_width.min(width - tile_col * tile_width);
            for row in 0..rows {
                let src_row = row as usize * tile_width as usize * samples_per_pixel as usize;
                let dst_row =
                    (tile_row * tile_length + row) as usize * width as usize + (tile_col * tile_width) as usize;
                for col in 0..cols as usize {
                    let sample = (src_row + col * samples_per_pixel as usize) * 4;
                    values[dst_row + col] = tile.f32_at(sample)?;
                }
            }
        }
    }

    Ok(Cog {
        width,
        height,
        samples_per_pixel,
        tile_width,
        tile_length,
        ul_x: tiepoint[3],
        ul_y: tiepoint[4],
        pixel_x: scale[0],
        pixel_y: scale[1],
        nodata,
        geo_double_params: ifd.doubles(TAG_GEO_DOUBLE_PARAMS)?,
        geo_key_directory: ifd.integers(TAG_GEO_KEY_DIRECTORY)?,
        metadata: ifd.ascii(TAG_GDAL_METADATA)?,
        values,
    })
}

impl Cog {
    /// One GeoTIFF key's inline `SHORT` value — `ProjCoordTransGeoKey` (3075) and friends.
    ///
    /// The directory is a flat run of four-`uint16` entries after a four-value header; an entry
    /// whose second field is `0` stores its value inline in the fourth, which is the only form
    /// this answers (the others point into `GeoDoubleParams` or `GeoAsciiParams`, which callers
    /// read directly).
    pub fn geo_key(&self, key: u16) -> Option<u16> {
        let count = *self.geo_key_directory.get(3)? as usize;
        (0..count).find_map(|index| {
            let entry = self.geo_key_directory.get(4 + index * 4..8 + index * 4)?;
            (entry[0] == u32::from(key) && entry[1] == 0).then(|| entry[3] as u16)
        })
    }
}

/// Read one `<Item name="...">value</Item>` out of the `GDAL_METADATA` XML.
///
/// The first match wins, which is exactly what a caller wants: GDAL writes the per-band items
/// (`_FillValue`, `DESCRIPTION`, `undetect`) once per sample in sample order, so the first
/// occurrence is band 0's — the band this reader decodes.
pub fn metadata_item<'a>(metadata: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("<Item name=\"{name}\"");
    let mut rest = metadata;
    loop {
        let at = rest.find(&needle)?;
        let after = &rest[at + needle.len()..];
        // The attribute must be complete: `name="date"` must not match `name="dateX"`.
        let boundary = after.starts_with(' ') || after.starts_with('>') || after.starts_with('/');
        if let (true, Some(open)) = (boundary, after.find('>')) {
            let body = &after[open + 1..];
            if let Some(close) = body.find("</Item>") {
                return Some(body[..close].trim());
            }
        }
        rest = after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a tiny classic TIFF in the shape this reader accepts, so the negatives below can
    /// break exactly one thing at a time.
    struct Builder {
        width: u32,
        height: u32,
        tile: u32,
        samples: u32,
        compression: u32,
        predictor: u32,
        sample_format: u32,
        nodata: &'static str,
        pixels: Vec<f32>,
    }

    impl Builder {
        fn new(width: u32, height: u32, tile: u32) -> Self {
            Self {
                width,
                height,
                tile,
                samples: 2,
                compression: COMPRESSION_DEFLATE,
                predictor: 1,
                sample_format: SAMPLE_FORMAT_IEEE_FLOAT,
                nodata: "-9999000",
                pixels: vec![0.0; (width * height) as usize],
            }
        }

        fn build(&self) -> Vec<u8> {
            let tile_cols = self.width.div_ceil(self.tile);
            let tile_rows = self.height.div_ceil(self.tile);
            let mut payloads = Vec::new();
            for tile_row in 0..tile_rows {
                for tile_col in 0..tile_cols {
                    let mut raw = Vec::new();
                    for row in 0..self.tile {
                        for col in 0..self.tile {
                            let y = tile_row * self.tile + row;
                            let x = tile_col * self.tile + col;
                            let value = if y < self.height && x < self.width {
                                self.pixels[(y * self.width + x) as usize]
                            } else {
                                f32::from_bits(0xDEAD_BEEF) // padding: must never be read
                            };
                            for sample in 0..self.samples {
                                let stored = if sample == 0 { value } else { 0.5 };
                                raw.extend_from_slice(&stored.to_le_bytes());
                            }
                        }
                    }
                    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                    encoder.write_all(&raw).expect("deflate");
                    payloads.push(encoder.finish().expect("deflate"));
                }
            }

            let metadata = b"<GDALMetadata>\n<Item name=\"date\">20260810</Item>\n\
                <Item name=\"prodname\">test composite</Item>\n</GDALMetadata>\n\0"
                .to_vec();
            let nodata = format!("{}\0", self.nodata).into_bytes();
            let bits: Vec<u32> = vec![32; self.samples as usize];
            let formats: Vec<u32> = vec![self.sample_format; self.samples as usize];

            // tag, type, values
            let mut heap: Vec<u8> = Vec::new();
            let mut long_values: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
            let mut short_values: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
            let mut push = |tag: u16, field_type: u16, count: u32, bytes: Vec<u8>| {
                if bytes.len() <= 4 {
                    short_values.push((tag, field_type, count, bytes));
                } else {
                    long_values.push((tag, field_type, count, bytes));
                }
            };
            let shorts =
                |values: &[u32]| -> Vec<u8> { values.iter().flat_map(|value| (*value as u16).to_le_bytes()).collect() };
            let longs = |values: &[u32]| -> Vec<u8> { values.iter().flat_map(|value| value.to_le_bytes()).collect() };
            let doubles = |values: &[f64]| -> Vec<u8> { values.iter().flat_map(|value| value.to_le_bytes()).collect() };

            push(TAG_IMAGE_WIDTH, 3, 1, shorts(&[self.width]));
            push(TAG_IMAGE_LENGTH, 3, 1, shorts(&[self.height]));
            push(TAG_BITS_PER_SAMPLE, 3, self.samples, shorts(&bits));
            push(TAG_COMPRESSION, 3, 1, shorts(&[self.compression]));
            push(TAG_SAMPLES_PER_PIXEL, 3, 1, shorts(&[self.samples]));
            push(TAG_PLANAR_CONFIGURATION, 3, 1, shorts(&[1]));
            push(TAG_PREDICTOR, 3, 1, shorts(&[self.predictor]));
            push(TAG_TILE_WIDTH, 3, 1, shorts(&[self.tile]));
            push(TAG_TILE_LENGTH, 3, 1, shorts(&[self.tile]));
            push(TAG_TILE_OFFSETS, 4, payloads.len() as u32, longs(&vec![0; payloads.len()]));
            push(
                TAG_TILE_BYTE_COUNTS,
                4,
                payloads.len() as u32,
                longs(&payloads.iter().map(|p| p.len() as u32).collect::<Vec<_>>()),
            );
            push(TAG_SAMPLE_FORMAT, 3, self.samples, shorts(&formats));
            push(TAG_MODEL_PIXEL_SCALE, 12, 3, doubles(&[1000.0, 1000.0, 0.0]));
            push(TAG_MODEL_TIEPOINT, 12, 6, doubles(&[0.0, 0.0, 0.0, -500.0, 500.0, 0.0]));
            push(TAG_GDAL_METADATA, 2, metadata.len() as u32, metadata.clone());
            push(TAG_GDAL_NODATA, 2, nodata.len() as u32, nodata.clone());

            let mut entries: Vec<(u16, u16, u32, Vec<u8>)> = short_values.into_iter().chain(long_values).collect();
            entries.sort_by_key(|(tag, _, _, _)| *tag);
            let ifd_at = 8usize;
            let ifd_len = 2 + entries.len() * 12 + 4;
            let heap_at = ifd_at + ifd_len;
            let mut positions = std::collections::BTreeMap::new();
            for (tag, _, _, bytes) in &entries {
                if bytes.len() > 4 {
                    if heap.len() % 2 == 1 {
                        heap.push(0);
                    }
                    positions.insert(*tag, heap_at + heap.len());
                    heap.extend_from_slice(bytes);
                }
            }
            let data_at = heap_at + heap.len();
            let mut tile_offsets = Vec::new();
            let mut cursor = data_at;
            for payload in &payloads {
                tile_offsets.push(cursor as u32);
                cursor += payload.len();
            }
            let patch = positions[&TAG_TILE_OFFSETS] - heap_at;
            heap[patch..patch + payloads.len() * 4].copy_from_slice(&longs(&tile_offsets));

            let mut out = Vec::new();
            out.extend_from_slice(b"II");
            out.extend_from_slice(&42u16.to_le_bytes());
            out.extend_from_slice(&(ifd_at as u32).to_le_bytes());
            out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for (tag, field_type, count, bytes) in &entries {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&field_type.to_le_bytes());
                out.extend_from_slice(&count.to_le_bytes());
                if bytes.len() <= 4 {
                    let mut inline = bytes.clone();
                    inline.resize(4, 0);
                    out.extend_from_slice(&inline);
                } else {
                    out.extend_from_slice(&(positions[tag] as u32).to_le_bytes());
                }
            }
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&heap);
            for payload in &payloads {
                out.extend_from_slice(payload);
            }
            out
        }
    }

    #[test]
    fn a_tiled_float_image_round_trips_and_padding_never_leaks() {
        // 40 x 24 over 16-pixel tiles: three tile columns and two tile rows, and the right and
        // bottom tiles are partial. Every stored pixel outside the image is 0xDEADBEEF, so a
        // reader that copied padding would show it.
        let mut builder = Builder::new(40, 24, 16);
        for (index, value) in builder.pixels.iter_mut().enumerate() {
            *value = index as f32;
        }
        builder.pixels[0] = -9_999_000.0;
        builder.pixels[1] = f32::NAN;
        let cog = decode_band0(&builder.build()).expect("decodes");
        assert_eq!((cog.width, cog.height, cog.tile_width), (40, 24, 16));
        assert_eq!(cog.values.len(), 40 * 24);
        assert_eq!(cog.nodata, -9_999_000.0);
        assert_eq!(cog.values[0], -9_999_000.0);
        assert!(cog.values[1].is_nan());
        for index in 2..40 * 24 {
            assert_eq!(cog.values[index], index as f32, "pixel {index}");
        }
        assert_eq!(metadata_item(&cog.metadata, "date"), Some("20260810"));
        assert_eq!(metadata_item(&cog.metadata, "prodname"), Some("test composite"));
        assert_eq!(metadata_item(&cog.metadata, "dat"), None);
        assert_eq!(metadata_item(&cog.metadata, "absent"), None);
    }

    #[test]
    fn the_pinned_contract_is_refused_when_it_is_broken() {
        fn refuse(expected: &str, builder: &Builder) {
            let error = decode_band0(&builder.build()).unwrap_err();
            assert!(error.contains(expected), "expected {expected:?}, got {error}");
        }
        let broken = |change: fn(&mut Builder)| {
            let mut builder = Builder::new(32, 32, 16);
            change(&mut builder);
            builder
        };
        refuse("compression", &broken(|b| b.compression = 1));
        refuse("predictor", &broken(|b| b.predictor = 2));
        refuse("IEEE float", &broken(|b| b.sample_format = 1));
        refuse("GDAL_NODATA", &broken(|b| b.nodata = "not-a-number"));
        refuse("samples per pixel", &broken(|b| b.samples = 9));
    }

    #[test]
    fn degenerate_objects_error_rather_than_panic() {
        for garbage in [vec![], vec![0u8; 3], b"II\x2a\x00\xff\xff\xff\xff".to_vec(), vec![0x4D; 4096]] {
            assert!(decode_band0(&garbage).is_err());
        }
        // A truncated object: the IFD says there are tiles, but the payload is gone.
        let good = Builder::new(32, 32, 16).build();
        assert!(decode_band0(&good[..good.len() / 2]).is_err());
    }
}
