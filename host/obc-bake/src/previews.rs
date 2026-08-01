//! Deterministic square previews for every catalog skin.
//!
//! A preview is presentation, so it must not depend on which region subset an
//! operator happened to refresh. The bakery therefore carries one small,
//! canonical Teningen map. It restamps only that map's style table through
//! [`obcm_assemble::Skin::resolve`] and renders the result through the production
//! [`obc_render::MapRenderer`]. Geometry, LOD selection, RGB565 expansion and
//! painter ordering are consequently the device's; only the host framebuffer and
//! PNG encoder are preview-specific.
//!
//! The fixture is deliberately an ordinary OBCM rather than a hand-authored
//! picture. A schema change that renumbers styles makes [`check_source`] fail
//! before a long bake begins, forcing the maintainer to refresh the Teningen cut
//! instead of publishing a plausible-looking but stale thumbnail.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use obc_formats::io::SliceSource;
use obc_formats::obcm::{HEADER_LEN, STYLE_RECORD_LEN};
use obc_pack::catalog::{feature_type_ids, Catalog};
use obc_pack::config::Config;
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader};
use obc_render::{zoom_for_mpp, MapRenderer, Viewport};
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::shard::pack_style_table;

/// Published beside `cells/`, `regions/`, and `skins/`.
pub const PREVIEWS_DIR: &str = "previews";

const SOURCE: &[u8] = include_bytes!("../assets/teningen-preview.obcm");
const WIDTH: u32 = 240;
const HEIGHT: u32 = 240;
/// Teningen town centre: residential streets, the B3, rail, water, fields,
/// buildings and the edge of the Black Forest all share one device-sized frame.
const CAMERA_LON: i32 = 7_814_000;
const CAMERA_LAT: i32 = 48_130_000;
/// Mid-riding scale: 1.2 km across the 240 px square.
const METERS_PER_PIXEL: f32 = 5.0;

const HEADER_STYLE_OFFSET_AT: usize = 21;
const HEADER_MARKER_COLOR_AT: usize = 30;

/// What one regeneration wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewReport {
    pub skins: usize,
    pub bytes: u64,
}

/// Refuse a stale canonical map before the operator spends hours cutting cells.
pub fn check_source(schema: &Config) -> Result<(), String> {
    let source = SliceSource(SOURCE);
    let tables = MapTables::parse(&source).map_err(|e| format!("Teningen preview fixture is not readable: {e:?}"))?;
    let have: Vec<u8> = tables.styles().iter().filter_map(|style| style.as_ref().map(|s| s.id)).collect();
    let mut want: Vec<u8> = feature_type_ids(schema).into_values().collect();
    want.sort_unstable();
    if have != want {
        return Err(format!(
            "Teningen preview fixture carries style ids {have:?}, but the schema assigns {want:?}. Repack \
             host/obc-bake/assets/teningen-preview.obcm from the bbox documented in assets/README.md before baking."
        ));
    }
    Ok(())
}

