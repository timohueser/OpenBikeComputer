//! Concrete cell output checks for the full-map writer.

#[cfg(test)]
mod tests {
    use obc_map_core::config::{Config, CELL_SCHEMA as SCHEMA};
    #[test]
    fn partial_empty_cells_keep_real_artifacts_and_full_empty_cells_have_no_payload() {
        use crate::serialize::serialize_lods;
        use obc_draw::serialize::{LodLayer, Node};
        use obc_map_core::cell::Job;
        use obc_map_core::grid::CellId;
        let dir = obcm_testkit::scratch::scratch_dir("step", "empty-coverage");
        let config = Config::parse(SCHEMA).unwrap();
        let ids = [CellId::new(18, 1204, 1052).unwrap(), CellId::new(18, 1204, 1053).unwrap()];
        let tree = dir.join("cut");
        let mut artifacts = Vec::new();
        for id in ids {
            let lods = config
                .lods
                .iter()
                .map(|lod| LodLayer {
                    max_mpp: lod.max_mpp,
                    chunk_size: config.chunk_size,
                    root: Node::Leaf { bbox: id.square(), features: Vec::new() },
                })
                .collect::<Vec<_>>();
            let (body, dropped) = serialize_lods(
                &lods,
                &config.styles(),
                config.marker_color,
                id.square(),
                &[],
                &Default::default(),
                &config.routing.profiles,
                &mut obc_elevation::NullElevation,
            );
            assert_eq!(dropped, 0);
            let path = format!("cells/network/{:04}/{:04}.obcm", id.i, id.j);
            std::fs::create_dir_all(tree.join(&path).parent().unwrap()).unwrap();
            std::fs::write(tree.join(&path), &body).unwrap();
            artifacts.push((id, tree.join(path), true));
        }
        let output = dir.join("output");
        let partial_body = std::fs::read(&artifacts[1].1).unwrap();
        let job = Job::parse(&serde_json::json!({
            "band": "network", "leaf": [22, 75, 65],
            "cells": ids.iter().map(|id| [id.i, id.j]).collect::<Vec<_>>(),
            "partial_cells": [ids[1].to_string()],
        }))
        .unwrap();
        job.finish(&output, &artifacts).unwrap();
        let empty: Vec<String> =
            serde_json::from_slice(&std::fs::read(output.join("metadata/empty.json")).unwrap()).unwrap();
        assert_eq!(empty, [ids[0].to_string()]);
        assert!(!output.join(obc_map_core::cell::cell_path(&job.band, &ids[0])).exists());
        assert_eq!(
            std::fs::read(output.join(obc_map_core::cell::cell_path(&job.band, &ids[1]))).unwrap(),
            partial_body
        );
    }
}
