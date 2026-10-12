//! `obc data ui`: the page of this machine on http://127.0.0.1:8700/. It serves the page that
//! `npm run build:data` builds into `builder/web/dist/data/`, and one route, `POST /api/cli`, that
//! runs this binary as `obc data --json ARGS`. So the page can do only what the CLI can.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use obc_data::cli::{Code, Error};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const ADDRESS: &str = "127.0.0.1:8700";

struct Page {
    /// `builder/web/dist/data/`.
    dir: PathBuf,
    /// This binary.
    exe: PathBuf,
    /// The repository root, where each command runs.
    root: PathBuf,
}

/// The body of `POST /api/cli`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    args: Vec<String>,
}

/// The answer to every `POST /api/cli`, a refusal too: the exit status and the JSON of the command.
#[derive(Serialize)]
struct Answer {
    exit: i32,
    output: Value,
}

impl Answer {
    fn failed(exit: i32, error: Error) -> Self {
        Self { exit, output: json!({ "error": error }) }
    }
}

/// Build the page, then serve it until the process stops.
pub fn run(root: &Path) -> Result<(), Error> {
    build(&root.join("builder/web"))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let page = Arc::new(Page { dir: root.join("builder/web/dist/data"), exe, root: root.to_path_buf() });
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(ADDRESS).await.map_err(|e| {
            Code::Failed
                .error(format!("{ADDRESS}: {e}"))
                .fix("Another `obc data ui` serves this machine: open http://127.0.0.1:8700/, or stop it first.")
        })?;
        let machine = obc_data::machine();
        eprintln!("obc data ui: the page of {machine} is at http://{ADDRESS}/ until you stop this command (Ctrl-C)");
        axum::serve(listener, router(page)).await.map_err(|e| Error::from(e.to_string()))
    })
}

/// `npm run build:data` in `builder/web`, with its output on standard error.
fn build(web: &Path) -> Result<(), Error> {
    if Command::new("npm").arg("--version").stdout(Stdio::null()).status().is_err() {
        return Err(Code::Failed
            .error("npm is not on PATH")
            .fix("Install Node 22 or newer, then run `obc data ui` again."));
    }
    if !web.join("node_modules").is_dir() {
        return Err(Code::Usage
            .error("builder/web has no node_modules")
            .fix("Run `npm ci` in builder/web, then run `obc data ui` again."));
    }
    eprintln!("obc data ui: npm run build:data (in builder/web)");
    let status = Command::new("npm")
        .args(["run", "build:data"])
        .current_dir(web)
        .stdout(std::io::stderr())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(Code::Failed
            .error("`npm run build:data` failed")
            .fix("Correct what it says above, then run `obc data ui` again."));
    }
    Ok(())
}

fn router(page: Arc<Page>) -> Router {
    Router::new().route("/api/cli", post(cli)).fallback(file).layer(middleware::from_fn(guard)).with_state(page)
}

async fn guard(request: Request, next: Next) -> Response {
    let why = {
        let header = |name| request.headers().get(name).and_then(|value| value.to_str().ok());
        refusal(request.method(), header(header::HOST), header(header::ORIGIN), header(header::CONTENT_TYPE))
    };
    let Some(why) = why else { return next.run(request).await };
    let fix = "Open the page at http://127.0.0.1:8700/ on this machine, or with `obc data open MACHINE`.";
    (StatusCode::FORBIDDEN, Json(Answer::failed(2, Code::Usage.error(why).fix(fix)))).into_response()
}

/// Why the page refuses a request. It answers only a browser on this machine or at the end of an
/// SSH tunnel to it: the Host is loopback at any port, which stops DNS rebinding; an Origin is
/// loopback, which stops other sites; and a POST is JSON, which a plain form cannot send.
fn refusal(
    method: &Method,
    host: Option<&str>,
    origin: Option<&str>,
    content_type: Option<&str>,
) -> Option<&'static str> {
    if !host.is_some_and(loopback) {
        return Some("the Host of the request is not this machine");
    }
    if origin.is_some_and(|origin| !origin.strip_prefix("http://").is_some_and(loopback)) {
        return Some("the request comes from a page of another site");
    }
    let json =
        content_type.is_some_and(|value| value.split(';').next().unwrap_or_default().trim() == "application/json");
    if method == Method::POST && !json {
        return Some("a POST to the page is application/json");
    }
    None
}

/// Whether `host`, with or without a port, is this machine.
fn loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split_once(']'),
        None => Some(host.split_once(':').unwrap_or((host, ""))),
    };
    matches!(name, Some(("127.0.0.1" | "localhost" | "::1", port)) if port.is_empty() || port.trim_start_matches(':').bytes().all(|b| b.is_ascii_digit()))
}

async fn cli(State(page): State<Arc<Page>>, Json(ask): Json<Ask>) -> Response {
    if let Some(error) = refused(&ask.args) {
        return (StatusCode::FORBIDDEN, Json(Answer::failed(2, error))).into_response();
    }
    Json(answer(&page.exe, &page.root, &ask.args).await).into_response()
}

