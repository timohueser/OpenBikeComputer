//! Target-independent core for the skin editor's fixed Teningen scene.

use embedded_graphics::pixelcolor::Rgb888;
use obc_formats::io::SliceSource;
use obc_formats::obcm::{HEADER_LEN, STYLE_RECORD_LEN};
use obc_host_core::RgbaFrame;
use obc_reader::{rgb565_to_device64, Error as ReadError, MapCache, MapTables, Reader};
use obc_render::{zoom_for_mpp, MapRenderer, Viewport};
use obcm_assemble::schema::{Schema, Skin};
use obcm_assemble::shard::pack_style_table;

pub const FRAME_W: u32 = 240;
pub const FRAME_H: u32 = 240;

const CAMERA_LON: i32 = 7_814_000;
const CAMERA_LAT: i32 = 48_130_000;
const METERS_PER_PIXEL: f32 = 5.0;

const HEADER_STYLE_OFFSET_AT: usize = 21;
const HEADER_MARKER_COLOR_AT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewErrorCode {
    NotAMap,
    Input,
    Internal,
}

impl PreviewErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            PreviewErrorCode::NotAMap => "not-a-map",
            PreviewErrorCode::Input => "input",
            PreviewErrorCode::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFailure {
    pub code: PreviewErrorCode,
    pub message: String,
}

impl PreviewFailure {
    fn input(message: impl Into<String>) -> Self {
        Self { code: PreviewErrorCode::Input, message: message.into() }
    }
}

fn read_failure(err: ReadError) -> PreviewFailure {
    match err {
        ReadError::BadMagic | ReadError::BadVersion | ReadError::TooShort | ReadError::BadOffset => PreviewFailure {
            code: PreviewErrorCode::NotAMap,
            message: "The Teningen preview map is missing, stale, or truncated.".into(),
        },
        ReadError::Source(_) | ReadError::CacheBusy => PreviewFailure {
            code: PreviewErrorCode::Internal,
            message: "The Teningen preview map could not be read.".into(),
        },
    }
}

pub struct MapPreview {
    bytes: Vec<u8>,
    tables: MapTables,
    schema: Schema,
    cache: Box<MapCache>,
    renderer: Box<MapRenderer>,
    frame: RgbaFrame,
    dirty: bool,
}

impl MapPreview {
    pub fn open(bytes: Vec<u8>, schema_json: &str, skin_json: &str) -> Result<Self, PreviewFailure> {
        let tables = MapTables::parse(&SliceSource(&bytes)).map_err(read_failure)?;
        let schema = Schema::parse(schema_json).map_err(PreviewFailure::input)?;
        schema.validate().map_err(PreviewFailure::input)?;
        let mut preview = Self {
            bytes,
            tables,
            schema,
            cache: MapCache::new_boxed(),
            renderer: Box::new(MapRenderer::new()),
            frame: RgbaFrame::new(FRAME_W, FRAME_H),
            dirty: true,
        };
        preview.set_skin(skin_json)?;
        Ok(preview)
    }

    pub fn set_skin(&mut self, skin_json: &str) -> Result<(), PreviewFailure> {
        let skin = Skin::parse(skin_json).map_err(PreviewFailure::input)?;
        let styles = skin.resolve(&self.schema).map_err(PreviewFailure::input)?;
        let packed = pack_style_table(&styles);

        if self.bytes.len() < HEADER_LEN {
            return Err(PreviewFailure::input("The Teningen preview is shorter than the OBCM header."));
        }
        let style_offset = u32::from_le_bytes(
            self.bytes[HEADER_STYLE_OFFSET_AT..HEADER_STYLE_OFFSET_AT + 4]
                .try_into()
                .expect("four bytes inside the checked header"),
        ) as usize;
        let count = *self
            .bytes
            .get(style_offset)
            .ok_or_else(|| PreviewFailure::input("The Teningen preview has a bad style offset."))?
            as usize;
        let end = style_offset
            .checked_add(1 + count * STYLE_RECORD_LEN)
            .ok_or_else(|| PreviewFailure::input("The Teningen preview style table overflows."))?;
        let slot = self
            .bytes
            .get_mut(style_offset..end)
            .ok_or_else(|| PreviewFailure::input("The Teningen preview style table is truncated."))?;
        if slot.len() != packed.len() {
            return Err(PreviewFailure::input(format!(
                "The preview has {count} styles, but this skin resolves to {}.",
                styles.len()
            )));
        }
        let have_ids: Vec<u8> = slot[1..].chunks_exact(STYLE_RECORD_LEN).map(|record| record[0]).collect();
        let want_ids: Vec<u8> = styles.iter().map(|style| style.id).collect();
        if have_ids != want_ids {
            return Err(PreviewFailure::input(
                "The preview map belongs to a different schema revision; refresh the builder deployment.",
            ));
        }
        slot.copy_from_slice(&packed);
        self.bytes[HEADER_MARKER_COLOR_AT..HEADER_MARKER_COLOR_AT + 2]
            .copy_from_slice(&skin.marker_color.to_le_bytes());
        self.tables = MapTables::parse(&SliceSource(&self.bytes)).map_err(read_failure)?;
        self.dirty = true;
        Ok(())
    }

