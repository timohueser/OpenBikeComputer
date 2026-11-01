//! The kind table of the step (`data/trailview/kinds.json`, in its options): which OSM objects become
//! which features, and the road classes (`data/trailview/roads.json`).

use std::collections::BTreeSet;

use serde_json::Value;

use crate::roads::{Class, Classes};
use crate::tiles::{Geometry, MAX_ZOOM, MIN_ZOOM};
use crate::Tags;

const FIELDS: [&str; 7] = ["kind", "geometry", "match", "keep", "height", "min_zoom", "active"];
const HEIGHTS: [&str; 4] = ["surface", "pad", "profile", "none"];
/// The fields of a road class that the road solver reads. The other fields of the table are the stamp
/// profile of the engine, which `builder/web` checks.
const CLASS_FIELDS: [&str; 7] = ["grade", "smooth_m", "plan_tol_m", "design_kmh", "bank", "widen_m", "keep_m"];
/// The classes of the tracks.
const RAILS: [&str; 5] = ["rail", "light_rail", "narrow_gauge", "rack", "funicular"];

/// The values that a selector takes for a key: any but `no`, or one of a list.
#[derive(Clone, Debug, PartialEq)]
pub enum Values {
    Any,
    List(Vec<String>),
}

/// The keys and values of a selector, in its order.
pub type Selector = Vec<(String, Values)>;

/// An entry of the kind table.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub geometry: Geometry,
    pub selectors: Vec<Selector>,
    pub keep: Vec<String>,
    pub min_zoom: u8,
    pub active: bool,
}

/// The kind table, or an error that names the first entry that breaks a rule of the spec.
pub fn validate(table: &Value) -> Result<Vec<Entry>, String> {
    let entries = table.as_array().filter(|a| !a.is_empty()).ok_or("The kind table is a list of entries")?;
    let mut names = BTreeSet::new();
    let word = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
    entries
        .iter()
        .map(|entry| {
            let name = entry.get("kind").and_then(Value::as_str).unwrap_or_default();
            let check = |ok: bool, rule: &str| if ok { Ok(()) } else { Err(format!("Kind {name:?}: {rule}")) };
            let object = entry.as_object();
            let kebab = name.split('-').enumerate().all(|(i, part)| {
                let mut chars = part.chars();
                chars.next().is_some_and(|c| c.is_ascii_lowercase() || i > 0 && c.is_ascii_digit())
                    && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            });
            check(kebab, "the name is kebab-case")?;
            check(names.insert(name.to_string()), "the name is unique")?;
            check(
                object.is_some_and(|o| o.keys().all(|k| FIELDS.contains(&k.as_str()))),
                "the fields are the known ones",
            )?;
            let geometry = match entry["geometry"].as_str() {
                Some("point") => Geometry::Point,
                Some("line") => Geometry::Line,
                Some("area") => Geometry::Area,
                _ => return Err(format!("Kind {name:?}: the geometry is point, line or area")),
            };
            check(entry["height"].as_str().is_some_and(|h| HEIGHTS.contains(&h)), "the height is a known one")?;
            let min_zoom = match entry.get("min_zoom") {
                None => MAX_ZOOM,
                Some(z) => z
                    .as_u64()
                    .filter(|z| (MIN_ZOOM as u64..=MAX_ZOOM as u64).contains(z))
                    .ok_or(format!("Kind {name:?}: min_zoom is {MIN_ZOOM} to {MAX_ZOOM}"))?
                    as u8,
            };
            let active = match entry.get("active") {
                None => true,
                Some(a) => a.as_bool().ok_or(format!("Kind {name:?}: active is true or false"))?,
            };
            let selectors = entry["match"].as_array().filter(|m| !m.is_empty());
            let selectors = selectors.ok_or(format!("Kind {name:?}: match is a list of selectors"))?;
            let selectors = selectors
                .iter()
                .map(|selector| {
                    let selector = selector.as_object().filter(|s| !s.is_empty());
                    let selector = selector.ok_or(format!("Kind {name:?}: a selector names at least one key"))?;
                    selector
                        .iter()
                        .map(|(key, values)| {
                            let values = match values {
                                Value::String(any) if any == "*" => Some(Values::Any),
                                Value::Array(list) if !list.is_empty() && list.iter().all(word) => {
                                    let list: Vec<String> =
                                        list.iter().map(|v| v.as_str().unwrap().to_string()).collect();
                                    (list.iter().collect::<BTreeSet<_>>().len() == list.len())
                                        .then_some(Values::List(list))
                                }
                                _ => None,
                            };
                            let values = values.filter(|_| !key.is_empty());
                            Ok((
                                key.clone(),
                                values
                                    .ok_or(format!("Kind {name:?}: {key:?} takes \"*\" or a list of unique values"))?,
                            ))
                        })
                        .collect::<Result<Selector, String>>()
                })
                .collect::<Result<Vec<_>, String>>()?;
            let keep = entry["keep"].as_array().filter(|k| k.iter().all(word));
            let keep: Vec<String> = keep
                .ok_or(format!("Kind {name:?}: keep is a list of unique keys"))?
                .iter()
                .map(|k| k.as_str().unwrap().to_string())
                .collect();
            check(keep.iter().collect::<BTreeSet<_>>().len() == keep.len(), "keep is a list of unique keys")?;
            Ok(Entry { kind: name.to_string(), geometry, selectors, keep, min_zoom, active })
        })
        .collect()
}

