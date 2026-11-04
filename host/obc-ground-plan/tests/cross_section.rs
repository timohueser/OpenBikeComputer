//! The cross-section of each way of the shared vector, which the viewer's reader
//! (`builder/web/src/lib/trailview/terrain/cross-section.ts`) asserts too.

use obc_ground_plan::sections::Sections;
use obc_ground_plan::tile::Tags;
use serde_json::Value;

#[test]
fn every_row_of_the_vector_holds() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("cross-section.json")).unwrap();
    assert!(rows.len() > 50, "{} rows", rows.len());
    for row in rows {
        let tags: Tags = serde_json::from_value(row["tags"].clone()).unwrap();
        let s = Sections::embedded().way(row["kind"].as_str().unwrap(), &tags, false);
        let got = [s.half_m, s.shoulder_m, s.cut, s.fill, s.reach_m];
        let want = ["half_m", "shoulder_m", "cut", "fill", "reach_m"].map(|k| row[k].as_f64().unwrap());
        assert_eq!((s.finish.name(), got), (row["finish"].as_str().unwrap(), want), "{row}");
    }
}
