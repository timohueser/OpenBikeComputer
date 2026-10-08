//! Acquisition declarations beside the dispatch that executes them.

use crate::engine::{Code, OwnerCode, Python};
use crate::sources::{FetchKind, Source};

pub(super) fn owner(source: &Source) -> OwnerCode {
    let sources: Vec<_> = std::iter::once(source.id.clone()).chain(source.fetch.from.clone()).collect();
    let mut paths = vec![
        "host/obc-data/src/fetch.rs",
        "host/obc-data/src/fetch/code.rs",
        "host/obc-data/src/fetch/http.rs",
        "host/obc-data/src/fetch/upstream.rs",
        "host/obc-data/src/date.rs",
        "host/obc-data/src/sources.rs",
    ];
    let mut python = None;
    match source.fetch.kind {
        FetchKind::Github if source.id == "protomaps-basemaps" => {
            paths.extend(["host/obc-data/src/fetch/tools.rs", "tools/basemap_tool.py"]);
            python = Some(Python::default());
        }
        FetchKind::Geofabrik | FetchKind::Osm => paths.push("host/obc-data/src/fetch/osm.rs"),
        _ if matches!(source.fetch.kind, FetchKind::Dtm)
            || (matches!(source.fetch.kind, FetchKind::ByHand) && source.id.starts_with("dtm-")) =>
        {
            paths.extend([
                "host/obc-data/src/fetch/capture.rs",
                "host/obc-dem/reference/ingest.py",
                "host/obc-dem/reference/ingest",
            ]);
            python = Some(Python { group: Some("terrain-reference".into()) });
        }
        FetchKind::Capture => {
            paths.push("host/obc-data/src/fetch/capture.rs");
            let group = match source.id.as_str() {
                "wikidata" | "wikipedia" | "commons" => {
                    paths.extend([
                        "tools/landmark_capture.py",
                        "tools/peak_capture.py",
                        "host/obc-pack/src/landmarks/policy.json",
                        "specs/content-languages.json",
                    ]);
                    None
                }
                "modis-snow" | "hr-wsi" | "osm-trails" => {
                    paths.extend(["tools/planner_snow.py", "tools/planner_geo.py", "tools/step_request.py"]);
                    Some("planner-snow")
                }
                "era5-land" => {
                    paths.extend(["tools/planner_climate.py", "tools/planner_geo.py", "tools/step_request.py"]);
                    Some("planner-climate")
                }
                _ => {
                    return OwnerCode {
                        crate_name: "obc-data".into(),
                        code: Code {
                            paths: paths.into_iter().map(String::from).collect(),
                            sources,
                            ..Default::default()
                        },
                    }
                }
            };
            python = Some(Python { group: group.map(String::from) });
        }
        _ => {}
    }
    OwnerCode {
        crate_name: "obc-data".into(),
        code: Code { paths: paths.into_iter().map(String::from).collect(), sources, python, ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn acquisition_owners_name_existing_code() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for source in crate::sources::Registry::load(&root).unwrap().sources {
            for path in super::owner(&source).code.paths {
                assert!(root.join(&path).exists(), "{} names missing {path}", source.id);
            }
        }
    }
}