fn matches(selector: &Selector, tags: &Tags) -> bool {
    selector.iter().all(|(key, values)| match (tags.get(key), values) {
        (Some(value), Values::Any) => value != "no",
        (Some(value), Values::List(list)) => list.contains(value),
        (None, _) => false,
    })
}

/// The index of the first entry of `geometry` with a selector that matches `tags`, active or not.
pub fn kind_of(entries: &[Entry], geometry: Geometry, tags: &Tags) -> Option<usize> {
    entries.iter().position(|e| e.geometry == geometry && e.selectors.iter().any(|s| matches(s, tags)))
}

/// The tags through which a selector of `entry` matches.
pub fn selected_by(entry: &Entry, tags: &Tags) -> BTreeSet<(String, String)> {
    let matching = entry.selectors.iter().filter(|s| matches(s, tags));
    matching.flat_map(|s| s.iter().map(|(key, _)| (key.clone(), tags[key].clone()))).collect()
}

/// The road classes of the step's options: the name and the fields of `CLASS_FIELDS` of each class of
/// the table. The stamp profile is not an option, so a change to it does not bake again. The rows are an
/// array, because the key of a step sorts the keys of an object and the junctions rank by this order.
pub fn solver_rows(table: &Value) -> Value {
    let rows = table.as_object().into_iter().flatten().map(|(name, row)| {
        let fields = row.as_object().into_iter().flatten().filter(|(field, _)| CLASS_FIELDS.contains(&field.as_str()));
        let name = ("class".to_string(), Value::from(name.as_str()));
        Value::Object(fields.map(|(field, value)| (field.clone(), value.clone())).chain([name]).collect())
    });
    Value::Array(rows.collect())
}

/// The road classes of `solver_rows`, or an error that names the first row that breaks a rule. The table lists the
/// classes from the most important down, as the junctions rank them; `path` and `road` are the rows
/// of a way whose class the table lacks.
pub fn classes(table: &Value) -> Result<(Vec<String>, Classes), String> {
    let name_of = |row: &Value| row["class"].as_str().unwrap_or_default().to_string();
    let names: Vec<String> = table.as_array().into_iter().flatten().map(name_of).collect();
    if !names.iter().any(|n| n == "road") || !names.iter().any(|n| n == "path") {
        return Err("The road classes are an array with the rows road and path".into());
    }
    let rows = table.as_array().unwrap();
    let number = |v: &Value| v.as_f64().is_some_and(|n| n >= 0.0);
    let mut out = Vec::new();
    for (name, row) in names.iter().zip(rows) {
        let fields = row
            .as_object()
            .is_some_and(|r| r.len() == CLASS_FIELDS.len() + 1 && CLASS_FIELDS.iter().all(|f| r.contains_key(*f)));
        let grade = &row["grade"];
        let ok = fields
            && CLASS_FIELDS.iter().filter(|f| **f != "grade").all(|f| number(&row[*f]))
            && (grade.is_null() || number(grade) && grade.as_f64().is_some_and(|g| g > 0.0 && g < 1.0));
        if !ok {
            return Err(format!(
                "Road class {name:?}: the fields are {CLASS_FIELDS:?}, numbers of 0 or more, and grade is null or below 1"
            ));
        }
        let grade = grade.as_f64().unwrap_or(f64::INFINITY);
        let field = |key: &str| row[key].as_f64().unwrap();
        out.push(Class {
            grade,
            smooth_m: field("smooth_m"),
            rail: RAILS.contains(&name.as_str()),
            plan_tol_m: field("plan_tol_m"),
            design_kmh: field("design_kmh"),
            bank: field("bank"),
            widen_m: field("widen_m"),
            keep_m: field("keep_m"),
        });
    }
    let path = names.iter().position(|n| n == "path").unwrap();
    Ok((names, Classes { rows: out, path }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_valid_and_a_broken_entry_is_not() {
        let table: Value = serde_json::from_str(include_str!("../../../data/trailview/kinds.json")).unwrap();
        validate(&table).unwrap();
        let entry = &table[0];
        let with = |key: &str, value: Value| {
            let mut broken = entry.clone();
            broken[key] = value;
            Value::Array(vec![broken])
        };
        let broken = [
            with("geometry", "volume".into()),
            with("colour", "red".into()),
            with("match", serde_json::json!([{"highway": []}])),
            with("keep", serde_json::json!(["width", "width"])),
            with("min_zoom", 7.into()),
            with("kind", "Main road".into()),
            Value::Array(vec![entry.clone(), entry.clone()]),
        ];
        for table in broken {
            assert!(validate(&table).is_err(), "{table}");
        }
    }

    #[test]
    fn the_road_classes_are_valid_and_a_grade_of_one_is_not() {
        let mut table: Value = serde_json::from_str(include_str!("../../../data/trailview/roads.json")).unwrap();
        let rows = solver_rows(&table);
        let (names, classes) = classes(&rows).unwrap();
        assert_eq!((names[0].as_str(), names[classes.path].as_str()), ("rail", "path"));
        table["path"]["tilt"] = 0.3.into();
        assert_eq!(solver_rows(&table), rows, "the stamp profile is not an option of the step");
        let reversed =
            Value::Object(table.as_object().unwrap().iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect());
        assert_ne!(solver_rows(&reversed), rows, "the order of the classes is part of the options");
        table["road"]["grade"] = 1.0.into();
        assert!(super::classes(&solver_rows(&table)).is_err());
    }
}
