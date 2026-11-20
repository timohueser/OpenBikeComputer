//! The TrailView tunnel parts in the browser: the builder of `obc-trailview-parts` behind `wasm-bindgen`, for the
//! viewer's stamp code in the stamp worker and in the bake of the stamp layer. The same functions build natively, so
//! the tests run them without a browser.
//!
//! An input is JSON: `{"square": [x0, y0, x1, y1], "ends": [{"at": [x, y], "z": z, "tags": {..}}],
//! "chords": [{"a": [x, y, z], "b": [x, y, z], "tags": {..}, "code": c, "marked": m}]}`, in world units.

use obc_trailview_parts::{Chord, End, Part, Rules, Tags};
use serde_json::Value;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub type Error = JsError;
#[cfg(not(target_arch = "wasm32"))]
pub type Error = String;

#[cfg(target_arch = "wasm32")]
fn error(text: String) -> Error {
    JsError::new(&text)
}
#[cfg(not(target_arch = "wasm32"))]
fn error(text: String) -> Error {
    text
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// The square, the ends and the chords of an input.
struct Input {
    square: [f64; 4],
    ends: Vec<End>,
    chords: Vec<Chord>,
}

fn numbers<const N: usize>(v: &Value, what: &str) -> Result<[f64; N], String> {
    let list: Option<Vec<f64>> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect());
    list.and_then(|l| l.try_into().ok()).ok_or(format!("{what}: not {N} numbers"))
}

fn tags(v: &Value) -> Result<Tags, String> {
    let object = v.as_object().ok_or("tags: not an object")?;
    object
        .iter()
        .map(|(k, v)| Ok((k.clone(), v.as_str().ok_or(format!("tag {k}: not a string"))?.to_string())))
        .collect()
}

fn input(text: &str) -> Result<Input, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("input: {e}"))?;
    let list = |name: &str| v[name].as_array().cloned().unwrap_or_default();
    let ends = (list("ends").iter())
        .map(|e| {
            End::from_tags(numbers(&e["at"], "at")?, e["z"].as_f64().ok_or("z: not a number")?, &tags(&e["tags"])?)
        })
        .collect::<Result<_, String>>()?;
    let chords = (list("chords").iter())
        .map(|c| {
            let mut chord = Chord::from_tags(numbers(&c["a"], "a")?, numbers(&c["b"], "b")?, &tags(&c["tags"])?)?;
            chord.code = c["code"].as_u64().ok_or("code: not a count")? as u32;
            chord.marked = c["marked"].as_bool().ok_or("marked: not true or false")?;
            Ok(chord)
        })
        .collect::<Result<_, String>>()?;
    Ok(Input { square: numbers(&v["square"], "square")?, ends, chords })
}

/// The faces of the parts that the input's square draws: the count of vertex words, the vertex words (each word's
/// bits), then the indices.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn meshes(input_json: &str) -> Result<Vec<u32>, Error> {
    let i = input(input_json).map_err(error)?;
    let mesh = obc_trailview_parts::meshes(i.square, &i.ends, &i.chords, Rules::embedded());
    let mut out = Vec::with_capacity(1 + mesh.vertices.len() + mesh.indices.len());
    out.push(mesh.vertices.len() as u32);
    out.extend(mesh.vertices);
    out.extend(mesh.indices);
    Ok(out)
}

/// The volumes near the input's square: the count of box words, the boxes, then the floor segments.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn volumes(input_json: &str) -> Result<Vec<f64>, Error> {
    let i = input(input_json).map_err(error)?;
    let v = obc_trailview_parts::volumes(i.square, &i.ends, &i.chords, Rules::embedded());
    Ok([vec![v.boxes.len() as f64], v.boxes, v.floors].concat())
}

/// The rings of the built ends of the input: their count, then per ring its count of points and their x, y.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn footprints(input_json: &str) -> Result<Vec<f64>, Error> {
    let i = input(input_json).map_err(error)?;
    let rings = obc_trailview_parts::footprints(&i.ends, &i.chords, Rules::embedded());
    let mut out = vec![rings.len() as f64];
    for ring in rings {
        out.push(ring.len() as f64);
        out.extend(ring.into_iter().flatten());
    }
    Ok(out)
}

/// Whether the kinds of a tag `parts` (or `ends`), separated by a space, are all built.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn built(parts: &str) -> bool {
    obc_trailview_parts::built(&parts.split(' ').map(Part::parse).collect::<Vec<_>>())
}
