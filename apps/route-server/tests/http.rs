use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use route_engine::{
    model::{Graph, Pace, Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION},
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
    let graph = Graph { points, roads: vec![road], forbidden: vec![], forbidden_foot: vec![], warnings: vec![] };
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
        }
    }
}