    pub fn frame(&mut self) -> &[u8] {
        if self.dirty {
            let source = SliceSource(&self.bytes);
            let reader = Reader::new(&source, &self.tables, &self.cache);
            let background = reader.backdrop_style().map_or(0xFFFF, |style| style.color);
            let viewport =
                Viewport::new(FRAME_W as f32, FRAME_H as f32, CAMERA_LON, CAMERA_LAT, zoom_for_mpp(METERS_PER_PIXEL));
            self.renderer.render(&mut self.frame, &reader, &viewport, device_color(background), device_color);
            self.dirty = false;
        }
        self.frame.as_rgba()
    }
}

fn device_color(color: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_device64(color);
    Rgb888::new(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    use obc_pack::catalog::feature_type_ids;
    use obc_pack::config::{Config, LineStyle};
    use obc_pack::grid::BandTable;

    const MAP: &[u8] = include_bytes!("../../../host/obc-bake/assets/teningen-preview.obcm");
    const SCHEMA_CONFIG: &str = include_str!("../../../builder/presets/schema.json");

    fn schema_json() -> String {
        let config = Config::parse(SCHEMA_CONFIG).expect("schema config parses");
        let bands = BandTable::recommended().bands;
        let mut styles = feature_type_ids(&config)
            .into_iter()
            .map(|(feature_type, id)| serde_json::json!({ "id": id, "feature_type": feature_type }))
            .collect::<Vec<_>>();
        styles.sort_by_key(|style| style["id"].as_u64().unwrap());
        serde_json::json!({
            "id": "bikepacking",
            "revision": 1,
            "obcm_version": obc_formats::obcm::VERSION,
            "lods": config.lods.iter().enumerate().map(|(index, lod)| serde_json::json!({
                "index": index,
                "max_mpp": lod.max_mpp,
                "band": bands.iter().find(|band| band.lods.contains(&index)).unwrap().id,
            })).collect::<Vec<_>>(),
            "bands": bands,
            "styles": styles,
            "routing": {
                "min_component_edges": config.routing.min_component_edges,
                "profiles": config.routing.profiles.iter().map(|profile| &profile.name).collect::<Vec<_>>(),
            },
            "chunk_size": config.chunk_size,
        })
        .to_string()
    }

    fn skin_json(id: &str) -> String {
        let text = match id {
            "default" => include_str!("../../../builder/presets/skins/default.json"),
            "dusk" => include_str!("../../../builder/presets/skins/dusk.json"),
            _ => panic!("unknown shipped skin"),
        };
        let config = Config::parse(text).expect("skin config parses");
        let ids = feature_type_ids(&config);
        let mut styles = config
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
                        "dashed": style.line_style == LineStyle::Dashed,
                        "color2": style.color2,
                    })
                })
            })
            .collect::<Vec<_>>();
        styles.sort_by_key(|style| ids[style["feature_type"].as_str().unwrap()]);
        serde_json::json!({
            "id": id,
            "name": id,
            "marker_color": config.marker_color,
            "styles": styles,
        })
        .to_string()
    }

    #[test]
    fn restyles_one_resident_map_through_the_device_renderer() {
        let schema = schema_json();
        let day_skin = skin_json("default");
        let dusk_skin = skin_json("dusk");
        let mut preview = MapPreview::open(MAP.to_vec(), &schema, &day_skin).expect("default opens");
        let day = preview.frame().to_vec();
        assert_eq!(day.len(), (FRAME_W * FRAME_H * 4) as usize);
        assert!(day.chunks_exact(4).all(|px| px[3] == 0xFF));

        preview.set_skin(&dusk_skin).expect("dusk restamps");
        let dusk = preview.frame().to_vec();
        assert_ne!(day, dusk, "a skin edit must change the rendered scene");
    }

    #[test]
    fn refuses_schema_space_edits() {
        let schema = schema_json();
        let day_skin = skin_json("default");
        let mut preview = MapPreview::open(MAP.to_vec(), &schema, &day_skin).expect("default opens");
        let mut skin: serde_json::Value = serde_json::from_str(&day_skin).unwrap();
        skin["styles"].as_array_mut().unwrap().remove(0);
        let err = preview.set_skin(&skin.to_string()).expect_err("missing schema type is refused");
        assert_eq!(err.code, PreviewErrorCode::Input);
    }
}
