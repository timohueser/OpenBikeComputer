use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use route_engine::{
    model::{Graph, Pace, Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION},
    osm::{Data, Id, Node, Relation, Way},
    package::digest,
};
use tower::ServiceExt;

#[tokio::test]
async fn http_contract_uses_a_closed_package_and_returns_typed_failures() {
    let path = std::env::temp_dir().join(format!("route-server-test-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    std::fs::create_dir(path.join("objects")).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    let points =
        vec![Point { lon: 0, lat: 0, elevation: NO_ELEVATION }, Point { lon: 10000, lat: 0, elevation: NO_ELEVATION }];
    let road = Road {
        from: 0,
        to: 1,
        way: 1,
        reversed: false,
        length_m: 1112,
        ascent_m: 0,
        descent_m: 0,
        surface: Surface::Paved,
        class: 1,
        access: BIKE | FOOT,
        difficulty: 0,
        hiking_difficulty: None,
        uncertain_access: false,
        structure: false,
        shape: points.clone(),
    };
    let graph = Graph {
        node_ids: vec![0, 1],
        node_access: vec![BIKE | FOOT; 2],
        osm: Data {
            nodes: points
                .iter()
                .enumerate()
                .map(|(i, point)| (i as i64, Node { id: i as i64, point: *point, tags: Default::default() }))
                .collect(),
            ways: [
                Way { id: 1, nodes: vec![0, 1], tags: [("highway".into(), "tertiary".into())].into() },
                Way { id: 2, nodes: vec![0, 1, 999, 0], tags: [("highway".into(), "construction".into())].into() },
                Way {
                    id: 3,
                    nodes: vec![0, 1],
                    tags: [("highway".into(), "footway".into()), ("bicycle".into(), "no".into())].into(),
                },
            ]
            .into_iter()
            .map(|w| (w.id, w))
            .collect(),
            relations: [(
                1,
                Relation {
                    id: 1,
                    tags: [
                        ("type".into(), "route".into()),
                        ("route".into(), "bicycle".into()),
                        ("network".into(), "rcn".into()),
                        ("website".into(), "https://example.org/route".into()),
                    ]
                    .into(),
                    members: vec![(Id::Way(1), String::new())],
                },
            )]
            .into(),
        },
        points,
        roads: vec![road],
        forbidden: vec![],
        forbidden_foot: vec![],
        warnings: vec![],
    };
    let manifest = route_build::prepare(
        &graph,
        "test".into(),
        [-1.0, -1.0, 1.0, 1.0],
        &Profile::presets()[..1],
        vec![],
        |bytes| {
            let key = digest(bytes);
            std::fs::write(path.join("objects").join(&key), bytes).map_err(|e| e.to_string())?;
            Ok(key)
        },
    )
    .unwrap();
    std::fs::write(path.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let app = route_server::app(&path, 1).unwrap();
    for (query, status, count) in [
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=12&layers=cycling,access", StatusCode::OK, 2),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=6&layers=cycling,access", StatusCode::OK, 0),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=8&layers=cycling,access", StatusCode::OK, 1),
        ("bbox=-10,-10,10,10&zoom=10&layers=cycling,access", StatusCode::OK, 2),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=12&layers=hiking", StatusCode::OK, 0),
        ("bbox=1,1,2,2&zoom=12&layers=cycling,access", StatusCode::OK, 0),
        ("bbox=NaN,0,1,1&zoom=12&layers=cycling", StatusCode::BAD_REQUEST, 0),
        ("bbox=-180,-90,180,90&zoom=12&layers=cycling", StatusCode::BAD_REQUEST, 0),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=15&layers=access&mode=cycling", StatusCode::OK, 2),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=15&layers=access&mode=walking", StatusCode::OK, 1),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=15&layers=access&mode=car", StatusCode::BAD_REQUEST, 0),
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(format!("/v1/overlays?{query}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        if status == StatusCode::OK {
            let data: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap();
            let features = data["features"].as_array().unwrap();
            assert_eq!(features.len(), count);
            assert!(features.iter().all(|f| f["geometry"]["coordinates"].as_array().unwrap().len() == 2));
            if count == 2 && query.contains("layers=cycling") {
                assert!(features
                    .iter()
                    .any(|f| f["properties"]["status"] == "construction" && f["properties"]["way"] == 2));
                assert!(features.iter().any(|f| f["properties"]["routes"][0]["network"] == "rcn"));
                assert!(features
                    .iter()
                    .any(|f| f["properties"]["routes"][0]["website"] == "https://example.org/route"));
            }
            if query.ends_with("mode=cycling") {
                assert!(features.iter().any(|f| f["properties"]["status"] == "push"));
            }
            if query.ends_with("mode=walking") {
                assert!(features.iter().all(|f| f["properties"]["status"] == "construction"));
            }
        }
    }
    let request = route_engine::Request {
        points: vec![[0.002, 0.0], [0.008, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        turnarounds: vec![],
    };
    for (body, status, code) in [
        (serde_json::to_string(&request).unwrap(), StatusCode::OK, None),
        (
            "{\"points\":[[2,2],[0,0]],\"profile\":\"touring\"}".into(),
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("missing_region"),
        ),
        ("{\"points\":[[0,0],[0,0]],\"profile\":\"absent\"}".into(), StatusCode::BAD_REQUEST, Some("invalid_request")),
        ("not json".into(), StatusCode::BAD_REQUEST, Some("invalid_request")),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/route").header("content-type", "application/json").body(Body::from(body)).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        if let Some(code) = code {
            assert_eq!(value["code"], code);
        } else {
            assert_eq!(value["routes"][0]["package"].as_str().unwrap().len(), 64);
            assert!(value["routes"][0]["totals"]["distance_m"].as_u64().unwrap() > 600);
            assert_eq!(value["routes"][0]["pushing"], serde_json::json!([false]));
        }
    }
}