/// Why the page does not run `obc data ARGS`: a command that runs until it is stopped, opens a
/// tunnel or a browser, or is plumbing belongs in a terminal. Each command that the page runs is
/// named here.
fn refused(args: &[String]) -> Option<Error> {
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let allowed = match args.first().map(String::as_str) {
        None | Some("update" | "sources" | "areas" | "clean" | "setup") => true,
        Some("runs") => !has("--follow"),
        Some("serve") => has("--detach"),
        Some(_) => false,
    };
    let command = ["obc", "data"].into_iter().chain(args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
    (!allowed).then(|| Code::Usage.error(format!("the page does not run `{command}`")).fix("Run it in a terminal."))
}

/// Run `exe --json ARGS` in `root`. Its standard error goes to the log of the page, and a child
/// that outlives the request keeps running.
async fn answer(exe: &Path, root: &Path, args: &[String]) -> Answer {
    let output = tokio::process::Command::new(exe)
        .arg("--json")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .await;
    let output = match output {
        Ok(output) => output,
        Err(e) => return Answer::failed(1, Code::Failed.error(format!("{} does not start: {e}", exe.display()))),
    };
    let exit = output.status.code().unwrap_or(1);
    match serde_json::from_slice(&output.stdout) {
        Ok(output) => Answer { exit, output },
        Err(_) => Answer::failed(
            if exit == 0 { 1 } else { exit },
            Code::Failed
                .error(format!("`obc data {}` wrote no JSON", args.join(" ")))
                .fix("Run the command in a terminal to see what it writes."),
        ),
    }
}

async fn file(State(page): State<Arc<Page>>, method: Method, uri: Uri) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let Some(path) = file_in(&page.dir, uri.path()) else { return StatusCode::NOT_FOUND.into_response() };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(&path))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The file of the URL path `path` in `dir`, where `/` is the page. A part that is empty, `.` or
/// `..`, or that holds `\`, names nothing, so no path leaves `dir`.
fn file_in(dir: &Path, path: &str) -> Option<PathBuf> {
    let path = path.strip_prefix('/')?;
    if path.is_empty() {
        return Some(dir.join("data.html"));
    }
    let mut file = dir.to_path_buf();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return None;
        }
        file.push(part);
    }
    Some(file)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_answers_only_this_machine_and_json_posts() {
        let (get, post) = (Method::GET, Method::POST);
        let json = Some("application/json");
        let cases = [
            (&get, Some("127.0.0.1:8700"), None, None, true),
            (&get, Some("localhost:8701"), None, None, true),
            (&get, Some("[::1]:8700"), None, None, true),
            (&get, Some("localhost"), None, None, true),
            (
                &post,
                Some("localhost:5173"),
                Some("http://localhost:5173"),
                Some("application/json; charset=utf-8"),
                true,
            ),
            (&get, None, None, None, false),
            (&get, Some("rebound.example:8700"), None, None, false),
            (&get, Some("127.0.0.1.rebound.example"), None, None, false),
            (&get, Some("localhost:80@rebound.example"), None, None, false),
            (&post, Some("127.0.0.1:8700"), Some("https://other.example"), json, false),
            (&post, Some("127.0.0.1:8700"), Some("null"), json, false),
            (&post, Some("127.0.0.1:8700"), None, Some("text/plain"), false),
            (&post, Some("127.0.0.1:8700"), None, None, false),
        ];
        for (method, host, origin, content_type, answered) in cases {
            let refusal = refusal(method, host, origin, content_type);
            assert_eq!(refusal.is_none(), answered, "{method} {host:?} {origin:?} {content_type:?}: {refusal:?}");
        }
    }

    #[test]
    fn a_url_path_names_only_a_file_in_the_page() {
        let dir = Path::new("/srv/page");
        assert_eq!(file_in(dir, "/"), Some(dir.join("data.html")));
        assert_eq!(file_in(dir, "/assets/data.js"), Some(dir.join("assets/data.js")));
        for path in
            ["/../secret", "/assets/../../secret", "/./data.html", "//etc/passwd", "/assets/..\\..\\secret", "relative"]
        {
            assert_eq!(file_in(dir, path), None, "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_page_runs_an_allowed_command_and_answers_its_json_and_exit_status() {
        use std::os::unix::fs::PermissionsExt;
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        for refused_args in
            [&["ui"][..], &["open", "bake"], &["runs", "r1", "--follow"], &["r2", "list"], &["serve"], &["--help"]]
        {
            assert!(refused(&args(refused_args)).is_some(), "{refused_args:?}");
        }
        for allowed in [&[][..], &["setup"], &["update", "local", "--yes"], &["runs", "r1"], &["serve", "--detach"]] {
            assert!(refused(&args(allowed)).is_none(), "{allowed:?}");
        }

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("obc-data");
        std::fs::write(&exe, "#!/bin/sh\nprintf '{\"args\":\"%s\",\"in\":\"%s\"}' \"$*\" \"$(pwd)\"\nexit 3\n")
            .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let answer = runtime.block_on(answer(&exe, dir.path(), &args(&["setup"])));
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(answer.exit, 3);
        assert_eq!(answer.output, json!({ "args": "--json setup", "in": root.to_str().unwrap() }));

        std::fs::write(&exe, "#!/bin/sh\necho 'not json'\n").unwrap();
        let answer = runtime.block_on(super::answer(&exe, dir.path(), &args(&["setup"])));
        assert_eq!(answer.exit, 1);
        assert_eq!(answer.output["error"]["message"], "`obc data setup` wrote no JSON");
    }
}
