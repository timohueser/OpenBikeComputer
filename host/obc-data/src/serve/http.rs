//! What `obc data serve` answers over HTTP: the API of each planner release whose services
//! answer, the pointers of the current release, the files of the target, and its tiles.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use axum::body::Body;
use axum::extract::{Path as Params, Request, State};
use axum::handler::Handler;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use super::services::SERVICES;
use super::{Served, PLANNER};

/// What the follower gives the HTTP side.
#[derive(Default)]
pub struct View {
    /// The release that clients get.
    pub current: Option<Served>,
    /// The port of each of the [`SERVICES`] of each planner release that answers, by its id.
    pub ports: BTreeMap<String, [u16; SERVICES.len()]>,
}

pub type Shared = Arc<RwLock<View>>;

/// The API of the planner releases at `/planner-api/releases/<id>/<service>/`, the pointers of the
/// current release, and the files of the dir target `dir`. A path that names no file goes to the
/// tile server at port `tiles` of this machine. Any origin may read it.
pub fn router(shared: Shared, dir: &Path, tiles: u16) -> Router {
    let files = ServeDir::new(dir).append_index_html_on_directories(false).fallback(forward_all.with_state(tiles));
    Router::new()
        .route("/planner-api/releases/{id}/{service}/{*path}", any(api))
        .route("/cell-catalog/catalog.json", get(pointer))
        .route(&format!("/{PLANNER}"), get(pointer))
        .fallback_service(files)
        .with_state(shared)
        .layer(CorsLayer::permissive())
}

async fn api(
    State(shared): State<Shared>,
    Params((id, name, _)): Params<(String, String, String)>,
    request: Request,
) -> Response {
    let service = SERVICES.iter().position(|service| service.name == name);
    let port = service.and_then(|at| Some(shared.read().unwrap().ports.get(&id)?[at]));
    let (Some(service), Some(port)) = (service, port) else {
        return (StatusCode::NOT_FOUND, format!("No {name} service of planner release {id} runs here."))
            .into_response();
    };
    let prefix = format!("/planner-api/releases/{id}/{name}");
    let rest = request.uri().path_and_query().map_or("", |path| path.as_str()).strip_prefix(&prefix).unwrap_or("/");
    let path = format!("{}{rest}", SERVICES[service].path);
    forward(port, request, &path).await
}

async fn pointer(State(shared): State<Shared>, uri: Uri, headers: HeaderMap) -> Response {
    let key = uri.path().trim_start_matches('/');
    let bytes = shared.read().unwrap().current.as_ref().and_then(|current| current.pointers.get(key).cloned());
    let Some(bytes) = bytes else {
        return (StatusCode::SERVICE_UNAVAILABLE, "The target serves no release yet.").into_response();
    };
    let bytes = match key {
        PLANNER => {
            let host = headers.get(header::HOST).and_then(|host| host.to_str().ok());
            let Some(host) =
                host.filter(|host| !host.contains('@') && host.parse::<axum::http::uri::Authority>().is_ok())
            else {
                return (StatusCode::BAD_REQUEST, "The request names no host.").into_response();
            };
            match local(&bytes, &format!("http://{host}")) {
                Ok(bytes) => bytes,
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
            }
        }
        _ => bytes,
    };
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], bytes).into_response()
}

/// The planner pointer `bytes` with the origin of each `https://` URL replaced by `origin`: the
/// origin at which the client reached `obc data serve`.
fn local(bytes: &[u8], origin: &str) -> Result<Vec<u8>, String> {
    fn replace(value: &mut Value, origin: &str) {
        match value {
            Value::String(text) => {
                if let Some(rest) = text.strip_prefix("https://") {
                    *text = format!("{origin}{}", rest.find('/').map_or("", |at| &rest[at..]));
                }
            }
            Value::Array(values) => values.iter_mut().for_each(|value| replace(value, origin)),
            Value::Object(values) => values.values_mut().for_each(|value| replace(value, origin)),
            _ => {}
        }
    }
    let mut pointer: Value = serde_json::from_slice(bytes).map_err(|e| format!("{PLANNER}: {e}"))?;
    replace(&mut pointer, origin);
    serde_json::to_vec(&pointer).map_err(|e| e.to_string())
}

async fn forward_all(State(port): State<u16>, request: Request) -> Response {
    let path = request.uri().path_and_query().map_or("/", |path| path.as_str()).to_string();
    forward(port, request, &path).await
}

