//! Test support: a **hand-written** float32 GeoTIFF, and the closed-form surface the bake is
//! checked against.
//!
//! The TIFF is assembled from the format's own field tables rather than through the `tiff` crate's
//! encoder, for the reason `obcm-testkit` exists on the OBCM side: a fixture built by the same
//! library that reads it proves only that the library is self-consistent. Written by hand, it also
//! exercises the *plainest* possible layout — uncompressed, one strip, `PixelIsArea` or
//! `PixelIsPoint` on request — which is the layout a reprojected or hand-cut source would have,
//! while the real GLO-30 tiles the gated test uses are tiled, DEFLATE'd and float-predicted.

#![allow(dead_code)] // each integration-test binary uses a different subset

use std::path::Path;

/// TIFF field types this builder emits.
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_ASCII: u16 = 2;
const TYPE_DOUBLE: u16 = 12;

/// `GTRasterTypeGeoKey` values.
pub const PIXEL_IS_AREA: u16 = 1;
pub const PIXEL_IS_POINT: u16 = 2;

/// A synthetic DEM raster: a regular north-up geographic grid of `f32` posts.
pub struct SyntheticDem {
    /// Longitude of the westernmost post (`PixelIsPoint`) or of the raster's west edge
    /// (`PixelIsArea`), degrees — whatever the tie point should say.
    pub tie_lon_deg: f64,
    /// Likewise for the north edge / northernmost post.
    pub tie_lat_deg: f64,
    pub step_deg: f64,
    pub rows: usize,
    pub cols: usize,
    /// Samples in **GeoTIFF order**: row 0 is the northernmost.
    pub north_up: Vec<f32>,
    pub raster_type: u16,
    pub nodata: Option<f64>,
}

impl SyntheticDem {
    /// Build a raster by evaluating `height(lat_deg, lon_deg)` at every post.
    ///
    /// `tie_*` is interpreted per `raster_type`, exactly as a reader must interpret it, so a fixture
    /// written as `PixelIsArea` really does sit half a step away from the same numbers written as
    /// `PixelIsPoint`.
    pub fn build(
        tie_lat_deg: f64,
        tie_lon_deg: f64,
        step_deg: f64,
        rows: usize,
        cols: usize,
        raster_type: u16,
        mut height: impl FnMut(f64, f64) -> f32,
    ) -> SyntheticDem {
        let half = if raster_type == PIXEL_IS_POINT { 0.0 } else { step_deg / 2.0 };
        let (post0_lat, post0_lon) = (tie_lat_deg - half, tie_lon_deg + half);
        let mut north_up = Vec::with_capacity(rows * cols);
        for row in 0..rows {
            for col in 0..cols {
                north_up.push(height(post0_lat - step_deg * row as f64, post0_lon + step_deg * col as f64));
            }
        }
        SyntheticDem { tie_lon_deg, tie_lat_deg, step_deg, rows, cols, north_up, raster_type, nodata: None }
    }

    /// Overwrite one post, in **north-up** indices — how a void is punched into a fixture.
    pub fn set_north_up(&mut self, row: usize, col: usize, value: f32) {
        self.north_up[row * self.cols + col] = value;
    }

