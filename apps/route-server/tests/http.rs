use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use route_build::Graph;
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
        osm: Default::default(),
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
    std::fs::write(path.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let app = route_server::app(&path, 1).unwrap();
    let path_string = CString::new(path.to_str().unwrap()).unwrap();
    let mut error = std::ptr::null_mut();
    // SAFETY: The test retains the path and error storage for initialization.
    let native_router = unsafe { native::planner_router_open(path_string.as_ptr(), 128 * 1024 * 1024, &mut error) };
    assert!(!native_router.is_null() && error.is_null());
    let mut native_status = 0;
    // SAFETY: This test serializes all calls to the live native router.
    let native_region = native_body(unsafe { native::planner_router_region(native_router, &mut native_status) });
    let response = app.clone().oneshot(Request::get("/v1/region").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status().as_u16(), native_status);
    assert_eq!(to_bytes(response.into_body(), 1024 * 1024).await.unwrap().as_ref(), native_region);

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
    // SAFETY: All native calls have completed; the handle is closed once.
    unsafe { native::planner_router_close(native_router) };
}