/// Send `request` to the server at `port` of this machine, with the path and query `path` and the
/// same host.
async fn forward(port: u16, mut request: Request, path: &str) -> Response {
    let answer = async {
        *request.uri_mut() = path.parse().map_err(|e| format!("{path}: {e}"))?;
        // The CORS layer answers the browser: a service that checks the origin itself sees none.
        request.headers_mut().remove(header::ORIGIN);
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| e.to_string())?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
        tokio::spawn(connection);
        sender.send_request(request).await.map_err(|e| e.to_string())
    };
    match answer.await {
        Ok(response) => response.map(Body::new),
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Request;

    use super::*;
    use crate::store::tests::Scratch;

    /// Serve `router` on a free port of this machine, and give the port.
    fn listen(runtime: &tokio::runtime::Runtime, router: Router) -> u16 {
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let port = listener.local_addr().unwrap().port();
        runtime.spawn(async move { axum::serve(listener, router).await.unwrap() });
        port
    }

    /// A server that answers with what it received.
    fn echo(request: Request) -> std::future::Ready<String> {
        let origin = request.headers().get(header::ORIGIN).map(|origin| origin.to_str().unwrap().to_string());
        let host = request.headers()[header::HOST].to_str().unwrap().to_string();
        std::future::ready(format!("{} {} host={host} origin={origin:?}", request.method(), request.uri()))
    }

    #[test]
    fn clients_reach_each_planner_release_by_its_id_and_the_target_at_the_origin_of_the_request() {
        let scratch = Scratch::new("serve-http");
        std::fs::create_dir_all(scratch.0.join("objects")).unwrap();
        std::fs::write(scratch.0.join("objects/abc"), b"0123456789").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let upstream = listen(&runtime, Router::new().fallback(echo));
        let planner = "a".repeat(64);
        let pointer = format!(
            r#"{{"format":1,"origins":{{"api_origin":"https://releases.example"}},"active":{{"id":"{planner}",
            "routing":"https://releases.example/planner-api/releases/{planner}/routing",
            "basemap":"https://tiles.example/releases/{planner}/basemap.json","attribution":"© OpenStreetMap",
            "layers":{{"snow":"https://tiles.example/releases/{planner}/snow.json"}}}}}}"#
        );
        let current = Served {
            id: "b".repeat(64),
            planner: planner.clone(),
            pointers: [("cell-catalog/catalog.json".into(), b"{}".to_vec()), (PLANNER.into(), pointer.clone().into())]
                .into(),
        };
        let view = View { current: Some(current), ports: [(planner.clone(), [upstream; 3])].into() };
        let port = listen(&runtime, router(Arc::new(RwLock::new(view)), &scratch.0, upstream));
        let origin = format!("http://127.0.0.1:{port}");
        let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into();
        let text = |mut response: axum::http::Response<ureq::Body>| response.body_mut().read_to_string().unwrap();

        let catalog: Value =
            serde_json::from_str(&text(agent.get(format!("{origin}/{PLANNER}")).call().unwrap())).unwrap();
        assert_eq!(catalog["origins"]["api_origin"], origin);
        assert_eq!(catalog["active"]["routing"], format!("{origin}/planner-api/releases/{planner}/routing"));
        assert_eq!(catalog["active"]["layers"]["snow"], format!("{origin}/releases/{planner}/snow.json"));
        assert_eq!(catalog["active"]["attribution"], "© OpenStreetMap");
        let phone = local(pointer.as_bytes(), "http://phone.test:8710").unwrap();
        let phone: Value = serde_json::from_slice(&phone).unwrap();
        assert_eq!(phone["active"]["basemap"], format!("http://phone.test:8710/releases/{planner}/basemap.json"));

        let api = format!("{origin}/planner-api/releases/{planner}");
        let routed = agent.post(format!("{api}/routing/v1/route?x=1")).header("Origin", "http://localhost:5173");
        let routed = routed.send("{}").unwrap();
        assert_eq!(routed.headers()["access-control-allow-origin"], "*");
        assert_eq!(text(routed), format!("POST /v1/route?x=1 host=127.0.0.1:{port} origin=None"));
        let searched = text(agent.post(format!("{api}/search/query")).send("{}").unwrap());
        assert!(searched.starts_with("POST /api/planner-search/query "), "{searched}");
        let other = agent.get(format!("{origin}/planner-api/releases/{}/routing/health", "c".repeat(64))).call();
        assert_eq!(other.unwrap().status(), 404, "only a planner release that answers has an API");

        let range = agent.get(format!("{origin}/objects/abc")).header("Range", "bytes=2-4").call().unwrap();
        assert_eq!((range.status().as_u16(), text(range)), (206, "234".to_string()));
        let tiles = text(agent.get(format!("{origin}/releases/{planner}/basemap.json")).call().unwrap());
        assert_eq!(tiles, format!("GET /releases/{planner}/basemap.json host=127.0.0.1:{port} origin=None"));
        assert_eq!(text(agent.get(format!("{origin}/cell-catalog/catalog.json")).call().unwrap()), "{}");
    }
}