    /// Serialise as an uncompressed single-strip float32 GeoTIFF.
    pub fn to_geotiff(&self) -> Vec<u8> {
        // Layout: 8-byte header, then the IFD, then out-of-line values, then the strip.
        let mut entries: Vec<(u16, u16, u32, Value)> = Vec::new();
        let mut push = |tag: u16, ty: u16, count: u32, value: Value| entries.push((tag, ty, count, value));

        push(256, TYPE_LONG, 1, Value::Inline((self.cols as u32).to_le_bytes()));
        push(257, TYPE_LONG, 1, Value::Inline((self.rows as u32).to_le_bytes()));
        push(258, TYPE_SHORT, 1, Value::Inline(short(32)));
        push(259, TYPE_SHORT, 1, Value::Inline(short(1))); // uncompressed
        push(262, TYPE_SHORT, 1, Value::Inline(short(1))); // BlackIsZero
        push(273, TYPE_LONG, 1, Value::StripOffset);
        push(277, TYPE_SHORT, 1, Value::Inline(short(1)));
        push(278, TYPE_LONG, 1, Value::Inline((self.rows as u32).to_le_bytes()));
        push(279, TYPE_LONG, 1, Value::Inline(((self.rows * self.cols * 4) as u32).to_le_bytes()));
        push(284, TYPE_SHORT, 1, Value::Inline(short(1))); // chunky
        push(339, TYPE_SHORT, 1, Value::Inline(short(3))); // IEEE floating point
        push(33550, TYPE_DOUBLE, 3, Value::Blob(doubles(&[self.step_deg, self.step_deg, 0.0])));
        push(33922, TYPE_DOUBLE, 6, Value::Blob(doubles(&[0.0, 0.0, 0.0, self.tie_lon_deg, self.tie_lat_deg, 0.0])));
        // GeoKeyDirectory: header (version, revision, minor, key count) + 4 shorts per key.
        let keys: Vec<u16> = vec![
            1,
            1,
            0,
            3, //
            1024,
            0,
            1,
            2, // GTModelTypeGeoKey = geographic
            1025,
            0,
            1,
            self.raster_type, // GTRasterTypeGeoKey
            2048,
            0,
            1,
            4326, // GeographicTypeGeoKey = WGS 84
        ];
        push(34735, TYPE_SHORT, keys.len() as u32, Value::Blob(keys.iter().flat_map(|k| k.to_le_bytes()).collect()));
        if let Some(nodata) = self.nodata {
            let mut text = format!("{nodata}").into_bytes();
            text.push(0);
            push(42113, TYPE_ASCII, text.len() as u32, Value::Blob(text));
        }
        entries.sort_by_key(|(tag, ..)| *tag);

        // Place the out-of-line blobs after the IFD, then the strip after them (word-aligned).
        let ifd_len = 2 + entries.len() * 12 + 4;
        let mut blob_at = 8 + ifd_len;
        let mut blobs: Vec<(usize, &[u8])> = Vec::new();
        for (_, _, _, value) in &entries {
            if let Value::Blob(bytes) = value {
                blobs.push((blob_at, bytes));
                blob_at += bytes.len() + bytes.len() % 2;
            }
        }
        let strip_at = blob_at;

        let mut out = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut blob = blobs.iter();
        for (tag, ty, count, value) in &entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            match value {
                Value::Inline(bytes) => out.extend_from_slice(bytes),
                Value::StripOffset => out.extend_from_slice(&(strip_at as u32).to_le_bytes()),
                Value::Blob(_) => {
                    let (at, _) = blob.next().expect("one placement per blob");
                    out.extend_from_slice(&(*at as u32).to_le_bytes());
                }
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        for (_, bytes) in &blobs {
            out.extend_from_slice(bytes);
            if bytes.len() % 2 == 1 {
                out.push(0);
            }
        }
        assert_eq!(out.len(), strip_at, "the strip must start where the IFD said it would");
        for v in &self.north_up {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Write the fixture into `dir` as `name.tif`.
    pub fn write(&self, dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{name}.tif"));
        std::fs::write(&path, self.to_geotiff()).expect("writing the synthetic GeoTIFF");
        path
    }
}

enum Value {
    Inline([u8; 4]),
    Blob(Vec<u8>),
    StripOffset,
}

/// A `SHORT` in a 4-byte value field: TIFF left-aligns it, so the second half stays zero.
fn short(v: u16) -> [u8; 4] {
    let [a, b] = v.to_le_bytes();
    [a, b, 0, 0]
}

fn doubles(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// A scratch directory that removes itself.
///
/// The name carries a process-wide counter as well as the caller's label: the test harness runs
/// these in parallel, and two `Scratch`es that agreed on a path would have one of them deleting the
/// other's fixture out from under it on drop.
pub struct Scratch(pub std::path::PathBuf);

static SCRATCH_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let seq = SCRATCH_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("obc-dem-test-{name}-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Lowercase hex SHA-256 — the digest the fixture pins are written against.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
