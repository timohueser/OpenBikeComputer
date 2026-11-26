//! The TrailView tunnel parts in the browser: the builder of `obc-trailview-parts` behind `wasm-bindgen`, for the
//! viewer's stamp code in the stamp worker and in the bake of the stamp layer. The same functions build natively, so
//! the tests run them without a browser.
//!
//! An input is JSON: `{"square": [x0, y0, x1, y1], "ends": [{"at": [x, y], "z": z, "tags": {..}}],
//! "chords": [{"a": [x, y, z], "b": [x, y, z], "tags": {..}, "code": [c, ..], "marked": [m, ..]}]}`, in world units, with
//! a `code` and a `marked` per lane of a chord.

use obc_trailview_parts::{Chord, End, Part, Rules, Tags, Volumes};
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
    let rules = Rules::embedded();
    let v: Value = serde_json::from_str(text).map_err(|e| format!("input: {e}"))?;
    let list = |name: &str| v[name].as_array().cloned().unwrap_or_default();
    let ends = (list("ends").iter())
        .map(|e| {
            End::from_tags(
                numbers(&e["at"], "at")?,
                e["z"].as_f64().ok_or("z: not a number")?,
                &tags(&e["tags"])?,
                rules,
            )
        })
        .collect::<Result<_, String>>()?;
    let chords = (list("chords").iter())
        .map(|c| {
            let mut chord =
                Chord::from_tags(numbers(&c["a"], "a")?, numbers(&c["b"], "b")?, &tags(&c["tags"])?, rules)?;
            let (codes, marks) = (c["code"].as_array(), c["marked"].as_array());
            let (codes, marks) = codes
                .zip(marks)
                .filter(|(c, m)| c.len() == chord.lanes.len() && m.len() == c.len())
                .ok_or("code, marked: not one per lane")?;
            for ((lane, code), marked) in chord.lanes.iter_mut().zip(codes).zip(marks) {
                lane.code = code.as_u64().ok_or("code: not a count")? as u32;
                lane.marked = marked.as_bool().ok_or("marked: not true or false")?;
            }
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

/// The volumes of all built parts of the input, whatever its square: the count of box words, the boxes, then the
/// floor segments.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn volumes(input_json: &str) -> Result<Vec<f64>, Error> {
    let i = input(input_json).map_err(error)?;
    Ok(packed(obc_trailview_parts::volumes(&i.ends, &i.chords, Rules::embedded())))
}

/// Those of `volumes`, as it gives them, that come near the square `[x0, y0, x1, y1]`, in the same layout.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn near_volumes(square: &[f64], volumes: &[f64]) -> Result<Vec<f64>, Error> {
    let square = square.try_into().map_err(|_| error("square: not 4 numbers".into()))?;
    let n = (volumes.first().map(|&n| n as usize).filter(|&n| n < volumes.len()))
        .ok_or_else(|| error("volumes: not as `volumes` gives them".into()))?;
    let (boxes, floors) = volumes[1..].split_at(n);
    Ok(packed(obc_trailview_parts::near_volumes(square, boxes, floors, Rules::embedded())))
}

fn packed(v: Volumes) -> Vec<f64> {
    [vec![v.boxes.len() as f64], v.boxes, v.floors].concat()
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