/// Regenerate one square PNG per skin in `tree/previews/`.
///
/// `catalog` is a just-generated view of the tree. Its schema and inlined skins
/// are the exact OBCC documents the assembler consumes, so no second config-to-
/// style-table translation is allowed to grow here.
pub fn generate(tree: &Path, catalog: &Catalog) -> Result<PreviewReport, String> {
    let schema_json = serde_json::to_string(&catalog.schema).map_err(|e| e.to_string())?;
    let schema = Schema::parse(&schema_json)?;
    schema.validate()?;

    let dir = tree.join(PREVIEWS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    prune(&dir, &catalog.skins.iter().map(|s| s.id.as_str()).collect::<Vec<_>>())?;

    let mut bytes = 0_u64;
    for entry in &catalog.skins {
        let skin_json = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        let skin = Skin::parse(&skin_json)?;
        let png = render(&schema, &skin)?;
        bytes += png.len() as u64;
        write_atomic_if_changed(&dir.join(format!("{}.png", entry.id)), &png)?;
    }
    Ok(PreviewReport { skins: catalog.skins.len(), bytes })
}

fn prune(dir: &Path, skin_ids: &[&str]) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() {
            return Err(format!("{}: preview directory contains a nested directory", path.display()));
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return Err(format!("{}: preview filename is not UTF-8", path.display()));
        };
        if name.ends_with(".tmp") {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            continue;
        }
        let Some(id) = name.strip_suffix(".png") else {
            return Err(format!("{}: only <skin-id>.png belongs in {PREVIEWS_DIR}/", path.display()));
        };
        if !skin_ids.contains(&id) {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

fn render(schema: &Schema, skin: &Skin) -> Result<Vec<u8>, String> {
    let styles = skin.resolve(schema)?;
    let packed = pack_style_table(&styles);
    let mut map = SOURCE.to_vec();
    if map.len() < HEADER_LEN {
        return Err("Teningen preview fixture is shorter than the OBCM header".into());
    }
    let style_offset = u32::from_le_bytes(
        map[HEADER_STYLE_OFFSET_AT..HEADER_STYLE_OFFSET_AT + 4]
            .try_into()
            .expect("four bytes inside the checked header"),
    ) as usize;
    let count = *map.get(style_offset).ok_or("Teningen preview fixture has a bad style offset")? as usize;
    let end = style_offset.checked_add(1 + count * STYLE_RECORD_LEN).ok_or("Teningen preview style table overflows")?;
    let slot = map.get_mut(style_offset..end).ok_or("Teningen preview style table runs past the file")?;
    if slot.len() != packed.len() {
        return Err(format!(
            "Teningen preview fixture has {count} styles, but skin {:?} resolves to {}",
            skin.id,
            styles.len()
        ));
    }
    let have_ids: Vec<u8> = slot[1..].chunks_exact(STYLE_RECORD_LEN).map(|record| record[0]).collect();
    let want_ids: Vec<u8> = styles.iter().map(|style| style.id).collect();
    if have_ids != want_ids {
        return Err(format!(
            "Teningen preview fixture style ids {have_ids:?} do not match skin {:?}'s {want_ids:?}",
            skin.id
        ));
    }
    slot.copy_from_slice(&packed);
    map[HEADER_MARKER_COLOR_AT..HEADER_MARKER_COLOR_AT + 2].copy_from_slice(&skin.marker_color.to_le_bytes());

    let source = SliceSource(&map);
    let tables = MapTables::parse(&source).map_err(|e| format!("restamped preview map is not readable: {e:?}"))?;
    let cache = MapCache::new();
    let reader = Reader::new(&source, &tables, &cache);
    let mut frame = Frame::new(WIDTH, HEIGHT);
    let vp = Viewport::new(WIDTH as f32, HEIGHT as f32, CAMERA_LON, CAMERA_LAT, zoom_for_mpp(METERS_PER_PIXEL));
    let background = reader.backdrop_style().map_or(0xFFFF, |style| style.color);
    let stats = MapRenderer::new().render(&mut frame, &reader, &vp, rgb(background), rgb);
    if stats.features_drawn == 0 {
        return Err("the canonical Teningen preview camera rendered no features".into());
    }

    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&frame.bytes, WIDTH, HEIGHT, ExtendedColorType::Rgb8)
        .map_err(|e| format!("encode Teningen preview PNG: {e}"))?;
    Ok(png)
}

fn rgb(color: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(color);
    Rgb888::new(r, g, b)
}

fn write_atomic_if_changed(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if std::fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let tmp = path.with_extension("png.tmp");
    let result = (|| {
        let mut file = File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.write_all(bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.sync_all().map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Minimal RGB888 target for the host-side PNG encoder. Pixel clipping and the
/// rectangle fast path match the simulator framebuffer, while map drawing itself
/// stays entirely in `obc-render`.
struct Frame {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl Frame {
    fn new(width: u32, height: u32) -> Frame {
        Frame { width, height, bytes: vec![0; (width * height * 3) as usize] }
    }

    fn put(&mut self, x: i32, y: i32, color: Rgb888) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let at = ((y as u32 * self.width + x as u32) * 3) as usize;
        self.bytes[at..at + 3].copy_from_slice(&[color.r(), color.g(), color.b()]);
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for Frame {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            self.put(point.x, point.y, color);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clipped = area.intersection(&self.bounding_box());
        if let Some(bottom_right) = clipped.bottom_right() {
            for y in clipped.top_left.y..=bottom_right.y {
                for x in clipped.top_left.x..=bottom_right.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        for pixel in self.bytes.chunks_exact_mut(3) {
            pixel.copy_from_slice(&[color.r(), color.g(), color.b()]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped_schema() -> Schema {
        let config = Config::parse(include_str!("../../../builder/presets/schema.json")).expect("schema config parses");
        let bands = obc_pack::grid::BandTable::recommended();
        let lods: Vec<_> = config
            .lods
            .iter()
            .enumerate()
            .map(|(index, lod)| {
                let band = bands
                    .bands
                    .iter()
                    .find(|band| band.lods.contains(&index))
                    .expect("recommended bands cover every LOD");
                serde_json::json!({ "index": index, "max_mpp": lod.max_mpp, "band": band.id })
            })
            .collect();
        let mut styles: Vec<_> = feature_type_ids(&config)
            .into_iter()
            .map(|(feature_type, id)| serde_json::json!({ "id": id, "feature_type": feature_type }))
            .collect();
        styles.sort_by_key(|style| style["id"].as_u64());
        let document = serde_json::json!({
            "id": "bikepacking",
            "revision": 1,
            "obcm_version": obc_formats::obcm::VERSION,
            "lods": lods,
            "bands": bands.bands,
            "styles": styles,
            "routing": {
                "min_component_edges": config.routing.min_component_edges,
                "profiles": config.routing.profiles.iter().map(|profile| &profile.name).collect::<Vec<_>>(),
            },
            "chunk_size": config.chunk_size,
        });
        let schema = Schema::parse(&document.to_string()).expect("assembly schema parses");
        schema.validate().expect("assembly schema validates");
        schema
    }

    fn shipped_skin(id: &str) -> Skin {
        let text = match id {
            "default" => include_str!("../../../builder/presets/skins/default.json"),
            "dusk" => include_str!("../../../builder/presets/skins/dusk.json"),
            _ => panic!("unknown shipped test skin"),
        };
        let config = Config::parse(text).expect("skin config parses");
        let mut styles: Vec<_> = config
            .features
            .iter()
            .flat_map(|(key, values)| {
                values.iter().map(move |(value, style)| {
                    serde_json::json!({
                        "feature_type": format!("{key}.{value}"),
                        "color": style.color,
                        "weight": style.weight,
                        "z_index": style.z_index,
                        "priority": style.priority,
                        "dashed": style.line_style == obc_pack::config::LineStyle::Dashed,
                        "color2": style.color2,
                    })
                })
            })
            .collect();
        let ids = feature_type_ids(&config);
        styles.sort_by_key(|style| ids[style["feature_type"].as_str().unwrap()]);
        Skin::parse(
            &serde_json::json!({ "id": id, "name": id, "marker_color": config.marker_color, "styles": styles })
                .to_string(),
        )
        .expect("assembly skin parses")
    }

    #[test]
    fn the_checked_in_schema_still_matches_the_teningen_fixture() {
        let schema = include_str!("../../../builder/presets/schema.json");
        let config = Config::parse(schema).expect("schema parses");
        check_source(&config).expect("fixture style ids match");
    }

    #[test]
    fn shipped_skins_render_distinct_deterministic_square_pngs() {
        let schema = shipped_schema();
        let default_skin = shipped_skin("default");
        let residential_color = default_skin
            .styles
            .iter()
            .find(|style| style.feature_type.as_deref() == Some("landuse.residential"))
            .expect("default skin carries residential landuse")
            .color;
        let default = render(&schema, &default_skin).expect("default preview renders");
        let dusk = render(&schema, &shipped_skin("dusk")).expect("dusk preview renders");

        assert_eq!(&default[..8], b"\x89PNG\r\n\x1a\n");
        let default_image = image::load_from_memory(&default).unwrap().to_rgb8();
        assert_eq!(default_image.width(), WIDTH);
        assert_eq!(default_image.height(), HEIGHT);
        assert_ne!(default, dusk, "two color schemes should not produce the same preview");
        assert_eq!(default, render(&schema, &shipped_skin("default")).unwrap());

        // Teningen's lower half is predominantly residential landuse. This is a
        // deliberately broad coverage assertion rather than a PNG hash: it
        // catches an ingest crop dropping the town's multipolygon while allowing
        // unrelated renderer or compression changes.
        let (r, g, b) = rgb565_to_rgb888(residential_color);
        let residential = [r, g, b];
        let lower_residential =
            default_image.enumerate_pixels().filter(|(_, y, pixel)| *y >= 140 && pixel.0 == residential).count();
        assert!(
            lower_residential > 10_000,
            "Teningen residential coverage regressed: only {lower_residential} pixels in the lower preview"
        );
    }
}
