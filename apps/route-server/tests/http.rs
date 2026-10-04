use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use route_build::{
    overlays,
    source::{Data, Id, Node, Relation, Way},
    Graph,
};
use route_engine::{
    directory::Writer,
    model::{Pace, Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION},
};
use route_server::native;
use std::ffi::{c_char, CStr, CString};
use tower::ServiceExt;

fn native_body(response: *mut c_char) -> Vec<u8> {
    assert!(!response.is_null());
    // SAFETY: Each test consumes its newly returned native response once.
    unsafe {
        let body = CStr::from_ptr(response).to_bytes().to_vec();
        native::planner_response_free(response);
        body
    }
}

#[tokio::test]
async fn http_contract_uses_a_closed_package_and_returns_typed_failures() {
    let path = std::env::temp_dir().join(format!("route-server-test-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let mut writer = Writer::create(&path).unwrap();
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
            relations: [
                (
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
                ),
                (
                    2,
                    Relation {
                        id: 2,
                        tags: [
                            ("type".into(), "route".into()),
                            ("route".into(), "hiking".into()),
                            ("network".into(), "rwn".into()),
                        ]
                        .into(),
                        members: vec![(Id::Way(1), String::new())],
                    },
                ),
                (
                    3,
                    Relation {
                        id: 3,
                        tags: [("type".into(), "route".into()), ("route".into(), "mtb".into())].into(),
                        members: vec![(Id::Way(1), String::new())],
                    },
                ),
            ]
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
        |bytes| writer.write(bytes),
    )
    .unwrap();
    writer.finish().unwrap();
    let manifest = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(path.join("manifest.json"), &manifest).unwrap();
    let package = route_engine::package::digest(&manifest);
    overlays::write(&path.join(overlays::FILE), &package, [-1.0, -1.0, 1.0, 1.0], &graph.osm).unwrap();
    let database =
        rusqlite::Connection::open_with_flags(path.join("overlays.sqlite"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let counts: (i64, i64, i64) = database
        .query_row(
            "SELECT (SELECT count(*) FROM features), (SELECT count(*) FROM geometries), (SELECT count(*) FROM routes)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(counts, (5, 3, 3));
    drop(database);
    let app = route_server::app(&path, 1).unwrap();
    let path_string = CString::new(path.to_str().unwrap()).unwrap();
    let mut error = std::ptr::null_mut();
    // SAFETY: The test retains the path and error storage for initialization.
    let native_overlays = unsafe { native::planner_overlays_open(path_string.as_ptr(), &mut error) };
    assert!(!native_overlays.is_null() && error.is_null());
    // SAFETY: The test retains the path and error storage for initialization.
    let native_router = unsafe { native::planner_router_open(path_string.as_ptr(), 128 * 1024 * 1024, &mut error) };
    assert!(!native_router.is_null() && error.is_null());
    let mut native_status = 0;
    // SAFETY: This test serializes all calls to the live native router.
    let native_region = native_body(unsafe { native::planner_router_region(native_router, &mut native_status) });
    let response = app.clone().oneshot(Request::get("/v1/region").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status().as_u16(), native_status);
    assert_eq!(to_bytes(response.into_body(), 1024 * 1024).await.unwrap().as_ref(), native_region);

    for (query, status, count) in [
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=12&layers=cycling,access", StatusCode::OK, 2),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=6&layers=cycling,access", StatusCode::OK, 0),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=8&layers=cycling,access", StatusCode::OK, 1),
        ("bbox=-10,-10,10,10&zoom=10&layers=cycling,access", StatusCode::OK, 2),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=12&layers=hiking", StatusCode::OK, 1),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=12&layers=mtb", StatusCode::OK, 1),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=10&layers=mtb", StatusCode::OK, 0),
        ("bbox=1,1,2,2&zoom=12&layers=cycling,access", StatusCode::OK, 0),
        ("bbox=NaN,0,1,1&zoom=12&layers=cycling", StatusCode::BAD_REQUEST, 0),
        ("bbox=-180,-90,180,90&zoom=12&layers=cycling", StatusCode::BAD_REQUEST, 0),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=15&layers=access&mode=cycling", StatusCode::OK, 2),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=15&layers=access&mode=walking", StatusCode::OK, 1),
        ("bbox=-0.1,-0.1,0.1,0.1&zoom=15&layers=access&mode=car", StatusCode::BAD_REQUEST, 0),
    ] {
        let params: std::collections::HashMap<_, _> =
            query.split('&').map(|pair| pair.split_once('=').unwrap()).collect();
        let params = serde_json::to_vec(&params).unwrap();
        // SAFETY: The test retains the handle and request bytes and serializes queries.
        let bytes = native_body(unsafe {
            native::planner_overlays_query(native_overlays, params.as_ptr(), params.len(), &mut native_status)
        });
        assert_eq!(status.as_u16(), native_status);
        if status == StatusCode::OK {
            let data: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let region: serde_json::Value = serde_json::from_slice(&native_region).unwrap();
            assert_eq!(data["package"], region["package"]);
            assert_eq!(serde_json::to_vec(&data).unwrap(), bytes);
            let features = data["features"].as_array().unwrap();
            assert_eq!(features.len(), count);
            assert!(features.iter().all(|f| f["geometry"]["coordinates"].as_array().unwrap().len() == 2));
            if count == 2 && query.contains("layers=cycling") {
                assert!(features
                    .iter()
                    .any(|f| f["properties"]["status"] == "construction" && f["properties"]["way"] == 2));
                assert!(features
                    .iter()
                    .any(|f| data["routes"][f["properties"]["routes"][0].to_string()]["network"] == "rcn"));
                assert!(features.iter().any(|f| data["routes"][f["properties"]["routes"][0].to_string()]["website"]
                    == "https://example.org/route"));
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
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
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
        // SAFETY: The test retains the handle and request bytes and serializes queries.
        let native_response = native_body(unsafe {
            native::planner_router_request(native_router, body.as_ptr(), body.len(), &mut native_status)
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/route").header("content-type", "application/json").body(Body::from(body)).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(status.as_u16(), native_status);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), native_response);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(code) = code {
            assert_eq!(value["code"], code);
        } else {
            assert_eq!(value["routes"][0]["package"].as_str().unwrap().len(), 64);
            assert!(value["routes"][0]["totals"]["distance_m"].as_u64().unwrap() > 600);
            assert_eq!(value["routes"][0]["edges"]["pushing"], serde_json::json!([[false, 1]]));
        }
    }
    // The road runs east along the equator from 0 to 0.01 degrees.
    for (body, status, code) in [
        (r#"{"line":[[0.001,0],[0.005,0.00002],[0.009,0]],"profile":"touring"}"#, StatusCode::OK, None),
        (
            r#"{"line":[[-0.95,0],[0.95,0]],"profile":"touring"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("line_too_long"),
        ),
        (
            r#"{"line":[[0.001,0],[0.005,0.005],[0.009,0]],"profile":"touring"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
            Some("line_not_reproducible"),
        ),
        (r#"{"line":[[0.005,0],[0.005,0]],"profile":"touring"}"#, StatusCode::BAD_REQUEST, Some("invalid_request")),
    ] {
        // SAFETY: The test retains the handle and request bytes and serializes queries.
        let native_response = native_body(unsafe {
            native::planner_router_shape(native_router, body.as_ptr(), body.len(), &mut native_status)
        });
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/shape").header("content-type", "application/json").body(Body::from(body)).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(status.as_u16(), native_status);
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), native_response);
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        match code {
            Some(code) => assert_eq!(value["code"], code),
            None => assert_eq!(value, serde_json::json!({"points": [[0.001, 0.0], [0.009, 0.0]], "turnarounds": []})),
        }
    }
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/route")
                .header("content-type", "application/json")
                .header("accept-encoding", "gzip, br")
                .body(Body::from(serde_json::to_string(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["content-encoding"], "br");
    // SAFETY: All native calls have completed; the handles are closed once.
    unsafe {
        native::planner_overlays_close(native_overlays);
        native::planner_router_close(native_router);
    }
}
